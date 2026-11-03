use tauri::{AppHandle, Emitter, Manager};

use crate::commands::pipeline::{
    ensure_supported_transcript_source, job_paths, source_revision, validate_task_id,
};
use crate::models::config::AppConfig;
use crate::services::ai::AiOptions;
use crate::services::jobs::{self, JobPaths};
use crate::services::script::{self, Script};
use crate::services::shorts::{self, RenderSettings, ShortItem, ShortsBundle};
use crate::services::{ffmpeg, subtitle};
use crate::state::AppState;

fn shorts_progress(app: &AppHandle, task_id: &str, message: &str) {
    // Separate channel: a failed short must not mark the task itself as failed.
    let _ = app.emit(
        "shorts-progress",
        serde_json::json!({"task_id":task_id,"message":message}),
    );
}

fn bundle_path(paths: &JobPaths) -> std::path::PathBuf {
    paths.shorts.join("shorts.json")
}

fn save_bundle(paths: &JobPaths, bundle: &ShortsBundle) -> Result<(), String> {
    jobs::atomic_write(
        bundle_path(paths),
        serde_json::to_vec_pretty(bundle).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

pub(crate) fn read_bundle(paths: &JobPaths) -> Result<Option<ShortsBundle>, String> {
    let path = bundle_path(paths);
    if !path.exists() {
        return Ok(None);
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub(super) fn stamp() -> Result<u128, String> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos())
}

pub(super) struct Material {
    pub(super) paths: JobPaths,
    pub(super) config: AppConfig,
    pub(super) entries: Vec<subtitle::SubtitleEntry>,
    pub(super) duration: f64,
    style: String,
}

impl Material {
    pub(super) fn film(&self) -> Vec<script::Shot> {
        let excluded = subtitle::detect_excluded_ranges(
            &self.entries,
            self.duration,
            self.config.skip_intro_outro,
            self.config.skip_ads,
        );
        script::film_shots(&self.entries, self.duration, &excluded)
    }

    pub(super) fn episode_film(&self) -> Vec<script::Shot> {
        let excluded = subtitle::detect_excluded_ranges(&self.entries, self.duration, self.config.skip_intro_outro, self.config.skip_ads);
        script::episode_shots(&self.entries, self.duration, &excluded)
    }

    pub(super) async fn ai(&self) -> Result<AiOptions, String> {
        AiOptions::for_task(&self.config, &self.style, self.duration).await
    }
}

pub(super) async fn material(app: &AppHandle, task_id: &str) -> Result<Material, String> {
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|e| e.to_string())?;
    let paths = job_paths(&config, task_id)?;
    ensure_supported_transcript_source(&paths)?;
    let entries = subtitle::parse_file(&paths.transcript).map_err(|e| e.to_string())?;
    let duration = ffmpeg::get_duration(&paths.source).map_err(|e| e.to_string())?;
    // The style chosen when the task was created wins over the current default.
    let style = std::fs::read(&paths.meta)
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .and_then(|meta| meta.get("style")?.as_str().map(str::to_string))
        .filter(|style| !style.trim().is_empty())
        .unwrap_or_else(|| config.default_style.clone());
    Ok(Material {
        paths,
        config,
        entries,
        duration,
        style,
    })
}

fn current_bundle(m: &Material) -> Result<ShortsBundle, String> {
    let bundle = read_bundle(&m.paths)?.ok_or("还没有短视频选题，请先生成")?;
    if bundle.transcript_revision != script::transcript_revision(&m.entries) {
        return Err("字幕已更新，请重新生成短视频选题，不能沿用旧字幕选片".into());
    }
    if bundle.source_revision != source_revision(&m.paths.source)? {
        return Err("原片文件已变化，请重新生成短视频选题".into());
    }
    Ok(bundle)
}

fn item_index(bundle: &ShortsBundle, short_id: &str) -> Result<usize, String> {
    bundle
        .items
        .iter()
        .position(|item| item.id == short_id)
        .ok_or_else(|| "找不到这条短片，请刷新任务".into())
}

#[tauri::command]
pub async fn load_shorts(app: AppHandle, task_id: String) -> Result<Option<ShortsBundle>, String> {
    validate_task_id(&task_id)?;
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|e| e.to_string())?;
    read_bundle(&job_paths(&config, &task_id)?)
}

