use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::models::config::{AppConfig, ExistingJob, PrepareResult};
use crate::services::{
    asr, brief, ffmpeg, jobs, jobs::JobPaths, subtitle, task_events, task_store,
    transcript_binding, video_url, ytdlp,
};
use crate::state::AppState;

const NO_SUBTITLE: &str = "平台没有独立字幕，视频也没有可提取的内封文字字幕。";
const INGEST_STAGES: [&str; 4] = ["download", "transcribe", "download_subtitle", "prepare"];
fn valid_source_duration(url: &str, duration: f64, min_yfsp_secs: u32) -> bool {
    duration.is_finite()
        && duration > 0.0
        && (video_url::detect_platform(url) != Some(video_url::VideoPlatform::Yfsp)
            || duration >= f64::from(min_yfsp_secs))
}

fn check_source_duration(url: &str, duration: f64, min_yfsp_secs: u32) -> Result<(), String> {
    if !duration.is_finite() || duration <= 0.0 {
        return Err("下载的视频无法读取有效时长，请重试下载".into());
    }
    if !valid_source_duration(url, duration, min_yfsp_secs) {
        return Err(format!(
            "一帆视频时长只有 {duration:.1} 秒，短于设置中的 {} 分钟检查线，疑似预览片或错误线路；请核对播放页后重试",
            min_yfsp_secs / 60
        ));
    }
    Ok(())
}

fn quarantine_invalid_source(
    paths: &JobPaths,
    url: &str,
    min_yfsp_secs: u32,
) -> Result<(), String> {
    if !paths.source.is_file() {
        return Ok(());
    }
    std::fs::File::open(&paths.source)
        .map_err(|_| "片源文件不可读，未移动文件；请先检查权限".to_string())?;
    let result = ffmpeg::get_duration(&paths.source);
    quarantine_with_probe(paths, url, min_yfsp_secs, result)
}

fn quarantine_with_probe(
    paths: &JobPaths,
    url: &str,
    min_yfsp_secs: u32,
    result: Result<f64, ffmpeg::FfmpegError>,
) -> Result<(), String> {
    let (duration, reason) = match result {
        Ok(duration) if valid_source_duration(url, duration, min_yfsp_secs) => return Ok(()),
        Ok(duration) => (Some(duration), "invalid_or_short_duration"),
        Err(ffmpeg::FfmpegError::NotFound) => {
            return Err("ffprobe不可用，无法确认片源是否损坏；未移动文件，请先修复依赖".into())
        }
        Err(ffmpeg::FfmpegError::Io(_)) => {
            return Err("无法启动/读取ffprobe检查，未移动片源；请检查依赖和文件权限".into())
        }
        Err(ffmpeg::FfmpegError::Failed(_)) => (None, "unreadable_or_corrupt_container"),
    };
    let original_revision = source_revision(&paths.source)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rejected = paths.root.join("rejected-sources");
    std::fs::create_dir_all(&rejected).map_err(|error| format!("无法隔离错误片源：{error}"))?;
    let target = rejected.join(format!("source-{nonce}.mp4"));
    let report = serde_json::json!({"version":1,"reason":reason,"original_revision":original_revision,"original_bytes":std::fs::metadata(&paths.source).map_err(|e|e.to_string())?.len(),"detected_duration":duration.filter(|d|d.is_finite()),"min_yfsp_duration_secs":min_yfsp_secs,"quarantined_path":target});
    // Mark the replacement before moving source so old unbound subtitles cannot
    // be resurrected during a retry or after a crash between these operations.
    jobs::atomic_write(
        paths.root.join("source-replacement.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    jobs::atomic_write(
        rejected.join(format!("source-{nonce}.json")),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(&paths.source, &target)
        .map_err(|error| format!("无法隔离错误片源：{error}"))?;
    // Do not resume partial fragments from a previously rejected download.
    for entry in std::fs::read_dir(&paths.root)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_file()
            && name.starts_with("source.")
            && (name.ends_with(".part") || name.ends_with(".ytdl") || name.contains(".part-Frag"))
        {
            std::fs::rename(entry.path(), rejected.join(format!("{nonce}-{name}")))
                .map_err(|e| format!("隔离旧下载片段失败：{e}"))?;
        }
    }
    Ok(())
}

/// Explain *why* there is no subtitle instead of the same sentence for every case:
/// a platform that offers nothing, a platform offering languages we do not accept,
/// and a subtitle fetch that simply failed are three different problems.
fn no_subtitle_message(platform_languages: &[String], platform_error: Option<&str>) -> String {
    if !platform_languages.is_empty() {
        return format!(
            "平台只提供 {} 字幕，不在接受的语种范围内（中文/英文/日文/韩文），视频也没有内封文字字幕。",
            platform_languages.join("、")
        );
    }
    match platform_error {
        Some(error) => format!("拉取平台字幕失败（{error}），视频也没有可提取的内封文字字幕。"),
        None => NO_SUBTITLE.to_string(),
    }
}

pub(crate) fn validate_task_id(task_id: &str) -> Result<(), String> {
    if task_id.is_empty()
        || task_id.len() > 128
        || !task_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err("任务 ID 无效".into());
    }
    Ok(())
}

