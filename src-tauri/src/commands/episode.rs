use crate::{
    commands::{
        pipeline::{job_paths, source_revision, validate_task_id},
        shorts::{self, Material},
    },
    services::{
        edit_plan::{self, Script},
        episode::{self, Episode},
        jobs::JobPaths,
        script,
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
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    let header: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|_| "整集数据损坏，不能当作没有稿件覆盖".to_string())?;
    if header["schema_version"] == 1 {
        let old: episode::LegacyEpisode =
            serde_json::from_slice(&raw).map_err(|_| "旧整集数据损坏".to_string())?;
        return episode::legacy_view(old).map(Some);
    }
    if header["schema_version"] != 2 {
        return Err("整集数据版本不支持".into());
    }
    let value: Episode = serde_json::from_slice(&raw).map_err(|_| "v2整集数据损坏".to_string())?;
    if value.revision != edit_plan::plan_revision(&value.plan) {
        return Err("整集稿件版本与正文不一致，拒绝沿用旧评审".into());
    }
    Ok(Some(value))
}
fn checked_episode_bytes(value: &Episode) -> Result<Vec<u8>, String> {
    if value.schema_version != 2 || value.revision != edit_plan::plan_revision(&value.plan) {
        return Err("保存前检测到正文版本不一致，拒绝覆盖当前稿件".into());
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "无法序列化整集稿件".to_string())?;
    let restored: Episode = serde_json::from_slice(&bytes)
        .map_err(|_| "整集稿件保存前JSON往返校验失败，旧稿未覆盖".to_string())?;
    if restored.revision != edit_plan::plan_revision(&restored.plan)
        || !script::same_source_windows(&value.shots, &restored.shots)
    {
        return Err("整集稿件JSON往返改变了时间或正文版本，旧稿未覆盖；无需重新调用AI".into());
    }
    Ok(bytes)
}
fn save(m: &Material, value: &Episode) -> Result<(), String> {
    crate::services::jobs::atomic_write(
        m.paths.root.join("episode/episode.json"),
        checked_episode_bytes(value)?,
    )
    .map_err(|e| e.to_string())
}
fn current(m: &Material) -> Result<Episode, String> {
    let value = read_episode(&m.paths)?.ok_or("还没有整集稿，请先生成")?;
    if value.schema_version != 2 {
        return Err("旧schema-v1仅供查看，请先显式迁移为v2".into());
    }
    validate_current_material(&value, m)?;
    Ok(value)
}
fn validate_current_material(value: &Episode, material: &Material) -> Result<(), String> {
    if value.transcript_revision != script::transcript_revision(&material.entries) {
        return Err("原语言字幕版本确实已变化，请重新生成整集；旧稿保留。".into());
    }
    if value.source_revision != source_revision(&material.paths.source)? {
        return Err("原片文件身份确实已变化，请重新生成整集；旧稿保留。".into());
    }
    // Keep errors from stale manual exclusions distinguishable from an actual
    // material change; a failed read is not proof that the source changed.
    let shots = material
        .episode_film()
        .map_err(|e| format!("无法核对当前选片设置：{e}。请先刷新并确认排除选择；尚未调用AI。"))?;
    if !script::same_source_windows(&value.shots, &shots) {
        return Err("当前字幕选片范围与稿件记录不同，请核对人工排除选择；确认需要改变材料后再生成整集，旧稿保留。".into());
    }
    Ok(())
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
    // Return current candidates even if saved manual choices have gone stale,
    // so the user can explicitly re-confirm instead of becoming locked out.
    Ok(m.episode_exclusion_report().unwrap_or_else(|_| {
        crate::services::subtitle::exclusion_report(
            &m.entries,
            m.duration,
            m.config.skip_intro_outro,
            m.config.skip_ads,
        )
    }))
}