/// Scout the whole film for independent highlights, then draft every short.
/// The bundle is saved after each short so an interrupted run keeps finished drafts.
#[tauri::command]
pub async fn generate_shorts(app: AppHandle, task_id: String) -> Result<ShortsBundle, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = material(&app, &task_id).await?;
    let options = m.ai().await?;
    shorts_progress(&app, &task_id, "正在等待 AI 空闲");
    let _permit = app.state::<AppState>().acquire_ai().await?;
    let run_root = m
        .paths
        .shorts
        .join("runs")
        .join(format!("run-{}", stamp()?));
    let film = m.film();
    let topics = shorts::scout(&film, &options, &run_root.join("scout"), |message| {
        shorts_progress(&app, &task_id, message)
    })
    .await?;
    let mut bundle = ShortsBundle {
        transcript_revision: script::transcript_revision(&m.entries),
        source_revision: source_revision(&m.paths.source)?,
        items: topics
            .into_iter()
            .enumerate()
            .map(|(i, topic)| ShortItem::pending(format!("s{:02}", i + 1), topic))
            .collect(),
    };
    save_bundle(&m.paths, &bundle)?;
    let total = bundle.items.len();
    for i in 0..total {
        let item = &bundle.items[i];
        let label = format!("短片 {}/{}「{}」", i + 1, total, item.topic.title);
        let outcome = shorts::write(
            &item.topic,
            &film,
            &options,
            &run_root.join(&item.id),
            |message| shorts_progress(&app, &task_id, &format!("{label}：{message}")),
        )
        .await;
        bundle.items[i].apply_draft(outcome);
        save_bundle(&m.paths, &bundle)?;
    }
    shorts_progress(&app, &task_id, "短片文案已生成，请审稿后逐条生成视频");
    Ok(bundle)
}

#[tauri::command]
pub async fn rewrite_short(
    app: AppHandle,
    task_id: String,
    short_id: String,
) -> Result<ShortsBundle, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = material(&app, &task_id).await?;
    let mut bundle = current_bundle(&m)?;
    let index = item_index(&bundle, &short_id)?;
    let options = m.ai().await?;
    shorts_progress(&app, &task_id, "正在等待 AI 空闲");
    let _permit = app.state::<AppState>().acquire_ai().await?;
    let root = m
        .paths
        .shorts
        .join("runs")
        .join(format!("rewrite-{}", stamp()?))
        .join(&short_id);
    let title = bundle.items[index].topic.title.clone();
    let outcome = shorts::write(
        &bundle.items[index].topic,
        &m.film(),
        &options,
        &root,
        |message| shorts_progress(&app, &task_id, &format!("重写「{title}」：{message}")),
    )
    .await;
    bundle.items[index].apply_draft(outcome);
    save_bundle(&m.paths, &bundle)?;
    Ok(bundle)
}

pub(super) fn render_settings(config: &AppConfig) -> Result<RenderSettings, String> {
    let music = config.background_music_path.trim();
    let music = if music.is_empty() {
        None
    } else if std::path::Path::new(music).is_file() {
        Some((music.into(), config.background_music_db))
    } else {
        return Err(format!("设置里的背景音乐文件不存在：{music}"));
    };
    Ok(RenderSettings {
        voice: config.tts_voice.clone(),
        mask_percent: config.source_caption_mask_percent,
        ambience: config.original_volume(),
        music,
    })
}

/// Earlier renders of one short are superseded by a successful new one.
fn remove_other_runs(short_dir: &std::path::Path, keep: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(short_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path != keep {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[tauri::command]
pub async fn render_short(
    app: AppHandle,
    task_id: String,
    short_id: String,
    plan: Script,
    expected_revision: String,
) -> Result<ShortsBundle, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = material(&app, &task_id).await?;
    let settings = render_settings(&m.config)?;
    let mut bundle = current_bundle(&m)?;
    let index = item_index(&bundle, &short_id)?;
    if bundle.items[index].revision != expected_revision {
        return Err("这条短片已有新版本，请刷新后再试".into());
    }
    script::validate(&plan, &bundle.items[index].shots)?;
    let short_dir = m.paths.shorts.join(&short_id);
    let root = short_dir.join(format!("run-{}", stamp()?));
    let tts_cache = m.paths.shorts.join("tts");
    shorts_progress(&app, &task_id, "正在等待渲染空闲");
    let _permit = app.state::<AppState>().acquire_render().await?;
    let mut item = bundle.items[index].clone();
    item.revision = shorts::plan_revision(&plan);
    item.plan = Some(plan);
    item.output_path = None;
    item.duration_secs = None;
    let source = m.paths.source.clone();
    let progress_app = app.clone();
    let progress_task = task_id.clone();
    let label = format!("「{}」", item.topic.title);
    let render_root = root.clone();
    let (item, outcome) = tauri::async_runtime::spawn_blocking(move || {
        let outcome = shorts::render(
            &mut item,
            &source,
            &render_root,
            &tts_cache,
            &settings,
            |message| {
                shorts_progress(
                    &progress_app,
                    &progress_task,
                    &format!("{label}：{message}"),
                )
            },
        );
        (item, outcome)
    })
    .await
    .map_err(|e| e.to_string())?;
    outcome?;
    bundle.items[index] = item;
    save_bundle(&m.paths, &bundle)?;
    remove_other_runs(&short_dir, &root);
    Ok(bundle)
}
