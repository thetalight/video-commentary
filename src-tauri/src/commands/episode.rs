use crate::{
    commands::{
        pipeline::{job_paths, source_revision, validate_task_id},
        shorts::{self, Material},
    },
    services::{
        episode::{self, Episode},
        jobs::JobPaths,
        script::{self, Script},
    },
    state::AppState,
};
use tauri::{AppHandle, Emitter, Manager};

fn progress(app: &AppHandle, task_id: &str, message: &str) {
    let _ = app.emit(
        "episode-progress",
        serde_json::json!({"task_id":task_id,"message":message}),
    );
}
pub(crate) fn read_episode(paths: &JobPaths) -> Result<Option<Episode>, String> {
    let path = paths.root.join("episode/episode.json");
    if !path.exists() {
        return Ok(None);
    }
    let value: Episode = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "整集数据损坏，不能当作没有稿件覆盖".to_string())?;
    if value.schema_version != 1 {
        return Err("整集数据版本不支持".into());
    }
    if value.revision != crate::services::shorts::plan_revision(&value.plan) {
        return Err("整集稿件版本与正文不一致，拒绝沿用旧评审".into());
    }
    Ok(Some(value))
}
fn save(m: &Material, value: &Episode) -> Result<(), String> {
    episode::save(&m.paths.root.join("episode/episode.json"), value)
}
fn current(m: &Material) -> Result<Episode, String> {
    let value = read_episode(&m.paths)?.ok_or("还没有整集稿，请先生成")?;
    if value.transcript_revision != script::transcript_revision(&m.entries)
        || value.source_revision != source_revision(&m.paths.source)?
        || !same_selection(&value, m)
    {
        return Err("原片、字幕或选片排除规则已变化，请重新生成全集方案和稿件，旧稿未覆盖".into());
    }
    Ok(value)
}
fn same_selection(value: &Episode, material: &Material) -> bool {
    serde_json::to_vec(&value.shots).ok() == serde_json::to_vec(&material.episode_film()).ok()
}
fn expected(value: &Episode, revision: &str) -> Result<(), String> {
    if value.revision != revision {
        return Err("全文已有新版本，请刷新后再操作".into());
    }
    Ok(())
}
/// Read-only exclusion diagnostics from current settings; never expose cue text.
#[tauri::command]
pub async fn episode_exclusions(
    app: AppHandle,
    task_id: String,
) -> Result<crate::services::subtitle::ExclusionReport, String> {
    validate_task_id(&task_id)?;
    let m = shorts::material(&app, &task_id).await?;
    Ok(crate::services::subtitle::exclusion_report(
        &m.entries,
        m.duration,
        m.config.skip_intro_outro,
        m.config.skip_ads,
    ))
}