#[tauri::command]
pub async fn save_episode_exclusions(
    app: AppHandle,
    task_id: String,
    expected_fingerprint: String,
    applied_indices: Vec<usize>,
) -> Result<crate::services::subtitle::ExclusionReport, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = shorts::material(&app, &task_id).await?;
    let mut report = crate::services::subtitle::exclusion_report(
        &m.entries,
        m.duration,
        m.config.skip_intro_outro,
        m.config.skip_ads,
    );
    if crate::services::subtitle::exclusion_fingerprint(&report) != expected_fingerprint {
        return Err("候选区间已变化，请刷新后确认".into());
    }
    let choices = crate::services::subtitle::ExclusionChoices {
        version: 1,
        transcript_revision: script::transcript_revision(&m.entries),
        candidate_fingerprint: expected_fingerprint,
        applied_indices,
    };
    crate::services::subtitle::apply_exclusion_choices(
        &mut report,
        &choices,
        &script::transcript_revision(&m.entries),
    )?;
    episode::save(
        &m.paths.root.join("episode/exclusion-choices.json"),
        &choices,
    )?;
    Ok(report)
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
    if let Some(episode) = value.as_mut() {
        let material = shorts::material(&app, &task_id).await;
        let result = match material {
            Ok(m) => validate_current_material(episode, &m),
            Err(error) => Err(format!(
                "无法读取当前素材：{error}。请先恢复素材读取；不能仅凭读取失败判断需要重新生成。"
            )),
        };
        if let Err(error) = result {
            episode.reviewed_revision = None;
            episode.audience_review=None;
            episode.output_path = None;
            episode.duration_secs = None;
            episode.review_error = Some(error);
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
    let report = m.episode_exclusion_report()?;
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
        m.episode_film()?,
        &options,
        script::transcript_revision(&m.entries),
        source_revision(&m.paths.source)?,
        &root,
        |message| progress(&app, &task_id, message),
    )
    .await?;
    let latest = shorts::material(&app, &task_id).await?;
    if let Err(error) = validate_current_material(&value, &latest) {
        episode::save(&root.join("candidate-material-conflict.json"), &value)?;
        return Err(format!(
            "生成期间材料确实发生变化，候选已保留且旧稿未覆盖：{error}"
        ));
    }
    if let Some(old) = previous {
        if let Some(legacy) = old.legacy.as_ref() {
            episode::save(&root.join("previous-episode.json"), legacy)?;
        } else {
            episode::save(&root.join("previous-episode.json"), &old)?;
        }
    }
    save(&m, &value)?;
    progress(&app, &task_id, "整集初稿已保存，请检查评审状态并通读全文");
    Ok(value)
}
#[derive(serde::Serialize)]
pub struct MigrationStatus {
    pub can_migrate: bool,
    pub reason: String,
}
fn check_migration_material(
    value: &Episode,
    transcript_revision: &str,
    source_revision: &str,
    shots: &[crate::services::script::Shot],
) -> Result<(), String> {
    if value.schema_version != 1 || value.legacy.is_none() {
        return Err("当前不是可迁移的旧schema-v1，请刷新查看当前稿件。".into());
    }
    if value.transcript_revision != transcript_revision {
        return Err(
            "原语言字幕已变化，旧稿证据不能直接迁移。请重新生成整集稿，旧稿保留；不必重新下载。"
                .into(),
        );
    }
    if value.source_revision != source_revision {
        return Err("原片身份已变化，请重新生成整集稿，旧稿保留。".into());
    }
    if !script::same_source_windows(&value.shots, shots) {
        return Err("选片材料已变化（例如已修正片头片尾排除规则），直接迁移会继续漏掉剧情。请重新生成整集稿，旧稿保留；不必重新下载或转写。".into());
    }
    Ok(())
}
#[tauri::command]
pub async fn episode_migration_status(
    app: AppHandle,
    task_id: String,
    expected_revision: String,
) -> Result<MigrationStatus, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().try_lock_task(&task_id)?;
    let config = app
        .state::<AppState>()
        .get_config()
        .await
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let paths = job_paths(&config, &task_id)?;
        let value = read_episode(&paths)?.ok_or("没有旧稿可迁移")?;
        expected(&value, &expected_revision)?;
        crate::commands::pipeline::ensure_supported_transcript_source(&paths)?;
        let entries = crate::services::subtitle::parse_file(&paths.transcript)
            .map_err(|_| "无法读取原语言字幕，请检查任务材料".to_string())?;
        let report = crate::services::subtitle::exclusion_report(
            &entries,
            crate::services::ffmpeg::get_duration(&paths.source).map_err(|e| e.to_string())?,
            config.skip_intro_outro,
            config.skip_ads,
        );
        let mut report = report;
        let choices = paths.root.join("episode/exclusion-choices.json");
        if choices.exists() {
            let choices =
                serde_json::from_slice(&std::fs::read(choices).map_err(|e| e.to_string())?)
                    .map_err(|_| "人工排除配置损坏".to_string())?;
            crate::services::subtitle::apply_exclusion_choices(
                &mut report,
                &choices,
                &script::transcript_revision(&entries),
            )?;
        }
        let shots = script::episode_shots(&entries, report.source_duration, &report.applied_ranges);
        match check_migration_material(
            &value,
            &script::transcript_revision(&entries),
            &source_revision(&paths.source)?,
            &shots,
        ) {
            Ok(()) => Ok(MigrationStatus {
                can_migrate: true,
                reason: "原片、字幕和选片范围一致，可以归档并迁移；不调用AI、不重新转写。".into(),
            }),
            Err(reason) => Ok(MigrationStatus {
                can_migrate: false,
                reason,
            }),
        }
    })
    .await
    .map_err(|_| "迁移检查线程中止，请重试".to_string())?
}

