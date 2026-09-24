use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::models::config::{AppConfig, ExcludedRange, ExistingJob, PrepareResult, RenderResult};
use crate::models::edit::EditPlan;
use crate::services::sample::{self, SamplePlan, SampleResult};
use crate::services::{
    brief, director, edit_timing, ffmpeg, hard_subtitle, jobs, jobs::JobPaths, subtitle,
    subtitle_correction,
    task_events, task_store, tts, ytdlp,
};
use crate::state::AppState;

fn sample_progress(app: &AppHandle, task_id: &str, message: &str) {
    // Deliberately separate from task-progress / task_store: a failed experiment
    // must not mark the already completed full-film task as failed.
    let _ = app.emit(
        "sample-progress",
        serde_json::json!({"task_id":task_id,"message":message}),
    );
}

fn sample_source_revision(path: &std::path::Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let modified = meta
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(jobs::stable_hash(&[
        path.to_string_lossy().as_bytes(),
        meta.len().to_string().as_bytes(),
        modified.to_string().as_bytes(),
    ]))
}

#[tauri::command]
pub async fn read_sample_reviews(
    app: AppHandle,
    task_id: String,
) -> Result<serde_json::Value, String> {
    validate_task_id(&task_id)?;
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|e| e.to_string())?;
    let root = job_paths(&config, &task_id)?
        .root
        .join("samples/reflection");
    if !root.exists() {
        return Ok(serde_json::json!({"reviews":[]}));
    }
    let mut dirs = std::fs::read_dir(&root)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_ok_and(|t| t.is_dir())
                && e.file_name().to_string_lossy().starts_with("run-")
        })
        .map(|e| e.path())
        .collect::<Vec<_>>();
    dirs.sort();
    let Some(latest) = dirs.last() else {
        return Ok(serde_json::json!({"reviews":[]}));
    };
    let mut reviews = Vec::new();
    for round in 0..=2 {
        let path = latest.join(format!("review-{round}.json"));
        if let Ok(raw) = std::fs::read(path) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) {
                reviews.push(value);
            }
        }
    }
    Ok(serde_json::json!({"path":latest.to_string_lossy(),"reviews":reviews}))
}

#[tauri::command]
pub async fn load_opening_sample(
    app: AppHandle,
    task_id: String,
) -> Result<Option<SampleResult>, String> {
    validate_task_id(&task_id)?;
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|e| e.to_string())?;
    let path = job_paths(&config, &task_id)?
        .root
        .join("samples/latest.json");
    if !path.exists() {
        return Ok(None);
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map(Some)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn generate_opening_sample(
    app: AppHandle,
    task_id: String,
) -> Result<SampleResult, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|e| e.to_string())?;
    let paths = job_paths(&config, &task_id)?;
    ensure_supported_transcript_source(&paths)?;
    let entries = subtitle::parse_file(&paths.transcript).map_err(|e| e.to_string())?;
    let duration = ffmpeg::get_duration(&paths.source).map_err(|e| e.to_string())?;
    let options = director::DirectorOptions {
        provider: "ollama".into(),
        base_url: config.ollama_base_url.clone(),
        api_key: String::new(),
        model: config.ollama_model.clone(),
        title: task_id.clone(),
        style: config.default_style.clone(),
        duration,
        skip_intro_outro: config.skip_intro_outro,
        skip_ads: config.skip_ads,
    };
    sample_progress(&app, &task_id, "样片：正在等待 AI 导演空闲");
    let _permit = app.state::<AppState>().acquire_director().await?;
    sample_progress(
        &app,
        &task_id,
        "样片：依据开头字幕提炼矛盾、重写文案并复核事实（不使用旧稿）",
    );
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let reflection_root = paths
        .root
        .join("samples/reflection")
        .join(format!("run-{stamp}"));
    std::fs::create_dir_all(&reflection_root).map_err(|e| e.to_string())?;
    jobs::atomic_write(reflection_root.join("run.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "status":"running", "provider":options.provider, "model":options.model, "transport":"rig-core-0.42.0",
        "max_repairs":2, "max_llm_calls":7, "transcript_revision":sample::transcript_revision(&entries)
    })).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    let outcome = sample::generate(&entries, &options, &reflection_root, |message| {
        sample_progress(&app, &task_id, message)
    })
    .await;
    jobs::atomic_write(reflection_root.join("outcome.json"),serde_json::to_vec_pretty(&serde_json::json!({
        "status":if outcome.is_ok(){"ready_for_review"}else{"failed"},"error":outcome.as_ref().err()
    })).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    let mut result =
        outcome.map_err(|e| format!("{e}。评审记录：{}", reflection_root.display()))?;
    result.source_revision = sample_source_revision(&paths.source)?;
    let root = paths.root.join("samples");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let raw = serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?;
    jobs::atomic_write(&root.join(format!("draft-{}.json", result.revision)), &raw)
        .map_err(|e| e.to_string())?;
    jobs::atomic_write(&root.join("latest.json"), &raw).map_err(|e| e.to_string())?;
    Ok(result)
}

