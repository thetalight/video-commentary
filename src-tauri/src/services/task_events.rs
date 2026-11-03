use tauri::{AppHandle, Emitter};

use crate::models::config::TaskProgress;
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

pub fn record_failure(task_id: &str, stage: &str, code: &str, message: &str) {
    let _ = task_store::fail(task_id, stage, code, message, true);
}