pub(crate) fn job_paths(config: &AppConfig, task_id: &str) -> Result<JobPaths, String> {
    jobs::migrate_legacy_job(&config.output_dir, task_id)
        .map_err(|error| format!("迁移旧任务到 inbox/{task_id} 失败：{error}"))
}

async fn config(app: &AppHandle) -> Result<AppConfig, String> {
    app.state::<AppState>()
        .get_config()
        .await
        .map_err(|error| error.to_string())
}

/// Identity of the downloaded source; shorts drafted against another file are stale.
pub(crate) fn source_revision(path: &std::path::Path) -> Result<String, String> {
    transcript_binding::source_revision(path)
}

fn read_meta(paths: &JobPaths) -> serde_json::Value {
    std::fs::read_to_string(&paths.meta)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn meta_str(meta: &serde_json::Value, key: &str) -> Option<String> {
    meta.get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn delete_all_task_directories(
    canonical_root: &std::path::Path,
    legacy_roots: &[std::path::PathBuf],
) -> Result<(), String> {
    if canonical_root.exists() {
        std::fs::remove_dir_all(canonical_root)
            .map_err(|error| format!("彻底删除任务目录失败：{error}"))?;
    }
    for legacy_root in legacy_roots {
        if legacy_root == canonical_root || !legacy_root.exists() {
            continue;
        }
        std::fs::remove_dir_all(legacy_root)
            .map_err(|error| format!("彻底删除旧版任务目录失败：{error}"))?;
    }

    let remaining = std::iter::once(canonical_root)
        .chain(legacy_roots.iter().map(std::path::PathBuf::as_path))
        .filter(|path| path.exists())
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if remaining.is_empty() {
        Ok(())
    } else {
        Err(format!("任务仍有磁盘资源未删除：{}", remaining.join("、")))
    }
}

fn transcript_source_file(paths: &JobPaths) -> std::path::PathBuf {
    paths.root.join("TRANSCRIPT.source.json")
}

fn current_transcript_source(paths: &JobPaths) -> String {
    std::fs::read(transcript_source_file(paths))
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .and_then(|value| value.get("source")?.as_str().map(str::to_string))
        .or_else(|| jobs::infer_subtitle_source(&paths.root))
        .unwrap_or_else(|| "未知来源".to_string())
}

pub(crate) fn ensure_supported_transcript_source(paths: &JobPaths) -> Result<(), String> {
    let source = current_transcript_source(paths);
    if source.starts_with("旧版语音字幕") || source.contains("OCR") || source.contains("硬字幕")
    {
        Err(format!("当前任务的字幕来自{source}，该来源已停用。可点击“重新获取字幕”再试平台字幕和视频内封字幕；都没有就放弃这个视频。"))
    } else {
        transcript_binding::verify(&paths.root, &paths.source, &paths.transcript)
    }
}

async fn acquire_best_available_subtitle(
    app: &AppHandle,
    config: &AppConfig,
    paths: &JobPaths,
    task_id: &str,
    video_path: &std::path::Path,
    platform_subtitle: Option<std::path::PathBuf>,
    platform_languages: &[String],
    platform_error: Option<&str>,
    progress_stage: &'static str,
) -> Result<(std::path::PathBuf, &'static str), String> {
    if let Some(path) = platform_subtitle {
        return Ok((path, "平台独立字幕"));
    }

    task_events::emit_progress(
        app,
        Some(task_id),
        progress_stage,
        8.0,
        if platform_languages.is_empty() {
            "平台没有独立字幕，正在检查视频内封字幕...".to_string()
        } else {
            format!(
                "平台只有 {} 字幕，正在检查视频内封字幕...",
                platform_languages.join("、")
            )
        },
    );
    let embedded_video = video_path.to_path_buf();
    let embedded_root = paths.root.clone();
    let embedded = tauri::async_runtime::spawn_blocking(move || {
        ffmpeg::extract_best_embedded_subtitle(&embedded_video, &embedded_root)
    })
    .await
    .map_err(|error| error.to_string())?;
    let embedded_error = match embedded {
        Ok(Some(path)) => return Ok((path, "视频内封字幕")),
        Ok(None) => None,
        Err(error) => Some(error.to_string()),
    };

    let missing = no_subtitle_message(platform_languages, platform_error);
    if !config.asr_fallback {
        return Err(match embedded_error {
            Some(error) => format!(
                "{missing}（内封字幕检查失败：{error}）本机语音转写已关闭，已放弃这个视频。"
            ),
            None => format!("{missing}本机语音转写已关闭，已放弃这个视频。"),
        });
    }
    if !asr::available(&config.asr_model_path) {
        return Err(format!(
            "{missing}\n\n本机语音转写不可用：需要 whisper-cli（brew install whisper-cpp）和 models/ 下的 ggml 模型。"
        ));
    }

    task_events::emit_progress(
        app,
        Some(task_id),
        progress_stage,
        10.0,
        "平台字幕与内封字幕都没有，正在用本机语音转写生成逐字稿...",
    );
    let handle = app.clone();
    let progress_handle = handle.clone();
    let progress_task = task_id.to_string();
    let task = task_id.to_string();
    let model = config.asr_model_path.clone();
    let language = config.asr_language.clone();
    let video = video_path.to_path_buf();
    let job_root = paths.root.clone();
    let transcribed = tauri::async_runtime::spawn_blocking(move || {
        asr::transcribe(
            &handle,
            &task,
            &video,
            &job_root,
            &model,
            &language,
            |percent, message| {
                task_events::emit_progress(
                    &progress_handle,
                    Some(&progress_task),
                    progress_stage,
                    percent,
                    message,
                )
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?;

    match transcribed {
        Ok(output) => {
            task_events::emit_progress(
                app,
                Some(task_id),
                progress_stage,
                95.0,
                format!("本机语音转写完成：{} 条对白", output.segments),
            );
            Ok((output.subtitle_path, "本机语音转写"))
        }
        Err(error) => Err(format!("{missing}\n\n本机语音转写失败：{error}")),
    }
}

/// Save the chosen subtitle as the canonical transcript and reject garbled text.
fn install_transcript(
    paths: &JobPaths,
    task_id: &str,
    subtitle_path: &std::path::Path,
    subtitle_source: &str,
) -> Result<Vec<subtitle::SubtitleEntry>, String> {
    let entries = subtitle::parse_file(subtitle_path)
        .map_err(|error| format!("读取候选字幕失败：{error}"))?;
    let quality = subtitle::assess_quality(&entries);
    if quality.needs_retranscription {
        return Err(format!(
            "{subtitle_source}质量检测未通过（{} 分）：{}。字幕不可用，已放弃这个视频",
            quality.score,
            quality.reasons.join("、")
        ));
    }
    let binding = transcript_binding::create(
        &paths.source,
        &paths.transcript,
        subtitle_path,
        subtitle_source,
        &entries,
    )?;
    // Fail closed after any interrupted install; canonical bytes + provenance +
    // artifact are committed before clearing this small installation marker.
    jobs::atomic_write(transcript_binding::pending(&paths.root),serde_json::to_vec(&serde_json::json!({"version":1,"source_revision":binding.source_revision,"transcript_revision":binding.transcript_revision})).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    // Install exactly the entries we quality-checked, not a second read of a
    // candidate that could have changed between validation and promotion.
    jobs::atomic_write(&paths.transcript, subtitle::to_srt(&entries).as_bytes())
        .map_err(|error| format!("保存字幕失败：{error}"))?;
    if source_revision(&paths.source)? != binding.source_revision {
        return Err("字幕安装期间原片已变化，已阻止使用此字幕；请重新获取".into());
    }
    transcript_binding::write(&paths.root, &binding)?;
    let transcript_raw = std::fs::read(&paths.transcript).map_err(|error| error.to_string())?;
    task_store::artifact(
        task_id,
        "transcript",
        &paths.transcript.to_string_lossy(),
        &jobs::stable_hash(&[&transcript_raw]),
        &binding.source_revision,
    )?;
    std::fs::remove_file(transcript_binding::pending(&paths.root)).map_err(|e| e.to_string())?;
    Ok(entries)
}

/// Recover only the retired whole-transcript translation failure. That path
/// already installed and quality-checked the original before calling the API.
/// Re-read it without rewriting it, changing its provenance or touching ASR caches.
fn translation_failure_transcript(
    paths: &JobPaths,
    previous_error: Option<&str>,
) -> Option<Vec<subtitle::SubtitleEntry>> {
    if !previous_error.is_some_and(|error| error.starts_with("翻译失败：")) {
        return None;
    }
    if !paths.source.is_file()
        || !transcript_binding::recover_legacy_translation(
            &paths.root,
            &paths.source,
            &paths.transcript,
        )
    {
        return None;
    }
    let source = current_transcript_source(paths);
    if !matches!(
        source.as_str(),
        "平台独立字幕" | "视频内封字幕" | "本机语音转写"
    ) {
        return None;
    }
    let entries = subtitle::parse_file(&paths.transcript).ok()?;
    if entries.is_empty() || subtitle::assess_quality(&entries).needs_retranscription {
        return None;
    }
    Some(entries)
}

fn prepared(
    paths: &JobPaths,
    config: &AppConfig,
    task_id: &str,
    title: &str,
    url: &str,
    duration: f64,
    entries: &[subtitle::SubtitleEntry],
) -> PrepareResult {
    PrepareResult {
        task_id: task_id.to_string(),
        job_dir: paths.root.to_string_lossy().into_owned(),
        source_path: paths.source.to_string_lossy().into_owned(),
        transcript_path: paths.transcript.to_string_lossy().into_owned(),
        source_available: paths.source.is_file()
            && valid_source_duration(url, duration, config.yfsp_min_duration_secs),
        transcript_available: paths.transcript.exists(),
        subtitle_source: current_transcript_source(paths),
        title: title.to_string(),
        duration_secs: duration,
        excluded_ranges: subtitle::detect_excluded_ranges(
            entries,
            duration,
            config.skip_intro_outro,
            config.skip_ads,
        ),
    }
}

async fn fetch_platform_subtitle(
    app: &AppHandle,
    config: &AppConfig,
    task_id: &str,
    url: &str,
    job_root: &std::path::Path,
) -> Result<ytdlp::SubtitleFetch, String> {
    let app_download = app.clone();
    let job_root = job_root.to_path_buf();
    let url = url.to_string();
    let options = ytdlp::DownloadOptions {
        cookies_browser: (!config.cookies_browser.is_empty())
            .then(|| config.cookies_browser.clone()),
        video_quality: config.video_quality.clone(),
        task_id: Some(task_id.to_string()),
    };
    let download_permit = app.state::<AppState>().acquire_download().await?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        ytdlp::download_subtitles_into_job(&app_download, &url, &job_root, options)
    })
    .await
    .map_err(|error| error.to_string())?;
    drop(download_permit);
    result.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn prepare_job(
    app: AppHandle,
    url: String,
    task_id: String,
    style: Option<String>,
) -> Result<PrepareResult, String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    task_store::ensure_task(&task_id, &task_id, &url)?;
    task_store::start_stage(&task_id, "download", "等待下载资源")?;
    let result = prepare_job_inner(&app, url, &task_id, style).await;
    if let Err(error) = &result {
        task_events::record_failure(&task_id, "prepare", "prepare_failed", error);
    }
    result
}

async fn prepare_job_inner(
    app: &AppHandle,
    url: String,
    task_id: &str,
    style: Option<String>,
) -> Result<PrepareResult, String> {
    let config = config(app).await?;
    let style = style
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| config.default_style.clone());
    let paths = job_paths(&config, task_id)?;
    paths.ensure().map_err(|e| e.to_string())?;
    quarantine_invalid_source(&paths, &url, config.yfsp_min_duration_secs)?;

    let app_dl = app.clone();
    let task_id_dl = task_id.to_string();
    let url_dl = url.clone();
    let job_root = paths.root.clone();
    let options = ytdlp::DownloadOptions {
        cookies_browser: (!config.cookies_browser.is_empty())
            .then(|| config.cookies_browser.clone()),
        video_quality: config.video_quality.clone(),
        task_id: Some(task_id_dl),
    };
    let download_permit = app.state::<AppState>().acquire_download().await?;
    let download = tauri::async_runtime::spawn_blocking(move || {
        ytdlp::download_into_job(&app_dl, &url_dl, &job_root, options)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    drop(download_permit);
    let duration = ffmpeg::get_duration(&download.video_path).map_err(|error| error.to_string())?;
    check_source_duration(&url, duration, config.yfsp_min_duration_secs)?;
    let subtitle_input_revision = source_revision(&paths.source)?;
    brief::write_meta(&paths, task_id, &download.title, &url, duration, &style)
        .map_err(|e| e.to_string())?;
    task_store::ensure_task(task_id, &download.title, &url)?;
    task_store::finish_stage(task_id, "download", "running", "原片下载完成")?;
    task_store::start_stage(task_id, "download_subtitle", "正在获取平台字幕")?;
    let subtitle_result = fetch_platform_subtitle(app, &config, task_id, &url, &paths.root).await;
    let (platform_subtitle, platform_languages, platform_error) = match subtitle_result {
        Ok(fetch) => (fetch.path, fetch.platform_languages, fetch.error),
        Err(error) => (None, Vec::new(), Some(error)),
    };
    task_store::finish_stage(task_id, "download_subtitle", "running", "平台字幕检查完成")?;
    task_store::start_stage(task_id, "transcribe", "正在整理字幕")?;
    let (subtitle_path, subtitle_source) = acquire_best_available_subtitle(
        app,
        &config,
        &paths,
        task_id,
        &download.video_path,
        platform_subtitle,
        &platform_languages,
        platform_error.as_deref(),
        "transcribe",
    )
    .await?;
    if source_revision(&paths.source)? != subtitle_input_revision {
        return Err("字幕获取期间原片已更换，未安装候选字幕；请重试获取当前原片字幕".into());
    }
    let entries = install_transcript(&paths, task_id, &subtitle_path, subtitle_source)?;

    let message = "原片与原语言字幕已就绪，可直接生成中文整集解说（不逐条翻译字幕）";
    task_events::emit_progress(app, Some(task_id), "transcribe", 100.0, message);
    task_store::finish_stage(task_id, "transcribe", "ready", message)?;
    Ok(prepared(
        &paths,
        &config,
        task_id,
        &download.title,
        &url,
        duration,
        &entries,
    ))
}

fn shorts_counts(paths: &JobPaths, config: &AppConfig, task_id: &str) -> (usize, usize) {
    // Existing IPC counters are retained for queue compatibility, but now count
    // the single episode output. Legacy shorts remain on disk, not in this UI.
    let Some(value) = crate::commands::episode::read_episode(paths).ok().flatten() else {
        return (0, 0);
    };
    let material_current = crate::commands::shorts::material_from_config(config.clone(), task_id)
        .ok()
        .is_some_and(|material| {
            value.transcript_revision
                == crate::services::script::transcript_revision(&material.entries)
                && source_revision(&paths.source)
                    .is_ok_and(|revision| revision == value.source_revision)
                && material.episode_film().is_ok_and(|shots| {
                    crate::services::script::same_source_windows(&value.shots, &shots)
                })
        });
    let rendered = material_current
        && value.reviewed_revision.as_deref() == Some(value.revision.as_str())
        && value
            .output_path
            .as_deref()
            .is_some_and(|path| std::path::Path::new(path).is_file());
    (1, usize::from(rendered))
}

#[tauri::command]
pub async fn list_existing_jobs(app: AppHandle) -> Result<Vec<ExistingJob>, String> {
    let config = config(&app).await?;
    jobs::migrate_all_legacy_jobs(&config.output_dir).map_err(|error| error.to_string())?;
    let jobs_root = jobs::inbox_root();
    if !jobs_root.exists() {
        return Ok(Vec::new());
    }

    let mut restored = Vec::new();
    let directories = std::fs::read_dir(&jobs_root).map_err(|error| error.to_string())?;
    for entry in directories.flatten() {
        let root = entry.path();
        let Some(task_id) = root
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .filter(|name| root.is_dir() && validate_task_id(name).is_ok())
        else {
            continue;
        };
        let paths = job_paths(&config, &task_id)?;
        let meta = read_meta(&paths);
        let snapshot = task_store::read_snapshot(&task_id);
        let stored = task_store::get(&task_id)?.or(snapshot.clone());
        let title = meta_str(&meta, "title")
            .or_else(|| {
                stored
                    .as_ref()
                    .map(|task| task.title.clone())
                    .filter(|title| !title.is_empty())
            })
            .unwrap_or_else(|| task_id.clone());
        let meta_url = stored
            .as_ref()
            .and_then(|task| {
                let url = task.source_url.trim();
                (!url.is_empty()).then(|| url.to_string())
            })
            .or_else(|| meta_str(&meta, "url"))
            .or_else(|| {
                snapshot
                    .as_ref()
                    .map(|task| task.source_url.trim().to_string())
                    .filter(|url| !url.is_empty())
            })
            .unwrap_or_default();
        task_store::ensure_task(&task_id, &title, &meta_url)?;
        let duration = if paths.source.exists() {
            ffmpeg::get_duration(&paths.source).unwrap_or(0.0)
        } else {
            0.0
        };
        let entries = subtitle::parse_file(&paths.transcript).unwrap_or_default();
        let unsupported = ensure_supported_transcript_source(&paths).err();
        let materials = paths.source.is_file()
            && valid_source_duration(&meta_url, duration, config.yfsp_min_duration_secs)
            && !entries.is_empty()
            && !subtitle::assess_quality(&entries).needs_retranscription
            && unsupported.is_none();
        let ingest_failed = stored.as_ref().is_some_and(|task| {
            task.status == "failed" && INGEST_STAGES.contains(&task.stage.as_str())
        });
        let (shorts_total, shorts_rendered) = shorts_counts(&paths, &config, &task_id);
        let (status, error) = if stored.as_ref().is_some_and(|task| task.status == "running") {
            ("running", None)
        } else if materials && !ingest_failed {
            (
                if shorts_rendered > 0 {
                    "completed"
                } else {
                    "ready"
                },
                None,
            )
        } else {
            let error = stored
                .as_ref()
                .filter(|_| ingest_failed)
                .and_then(|task| task.error_message.clone())
                .or(unsupported)
                .unwrap_or_else(|| {
                    if paths.source.exists() {
                        check_source_duration(&meta_url, duration, config.yfsp_min_duration_secs)
                            .err()
                            .unwrap_or_else(|| {
                                "这个任务没有可用字幕，可重新获取字幕；仍没有就放弃这个视频"
                                    .to_string()
                            })
                    } else {
                        "视频下载尚未完成，可使用原链接重试".to_string()
                    }
                });
            ("failed", Some(error))
        };
        let updated_at_ms = [
            paths.source.as_path(),
            paths.shorts.join("shorts.json").as_path(),
        ]
        .into_iter()
        .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
        .filter_map(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .max()
        .unwrap_or(0)
        .max(stored.as_ref().map(|task| task.updated_at_ms).unwrap_or(0));

        restored.push(ExistingJob {
            prepare: prepared(
                &paths, &config, &task_id, &title, &meta_url, duration, &entries,
            ),
            task_id,
            title,
            status: status.to_string(),
            error,
            updated_at_ms,
            stage: stored
                .as_ref()
                .map(|task| task.stage.clone())
                .unwrap_or_default(),
            progress: stored
                .as_ref()
                .map(|task| task.progress)
                .unwrap_or_default(),
            message: stored
                .as_ref()
                .map(|task| task.message.clone())
                .unwrap_or_default(),
            error_code: stored.as_ref().and_then(|task| task.error_code.clone()),
            retry_url: stored
                .as_ref()
                .map(|task| task.source_url.trim().to_string())
                .filter(|url| !url.is_empty())
                .or_else(|| (!meta_url.is_empty()).then_some(meta_url)),
            shorts_total,
            shorts_rendered,
        });
    }
    restored.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    Ok(restored)
}

#[tauri::command]
pub async fn read_job_transcript(app: AppHandle, task_id: String) -> Result<String, String> {
    validate_task_id(&task_id)?;
    let paths = job_paths(&config(&app).await?, &task_id)?;
    std::fs::read_to_string(&paths.transcript).map_err(|error| format!("读取原字幕失败：{error}"))
}

/// Retry the two accepted subtitle sources for an already downloaded film.
/// Existing shorts become stale through their transcript revision.
#[tauri::command]
pub async fn refetch_subtitles(app: AppHandle, task_id: String) -> Result<PrepareResult, String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    let result = refetch_subtitles_inner(&app, &task_id).await;
    if let Err(error) = &result {
        task_events::record_failure(&task_id, "download_subtitle", "subtitle_failed", error);
    }
    result
}

async fn refetch_subtitles_inner(app: &AppHandle, task_id: &str) -> Result<PrepareResult, String> {
    let config = config(app).await?;
    let paths = job_paths(&config, task_id)?;
    if !paths.source.exists() {
        return Err("任务原片不存在，无法获取字幕".into());
    }
    // Capture the failure before start_stage clears its error fields.
    let snapshot = task_store::read_snapshot(task_id);
    let stored = task_store::get(task_id)?.or_else(|| snapshot.clone());
    let recovered = translation_failure_transcript(
        &paths,
        stored
            .as_ref()
            .and_then(|task| task.error_message.as_deref()),
    );
    task_store::start_stage(task_id, "download_subtitle", "正在获取字幕")?;
    let meta = read_meta(&paths);
    let url = meta_str(&meta, "url")
        .or_else(|| {
            stored
                .as_ref()
                .map(|task| task.source_url.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            snapshot
                .as_ref()
                .map(|task| task.source_url.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .ok_or("任务缺少原始播放地址，无法拉取平台字幕")?;
    let title = meta_str(&meta, "title")
        .or_else(|| {
            stored
                .as_ref()
                .map(|task| task.title.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| task_id.to_string());
    let duration = ffmpeg::get_duration(&paths.source).map_err(|error| error.to_string())?;
    check_source_duration(&url, duration, config.yfsp_min_duration_secs)?;
    let subtitle_input_revision = source_revision(&paths.source)?;
    if meta_str(&meta, "url").is_none() || meta_str(&meta, "title").is_none() {
        brief::write_meta(
            &paths,
            task_id,
            &title,
            &url,
            duration,
            &config.default_style,
        )
        .map_err(|error| format!("修复任务 META.json 失败：{error}"))?;
    }
    task_store::ensure_task(task_id, &title, &url)?;

    if let Some(entries) = recovered {
        // Explicit retry completes provenance for the legacy translation-failure
        // path; no original transcript bytes or timestamps are rewritten.
        let mut binding = transcript_binding::create(
            &paths.source,
            &paths.transcript,
            &paths.transcript,
            &current_transcript_source(&paths),
            &entries,
        )?;
        // Retried historical originals need not have identical SRT whitespace;
        // bind existing bytes without silently normalizing/replacing their text.
        binding.canonical_bytes_revision =
            jobs::stable_hash(&[&std::fs::read(&paths.transcript).map_err(|e| e.to_string())?]);
        transcript_binding::write(&paths.root, &binding)?;
        task_store::artifact(
            task_id,
            "transcript",
            &paths.transcript.to_string_lossy(),
            &jobs::stable_hash(&[&std::fs::read(&paths.transcript).map_err(|e| e.to_string())?]),
            &binding.source_revision,
        )?;
        let message = "已复用翻译失败前保存的原语言字幕，可直接生成中文整集解说；不重下、不重转写、不全文翻译";
        task_events::emit_progress(app, Some(task_id), "download_subtitle", 100.0, message);
        task_store::finish_stage(task_id, "download_subtitle", "ready", message)?;
        return Ok(prepared(
            &paths, &config, task_id, &title, &url, duration, &entries,
        ));
    }
    let subtitle_result = fetch_platform_subtitle(app, &config, task_id, &url, &paths.root).await;
    let (platform_subtitle, platform_languages, platform_error) = match subtitle_result {
        Ok(fetch) => (fetch.path, fetch.platform_languages, fetch.error),
        Err(error) => {
            // The downloaded source may still carry an embedded text track.
            let message = format!("平台字幕获取失败，正在继续检查视频内封字幕：{error}");
            task_events::emit_progress(app, Some(task_id), "download_subtitle", 8.0, message);
            (None, Vec::new(), Some(error.to_string()))
        }
    };
    let (subtitle_path, subtitle_source) = acquire_best_available_subtitle(
        app,
        &config,
        &paths,
        task_id,
        &paths.source,
        platform_subtitle,
        &platform_languages,
        platform_error.as_deref(),
        "download_subtitle",
    )
    .await?;
    if source_revision(&paths.source)? != subtitle_input_revision {
        return Err("字幕重试期间原片已更换，未安装候选字幕；请重新获取当前原片字幕".into());
    }
    let entries = install_transcript(&paths, task_id, &subtitle_path, subtitle_source)?;
    let message =
        format!("{subtitle_source}已保存为原语言，可直接生成中文整集解说（不逐条翻译字幕）");
    task_events::emit_progress(app, Some(task_id), "download_subtitle", 100.0, &message);
    task_store::finish_stage(task_id, "download_subtitle", "ready", &message)?;
    Ok(prepared(
        &paths, &config, task_id, &title, &url, duration, &entries,
    ))
}

#[tauri::command]
pub async fn delete_job(app: AppHandle, task_id: String) -> Result<(), String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = config(&app).await?;
    let paths = job_paths(&config, &task_id)?;
    let legacy_roots = [
        std::path::PathBuf::from(&config.output_dir)
            .join("jobs")
            .join(&task_id),
        jobs::repo_root().join("jobs").join(&task_id),
        jobs::inbox_root().join("jobs").join(&task_id),
    ];
    delete_all_task_directories(&paths.root, &legacy_roots)?;
    task_store::delete(&task_id)?;
    if task_store::get(&task_id)?.is_some() {
        return Err("任务磁盘资源已删除，但数据库记录仍然存在".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn reveal_in_finder(app: AppHandle, path: String) -> Result<(), String> {
    let requested = std::path::Path::new(&path)
        .canonicalize()
        .map_err(|_| "成片文件不存在或已被移动，请检查任务目录".to_string())?;
    let inbox = jobs::inbox_root()
        .canonicalize()
        .map_err(|_| "任务目录不存在".to_string())?;
    if !requested.starts_with(&inbox) {
        return Err("只能定位inbox中的任务资源".into());
    }
    app.opener()
        .reveal_item_in_dir(&requested)
        .map_err(|_| "无法在Finder中定位文件，请检查系统文件管理器".to_string())
}

#[tauri::command]
pub async fn reveal_path(app: AppHandle, path: String) -> Result<(), String> {
    let requested = std::path::Path::new(&path)
        .canonicalize()
        .map_err(|error| format!("路径不存在：{error}"))?;
    let inbox = jobs::inbox_root()
        .canonicalize()
        .map_err(|error| format!("任务目录不存在：{error}"))?;
    if !requested.starts_with(&inbox) {
        return Err("只能打开 inbox 中的任务资源".to_string());
    }
    app.opener()
        .open_path(requested.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod pipeline_tests {
    use super::{
        check_source_duration, delete_all_task_directories, install_transcript,
        no_subtitle_message, translation_failure_transcript, validate_task_id,
    };

    #[test]
    fn translation_failure_retry_reuses_original_without_rewriting_any_file() {
        use crate::services::jobs::JobPaths;
        let root = std::env::temp_dir().join(format!(
            "vc-original-retry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = JobPaths {
            source: root.join("source.mp4"),
            transcript: root.join("TRANSCRIPT.srt"),
            meta: root.join("META.json"),
            shorts: root.join("shorts"),
            root: root.clone(),
        };
        std::fs::write(&paths.source, b"synthetic-placeholder").unwrap();
        let original = "1\n00:00:01,000 --> 00:00:04,000\nThe key is in the drawer.\n\n2\n00:00:05,000 --> 00:00:08,000\nPlease bring it here.\n";
        std::fs::write(&paths.transcript, original).unwrap();
        let provenance = r#"{"source":"本机语音转写","source_path":"source.asr.srt"}"#.as_bytes();
        std::fs::write(root.join("TRANSCRIPT.source.json"), provenance).unwrap();
        let error = "翻译失败：853 条字幕未能翻译（超过 10%），已保留原文";
        let recovered = translation_failure_transcript(&paths, Some(error)).unwrap();
        assert_eq!(recovered[0].text, "The key is in the drawer.");
        assert_eq!(recovered[0].start, "00:00:01,000");
        assert_eq!(
            std::fs::read(&paths.transcript).unwrap(),
            original.as_bytes()
        );
        assert_eq!(
            std::fs::read(root.join("TRANSCRIPT.source.json")).unwrap(),
            provenance
        );
        assert!(translation_failure_transcript(&paths, Some("下载失败")).is_none());
        assert!(translation_failure_transcript(&paths, None).is_none());
        std::fs::write(
            root.join("TRANSCRIPT.source.json"),
            r#"{"source":"本机语音转写+中文翻译"}"#.as_bytes(),
        )
        .unwrap();
        assert!(translation_failure_transcript(&paths, Some(error)).is_none());
        std::fs::write(root.join("TRANSCRIPT.source.json"), provenance).unwrap();
        std::fs::write(&paths.transcript, "").unwrap();
        assert!(translation_failure_transcript(&paths, Some(error)).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_settings_default_the_subtitle_mask_and_duck_mode() {
        let mut raw = serde_json::to_value(crate::models::config::AppConfig::default()).unwrap();
        let object = raw.as_object_mut().unwrap();
        object.remove("source_caption_mask_percent");
        object.remove("original_audio_mode");
        object.insert("burn_captions".into(), true.into());
        let old: crate::models::config::AppConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(old.source_caption_mask_percent, 18);
        assert!(old.original_volume() > 0.0);
    }

    #[test]
    fn task_ids_cannot_escape_job_directories() {
        assert!(validate_task_id("job-1788005857370-0lwma4").is_ok());
        assert!(validate_task_id("../jobs").is_err());
        assert!(validate_task_id("task/child").is_err());
        assert!(validate_task_id("").is_err());
    }

    #[test]
    fn no_subtitle_message_says_which_case_it_is() {
        let offered = no_subtitle_message(&["vi".to_string(), "th".to_string()], None);
        assert!(offered.contains("vi、th"));
        assert!(offered.contains("不在接受的语种范围内"));

        let failed = no_subtitle_message(&[], Some("yt-dlp timed out"));
        assert!(failed.contains("yt-dlp timed out"));

        let nothing = no_subtitle_message(&[], None);
        assert!(nothing.contains("平台没有独立字幕"));
    }

    #[test]
    fn corrupt_sources_are_quarantined_but_probe_dependency_failures_never_move_them() {
        use crate::services::{ffmpeg::FfmpegError, jobs::JobPaths};
        let root = std::env::temp_dir().join(format!(
            "quarantine-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = JobPaths {
            source: root.join("source.mp4"),
            transcript: root.join("TRANSCRIPT.srt"),
            meta: root.join("META.json"),
            shorts: root.join("shorts"),
            root: root.clone(),
        };
        std::fs::write(&paths.source, b"synthetic corrupt container").unwrap();
        std::fs::write(&paths.transcript, b"old transcript must remain").unwrap();
        assert!(super::quarantine_with_probe(
            &paths,
            "https://example.com/video",
            0,
            Err(FfmpegError::NotFound)
        )
        .is_err());
        assert!(paths.source.exists());
        assert!(!root.join("source-replacement.json").exists());
        assert!(super::quarantine_with_probe(
            &paths,
            "https://example.com/video",
            0,
            Err(FfmpegError::Io(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            )))
        )
        .is_err());
        assert!(paths.source.exists());
        std::fs::write(root.join("source.mp4.part"), b"old partial").unwrap();
        std::fs::write(root.join("source.mp4.ytdl"), b"old resume state").unwrap();
        super::quarantine_with_probe(
            &paths,
            "https://example.com/video",
            0,
            Err(FfmpegError::Failed("synthetic corrupt data".into())),
        )
        .unwrap();
        assert!(!paths.source.exists());
        assert!(!root.join("source.mp4.part").exists());
        assert!(!root.join("source.mp4.ytdl").exists());
        assert_eq!(
            std::fs::read(&paths.transcript).unwrap(),
            b"old transcript must remain"
        );
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("source-replacement.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["reason"], "unreadable_or_corrupt_container");
        assert!(manifest["original_revision"].is_string());
        assert!(root
            .join("rejected-sources")
            .join(manifest["quarantined_path"].as_str().unwrap())
            .exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn valid_sources_are_preserved_and_only_yfsp_uses_configured_minimum() {
        use crate::services::jobs::JobPaths;
        let root = std::env::temp_dir().join(format!("valid-source-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let paths = JobPaths {
            source: root.join("source.mp4"),
            transcript: root.join("TRANSCRIPT.srt"),
            meta: root.join("META.json"),
            shorts: root.join("shorts"),
            root: root.clone(),
        };
        std::fs::write(&paths.source, b"synthetic video").unwrap();
        super::quarantine_with_probe(&paths, "https://example.com/video", 600, Ok(12.0)).unwrap();
        assert!(paths.source.exists());
        super::quarantine_with_probe(
            &paths,
            "https://www.yfsp.tv/play/example?id=episode",
            600,
            Ok(429.0),
        )
        .unwrap();
        assert!(!paths.source.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn short_yfsp_preview_is_rejected_but_short_generic_video_is_allowed() {
        let url = "https://www.yfsp.tv/play/example?id=episode";
        assert!(check_source_duration(url, 429.67, 600)
            .unwrap_err()
            .contains("疑似预览片"));
        assert!(check_source_duration(url, 429.67, 0).is_ok());
        assert!(check_source_duration(url, 2725.48, 600).is_ok());
        assert!(check_source_duration("https://example.com/video", 429.67, 600).is_ok());
        assert!(check_source_duration(url, 0.0, 600).is_err());
    }

    #[test]
    fn rejected_candidate_does_not_replace_existing_transcript() {
        use crate::services::jobs::JobPaths;
        let root = std::env::temp_dir().join(format!(
            "vc-rejected-subtitle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = JobPaths {
            source: root.join("source.mp4"),
            transcript: root.join("TRANSCRIPT.srt"),
            meta: root.join("META.json"),
            shorts: root.join("shorts"),
            root: root.clone(),
        };
        let original = b"1\n00:00:01,000 --> 00:00:02,000\nOriginal subtitle\n";
        std::fs::write(&paths.transcript, original).unwrap();
        let candidate = root.join("candidate.srt");
        std::fs::write(
            &candidate,
            "1\n00:00:01,000 --> 00:00:02,000\n以下是影视剧对白\n",
        )
        .unwrap();
        assert!(install_transcript(&paths, "test", &candidate, "平台独立字幕").is_err());
        assert_eq!(std::fs::read(&paths.transcript).unwrap(), original);
        assert!(!root.join("TRANSCRIPT.source.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_a_task_removes_canonical_and_legacy_directories() {
        let base = std::env::temp_dir().join(format!(
            "vc-delete-all-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let canonical = base.join("inbox/job-test");
        let old_output = base.join("Video Commentary/jobs/job-test");
        let old_repo = base.join("jobs/job-test");
        for root in [&canonical, &old_output, &old_repo] {
            std::fs::create_dir_all(root.join("clips")).unwrap();
            std::fs::write(root.join("clips/segment.mp4"), b"clip").unwrap();
        }

        delete_all_task_directories(&canonical, &[old_output.clone(), old_repo.clone()]).unwrap();

        assert!(!canonical.exists());
        assert!(!old_output.exists());
        assert!(!old_repo.exists());
        let _ = std::fs::remove_dir_all(base);
    }
}