#[tauri::command]
pub async fn load_episode(app: AppHandle, task_id: String) -> Result<Option<Episode>, String> {
    validate_task_id(&task_id)?;
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|e| e.to_string())?;
    let paths = job_paths(&config, &task_id)?;
    let mut value = read_episode(&paths)?;
    let material = shorts::material(&app, &task_id).await;
    if let Some(episode) = value.as_mut() {
        let selection_current = material.as_ref().is_ok_and(|m| same_selection(episode, m));
        let transcript = crate::services::subtitle::parse_file(&paths.transcript)
            .map(|entries| script::transcript_revision(&entries));
        let current_source = source_revision(&paths.source);
        if !transcript.is_ok_and(|revision| revision == episode.transcript_revision)
            || !current_source.is_ok_and(|revision| revision == episode.source_revision)
            || !selection_current
        {
            episode.reviewed_revision = None;
            episode.output_path = None;
            episode.duration_secs = None;
            episode.review_error = Some(
                "原片、字幕或选片排除规则已变化；旧稿仅供查看，请重新生成整集，不能保存或出片"
                    .into(),
            );
        }
    }
    Ok(value)
}
#[tauri::command]
pub async fn generate_episode(
    app: AppHandle,
    task_id: String,
    expected_revision: Option<String>,
) -> Result<Episode, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = shorts::material(&app, &task_id).await?;
    let previous = read_episode(&m.paths)?;
    if previous.as_ref().map(|e| e.revision.as_str()) != expected_revision.as_deref() {
        return Err("全文版本已变化，请刷新后确认是否重新生成".into());
    }
    let options = m.ai().await?;
    let _permit = app.state::<AppState>().acquire_ai().await?;
    let root = m
        .paths
        .root
        .join(format!("episode/runs/generate-{}", shorts::stamp()?));
    let report = crate::services::subtitle::exclusion_report(
        &m.entries,
        m.duration,
        m.config.skip_intro_outro,
        m.config.skip_ads,
    );
    episode::save(
        &root.join("exclusion-report.json"),
        &serde_json::json!({
            "transcript_revision":script::transcript_revision(&m.entries),
            "source_revision":source_revision(&m.paths.source)?,
            "skip_intro_outro":m.config.skip_intro_outro,
            "skip_ads":m.config.skip_ads,
            "report":report,
        }),
    )?;
    let value = episode::generate(
        m.episode_film(),
        &options,
        script::transcript_revision(&m.entries),
        source_revision(&m.paths.source)?,
        &root,
        |message| progress(&app, &task_id, message),
    )
    .await?;
    if let Some(old) = previous {
        episode::save(&root.join("previous-episode.json"), &old)?;
    }
    save(&m, &value)?;
    progress(&app, &task_id, "整集初稿已保存，请检查评审状态并通读全文");
    Ok(value)
}
#[tauri::command]
pub async fn save_episode(
    app: AppHandle,
    task_id: String,
    plan: Script,
    expected_revision: String,
) -> Result<Episode, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = shorts::material(&app, &task_id).await?;
    let mut value = current(&m)?;
    expected(&value, &expected_revision)?;
    // Draft saves permit rule errors so the user can save an in-progress repair.
    episode::clear_after_edit(&mut value, plan);
    save(&m, &value)?;
    Ok(value)
}
#[tauri::command]
pub async fn review_episode(
    app: AppHandle,
    task_id: String,
    expected_revision: String,
) -> Result<Episode, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = shorts::material(&app, &task_id).await?;
    let mut value = current(&m)?;
    expected(&value, &expected_revision)?;
    let options = m.ai().await?;
    let _permit = app.state::<AppState>().acquire_ai().await?;
    let root = m
        .paths
        .root
        .join(format!("episode/runs/review-{}", shorts::stamp()?));
    episode::save(&root.join("before.json"), &value)?;
    // Reassessment must never leave a previous rendered file labeled as this candidate.
    value.output_path = None;
    value.duration_secs = None;
    episode::audit_episode(&mut value, &options, &root, |message| {
        progress(&app, &task_id, message)
    })
    .await?;
    save(&m, &value)?;
    Ok(value)
}
#[tauri::command]
pub async fn render_episode(
    app: AppHandle,
    task_id: String,
    expected_revision: String,
    confirmed_review: bool,
) -> Result<Episode, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    if !confirmed_review {
        return Err("请先人工通读全文并确认审稿".into());
    }
    let m = shorts::material(&app, &task_id).await?;
    let mut value = current(&m)?;
    expected(&value, &expected_revision)?;
    episode::validate_plan(&value.plan, &value.story, &value.shots)?;
    if value.reviewed_revision.as_deref() != Some(value.revision.as_str()) {
        return Err("当前全文未通过双层评审，不能出片".into());
    }
    let settings = shorts::render_settings(&m.config)?;
    let _permit = app.state::<AppState>().acquire_render().await?;
    let root = m
        .paths
        .root
        .join(format!("episode/renders/run-{}", shorts::stamp()?));
    let cache = m.paths.root.join("episode/tts");
    let source = m.paths.source.clone();
    let handle = app.clone();
    let id = task_id.clone();
    value = tauri::async_runtime::spawn_blocking(move || {
        episode::render(&mut value, &source, &root, &cache, &settings, |message| {
            progress(&handle, &id, message)
        })?;
        Ok::<_, String>(value)
    })
    .await
    .map_err(|_| "整集渲染线程意外停止，旧输出未覆盖".to_string())??;
    save(&m, &value)?;
    Ok(value)
}
