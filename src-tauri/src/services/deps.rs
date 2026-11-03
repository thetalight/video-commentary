use std::process::Command;

use crate::models::config::DependencyStatus;
use crate::services::{asr, ffmpeg, tts};

pub fn check(model_setting: &str) -> DependencyStatus {
    let ytdlp = Command::new("yt-dlp").arg("--version").output();

    let ytdlp_version = ytdlp.as_ref().ok().and_then(|o| {
        if o.status.success() {
            Some(
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string(),
            )
        } else {
            None
        }
    });

    let subtitle_ffmpeg = ffmpeg::resolve_ffmpeg();
    let ffmpeg_burn = subtitle_ffmpeg.is_some();
    let ffmpeg_path = subtitle_ffmpeg.or_else(ffmpeg::resolve_ffmpeg_bin);

    let ffmpeg_version = ffmpeg_path.as_ref().and_then(|path| {
        Command::new(path)
            .arg("-version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
    });

    let ffmpeg_ok = ffmpeg_path.is_some();

    let whisper_model = asr::model_path(model_setting);
    let whisper = asr::whisper_bin().is_some() && whisper_model.is_some();

    DependencyStatus {
        ytdlp: ytdlp.map(|o| o.status.success()).unwrap_or(false),
        ffmpeg: ffmpeg_ok,
        ffmpeg_burn,
        local_tts: tts::is_local_tts_available(),
        whisper,
        whisper_model: whisper_model.map(|p| p.to_string_lossy().into_owned()),
        ytdlp_version,
        ffmpeg_version,
        ffmpeg_path: ffmpeg_path.map(|p| p.to_string_lossy().into_owned()),
    }
}
