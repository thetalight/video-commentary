use std::process::Command;

use crate::models::config::DependencyStatus;
use crate::services::{ffmpeg, tts};

pub fn check(hard_subtitle_ocr: bool) -> DependencyStatus {
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

    DependencyStatus {
        ytdlp: ytdlp.map(|o| o.status.success()).unwrap_or(false),
        ffmpeg: ffmpeg_ok,
        ffmpeg_burn,
        hard_subtitle_ocr,
        edge_tts: tts::is_edge_tts_available(),
        ytdlp_version,
        ffmpeg_version,
        ffmpeg_path: ffmpeg_path.map(|p| p.to_string_lossy().into_owned()),
    }
}