#[tauri::command]
pub async fn migrate_episode(
    app: AppHandle,
    task_id: String,
    expected_revision: String,
) -> Result<Episode, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().try_lock_task(&task_id)?;
    progress(&app, &task_id, "已收到迁移请求，正在检查旧稿和当前素材…");
    let m = shorts::material(&app, &task_id).await?;
    let mut value = read_episode(&m.paths)?.ok_or("没有旧稿可迁移")?;
    expected(&value, &expected_revision)?;
    let old = value.legacy.clone().ok_or("当前不是schema-v1")?;
    check_migration_material(
        &value,
        &script::transcript_revision(&m.entries),
        &source_revision(&m.paths.source)?,
        &m.episode_film()?,
    )?;
    progress(
        &app,
        &task_id,
        "素材检查通过，正在归档旧稿并写入v2（不调用AI）…",
    );
    let paths = m.paths.clone();
    value = tauri::async_runtime::spawn_blocking(move || {
        let root = paths
            .root
            .join(format!("episode/runs/migrate-{}", shorts::stamp()?));
        episode::save(&root.join("schema-v1-original.json"), &old)?;
        episode::migrate_legacy(&mut value)?;
        crate::services::jobs::atomic_write(
            paths.root.join("episode/episode.json"),
            checked_episode_bytes(&value)?,
        )
        .map_err(|e| e.to_string())?;
        Ok::<_, String>(value)
    })
    .await
    .map_err(|_| "迁移保存线程中止，请刷新检查当前稿件；不要重复修改旧文件".to_string())??;
    progress(
        &app,
        &task_id,
        "迁移完成：旧稿已归档，当前为v2；下一步核对证据/画面后运行AI审查或旁白试听。",
    );
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
    let _guard = app.state::<AppState>().try_lock_task(&task_id)?;
    progress(
        &app,
        &task_id,
        "已收到AI审稿请求，正在校验当前全文与素材版本…",
    );
    let m = shorts::material(&app, &task_id).await?;
    let mut value = current(&m)?;
    expected(&value, &expected_revision)?;
    let original_before_resume=value.clone();
    let review_only=value.review_error.as_deref().is_some_and(episode::review_service_pending);
    let archive=if review_only{None}else{episode::budget_recovery_candidate(&value,&m.paths.root.join("episode/runs"))?};
    let resumed=if let Some(candidate)=archive {
        validate_current_material(&candidate,&m)?;
        value=candidate;
        progress(&app,&task_id,"已读取归档恢复稿，直接修复剩余画面字数预算；完成后全文复审，不重写整集…");
        true
    }else{false};
    let options = m.ai().await?;
    progress(
        &app,
        &task_id,
        "版本检查通过，正在等待AI并发空闲；不会重复提交请求…",
    );
    let _permit = app.state::<AppState>().acquire_ai().await?;
    progress(
        &app,
        &task_id,
        "AI审查已开始：核对事实、连贯性，并独立检查陌生观众能否听懂；需要时定向修稿或恢复全文…",
    );
    let root = m
        .paths
        .root
        .join(format!("episode/runs/review-{}", shorts::stamp()?));
    episode::save(&root.join("before.json"), &value)?;
    if resumed {episode::save(&root.join("original-before-resume.json"),&original_before_resume)?;}
    // Reassessment must never leave a previous rendered file labeled as this candidate.
    value.output_path = None;
    value.duration_secs = None;
    episode::audit_episode(&mut value, &options, &root, !resumed && !review_only, |message| {
        progress(&app, &task_id, message)
    })
    .await?;
    if (resumed || review_only) && value.reviewed_revision.as_deref()!=Some(value.revision.as_str()) && !value.review_error.as_deref().is_some_and(episode::review_service_pending) {
        value.review_error=Some(format!("恢复稿继续修复已结束：{}。候选已保存，请修改具体问题后重评。",value.review_error.as_deref().unwrap_or("未取得完整评审")));
    }
    validate_current_material(&value, &shorts::material(&app, &task_id).await?)?;
    save(&m, &value)?;
    progress(
        &app,
        &task_id,
        if value.reviewed_revision.as_deref() == Some(value.revision.as_str()) {
            "AI审查完成：当前版本已通过，请通读确认后出片"
        } else {
            "AI审查完成：已保存候选和待核问题，尚未通过，详情见审稿结果"
        },
    );
    Ok(value)
}
#[tauri::command]
pub async fn detect_episode_scenes(
    app: AppHandle,
    task_id: String,
) -> Result<crate::services::scene_index::SceneIndex, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = shorts::material(&app, &task_id).await?;
    let _permit = app.state::<AppState>().acquire_render().await?;
    let source = m.paths.source;
    let output = m.paths.root.join("episode/scene-index.json");
    tauri::async_runtime::spawn_blocking(move || {
        crate::services::scene_index::detect(&source, &output)
    })
    .await
    .map_err(|_| "切点检测线程中止".to_string())?
}

