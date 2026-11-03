use tauri::State;

use crate::models::config::{AiStatus, AppConfig, DependencyStatus};
use crate::services::{ai, deps};
use crate::state::AppState;

#[tauri::command]
pub async fn preview_local_voice(
    state: State<'_, AppState>,
    voice: String,
) -> Result<String, String> {
    let voice = crate::services::tts::validate_voice(&voice)?.to_string();
    let _permit = state.acquire_render().await?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::services::tts::preview_voice(&voice)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|e| format!("本机音色试听失败：{e}。请在设置里选择已安装的中文音色。"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_local_voices() -> Vec<crate::services::tts::LocalVoice> {
    crate::services::tts::local_voices()
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
    if config.original_audio_mode != "duck" {
        config.original_audio_mode = "mute".into();
    }
    state.save_config(config).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_dependencies(state: State<'_, AppState>) -> Result<DependencyStatus, String> {
    let model = state
        .get_config()
        .await
        .map(|config| config.asr_model_path)
        .unwrap_or_default();
    Ok(deps::check(&model))
}

#[tauri::command]
pub async fn check_ai() -> AiStatus {
    ai::check_ai().await
}
