use tauri::{AppHandle, State};

use crate::models::config::{AppConfig, DependencyStatus, OllamaStatus};
use crate::services::{deps, director};
use crate::state::AppState;

#[tauri::command]
pub async fn preview_edge_voice(
    state: State<'_, AppState>,
    voice: String,
) -> Result<String, String> {
    let voice = crate::services::tts::validate_voice(&voice)?.to_string();
    let _permit = state.acquire_render().await?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::services::tts::preview_voice(&voice)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|e| {
                format!("Edge TTS 试听失败：{e}。未使用系统声音代替；可选择其他音色或检查网络。")
            })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<AppConfig, String> {
    state.get_config().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    mut config: AppConfig,
) -> Result<(), String> {
    config.tts_voice = crate::services::tts::validate_voice(&config.tts_voice)?.to_string();
    if config.source_caption_mask_percent > 40 {
        return Err("原字幕遮盖高度应为 0–40%".into());
    }
    config.background_music_path = config.background_music_path.trim().to_string();
    if !config.background_music_path.is_empty() {
        let path = std::path::Path::new(&config.background_music_path);
        let allowed = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "mp3" | "wav" | "m4a" | "aac" | "flac"
            )
        });
        if !path.is_absolute() || !path.is_file() || !allowed {
            return Err("请选择本机已有的 MP3、WAV、M4A、AAC 或 FLAC 音乐文件".into());
        }
    }
    if !config.background_music_db.is_finite()
        || !(-40.0..=-8.0).contains(&config.background_music_db)
    {
        return Err("背景音乐音量应为 -40 至 -8 dB".into());
    }
    if config.source_caption_mask_percent > 0 {
        config.burn_captions = true;
    }
    state.save_config(config).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_dependencies(app: AppHandle) -> Result<DependencyStatus, String> {
    Ok(deps::check(crate::services::hard_subtitle::is_available(
        &app,
    )))
}

#[tauri::command]
pub async fn check_ollama(base_url: String) -> Result<OllamaStatus, String> {
    Ok(director::check_ollama(&base_url).await)
}
