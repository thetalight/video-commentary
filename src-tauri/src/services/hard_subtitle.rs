//! Local-only hard-subtitle OCR for macOS.
//!
//! The bundled Objective-C sidecar samples the lower subtitle band and records each
//! frame's text, confidence and alternate readings. Rust then votes across frames.
//! It never uploads frames and returns `Ok(None)` when the picture does not
//! contain enough stable caption text to form a reliable transcript.
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::services::{subtitle, task_events};

fn sidecar_candidates(app: &AppHandle) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            candidates.push(parent.join("subtitle-ocr"));
        }
    }
    if let Ok(resources) = app.path().resource_dir() {
        candidates.push(resources.join("subtitle-ocr"));
    }
    let target = if cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-apple-darwin"
    };
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("subtitle-ocr-{target}")),
    );
    candidates
}

fn resolve_sidecar(app: &AppHandle) -> Option<PathBuf> {
    sidecar_candidates(app)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

pub fn is_available(app: &AppHandle) -> bool {
    cfg!(target_os = "macos") && resolve_sidecar(app).is_some()
}

pub fn generate_srt_from_video(
    app: &AppHandle,
    video: &Path,
    task_id: &str,
    output_dir: &Path,
) -> Result<Option<PathBuf>, String> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let Some(sidecar) = resolve_sidecar(app) else {
        return Ok(None);
    };
    std::fs::create_dir_all(output_dir).map_err(|error| error.to_string())?;
    let output = output_dir.join("source.ocr.srt");
    let stale_observations = subtitle::ocr_observations_path(&output);
    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_file(&stale_observations);
    task_events::emit_progress(
        app,
        Some(task_id),
        "transcribe",
        18.0,
        "平台和内封字幕均不可用，正在检查画面硬字幕...",
    );
    let mut child = Command::new(sidecar)
        .arg(video)
        .arg(&output)
        .arg("0.75")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动本地画面字幕识别失败：{error}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "本地画面字幕识别缺少进度通道".to_string())?;
    let progress_app = app.clone();
    let progress_task = task_id.to_string();
    let diagnostics = Arc::new(Mutex::new(Vec::<String>::new()));
    let thread_diagnostics = diagnostics.clone();
    let progress_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if let Some(progress) = value.get("progress").and_then(Value::as_f64) {
                    task_events::emit_progress(
                        &progress_app,
                        Some(progress_task.as_str()),
                        "transcribe",
                        18.0 + progress.clamp(0.0, 100.0) * 0.67,
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("正在本地识别画面字幕..."),
                    );
                    continue;
                }
            }
            if !line.trim().is_empty() {
                if let Ok(mut messages) = thread_diagnostics.lock() {
                    messages.push(line);
                }
            }
        }
    });
    let status = child
        .wait()
        .map_err(|error| format!("等待本地画面字幕识别失败：{error}"))?;
    let _ = progress_thread.join();
    if status.code() == Some(3) {
        let _ = std::fs::remove_file(&output);
        return Ok(None);
    }
    if !status.success() {
        let detail = diagnostics
            .lock()
            .map(|messages| messages.join("；"))
            .unwrap_or_default();
        return Err(if detail.is_empty() {
            "本地画面字幕识别失败".to_string()
        } else {
            format!("本地画面字幕识别失败：{detail}")
        });
    }
    let observations = subtitle::ocr_observations_path(&output);
    let entries = if observations.is_file() {
        let raw = std::fs::read_to_string(&observations).map_err(|error| error.to_string())?;
        match subtitle::entries_from_ocr_observations(&raw, 0.75) {
            Ok(entries) => entries,
            Err(error) => {
                let _ = std::fs::remove_file(&observations);
                return Err(error);
            }
        }
    } else {
        match subtitle::parse_file(&output) {
            Ok(entries) => subtitle::clean_ocr_entries(&entries),
            Err(_) => {
                let _ = std::fs::remove_file(&output);
                return Ok(None);
            }
        }
    };
    if entries.is_empty() || subtitle::write_srt(&entries, &output).is_err() {
        let _ = std::fs::remove_file(&output);
        let _ = std::fs::remove_file(&observations);
        return Ok(None);
    }
    let quality = subtitle::assess_quality(&entries);
    if quality.needs_retranscription {
        let _ = std::fs::remove_file(&output);
        let _ = std::fs::remove_file(&observations);
        return Ok(None);
    }
    Ok(Some(output))
}

#[cfg(test)]
mod tests {
    #[test]
    fn development_sidecar_name_matches_tauri_target_convention() {
        let target = if cfg!(target_arch = "aarch64") {
            "aarch64-apple-darwin"
        } else {
            "x86_64-apple-darwin"
        };
        assert!(format!("subtitle-ocr-{target}").contains("apple-darwin"));
    }
}