#[tauri::command]
pub async fn render_opening_sample(
    app: AppHandle,
    task_id: String,
    plan: SamplePlan,
    expected_revision: String,
) -> Result<SampleResult, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|e| e.to_string())?;
    let paths = job_paths(&config, &task_id)?;
    ensure_supported_transcript_source(&paths)?;
    let latest = paths.root.join("samples/latest.json");
    let mut result: SampleResult =
        serde_json::from_slice(&std::fs::read(&latest).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if result.revision != expected_revision {
        return Err("样片稿已有新版本，请重新打开任务后再试".into());
    }
    let entries = subtitle::parse_file(&paths.transcript).map_err(|e| e.to_string())?;
    if result.transcript_revision != sample::transcript_revision(&entries) {
        return Err("字幕已更新，请重新生成样片稿，不能沿用旧字幕选片".into());
    }
    if result.source_revision != sample_source_revision(&paths.source)? {
        return Err("原片文件已变化，请重新生成样片稿".into());
    }
    result.plan = plan;
    sample::validate(&result.plan, &result.shots)?;
    result.output_path = None;
    result.duration_secs = None;
    // Unique run directory preserves previous samples even after voice/settings edits.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let root = paths.root.join("samples").join(format!("run-{stamp}"));
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    result.revision =
        jobs::stable_hash(&[&serde_json::to_vec(&result.plan).map_err(|e| e.to_string())?]);
    jobs::atomic_write(
        &root.join("draft.json"),
        serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    sample_progress(&app, &task_id, "样片：正在等待渲染空闲");
    let _permit = app.state::<AppState>().acquire_render().await?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        sample::render(
            &mut result,
            &paths.source,
            &root,
            &config.tts_voice,
            config.burn_captions,
            config.subtitle_font_size,
            config.source_caption_mask_percent,
            |message| sample_progress(&app, &task_id, message),
        )?;
        let raw = serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?;
        jobs::atomic_write(&root.join("result.json"), &raw).map_err(|e| e.to_string())?;
        jobs::atomic_write(&latest, &raw).map_err(|e| e.to_string())?;
        Ok::<_, String>(result)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(result)
}

fn validate_task_id(task_id: &str) -> Result<(), String> {
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

fn job_paths(config: &AppConfig, task_id: &str) -> Result<JobPaths, String> {
    jobs::migrate_legacy_job(&config.output_dir, task_id)
        .map_err(|error| format!("迁移旧任务到 inbox/{task_id} 失败：{error}"))
}

#[derive(Debug, Serialize, Deserialize)]
struct RenderManifest {
    schema_version: u32,
    edit_revision: String,
    settings_revision: String,
    output_path: String,
}

#[derive(Debug, Serialize)]
pub struct CommentaryExportResult {
    directory: String,
    text_path: String,
    srt_path: Option<String>,
}

fn file_revision(path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read(path).ok()?;
    Some(jobs::stable_hash(&[&raw]))
}

fn render_settings_revision(config: &AppConfig) -> String {
    jobs::stable_hash(&[
        b"paragraph-word-aligned-v5-48khz-stereo-media-contract",
        config.tts_voice.as_bytes(),
        config.original_audio_mode.as_bytes(),
        config.duck_db.to_string().as_bytes(),
        config.burn_captions.to_string().as_bytes(),
        config.source_caption_mask_percent.to_string().as_bytes(),
        config.subtitle_font_size.to_string().as_bytes(),
        config.background_music_path.as_bytes(),
        config.background_music_db.to_string().as_bytes(),
        config.ollama_vision_model.as_bytes(),
    ])
}

fn local_vision_model(configured: &str, director_model: &str, installed: &[String]) -> String {
    if !configured.trim().is_empty() {
        return configured.trim().to_string();
    }
    let looks_visual = |name: &str| {
        let name = name.to_ascii_lowercase();
        [
            "vision",
            "llava",
            "-vl",
            ":vl",
            "minicpm-v",
            "gemma3",
            "mistral-small3.1",
        ]
        .iter()
        .any(|marker| name.contains(marker))
    };
    installed
        .iter()
        .find(|model| looks_visual(model))
        .cloned()
        .unwrap_or_else(|| director_model.trim().to_string())
}

fn output_is_current(paths: &JobPaths, config: &AppConfig) -> bool {
    if !paths.output.exists() || !paths.render_manifest.exists() {
        return false;
    }
    let Some(edit_revision) = file_revision(&paths.edit) else {
        return false;
    };
    let Ok(raw) = std::fs::read(&paths.render_manifest) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<RenderManifest>(&raw) else {
        return false;
    };
    manifest.edit_revision == edit_revision
        && manifest.settings_revision == render_settings_revision(config)
        && std::path::Path::new(&manifest.output_path) == paths.output
}

fn migrate_legacy_render_manifest(paths: &JobPaths, config: &AppConfig) {
    if paths.render_manifest.exists() || !paths.output.exists() || !paths.edit.exists() {
        return;
    }
    let output_modified = std::fs::metadata(&paths.output).and_then(|meta| meta.modified());
    let edit_modified = std::fs::metadata(&paths.edit).and_then(|meta| meta.modified());
    if !matches!((output_modified, edit_modified), (Ok(output), Ok(edit)) if output >= edit) {
        return;
    }
    let Some(edit_revision) = file_revision(&paths.edit) else {
        return;
    };
    let manifest = RenderManifest {
        schema_version: 1,
        edit_revision,
        settings_revision: render_settings_revision(config),
        output_path: paths.output.to_string_lossy().into_owned(),
    };
    if let Ok(raw) = serde_json::to_vec_pretty(&manifest) {
        let _ = jobs::atomic_write(&paths.render_manifest, raw);
    }
}

fn archive_plan(paths: &JobPaths, raw: &[u8]) -> Result<String, String> {
    let revision = jobs::stable_hash(&[raw]);
    let archive = paths.review_dir.join(format!("plan-{revision}.json"));
    if !archive.exists() {
        jobs::atomic_write(archive, raw).map_err(|error| error.to_string())?;
    }
    Ok(revision)
}

fn export_timestamp(seconds: f64) -> String {
    let millis = (seconds.max(0.0) * 1_000.0).round() as u64;
    let hours = millis / 3_600_000;
    let minutes = (millis % 3_600_000) / 60_000;
    let secs = (millis % 60_000) / 1_000;
    let millis = millis % 1_000;
    format!("{hours:02}:{minutes:02}:{secs:02}.{millis:03}")
}

fn commentary_export_text(plan: &EditPlan) -> String {
    let narration_chars = plan
        .segments
        .iter()
        .map(|segment| segment.narration.chars().count())
        .sum::<usize>();
    let mut output = format!(
        "{}\n风格：{}\n共 {} 段 · {} 字\n\n",
        plan.title.trim(),
        if plan.style.trim().is_empty() {
            "未指定"
        } else {
            plan.style.trim()
        },
        plan.segments.len(),
        narration_chars
    );
    for (index, segment) in plan.segments.iter().enumerate() {
        let original_audio = if segment.keep_original_audio {
            " · 接原声"
        } else {
            ""
        };
        output.push_str(&format!(
            "{:02}  原片 {} – {}{}\n{}\n\n",
            index + 1,
            export_timestamp(segment.src_start),
            export_timestamp(segment.src_end),
            original_audio,
            segment.narration.trim()
        ));
    }
    output
}

fn current_commentary_srt(paths: &JobPaths, config: &AppConfig) -> Option<std::path::PathBuf> {
    if output_is_current(paths, config)
        && paths.output_srt.metadata().is_ok_and(|meta| meta.len() > 0)
    {
        return Some(paths.output_srt.clone());
    }
    let pending = paths.root.join("rough-cut.pending.srt");
    let revision = file_revision(&paths.edit)?;
    let matching_quality_review = paths.root.join("quality").join(revision);
    (matching_quality_review.is_dir() && pending.metadata().is_ok_and(|meta| meta.len() > 0))
        .then_some(pending)
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

fn record_transcript_source(
    paths: &JobPaths,
    source: &str,
    source_path: &std::path::Path,
) -> Result<(), String> {
    jobs::atomic_write(
        transcript_source_file(paths),
        serde_json::to_vec_pretty(&serde_json::json!({
            "source": source,
            "source_path": source_path.to_string_lossy(),
            "canonical_path": paths.transcript.to_string_lossy()
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn current_transcript_source(paths: &JobPaths) -> String {
    std::fs::read(transcript_source_file(paths))
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .and_then(|value| value.get("source")?.as_str().map(str::to_string))
        .or_else(|| jobs::infer_subtitle_source(&paths.root))
        .unwrap_or_else(|| "未知来源".to_string())
}

fn ensure_supported_transcript_source(paths: &JobPaths) -> Result<(), String> {
    let source = current_transcript_source(paths);
    if source.starts_with("旧版语音字幕") {
        Err("当前任务仍在使用旧版语音识别字幕。ASR 已从项目移除；请先点击“重新获取最佳字幕”，应用会按平台字幕、视频内封字幕、画面硬字幕 OCR 的顺序重新获取。".to_string())
    } else {
        Ok(())
    }
}

async fn acquire_best_available_subtitle(
    app: &AppHandle,
    paths: &JobPaths,
    task_id: &str,
    video_path: &std::path::Path,
    platform_subtitle: Option<std::path::PathBuf>,
) -> Result<(std::path::PathBuf, &'static str), String> {
    if let Some(path) = platform_subtitle {
        return Ok((path, "平台独立字幕"));
    }

    task_events::emit_progress(
        app,
        Some(task_id),
        "transcribe",
        8.0,
        "平台没有独立字幕，正在检查视频内封字幕...",
    );
    let embedded_video = video_path.to_path_buf();
    let embedded_root = paths.root.clone();
    let embedded = tauri::async_runtime::spawn_blocking(move || {
        ffmpeg::extract_best_embedded_subtitle(&embedded_video, &embedded_root)
    })
    .await
    .map_err(|error| error.to_string())?;
    match embedded {
        Ok(Some(path)) => return Ok((path, "视频内封字幕")),
        Ok(None) => {}
        Err(error) => eprintln!("Embedded subtitle probe failed: {error}"),
    }

    let permit = app.state::<AppState>().acquire_subtitle_ocr().await?;
    let ocr_app = app.clone();
    let ocr_video = video_path.to_path_buf();
    let ocr_root = paths.root.clone();
    let ocr_task = task_id.to_string();
    let ocr = tauri::async_runtime::spawn_blocking(move || {
        hard_subtitle::generate_srt_from_video(&ocr_app, &ocr_video, &ocr_task, &ocr_root)
    })
    .await
    .map_err(|error| error.to_string())?;
    drop(permit);
    match ocr {
        Ok(Some(path)) => Ok((path, "画面硬字幕 OCR")),
        Ok(None) => Err(
            "未能获取字幕：平台没有独立字幕，视频没有可提取的内封文字字幕，画面中也没有检测到足够稳定的硬字幕。项目已禁用语音识别，因此不会从音轨猜写字幕。"
                .to_string(),
        ),
        Err(error) => Err(format!(
            "平台和内封字幕均不可用，本地画面硬字幕 OCR 也失败：{error}。项目已禁用语音识别。"
        )),
    }
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
    let result = prepare_job_inner(app, url, task_id.clone(), style).await;
    if let Err(error) = &result {
        task_events::record_failure(&task_id, "prepare", "prepare_failed", error);
    }
    result
}

async fn prepare_job_inner(
    app: AppHandle,
    url: String,
    task_id: String,
    style: Option<String>,
) -> Result<PrepareResult, String> {
    let config = {
        let state = app.state::<AppState>();
        state.get_config().await.map_err(|e| e.to_string())?
    };

    let style = style
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| config.default_style.clone());
    let paths = job_paths(&config, &task_id)?;
    paths.ensure().map_err(|e| e.to_string())?;

    let cookies = config.cookies_browser.clone();
    let quality = config.video_quality.clone();
    let app_dl = app.clone();
    let task_id_dl = task_id.clone();
    let url_dl = url.clone();
    let job_root = paths.root.clone();

    let download_permit = app.state::<AppState>().acquire_download().await?;
    let download = tauri::async_runtime::spawn_blocking(move || {
        ytdlp::download_into_job(
            &app_dl,
            &url_dl,
            &job_root,
            ytdlp::DownloadOptions {
                cookies_browser: if cookies.is_empty() {
                    None
                } else {
                    Some(cookies)
                },
                video_quality: quality,
                task_id: Some(task_id_dl),
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    drop(download_permit);
    task_store::ensure_task(&task_id, &download.title, &url)?;
    task_store::finish_stage(&task_id, "download", "running", "原片下载完成")?;
    task_store::start_stage(&task_id, "transcribe", "正在获取字幕")?;

    let (subtitle_path, subtitle_source) = acquire_best_available_subtitle(
        &app,
        &paths,
        &task_id,
        &download.video_path,
        download.subtitle_path,
    )
    .await?;

    brief::copy_transcript_into_job(&subtitle_path, &paths.transcript)
        .map_err(|e| e.to_string())?;
    record_transcript_source(&paths, subtitle_source, &subtitle_path)?;
    let entries = subtitle_correction::assemble_transcript(
        &app,
        &config,
        &paths,
        &task_id,
        subtitle_source,
    )
    .await?;
    let transcript_raw = std::fs::read(&paths.transcript).map_err(|error| error.to_string())?;
    let transcript_revision = jobs::stable_hash(&[&transcript_raw]);
    task_store::artifact(
        &task_id,
        "transcript",
        &paths.transcript.to_string_lossy(),
        &transcript_revision,
        "",
    )?;
    task_store::finish_stage(&task_id, "transcribe", "running", "字幕已就绪")?;

    let duration = ffmpeg::get_duration(&download.video_path).unwrap_or(0.0);

    brief::write_brief(
        &paths,
        &task_id,
        &download.title,
        &url,
        duration,
        &style,
        &entries,
    )
    .map_err(|e| e.to_string())?;

    let final_quality = subtitle::assess_quality(&entries);
    if final_quality.needs_retranscription {
        let reason = format!(
            "{}质量检测未通过（{} 分）：{}。已停止 AI 导演；项目不会改用语音识别",
            subtitle_source,
            final_quality.score,
            final_quality.reasons.join("、")
        );
        task_events::emit_progress(&app, Some(task_id.as_str()), "transcribe", 100.0, &reason);
        task_store::finish_stage(&task_id, "transcribe", "awaiting_director", &reason)?;
        let excluded = director::detect_excluded_ranges(
            &entries,
            duration,
            config.skip_intro_outro,
            config.skip_ads,
        );
        return Ok(ingest_waiting(
            &paths,
            &task_id,
            &download.video_path,
            &download.title,
            duration,
            excluded,
            Some(reason),
        ));
    }

    let excluded = director::detect_excluded_ranges(
        &entries,
        duration,
        config.skip_intro_outro,
        config.skip_ads,
    );
    if let Some(result) = try_load_prepared(
        &paths,
        &task_id,
        &download.video_path,
        duration,
        excluded.clone(),
    ) {
        brief::mark_current_ready(&task_id).ok();
        task_events::emit_complete(&app, Some(task_id.as_str()), &paths.edit.to_string_lossy());
        return Ok(result);
    }

    task_events::emit_progress(
        &app,
        Some(task_id.as_str()),
        "director",
        70.0,
        "AI 导演正在理解剧情...",
    );
    let director_permit = app.state::<AppState>().acquire_director().await?;
    let result = run_ai_director(
        &app,
        &config,
        &paths,
        &task_id,
        &download.title,
        &style,
        duration,
        &entries,
    )
    .await;
    drop(director_permit);
    match result {
        Ok(result) => {
            task_events::emit_progress(
                &app,
                Some(task_id.as_str()),
                "director",
                100.0,
                "AI 导演稿已生成，等待审稿",
            );
            task_events::emit_complete(&app, Some(task_id.as_str()), &paths.edit.to_string_lossy());
            Ok(result)
        }
        Err(error) => {
            task_events::record_failure(&task_id, "director", "director_failed", &error);
            task_events::emit_progress(
                &app,
                Some(task_id.as_str()),
                "director",
                100.0,
                format!("AI 导演暂停：{error}"),
            );
            task_events::emit_complete(
                &app,
                Some(task_id.as_str()),
                &paths.brief.to_string_lossy(),
            );
            Ok(ingest_waiting(
                &paths,
                &task_id,
                &download.video_path,
                &download.title,
                duration,
                excluded,
                Some(error),
            ))
        }
    }
}

fn ingest_waiting(
    paths: &JobPaths,
    task_id: &str,
    source: &std::path::Path,
    title: &str,
    duration: f64,
    excluded_ranges: Vec<ExcludedRange>,
    director_error: Option<String>,
) -> PrepareResult {
    PrepareResult {
        task_id: task_id.to_string(),
        job_dir: paths.root.to_string_lossy().into_owned(),
        inbox_dir: paths.brief.to_string_lossy().into_owned(),
        source_path: source.to_string_lossy().into_owned(),
        transcript_path: paths.transcript.to_string_lossy().into_owned(),
        source_available: source.exists(),
        transcript_available: paths.transcript.exists(),
        subtitle_source: current_transcript_source(paths),
        title: title.to_string(),
        duration_secs: duration,
        plan: None,
        commentary: String::new(),
        waiting_for_director: true,
        excluded_ranges,
        director_error,
    }
}

fn try_load_prepared(
    paths: &JobPaths,
    task_id: &str,
    source: &std::path::Path,
    duration: f64,
    excluded_ranges: Vec<ExcludedRange>,
) -> Option<PrepareResult> {
    let mut plan = load_plan(paths).ok()?;
    plan.validate_and_clamp(duration).ok()?;
    let entries = subtitle::parse_file(&paths.transcript).ok()?;
    director::compile_renderable_timeline(&mut plan, &entries, &excluded_ranges, duration);
    plan.validate_and_clamp(duration).ok()?;
    director::validate_existing_plan(&plan, duration, &entries, &excluded_ranges).ok()?;
    let commentary = if paths.commentary.exists() {
        std::fs::read_to_string(&paths.commentary).unwrap_or_else(|_| plan.commentary_markdown())
    } else {
        plan.commentary_markdown()
    };
    Some(PrepareResult {
        task_id: task_id.to_string(),
        job_dir: paths.root.to_string_lossy().into_owned(),
        inbox_dir: paths.brief.to_string_lossy().into_owned(),
        source_path: source.to_string_lossy().into_owned(),
        transcript_path: paths.transcript.to_string_lossy().into_owned(),
        source_available: source.exists(),
        transcript_available: paths.transcript.exists(),
        subtitle_source: current_transcript_source(paths),
        title: plan.title.clone(),
        duration_secs: duration,
        plan: Some(plan),
        commentary,
        waiting_for_director: false,
        excluded_ranges,
        director_error: None,
    })
}

async fn run_ai_director(
    app: &AppHandle,
    config: &AppConfig,
    paths: &JobPaths,
    task_id: &str,
    title: &str,
    style: &str,
    duration: f64,
    entries: &[subtitle::SubtitleEntry],
) -> Result<PrepareResult, String> {
    task_store::start_stage(task_id, "director", "AI 导演正在分析字幕")?;
    let options = director::DirectorOptions {
        provider: "ollama".into(),
        base_url: config.ollama_base_url.clone(),
        api_key: String::new(),
        model: config.ollama_model.clone(),
        title: title.to_string(),
        style: style.to_string(),
        duration,
        skip_intro_outro: config.skip_intro_outro,
        skip_ads: config.skip_ads,
    };
    let (plan, excluded_ranges) =
        director::generate_plan(app, task_id, entries, &options, &paths.director_dir).await?;
    let plan_json = serde_json::to_vec_pretty(&plan).map_err(|error| error.to_string())?;
    jobs::atomic_write(&paths.edit, &plan_json).map_err(|error| error.to_string())?;
    jobs::atomic_write(&paths.commentary, plan.commentary_markdown())
        .map_err(|error| error.to_string())?;
    let revision = archive_plan(paths, &plan_json)?;
    task_store::artifact(
        task_id,
        "edit_plan",
        &paths.edit.to_string_lossy(),
        &revision,
        "",
    )?;
    task_store::finish_stage(task_id, "director", "ready", "导演稿已生成，等待审稿")?;
    brief::mark_current_ready(task_id).ok();
    try_load_prepared(paths, task_id, &paths.source, duration, excluded_ranges)
        .ok_or_else(|| "AI 导演稿已写入，但重新加载失败".to_string())
}

fn load_plan(paths: &JobPaths) -> Result<EditPlan, String> {
    if !paths.edit.exists() {
        return Err("AI 导演尚未生成 edit.json".into());
    }
    let raw = std::fs::read_to_string(&paths.edit).map_err(|e| e.to_string())?;
    EditPlan::from_json_str(&raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_existing_jobs(app: AppHandle) -> Result<Vec<ExistingJob>, String> {
    let config = {
        let state = app.state::<AppState>();
        state
            .get_config()
            .await
            .map_err(|error| error.to_string())?
    };
    jobs::migrate_all_legacy_jobs(&config.output_dir).map_err(|error| error.to_string())?;
    let jobs_root = jobs::inbox_root();
    if !jobs_root.exists() {
        return Ok(Vec::new());
    }

    let mut restored = Vec::new();
    let directories = std::fs::read_dir(&jobs_root).map_err(|error| error.to_string())?;
    for entry in directories.flatten() {
        let root = entry.path();
        if !root.is_dir() {
            continue;
        }
        let Some(task_id) = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        if validate_task_id(&task_id).is_err() {
            continue;
        }
        let paths = job_paths(&config, &task_id)?;
        let meta = std::fs::read_to_string(&paths.meta)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .unwrap_or_default();
        let stored_before = task_store::get(&task_id)?;
        let fallback_title = meta
            .get("title")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .or_else(|| {
                stored_before
                    .as_ref()
                    .map(|task| task.title.clone())
                    .filter(|title| !title.is_empty())
            })
            .unwrap_or_else(|| task_id.clone());
        let meta_url = meta
            .get("url")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        task_store::ensure_task(&task_id, &fallback_title, &meta_url)?;
        let duration = if paths.source.exists() {
            ffmpeg::get_duration(&paths.source).unwrap_or(0.0)
        } else {
            0.0
        };
        let entries = if paths.transcript.exists() {
            subtitle::parse_file(&paths.transcript).unwrap_or_default()
        } else {
            Vec::new()
        };
        let excluded = director::detect_excluded_ranges(
            &entries,
            duration,
            config.skip_intro_outro,
            config.skip_ads,
        );
        let prepared =
            try_load_prepared(&paths, &task_id, &paths.source, duration, excluded.clone());
        migrate_legacy_render_manifest(&paths, &config);
        let render_is_current = output_is_current(&paths, &config);
        let stored = task_store::get(&task_id)?;
        let stored_running = stored.as_ref().is_some_and(|task| task.status == "running");
        let stored_failed = stored.as_ref().is_some_and(|task| task.status == "failed");
        let (status, error) = if stored_running {
            ("running", None)
        } else if render_is_current && prepared.is_some() {
            ("completed", None)
        } else if stored_failed {
            (
                "failed",
                stored.as_ref().and_then(|task| task.error_message.clone()),
            )
        } else if prepared.is_some() {
            (
                "ready",
                stored.as_ref().and_then(|task| task.error_message.clone()),
            )
        } else if paths.transcript.exists() {
            (
                "awaiting_director",
                stored
                    .as_ref()
                    .and_then(|task| task.error_message.clone())
                    .or_else(|| {
                        paths.edit.exists().then(|| {
                            "旧导演稿存在元话语、内容过薄或其他新版质量问题，请重新生成".to_string()
                        })
                    }),
            )
        } else {
            (
                "failed",
                Some(if paths.source.exists() {
                    "历史任务的字幕尚未完成，可从失败阶段重试".to_string()
                } else {
                    "视频下载尚未完成，可使用原链接重试".to_string()
                }),
            )
        };
        let prepare = prepared.unwrap_or_else(|| {
            ingest_waiting(
                &paths,
                &task_id,
                &paths.source,
                &fallback_title,
                duration,
                excluded,
                error.clone(),
            )
        });
        let title = prepare.title.clone();
        let output_path = render_is_current.then(|| paths.output.to_string_lossy().into_owned());
        let updated_at_ms = [
            paths.output.as_path(),
            paths.edit.as_path(),
            paths.source.as_path(),
        ]
        .into_iter()
        .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
        .filter_map(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .max()
        .unwrap_or(0)
        .max(stored.as_ref().map(|task| task.updated_at_ms).unwrap_or(0));

        restored.push(ExistingJob {
            task_id,
            title,
            status: status.to_string(),
            prepare,
            output_path,
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
        });
    }
    restored.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    Ok(restored)
}

#[tauri::command]
pub async fn read_job_transcript(app: AppHandle, task_id: String) -> Result<String, String> {
    validate_task_id(&task_id)?;
    let config = {
        let state = app.state::<AppState>();
        state
            .get_config()
            .await
            .map_err(|error| error.to_string())?
    };
    let paths = job_paths(&config, &task_id)?;
    std::fs::read_to_string(&paths.transcript).map_err(|error| format!("读取原字幕失败：{error}"))
}

#[tauri::command]
pub async fn refetch_platform_subtitle_and_generate(
    app: AppHandle,
    task_id: String,
) -> Result<PrepareResult, String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = {
        let state = app.state::<AppState>();
        state
            .get_config()
            .await
            .map_err(|error| error.to_string())?
    };
    let paths = job_paths(&config, &task_id)?;
    if !paths.source.exists() {
        return Err("任务原片不存在，无法拉取平台字幕".into());
    }
    task_store::start_stage(&task_id, "download_subtitle", "正在获取最佳字幕")?;

    let meta = std::fs::read_to_string(&paths.meta)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_default();
    let url = meta
        .get("url")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "任务缺少原始播放地址，无法拉取平台字幕".to_string())?
        .to_string();
    let title = meta
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or(&task_id)
        .to_string();
    let style = meta
        .get("style")
        .and_then(|value| value.as_str())
        .unwrap_or(&config.default_style)
        .to_string();

    let app_download = app.clone();
    let job_root = paths.root.clone();
    let task_id_download = task_id.clone();
    let cookies_browser =
        (!config.cookies_browser.is_empty()).then(|| config.cookies_browser.clone());
    let video_quality = config.video_quality.clone();
    let download_permit = app.state::<AppState>().acquire_download().await?;
    let subtitle_result = tauri::async_runtime::spawn_blocking(move || {
        ytdlp::download_subtitles_into_job(
            &app_download,
            &url,
            &job_root,
            ytdlp::DownloadOptions {
                cookies_browser,
                video_quality,
                task_id: Some(task_id_download),
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?;
    drop(download_permit);
    let platform_subtitle = match subtitle_result {
        Ok(path) => path,
        Err(error) => {
            // A platform-only subtitle request may fail even though the already
            // downloaded source remains usable. Continue with embedded text and
            // local picture OCR instead of turning that recoverable miss into a
            // failed task.
            task_events::emit_progress(
                &app,
                Some(task_id.as_str()),
                "download_subtitle",
                8.0,
                format!("平台字幕获取失败，正在继续检查视频内封字幕：{}", error),
            );
            None
        }
    };
    let (subtitle_path, subtitle_source) =
        acquire_best_available_subtitle(&app, &paths, &task_id, &paths.source, platform_subtitle)
            .await?;

    brief::copy_transcript_into_job(&subtitle_path, &paths.transcript)
        .map_err(|error| format!("保存字幕失败：{error}"))?;
    record_transcript_source(&paths, subtitle_source, &subtitle_path)?;
    let entries = subtitle_correction::assemble_transcript(
        &app,
        &config,
        &paths,
        &task_id,
        subtitle_source,
    )
    .await?;
    jobs::atomic_write(
        &paths.compact,
        subtitle::compact_transcript(&entries, 60_000),
    )
    .map_err(|error| format!("更新导演字幕摘要失败：{error}"))?;
    jobs::atomic_write(
        &paths.normalized_transcript,
        serde_json::to_vec_pretty(&entries).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("更新标准化字幕失败：{error}"))?;

    for stale in [&paths.edit, &paths.commentary, &paths.render_manifest] {
        if stale.exists() {
            std::fs::remove_file(stale).map_err(|error| format!("清理旧导演稿失败：{error}"))?;
        }
    }
    if paths.director_dir.exists() {
        std::fs::remove_dir_all(&paths.director_dir)
            .map_err(|error| format!("清理旧导演检查点失败：{error}"))?;
        std::fs::create_dir_all(&paths.director_dir).map_err(|error| error.to_string())?;
    }
    let transcript_raw = std::fs::read(&paths.transcript).map_err(|error| error.to_string())?;
    let transcript_revision = jobs::stable_hash(&[&transcript_raw]);
    task_store::artifact(
        &task_id,
        "transcript",
        &paths.transcript.to_string_lossy(),
        &transcript_revision,
        "",
    )?;
    task_store::finish_stage(
        &task_id,
        "download_subtitle",
        "running",
        &format!("{subtitle_source}已保存"),
    )?;
    brief::refresh_inbox_index().map_err(|error| error.to_string())?;

    let quality = subtitle::assess_quality(&entries);
    if quality.needs_retranscription {
        let error = format!(
            "{}已保存，但质量检测未通过（{} 分）：{}。已停止 AI 导演；项目不会使用语音识别",
            subtitle_source,
            quality.score,
            quality.reasons.join("、")
        );
        task_events::record_failure(&task_id, "download_subtitle", "subtitle_quality", &error);
        return Err(error);
    }

    let duration = ffmpeg::get_duration(&paths.source).map_err(|error| error.to_string())?;
    task_events::emit_progress(
        &app,
        Some(task_id.as_str()),
        "director",
        70.0,
        format!("{subtitle_source}已保存，正在重新生成 AI 导演稿..."),
    );
    let director_permit = app.state::<AppState>().acquire_director().await?;
    let result = run_ai_director(
        &app, &config, &paths, &task_id, &title, &style, duration, &entries,
    )
    .await;
    drop(director_permit);
    let result = result.map_err(|error| {
        task_events::record_failure(&task_id, "director", "director_failed", &error);
        error
    })?;
    task_events::emit_progress(
        &app,
        Some(task_id.as_str()),
        "director",
        100.0,
        "平台字幕和 AI 导演稿已重新生成",
    );
    task_events::emit_complete(&app, Some(task_id.as_str()), &paths.edit.to_string_lossy());
    Ok(result)
}

#[tauri::command]
pub async fn delete_job(app: AppHandle, task_id: String) -> Result<(), String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = {
        let state = app.state::<AppState>();
        state
            .get_config()
            .await
            .map_err(|error| error.to_string())?
    };
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
    // CURRENT.md is a derived compatibility file. Its cleanup must not turn a
    // completed destructive operation into a UI-visible failure.
    let _ = brief::refresh_inbox_index();
    Ok(())
}

#[tauri::command]
pub async fn save_edit_plan(
    app: AppHandle,
    task_id: String,
    plan: EditPlan,
) -> Result<EditPlan, String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = {
        let state = app.state::<AppState>();
        state.get_config().await.map_err(|e| e.to_string())?
    };
    let paths = job_paths(&config, &task_id)?;
    let duration = ffmpeg::get_duration(&paths.source).unwrap_or(0.0);
    let mut plan = plan;
    plan.validate_and_clamp(duration)
        .map_err(|e| e.to_string())?;
    let entries = subtitle::parse_file(&paths.transcript).map_err(|error| error.to_string())?;
    let excluded = director::detect_excluded_ranges(
        &entries,
        duration,
        config.skip_intro_outro,
        config.skip_ads,
    );
    director::compile_renderable_timeline(&mut plan, &entries, &excluded, duration);
    plan.validate_and_clamp(duration)
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&paths.brief).map_err(|e| e.to_string())?;
    let raw = serde_json::to_vec_pretty(&plan).map_err(|e| e.to_string())?;
    jobs::atomic_write(&paths.edit, &raw).map_err(|e| e.to_string())?;
    jobs::atomic_write(&paths.commentary, plan.commentary_markdown()).map_err(|e| e.to_string())?;
    let revision = archive_plan(&paths, &raw)?;
    task_store::artifact(
        &task_id,
        "edit_plan",
        &paths.edit.to_string_lossy(),
        &revision,
        "manual_review",
    )?;
    task_store::finish_stage(&task_id, "review", "ready", "人工审稿已保存")?;
    Ok(plan)
}

#[tauri::command]
pub async fn export_commentary_script(
    app: AppHandle,
    task_id: String,
) -> Result<CommentaryExportResult, String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|error| error.to_string())?;
    let paths = job_paths(&config, &task_id)?;
    let plan = load_plan(&paths)?;
    let exports = paths.root.join("exports");
    std::fs::create_dir_all(&exports).map_err(|error| format!("创建导出目录失败：{error}"))?;

    let text_path = exports.join("解说文稿.txt");
    jobs::atomic_write(&text_path, commentary_export_text(&plan))
        .map_err(|error| format!("导出解说文稿失败：{error}"))?;

    let srt_destination = exports.join("解说字幕.srt");
    let srt_path = if let Some(source) = current_commentary_srt(&paths, &config) {
        let bytes = std::fs::read(&source).map_err(|error| format!("读取解说字幕失败：{error}"))?;
        jobs::atomic_write(&srt_destination, bytes)
            .map_err(|error| format!("导出解说字幕失败：{error}"))?;
        Some(srt_destination)
    } else {
        if srt_destination.exists() {
            std::fs::remove_file(&srt_destination)
                .map_err(|error| format!("清理旧版解说字幕失败：{error}"))?;
        }
        None
    };

    Ok(CommentaryExportResult {
        directory: exports.to_string_lossy().into_owned(),
        text_path: text_path.to_string_lossy().into_owned(),
        srt_path: srt_path.map(|path| path.to_string_lossy().into_owned()),
    })
}

#[tauri::command]
pub async fn load_job(app: AppHandle, task_id: String) -> Result<PrepareResult, String> {
    validate_task_id(&task_id)?;
    let config = {
        let state = app.state::<AppState>();
        state.get_config().await.map_err(|e| e.to_string())?
    };
    let paths = job_paths(&config, &task_id)?;
    let duration = ffmpeg::get_duration(&paths.source).unwrap_or(0.0);
    let entries = subtitle::parse_file(&paths.transcript).map_err(|error| error.to_string())?;
    let excluded = director::detect_excluded_ranges(
        &entries,
        duration,
        config.skip_intro_outro,
        config.skip_ads,
    );
    if let Some(result) =
        try_load_prepared(&paths, &task_id, &paths.source, duration, excluded.clone())
    {
        brief::mark_current_ready(&task_id).ok();
        return Ok(result);
    }
    let title = std::fs::read_to_string(&paths.meta)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| task_id.clone());
    Ok(ingest_waiting(
        &paths,
        &task_id,
        &paths.source,
        &title,
        duration,
        excluded,
        None,
    ))
}

#[tauri::command]
pub async fn generate_director_plan(
    app: AppHandle,
    task_id: String,
) -> Result<PrepareResult, String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = {
        let state = app.state::<AppState>();
        state
            .get_config()
            .await
            .map_err(|error| error.to_string())?
    };
    let paths = job_paths(&config, &task_id)?;
    ensure_supported_transcript_source(&paths)?;
    let duration = ffmpeg::get_duration(&paths.source).map_err(|error| error.to_string())?;
    let entries = subtitle_correction::assemble_transcript(
        &app,
        &config,
        &paths,
        &task_id,
        &current_transcript_source(&paths),
    )
    .await?;
    let meta = std::fs::read_to_string(&paths.meta)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_default();
    let title = meta
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or(&task_id)
        .to_string();
    let style = meta
        .get("style")
        .and_then(|value| value.as_str())
        .unwrap_or(&config.default_style)
        .to_string();
    let excluded = director::detect_excluded_ranges(
        &entries,
        duration,
        config.skip_intro_outro,
        config.skip_ads,
    );
    let existing = try_load_prepared(&paths, &task_id, &paths.source, duration, excluded.clone());
    if let Some(existing_plan) = existing.as_ref().and_then(|prepared| prepared.plan.clone()) {
        let has_blocking_picture = edit_timing::reserve_issues(&existing_plan)
            .into_iter()
            .any(|issue| issue.severity == edit_timing::ReserveSeverity::Blocking);
        if has_blocking_picture {
            task_events::emit_progress(
                &app,
                Some(task_id.as_str()),
                "director",
                96.0,
                "正在保留现有导演稿，自动扩选画面并压缩超时口播...",
            );
            let options = director::DirectorOptions {
                provider: "ollama".into(),
                base_url: config.ollama_base_url.clone(),
                api_key: String::new(),
                model: config.ollama_model.clone(),
                title: title.clone(),
                style: style.clone(),
                duration,
                skip_intro_outro: config.skip_intro_outro,
                skip_ads: config.skip_ads,
            };
            let director_permit = app.state::<AppState>().acquire_director().await?;
            let repaired =
                director::repair_existing_picture_fit(existing_plan, &entries, &options, &excluded)
                    .await;
            drop(director_permit);
            let repaired = repaired.map_err(|error| {
                task_events::record_failure(&task_id, "director", "director_failed", &error);
                error
            })?;
            if let Ok(raw) = std::fs::read(&paths.edit) {
                let _ = archive_plan(&paths, &raw);
            }
            let raw = serde_json::to_vec_pretty(&repaired).map_err(|error| error.to_string())?;
            jobs::atomic_write(&paths.edit, &raw).map_err(|error| error.to_string())?;
            jobs::atomic_write(&paths.commentary, repaired.commentary_markdown())
                .map_err(|error| error.to_string())?;
            if paths.render_manifest.exists() {
                std::fs::remove_file(&paths.render_manifest).map_err(|error| error.to_string())?;
            }
            let revision = archive_plan(&paths, &raw)?;
            task_store::artifact(
                &task_id,
                "edit_plan",
                &paths.edit.to_string_lossy(),
                &revision,
                "picture_fit_repair",
            )?;
            task_store::finish_stage(
                &task_id,
                "director",
                "ready",
                "画面与口播已自动适配，等待审稿",
            )?;
            return try_load_prepared(&paths, &task_id, &paths.source, duration, excluded)
                .ok_or_else(|| "画面与口播已修复，但重新加载失败".to_string());
        }
    }
    // A successful existing plan means the user explicitly requested a fresh
    // creative pass. Failed/incomplete runs keep their checkpoints and resume.
    if existing.is_some() {
        if let Ok(raw) = std::fs::read(&paths.edit) {
            let _ = archive_plan(&paths, &raw);
        }
        if paths.director_dir.exists() {
            std::fs::remove_dir_all(&paths.director_dir)
                .map_err(|error| format!("清理上一版导演检查点失败：{error}"))?;
        }
        std::fs::create_dir_all(&paths.director_dir).map_err(|error| error.to_string())?;
        for stale in [&paths.edit, &paths.commentary, &paths.render_manifest] {
            if stale.exists() {
                std::fs::remove_file(stale).map_err(|error| error.to_string())?;
            }
        }
    }
    task_events::emit_progress(
        &app,
        Some(task_id.as_str()),
        "director",
        70.0,
        "字幕已存在，正在直接重新生成 AI 导演稿...",
    );
    let director_permit = app.state::<AppState>().acquire_director().await?;
    let result = run_ai_director(
        &app, &config, &paths, &task_id, &title, &style, duration, &entries,
    )
    .await;
    drop(director_permit);
    result.map_err(|error| {
        task_events::record_failure(&task_id, "director", "director_failed", &error);
        error
    })
}

#[tauri::command]
pub async fn render_job(app: AppHandle, task_id: String) -> Result<RenderResult, String> {
    validate_task_id(&task_id)?;
    let _task_guard = app.state::<AppState>().lock_task(&task_id).await;
    let config = {
        let state = app.state::<AppState>();
        state.get_config().await.map_err(|e| e.to_string())?
    };
    let paths = job_paths(&config, &task_id)?;
    ensure_supported_transcript_source(&paths)?;
    let duration = ffmpeg::get_duration(&paths.source).map_err(|e| e.to_string())?;
    let mut plan = load_plan(&paths)?;
    let source_subtitles = subtitle::parse_file(&paths.transcript).map_err(|e| e.to_string())?;
    let excluded = director::detect_excluded_ranges(
        &source_subtitles,
        duration,
        config.skip_intro_outro,
        config.skip_ads,
    );
    task_store::start_stage(&task_id, "render", "正在编译并校验整片时间轴")?;
    let before_compile = serde_json::to_vec(&plan).map_err(|error| error.to_string())?;
    let compile_report =
        director::compile_renderable_timeline(&mut plan, &source_subtitles, &excluded, duration);
    let raw = serde_json::to_vec_pretty(&plan).map_err(|error| error.to_string())?;
    if before_compile != serde_json::to_vec(&plan).map_err(|error| error.to_string())? {
        jobs::atomic_write(&paths.edit, &raw).map_err(|error| error.to_string())?;
        jobs::atomic_write(&paths.commentary, plan.commentary_markdown())
            .map_err(|error| error.to_string())?;
        archive_plan(&paths, &raw)?;
    }
    if compile_report.changed() {
        task_events::emit_progress(
            &app,
            Some(&task_id),
            "render",
            0.0,
            format!(
                "时间轴已自动适配：{} 段原声交接前移，{} 段改为纯解说，{} 段口播压缩，{} 个无法承载口播的碎片已移除",
                compile_report.realigned_original_audio.len(),
                compile_report.downgraded_original_audio.len(),
                compile_report.shortened_narration.len(),
                compile_report.dropped_unrenderable.len()
            ),
        );
    }
    plan.validate_and_clamp(duration)
        .map_err(|e| e.to_string())?;
    // Fail before spending money and time on TTS: a paragraph whose pictures cannot
    // even cover the estimated voice track can never be built at native speed.
    let blocking: Vec<_> = edit_timing::reserve_issues(&plan)
        .into_iter()
        .filter(|issue| issue.severity == edit_timing::ReserveSeverity::Blocking)
        .collect();
    if !blocking.is_empty() {
        return Err(edit_timing::blocking_reserve_message(&blocking));
    }
    task_store::finish_stage(
        &task_id,
        "render",
        "running",
        "时间轴校验通过，等待配音和渲染资源",
    )?;

    let voice = config.tts_voice.clone();
    let orig_volume = config.original_volume();
    let mask_percent = config.source_caption_mask_percent.min(40);
    let burn = config.burn_captions || mask_percent > 0;
    let font_size = config.subtitle_font_size;
    let background_music = (!config.background_music_path.is_empty())
        .then(|| std::path::PathBuf::from(&config.background_music_path));
    let background_music_db = config.background_music_db;
    let app_r = app.clone();
    let tid = task_id.clone();
    let source = paths.source.clone();
    let tts_dir = paths.tts_dir.clone();
    let clips_dir = paths.clips_dir.clone();
    let output = paths.root.join("rough-cut.pending.mp4");
    let output_srt = paths.root.join("rough-cut.pending.srt");

    let title = plan.title.clone();

    let edit_revision =
        file_revision(&paths.edit).ok_or_else(|| "无法计算导演稿版本".to_string())?;
    let settings_revision = render_settings_revision(&config);
    let render_permit = app.state::<AppState>().acquire_render().await?;
    let render_result = tauri::async_runtime::spawn_blocking(move || {
        render_inner(
            &app_r,
            &tid,
            &plan,
            &source,
            &tts_dir,
            &clips_dir,
            &output,
            &output_srt,
            &voice,
            orig_volume,
            burn,
            font_size,
            mask_percent,
            background_music.as_deref(),
            background_music_db,
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    drop(render_permit);
    if let Err(error) = render_result {
        task_events::record_failure(&task_id, "render", "render_failed", &error);
        return Err(error);
    }
    let pending_output = paths.root.join("rough-cut.pending.mp4");
    let pending_srt = paths.root.join("rough-cut.pending.srt");
    let rough_duration = match ffmpeg::get_duration(&pending_output) {
        Ok(duration) => duration,
        Err(error) => {
            let error = format!("粗剪无法播放：{error}");
            task_events::record_failure(&task_id, "quality_review", "render_failed", &error);
            return Err(error);
        }
    };
    let rough_size = std::fs::metadata(&pending_output)
        .map_err(|error| format!("无法读取粗剪文件：{error}"))?
        .len();
    if rough_duration <= 0.1 || rough_size < 1_024 {
        let error = "粗剪文件为空或无法正常播放，未发布损坏文件".to_string();
        task_events::record_failure(&task_id, "quality_review", "render_failed", &error);
        return Err(error);
    }
    task_events::emit_progress(
        &app,
        Some(&task_id),
        "quality_review",
        96.0,
        "正在本机检查静止画面、黑场和静音...",
    );
    let quality_failure = |error: String| {
        task_events::record_failure(&task_id, "quality_review", "render_failed", &error);
        error
    };
    let quality_root = paths.root.join("quality").join(&edit_revision);
    std::fs::create_dir_all(&quality_root)
        .map_err(|e| quality_failure(format!("无法保存本地质量报告：{e}")))?;
    let mut quality_warnings = Vec::<String>::new();
    let commentary_captions = subtitle::parse_file(&pending_srt)
        .map_err(|error| quality_failure(format!("无法读取粗剪解说字幕进行音轨核验：{error}")))?;
    match crate::services::technical_review::review(&pending_output) {
        Ok(technical) => {
            jobs::atomic_write(
                quality_root.join("technical-review.json"),
                serde_json::to_vec_pretty(&technical).map_err(|e| e.to_string())?,
            )
            .map_err(|e| quality_failure(format!("无法保存本地技术检查：{e}")))?;
            let technical_failures = technical.blocking_reasons_for_narration(&commentary_captions);
            if !technical_failures.is_empty() {
                return Err(quality_failure(format!(
                    "本地粗剪技术检查未通过：{}。粗剪和报告已保留，未覆盖已有成片",
                    technical_failures.join("；")
                )));
            }
            quality_warnings.extend(technical.warnings.clone());
        }
        Err(error) => {
            quality_warnings.push(format!(
                "技术分析器不可用，但粗剪文件已通过可播放性检查：{error}"
            ));
            jobs::atomic_write(
                quality_root.join("technical-review-unavailable.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"warning": error, "publish_blocked": false}),
                )
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| quality_failure(format!("无法保存本地技术检查警告：{e}")))?;
        }
    }
    task_events::emit_progress(
        &app,
        Some(&task_id),
        "quality_review",
        98.0,
        "正在使用本机 Ollama 审查粗剪画面与解说是否对应...",
    );
    let local_models = director::check_ollama(&config.ollama_base_url).await;
    let vision_model = local_vision_model(
        &config.ollama_vision_model,
        &config.ollama_model,
        &local_models.models,
    );
    let review_options = director::DirectorOptions {
        provider: "ollama".into(),
        base_url: config.ollama_base_url.clone(),
        api_key: String::new(),
        model: vision_model,
        title: title.clone(),
        style: config.default_style.clone(),
        duration: rough_duration,
        skip_intro_outro: false,
        skip_ads: false,
    };
    match crate::services::visual_review::review(
        &pending_output,
        &pending_srt,
        &review_options,
        &quality_root,
    )
    .await
    {
        Ok(visual)
            if visual.verdict == "revise"
                && visual.issues.iter().any(|issue| issue.severity != "low") =>
        {
            let summary = visual
                .issues
                .iter()
                .filter(|i| i.severity != "low")
                .take(3)
                .map(|i| format!("样本{}：{}", i.sample_index + 1, i.reason))
                .collect::<Vec<_>>()
                .join("；");
            quality_warnings.push(format!("视觉抽检建议人工复审：{summary}"));
        }
        Ok(_) => {}
        Err(error) => {
            quality_warnings.push(format!("本地视觉模型评审不可用：{error}"));
            jobs::atomic_write(
                quality_root.join("visual-review-unavailable.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"warning": error, "publish_blocked": false}),
                )
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| quality_failure(format!("无法保存本地视觉检查警告：{e}")))?;
        }
    }
    let published_with_warnings = !quality_warnings.is_empty();
    jobs::atomic_write(
        quality_root.join("publish-decision.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "playable": true,
            "published": true,
            "warnings": &quality_warnings
        }))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| quality_failure(format!("无法保存发布决策：{e}")))?;
    std::fs::rename(&pending_output, &paths.output)
        .map_err(|e| quality_failure(format!("粗剪通过检查但发布成片失败：{e}")))?;
    std::fs::rename(&pending_srt, &paths.output_srt)
        .map_err(|e| quality_failure(format!("成片已发布但解说字幕发布失败：{e}")))?;
    task_events::emit_progress(
        &app,
        Some(&task_id),
        "render",
        100.0,
        if published_with_warnings {
            "成片已发布；本地评审警告已保存，可在质量报告中查看"
        } else {
            "本地质量检查通过，成片已发布"
        },
    );
    let manifest = RenderManifest {
        schema_version: 1,
        edit_revision: edit_revision.clone(),
        settings_revision,
        output_path: paths.output.to_string_lossy().into_owned(),
    };
    let manifest_raw = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    jobs::atomic_write(&paths.render_manifest, manifest_raw).map_err(|error| error.to_string())?;
    let versioned_output = paths.renders_dir.join(&edit_revision).join("output.mp4");
    if !versioned_output.exists() {
        if let Some(parent) = versioned_output.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        if std::fs::hard_link(&paths.output, &versioned_output).is_err() {
            std::fs::copy(&paths.output, &versioned_output).map_err(|error| error.to_string())?;
        }
    }
    let output_revision = file_revision(&paths.output).unwrap_or_default();
    task_store::artifact(
        &task_id,
        "render",
        &versioned_output.to_string_lossy(),
        &output_revision,
        &edit_revision,
    )?;

    task_events::emit_complete(
        &app,
        Some(task_id.as_str()),
        &paths.output.to_string_lossy(),
    );

    Ok(RenderResult {
        task_id,
        output_path: paths.output.to_string_lossy().into_owned(),
        title,
    })
}

fn render_inner(
    app: &AppHandle,
    task_id: &str,
    plan: &EditPlan,
    source: &std::path::Path,
    tts_dir: &std::path::Path,
    clips_dir: &std::path::Path,
    output: &std::path::Path,
    output_srt: &std::path::Path,
    voice: &str,
    orig_volume: f64,
    burn: bool,
    font_size: u32,
    mask_percent: u32,
    background_music: Option<&std::path::Path>,
    background_music_db: f64,
) -> Result<(), String> {
    std::fs::create_dir_all(tts_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(clips_dir).map_err(|e| e.to_string())?;

    let (frame_w, frame_h) = ffmpeg::get_dimensions(source).map_err(|e| e.to_string())?;
    let source_metadata = std::fs::metadata(source).map_err(|error| error.to_string())?;
    let source_modified = source_metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let source_revision = jobs::stable_hash(&[
        source.to_string_lossy().as_bytes(),
        source_metadata.len().to_string().as_bytes(),
        source_modified.to_string().as_bytes(),
    ]);
    let total = plan.segments.len().max(1);
    let mut clips = Vec::new();
    let mut srt_entries = Vec::new();
    let mut cursor = 0.0_f64;

    let source_subtitles = subtitle::parse_file(
        &source
            .parent()
            .ok_or("素材目录无效")?
            .join("TRANSCRIPT.srt"),
    )
    .map_err(|e| e.to_string())?;

    for (i, seg) in plan.segments.iter().enumerate() {
        let pct = (i as f64 / total as f64) * 70.0;
        task_events::emit_progress(
            app,
            Some(task_id),
            "tts",
            pct,
            format!("配音片段 {}/{}", i + 1, total),
        );

        let audio_key = jobs::stable_hash(&[seg.narration.as_bytes(), voice.as_bytes()]);
        let audio_base = tts_dir.join(&audio_key);
        let audio = tts::cached_or_synthesize(&seg.narration, voice, &audio_base)
            .map_err(|e| format!("第 {} 段配音失败：{e}", i + 1))?;
        // A repaired or fallback voice must not reuse a clip made from different audio.
        let audio_revision =
            file_revision(&audio).ok_or_else(|| "无法读取配音文件版本".to_string())?;
        let spoken_duration = ffmpeg::get_duration(&audio).map_err(|e| e.to_string())?;
        let handoff = if seg.keep_original_audio {
            crate::services::edit_timing::dialogue_handoff(
                &source_subtitles,
                seg.src_start,
                seg.src_end,
                spoken_duration,
            )
            .ok()
        } else {
            None
        };
        let use_original_audio = seg.keep_original_audio && handoff.is_some();
        if seg.keep_original_audio && !use_original_audio {
            task_events::emit_progress(
                app,
                Some(task_id),
                "render",
                pct,
                format!(
                    "第 {} 段实测配音后原声对白空间不足，已自动改为纯解说",
                    i + 1
                ),
            );
        }
        let pictures = crate::services::edit_timing::pictures(seg, spoken_duration)
            .map_err(|e| format!("第 {} 段：{e}", i + 1))?;
        let picture_key = serde_json::to_vec(&pictures).map_err(|e| e.to_string())?;

        task_events::emit_progress(
            app,
            Some(task_id),
            "render",
            pct + 5.0,
            format!("剪辑片段 {}/{}", i + 1, total),
        );

        let clip_key = jobs::stable_hash(&[
            b"paragraph-word-aligned-v5-media-contract",
            &picture_key,
            format!("{handoff:?}").as_bytes(),
            audio_key.as_bytes(),
            audio_revision.as_bytes(),
            source_revision.as_bytes(),
            seg.src_start.to_string().as_bytes(),
            seg.src_end.to_string().as_bytes(),
            orig_volume.to_string().as_bytes(),
            use_original_audio.to_string().as_bytes(),
            frame_w.to_string().as_bytes(),
            frame_h.to_string().as_bytes(),
        ]);
        let clip_path = clips_dir.join(format!("{clip_key}.mp4"));
        let staged_clip = clips_dir.join(format!("{clip_key}.pending.mp4"));
        let reusable_clip = clip_path.exists() && ffmpeg::has_render_audio_contract(&clip_path);
        if clip_path.exists() && !reusable_clip {
            std::fs::remove_file(&clip_path)
                .map_err(|error| format!("无法清理音频格式不一致的旧剪辑缓存：{error}"))?;
        }
        let audio_dur = if reusable_clip {
            ffmpeg::get_duration(&clip_path).map_err(|e| e.to_string())?
        } else if !use_original_audio {
            ffmpeg::narration_montage(
                source,
                &pictures,
                &audio,
                &staged_clip,
                spoken_duration,
                frame_w,
                frame_h,
                orig_volume,
            )
            .map_err(|e| format!("第 {} 段：{e}", i + 1))?;
            if !ffmpeg::has_render_audio_contract(&staged_clip) {
                return Err(format!(
                    "第 {} 段剪辑音频未达到统一的 48kHz 双声道格式，已停止缓存",
                    i + 1
                ));
            }
            let measured = ffmpeg::get_duration(&staged_clip).map_err(|e| e.to_string())?;
            std::fs::rename(&staged_clip, &clip_path).map_err(|e| e.to_string())?;
            measured
        } else {
            let measured = ffmpeg::fit_clip_to_narration(
                source,
                seg.src_start,
                seg.src_end,
                &audio,
                &staged_clip,
                orig_volume,
                use_original_audio,
                frame_w,
                frame_h,
                handoff,
            )
            .map_err(|e| {
                format!(
                    "第 {} 段（原片 {:.2}–{:.2} 秒）：{e}",
                    i + 1,
                    seg.src_start,
                    seg.src_end
                )
            })?;
            if !ffmpeg::has_render_audio_contract(&staged_clip) {
                return Err(format!(
                    "第 {} 段原声剪辑音频未达到统一的 48kHz 双声道格式，已停止缓存",
                    i + 1
                ));
            }
            ffmpeg::get_duration(&staged_clip).map_err(|e| e.to_string())?;
            std::fs::rename(&staged_clip, &clip_path).map_err(|e| e.to_string())?;
            measured
        };

        let words = tts::read_word_boundaries(&audio).map_err(|e| e.to_string())?;
        srt_entries.extend(crate::services::edit_timing::aligned_captions(
            &words,
            cursor,
            srt_entries.len() as u32 + 1,
        ));
        cursor += audio_dur;
        clips.push(clip_path);
    }

    subtitle::write_srt(&srt_entries, output_srt).map_err(|e| e.to_string())?;

    task_events::emit_progress(app, Some(task_id), "render", 80.0, "正在拼接成片...");
    let needs_post = burn || background_music.is_some();
    let concat_out = if needs_post {
        clips_dir.join("_concat.mp4")
    } else {
        output.to_path_buf()
    };
    ffmpeg::concat_videos(&clips, &concat_out).map_err(|e| e.to_string())?;

    let music_out = clips_dir.join("_music.mp4");
    let post_music = if let Some(music) = background_music {
        task_events::emit_progress(
            app,
            Some(task_id),
            "mix",
            86.0,
            "正在混合本地背景音乐并自动压低人声下的音量...",
        );
        ffmpeg::mix_background_music(&concat_out, music, &music_out, background_music_db)
            .map_err(|e| format!("本地背景音乐混音失败：{e}"))?;
        music_out.as_path()
    } else {
        concat_out.as_path()
    };

    if burn {
        task_events::emit_progress(app, Some(task_id), "burn", 90.0, "正在烧录解说字幕...");
        ffmpeg::burn_subtitles(post_music, output_srt, output, font_size, mask_percent).map_err(
            |e| format!("解说字幕处理失败：{e}。没有发布带原字幕的临时视频，请修复后重试成片。"),
        )?;
        let _ = std::fs::remove_file(&concat_out);
        let _ = std::fs::remove_file(&music_out);
    } else if background_music.is_some() {
        std::fs::rename(&music_out, output).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&concat_out);
    }

    task_events::emit_progress(
        app,
        Some(task_id),
        "render",
        95.0,
        "粗剪已生成，等待本地质量检查",
    );
    Ok(())
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
        commentary_export_text, delete_all_task_directories, export_timestamp, local_vision_model,
        validate_task_id,
    };

    #[test]
    fn commentary_export_is_readable_and_keeps_source_timestamps() {
        let plan = crate::models::edit::EditPlan {
            title: "测试电影".into(),
            style: "剧情解说".into(),
            target_duration_secs: 10.0,
            segments: vec![crate::models::edit::EditSegment {
                shots: vec![],
                src_start: 62.345,
                src_end: 70.0,
                narration: "人物终于作出选择。".into(),
                keep_original_audio: true,
            }],
        };
        let text = commentary_export_text(&plan);
        assert!(text.contains("测试电影"));
        assert!(text.contains("01  原片 00:01:02.345 – 00:01:10.000 · 接原声"));
        assert!(text.contains("人物终于作出选择。"));
        assert_eq!(export_timestamp(-1.0), "00:00:00.000");
    }

    #[test]
    fn local_visual_review_prefers_an_installed_multimodal_model() {
        let installed = vec!["qwen3.8:27b-mlx".into(), "qwen3-vl:8b".into()];
        assert_eq!(
            local_vision_model("", "qwen3.8:27b-mlx", &installed),
            "qwen3-vl:8b"
        );
        assert_eq!(
            local_vision_model("llava:7b", "qwen3.8:27b-mlx", &installed),
            "llava:7b"
        );
    }

    #[test]
    fn subtitle_mask_changes_output_revision_and_defaults_for_old_settings() {
        let mut config = crate::models::config::AppConfig::default();
        let masked = super::render_settings_revision(&config);
        config.source_caption_mask_percent = 0;
        assert_ne!(masked, super::render_settings_revision(&config));
        let mut raw = serde_json::to_value(config).unwrap();
        raw.as_object_mut()
            .unwrap()
            .remove("source_caption_mask_percent");
        let old: crate::models::config::AppConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(old.source_caption_mask_percent, 18);
    }

    #[test]
    fn task_ids_cannot_escape_job_directories() {
        assert!(validate_task_id("job-1788005857370-0lwma4").is_ok());
        assert!(validate_task_id("../jobs").is_err());
        assert!(validate_task_id("task/child").is_err());
        assert!(validate_task_id("").is_err());
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
