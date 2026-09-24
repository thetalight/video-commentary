use tauri::{AppHandle, Emitter};

use crate::models::config::{TaskCompletePayload, TaskProgress};
use crate::services::task_store;

pub fn emit_progress(
    app: &AppHandle,
    task_id: Option<&str>,
    step: &str,
    percent: f64,
    message: impl Into<String>,
) {
    let message = message.into();
    if let Some(task_id) = task_id {
        let _ = task_store::progress(task_id, step, percent, &message);
    }
    let _ = app.emit(
        "task-progress",
        TaskProgress {
            task_id: task_id.map(str::to_string),
            step: step.to_string(),
            percent,
            message,
        },
    );
}

pub fn emit_complete(app: &AppHandle, task_id: Option<&str>, output_path: &str) {
    if let Some(task_id) = task_id {
        let is_render = output_path.to_ascii_lowercase().ends_with(".mp4");
        let is_plan = output_path.to_ascii_lowercase().ends_with("edit.json");
        if is_render || is_plan {
            let _ = task_store::finish_stage(
                task_id,
                if is_render { "render" } else { "director" },
                if is_render { "completed" } else { "ready" },
                if is_render {
                    "成片已生成"
                } else {
                    "导演稿已生成，等待审稿"
                },
            );
        }
    }
    let _ = app.emit(
        "task-complete",
        TaskCompletePayload {
            task_id: task_id.map(str::to_string),
            output_path: output_path.to_string(),
        },
    );
}

pub fn record_failure(task_id: &str, stage: &str, code: &str, message: &str) {
    let _ = task_store::fail(task_id, stage, code, message, true);
}