#[tauri::command]
pub async fn import_reviewed_episode(app:AppHandle,task_id:String,expected_revision:String,candidate_path:String,accept_story_change:Option<bool>)->Result<Episode,String> {
    validate_task_id(&task_id)?;
    let _guard=app.state::<AppState>().try_lock_task(&task_id)?;
    let path=std::path::Path::new(candidate_path.trim());
    if !path.is_absolute() || path.extension().is_none_or(|e|e!="json"){return Err("请选择已审稿候选JSON文件的绝对路径".into());}
    let size=std::fs::metadata(path).map_err(|_|"无法读取候选文件，请检查路径和权限")?.len();
    if size>20*1024*1024{return Err("候选包超出20MB读取上限".into());}
    let package:episode::ReviewedCandidatePackage=serde_json::from_slice(&std::fs::read(path).map_err(|_|"无法读取候选包")?)
        .map_err(|_|"候选包格式无效，需要包含原稿版本、完整候选和同版本AI评审")?;
    let m=shorts::material(&app,&task_id).await?;
    let current=current(&m)?;expected(&current,&expected_revision)?;
    let candidate=episode::validate_reviewed_package_with_story(&package,&current,accept_story_change.unwrap_or(false))?;
    validate_current_material(&candidate,&m)?;
    let root=m.paths.root.join(format!("episode/runs/import-{}",shorts::stamp()?));
    episode::save(&root.join("before.json"),&current)?;
    episode::save(&root.join("verified-package.json"),&package)?;
    episode::save(&root.join("accepted.json"),&serde_json::json!({"revision":candidate.revision,"audit":package.audit,"status":"ai_reviewed_pending_human","origin":"imported_verified_candidate"}))?;
    save(&m,&candidate)?;
    progress(&app,&task_id,"已恢复同源同版本且完整复审通过的候选，请人工通读确认后出片");
    Ok(candidate)
}

#[tauri::command]
pub async fn preview_episode_narration(
    app: AppHandle,
    task_id: String,
    expected_revision: String,
) -> Result<String, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = shorts::material(&app, &task_id).await?;
    let value = current(&m)?;
    expected(&value, &expected_revision)?;
    let _permit = app.state::<AppState>().acquire_render().await?;
    let root = m
        .paths
        .root
        .join(format!("episode/previews/run-{}", shorts::stamp()?));
    let cache = m.paths.root.join("episode/tts");
    let voice = m.config.tts_voice.clone();
    let instruction = m.config.tts_instruction.clone();
    tauri::async_runtime::spawn_blocking(move || {
        episode::preview_narration(&value, &voice, &instruction, &cache, &root)
    })
    .await
    .map_err(|_| "旁白预览线程中止".to_string())?
}

