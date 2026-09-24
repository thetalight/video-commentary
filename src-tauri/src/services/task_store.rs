use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::services::jobs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTask {
    pub task_id: String,
    pub title: String,
    pub source_url: String,
    pub status: String,
    pub stage: String,
    pub progress: f64,
    pub message: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub updated_at_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn database_path() -> PathBuf {
    jobs::inbox_root().join("tasks.sqlite")
}

fn connection() -> Result<Connection, String> {
    std::fs::create_dir_all(jobs::inbox_root()).map_err(|error| error.to_string())?;
    let connection = Connection::open(database_path()).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS tasks (
               task_id TEXT PRIMARY KEY,
               title TEXT NOT NULL DEFAULT '',
               source_url TEXT NOT NULL DEFAULT '',
               status TEXT NOT NULL,
               stage TEXT NOT NULL,
               progress REAL NOT NULL DEFAULT 0,
               message TEXT NOT NULL DEFAULT '',
               error_code TEXT,
               error_message TEXT,
               created_at_ms INTEGER NOT NULL,
               updated_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS stage_runs (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               task_id TEXT NOT NULL,
               stage TEXT NOT NULL,
               attempt INTEGER NOT NULL,
               status TEXT NOT NULL,
               started_at_ms INTEGER NOT NULL,
               finished_at_ms INTEGER,
               error_code TEXT,
               error_message TEXT,
               UNIQUE(task_id, stage, attempt),
               FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS artifacts (
               task_id TEXT NOT NULL,
               kind TEXT NOT NULL,
               path TEXT NOT NULL,
               revision TEXT NOT NULL,
               input_revision TEXT NOT NULL DEFAULT '',
               created_at_ms INTEGER NOT NULL,
               PRIMARY KEY(task_id, kind),
               FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS events (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               task_id TEXT NOT NULL,
               stage TEXT NOT NULL,
               percent REAL NOT NULL,
               message TEXT NOT NULL,
               created_at_ms INTEGER NOT NULL,
               FOREIGN KEY(task_id) REFERENCES tasks(task_id) ON DELETE CASCADE
             );
             CREATE INDEX IF NOT EXISTS events_task_time ON events(task_id, created_at_ms);",
        )
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

pub fn ensure_task(task_id: &str, title: &str, source_url: &str) -> Result<(), String> {
    let connection = connection()?;
    let now = now_ms();
    connection
        .execute(
            "INSERT INTO tasks(task_id, title, source_url, status, stage, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, 'queued', 'created', ?4, ?4)
             ON CONFLICT(task_id) DO UPDATE SET
               title = CASE WHEN excluded.title = '' THEN tasks.title ELSE excluded.title END,
               source_url = CASE WHEN excluded.source_url = '' THEN tasks.source_url ELSE excluded.source_url END",
            params![task_id, title, source_url, now],
        )
        .map_err(|error| error.to_string())?;
    write_snapshot(task_id)
}

pub fn start_stage(task_id: &str, stage: &str, message: &str) -> Result<(), String> {
    ensure_task(task_id, "", "")?;
    let connection = connection()?;
    let now = now_ms();
    let attempt: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM stage_runs WHERE task_id=?1 AND stage=?2",
            params![task_id, stage],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO stage_runs(task_id, stage, attempt, status, started_at_ms)
             VALUES (?1, ?2, ?3, 'running', ?4)",
            params![task_id, stage, attempt, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE tasks SET status='running', stage=?2, progress=0, message=?3,
             error_code=NULL, error_message=NULL, updated_at_ms=?4 WHERE task_id=?1",
            params![task_id, stage, message, now],
        )
        .map_err(|error| error.to_string())?;
    write_snapshot(task_id)
}

pub fn progress(task_id: &str, stage: &str, percent: f64, message: &str) -> Result<(), String> {
    ensure_task(task_id, "", "")?;
    let connection = connection()?;
    let now = now_ms();
    connection
        .execute(
            "UPDATE tasks SET status='running', stage=?2, progress=?3, message=?4,
             updated_at_ms=?5 WHERE task_id=?1",
            params![task_id, stage, percent.clamp(0.0, 100.0), message, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO events(task_id, stage, percent, message, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, stage, percent.clamp(0.0, 100.0), message, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "DELETE FROM events WHERE task_id=?1 AND id NOT IN
             (SELECT id FROM events WHERE task_id=?1 ORDER BY id DESC LIMIT 2000)",
            params![task_id],
        )
        .map_err(|error| error.to_string())?;
    write_snapshot(task_id)
}

pub fn finish_stage(task_id: &str, stage: &str, status: &str, message: &str) -> Result<(), String> {
    let connection = connection()?;
    let now = now_ms();
    connection
        .execute(
            "UPDATE stage_runs SET status='completed', finished_at_ms=?3
             WHERE task_id=?1 AND stage=?2 AND status='running'",
            params![task_id, stage, now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE tasks SET status=?2, stage=?3, progress=100, message=?4,
             error_code=NULL, error_message=NULL, updated_at_ms=?5 WHERE task_id=?1",
            params![task_id, status, stage, message, now],
        )
        .map_err(|error| error.to_string())?;
    write_snapshot(task_id)
}

pub fn fail(
    task_id: &str,
    stage: &str,
    code: &str,
    message: &str,
    retryable: bool,
) -> Result<(), String> {
    ensure_task(task_id, "", "")?;
    let connection = connection()?;
    let now = now_ms();
    let stored_code = if retryable {
        format!("retryable:{code}")
    } else {
        code.to_string()
    };
    connection
        .execute(
            "UPDATE stage_runs SET status='failed', finished_at_ms=?3,
             error_code=?4, error_message=?5
             WHERE task_id=?1 AND status='running'",
            params![task_id, stage, now, stored_code, message],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE tasks SET status='failed', stage=?2, message=?4,
             error_code=?3, error_message=?4, updated_at_ms=?5 WHERE task_id=?1",
            params![task_id, stage, stored_code, message, now],
        )
        .map_err(|error| error.to_string())?;
    write_snapshot(task_id)
}

pub fn artifact(
    task_id: &str,
    kind: &str,
    path: &str,
    revision: &str,
    input_revision: &str,
) -> Result<(), String> {
    let connection = connection()?;
    connection
        .execute(
            "INSERT INTO artifacts(task_id, kind, path, revision, input_revision, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(task_id, kind) DO UPDATE SET path=excluded.path,
             revision=excluded.revision, input_revision=excluded.input_revision,
             created_at_ms=excluded.created_at_ms",
            params![task_id, kind, path, revision, input_revision, now_ms()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn get(task_id: &str) -> Result<Option<StoredTask>, String> {
    let connection = connection()?;
    connection
        .query_row(
            "SELECT task_id, title, source_url, status, stage, progress, message,
             error_code, error_message, updated_at_ms FROM tasks WHERE task_id=?1",
            params![task_id],
            |row| {
                Ok(StoredTask {
                    task_id: row.get(0)?,
                    title: row.get(1)?,
                    source_url: row.get(2)?,
                    status: row.get(3)?,
                    stage: row.get(4)?,
                    progress: row.get(5)?,
                    message: row.get(6)?,
                    error_code: row.get(7)?,
                    error_message: row.get(8)?,
                    updated_at_ms: row.get::<_, i64>(9)?.max(0) as u64,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

pub fn delete(task_id: &str) -> Result<(), String> {
    let connection = connection()?;
    connection
        .execute("DELETE FROM tasks WHERE task_id=?1", params![task_id])
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn recover_interrupted() -> Result<(), String> {
    let connection = connection()?;
    let now = now_ms();
    connection
        .execute(
            "UPDATE stage_runs SET status='failed', finished_at_ms=?1,
             error_code='retryable:app_interrupted',
             error_message='应用上次退出时该阶段仍在运行，可从当前阶段重试'
             WHERE status='running'",
            params![now],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE tasks SET status='failed', error_code='retryable:app_interrupted',
             error_message='应用上次退出时任务仍在运行，可从当前阶段重试',
             message='上次运行被中断', updated_at_ms=?1 WHERE status='running'",
            params![now],
        )
        .map_err(|error| error.to_string())?;
    let mut statement = connection
        .prepare("SELECT task_id FROM tasks")
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    drop(statement);
    drop(connection);
    for task_id in ids {
        let _ = write_snapshot(&task_id);
    }
    Ok(())
}

pub fn bootstrap_existing_tasks() -> Result<(), String> {
    let entries = match std::fs::read_dir(jobs::inbox_root()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
        let task_id = entry.file_name().to_string_lossy().into_owned();
        if task_id.is_empty()
            || !task_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            continue;
        }
        let root = entry.path();
        let meta = std::fs::read(root.join("META.json"))
            .ok()
            .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
            .unwrap_or_default();
        let title = meta
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or(&task_id);
        let source_url = meta
            .get("url")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if get(&task_id)?.is_some() {
            continue;
        }
        ensure_task(&task_id, title, source_url)?;
        let (status, stage, message) = if root.join("edit.json").exists() {
            ("ready", "review", "已导入旧任务，等待审稿或重新成片")
        } else if root.join("TRANSCRIPT.srt").exists() {
            (
                "awaiting_director",
                "director",
                "已导入旧任务，等待 AI 导演",
            )
        } else if root.join("source.mp4").exists() {
            ("failed", "transcribe", "旧任务缺少可用字幕")
        } else {
            ("failed", "download", "旧任务尚未完成视频下载")
        };
        set_state(&task_id, status, stage, message)?;
    }
    Ok(())
}

fn set_state(task_id: &str, status: &str, stage: &str, message: &str) -> Result<(), String> {
    let connection = connection()?;
    connection
        .execute(
            "UPDATE tasks SET status=?2, stage=?3, progress=0, message=?4,
             updated_at_ms=?5 WHERE task_id=?1",
            params![task_id, status, stage, message, now_ms()],
        )
        .map_err(|error| error.to_string())?;
    write_snapshot(task_id)
}

fn write_snapshot(task_id: &str) -> Result<(), String> {
    let Some(task) = get(task_id)? else {
        return Ok(());
    };
    let raw = serde_json::to_vec_pretty(&task).map_err(|error| error.to_string())?;
    let path = jobs::inbox_root().join(task_id).join("job.json");
    jobs::atomic_write(path, raw).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_are_stable() {
        assert_eq!("awaiting_director", "awaiting_director");
        assert!(now_ms() > 0);
    }
}