#[tauri::command]
pub async fn save_episode_story(
    app: AppHandle,
    task_id: String,
    expected_revision: String,
    expected_story: episode::Story,
    story: episode::Story,
) -> Result<Episode, String> {
    validate_task_id(&task_id)?;
    let _guard = app.state::<AppState>().lock_task(&task_id).await;
    let m = shorts::material(&app, &task_id).await?;
    let mut value = current(&m)?;
    expected(&value, &expected_revision)?;
    if serde_json::to_vec(&expected_story).ok() != serde_json::to_vec(&value.story).ok() {
        return Err("叙事方案已有新版本，请刷新".into());
    }
    episode::validate_story(&story, &value.events, &value.shots)?;
    // Plan revision alone is not sufficient to distinguish two different story
    // edits. Story save has its own expected fingerprint and clears approval.
    value.story = story;
    value.reviewed_revision = None;
    value.review_error = Some("叙事方案已修改，请检查现有正文并重新评审；不会自动重写全文".into());
    value.audience_review=None;
    value.output_path = None;
    value.duration_secs = None;
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
    validate_current_material(&value, &shorts::material(&app, &task_id).await?)?;
    save(&m, &value)?;
    Ok(value)
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    fn legacy() -> Episode {
        let shots = vec![script::Shot {
            id: 0,
            start: 10.0,
            end: 20.0,
            text: "合成测试原文".into(),
        }];
        let plan = script::Script {
            title: "测试".into(),
            paragraphs: vec![script::Paragraph {
                narration: "合成测试叙述".into(),
                evidence_quote: "合成测试原文".into(),
                shot_ids: vec![0],
            }],
        };
        let old = episode::LegacyEpisode {
            schema_version: 1,
            transcript_revision: "t1".into(),
            source_revision: "s1".into(),
            revision: crate::services::shorts::plan_revision(&plan),
            events: vec![],
            story: episode::Story {
                narration_brief: None,
                spine: "测试主线".into(),
                characters: vec![],
                reveal_order: String::new(),
                beats: vec![],
                covered_events: vec![],
            },
            plan,
            shots,
            target_secs: 10.0,
            reviewed_revision: None,
            review_error: None,
            research: vec![],
            warnings: vec![],
            output_path: None,
            duration_secs: None,
        };
        episode::legacy_view(old).unwrap()
    }
    /// Local metadata-only diagnostics: never prints task text, titles or reasons.
    #[test]
    #[ignore]
    fn diagnose_material_identity_metadata_only() {
        let task = std::env::var("VC_DIAG_TASK").expect("set task ID");
        let m = shorts::material_from_config_for_diagnostics(&task).unwrap();
        let value = read_episode(&m.paths).unwrap().unwrap();
        let current = m.episode_film().unwrap();
        eprintln!(
            "IDENTITY current_material_guard_passes={}",
            validate_current_material(&value, &m).is_ok()
        );
        eprintln!(
            "IDENTITY in_memory_save_reload_guard_passes={}",
            checked_episode_bytes(&value).is_ok()
        );
        eprintln!("IDENTITY transcript_match={} source_match={} saved_windows={} current_windows={} serialized_equal={}",value.transcript_revision==script::transcript_revision(&m.entries),value.source_revision==source_revision(&m.paths.source).unwrap(),value.shots.len(),current.len(),serde_json::to_vec(&value.shots).unwrap()==serde_json::to_vec(&current).unwrap());
        let mut texts = 0;
        let mut ids = 0;
        let mut times = 0;
        let mut max_delta = 0.0_f64;
        for (a, b) in value.shots.iter().zip(&current) {
            texts += usize::from(a.text != b.text);
            ids += usize::from(a.id != b.id);
            times += usize::from(a.start != b.start || a.end != b.end);
            max_delta = max_delta
                .max((a.start - b.start).abs())
                .max((a.end - b.end).abs());
        }
        eprintln!("IDENTITY mismatch_counts text={texts} id={ids} time={times} max_time_delta={max_delta:e}");
        let mut repaired_plan = value.plan.clone();
        let repairs = edit_plan::repair_unique_evidence_links(&mut repaired_plan, &value.shots);
        let bad_before: Vec<_> = value
            .plan
            .paragraphs
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                edit_plan::validate_paragraph(p, &value.shots)
                    .is_err()
                    .then_some(i + 1)
            })
            .collect();
        for (i, p) in repaired_plan.paragraphs.iter().enumerate() {
            if let Err(error) = edit_plan::validate_paragraph(p, &value.shots) {
                let category = if error.contains("引文") {
                    "quote_match"
                } else if error.contains("储备") {
                    "picture_reserve"
                } else if error.contains("1秒") {
                    "visual_range_or_minimum"
                } else if error.contains("必保") {
                    "protected_range"
                } else {
                    "other"
                };
                eprintln!("RULE paragraph={} category={} picture_count={} narration_chars={} evidence_count={}",i+1,category,p.visual_selections.len(),p.narration.chars().count(),p.evidence_refs.len());
            }
        }
        let allocation = edit_plan::repair_picture_overlaps(&mut repaired_plan, &value.shots);
        eprintln!(
            "PICTURE allocation_groups={:?}",
            allocation
                .iter()
                .map(|r| &r.paragraph_numbers)
                .collect::<Vec<_>>()
        );
        for group in edit_plan::picture_conflicts(&repaired_plan) {
            eprintln!("PICTURE unresolved_group={:?} unique_seconds={:.3} required_seconds={:.3} deficit_seconds={:.3} protected_or_original={}",group.paragraph_numbers,group.unique_selected_seconds,group.required_seconds,group.deficit_seconds,group.has_protected_or_original_audio);
        }
        let bad_after: Vec<_> = repaired_plan
            .paragraphs
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                edit_plan::validate_paragraph(p, &value.shots)
                    .is_err()
                    .then_some(i + 1)
            })
            .collect();
        eprintln!("EVIDENCE repairs_metadata={:?} invalid_paragraphs_before={bad_before:?} after={bad_after:?} full_program_pass_after={}",repairs.iter().map(|r|(r.paragraph_number,r.added_evidence_id)).collect::<Vec<_>>(),episode::validate_plan(&repaired_plan,&value.story,&value.shots).is_ok());
    }

    #[test]
    fn preflight_checks_each_material_identity_without_mutating_the_legacy_view() {
        let value = legacy();
        let before = serde_json::to_vec(value.legacy.as_ref().unwrap()).unwrap();
        check_migration_material(&value, "t1", "s1", &value.shots).unwrap();
        assert!(check_migration_material(&value, "t2", "s1", &value.shots)
            .unwrap_err()
            .contains("字幕已变化"));
        assert!(check_migration_material(&value, "t1", "s2", &value.shots)
            .unwrap_err()
            .contains("原片身份"));
        let mut shots = value.shots.clone();
        shots[0].start = 11.0;
        assert!(check_migration_material(&value, "t1", "s1", &shots)
            .unwrap_err()
            .contains("选片材料已变化"));
        assert_eq!(
            serde_json::to_vec(value.legacy.as_ref().unwrap()).unwrap(),
            before
        );
    }
    #[test]
    fn generate_save_reload_review_and_render_identity_roundtrip_is_stable() {
        let mut value = legacy();
        episode::migrate_legacy(&mut value).unwrap();
        // Decimal timestamps that historically lose a binary ulp under the
        // default serde_json fast decoder. Cover both source and chosen picture.
        for time in [310.253, 249.268, 471.683, 0.845, 1655.556, 3210.077] {
            value.shots[0].start = time;
            value.shots[0].end = time + 10.0;
            let visual = &mut value.plan.paragraphs[0].visual_selections[0];
            visual.start = time;
            visual.end = time + 10.0;
            value.revision = edit_plan::plan_revision(&value.plan);
            for _ in 0..3 {
                let raw = checked_episode_bytes(&value).unwrap();
                let loaded: Episode = serde_json::from_slice(&raw).unwrap();
                assert_eq!(loaded.revision, edit_plan::plan_revision(&loaded.plan));
                assert!(script::same_source_windows(&loaded.shots, &value.shots));
                assert_eq!(
                    loaded.shots[0].start.to_bits(),
                    value.shots[0].start.to_bits()
                );
                episode::validate_plan(&loaded.plan, &loaded.story, &loaded.shots).unwrap();
                value = loaded;
            }
        }
    }

    #[test]
    fn preflight_does_not_migrate_an_already_upgraded_script() {
        let mut value = legacy();
        episode::migrate_legacy(&mut value).unwrap();
        assert!(check_migration_material(&value, "t1", "s1", &value.shots).is_err());
    }
}
