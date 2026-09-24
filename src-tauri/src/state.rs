use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::AppHandle;
use tauri_plugin_store::StoreExt;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

use crate::models::config::{default_output_dir, is_legacy_output_dir, AppConfig};

const STORE_PATH: &str = "settings.json";
const CONFIG_KEY: &str = "config";

pub struct AppState {
    app: AppHandle,
    task_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    download_slots: Arc<Semaphore>,
    director_slots: Arc<Semaphore>,
    subtitle_ocr_slots: Arc<Semaphore>,
    render_slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            task_locks: Mutex::new(HashMap::new()),
            download_slots: Arc::new(Semaphore::new(2)),
            director_slots: Arc::new(Semaphore::new(2)),
            subtitle_ocr_slots: Arc::new(Semaphore::new(1)),
            render_slots: Arc::new(Semaphore::new(1)),
        }
    }

    pub async fn lock_task(&self, task_id: &str) -> OwnedMutexGuard<()> {
        let lock = self
            .task_locks
            .lock()
            .expect("task lock registry poisoned")
            .entry(task_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        lock.lock_owned().await
    }

    pub async fn acquire_download(&self) -> Result<OwnedSemaphorePermit, String> {
        self.download_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "下载调度器已关闭".to_string())
    }

    pub async fn acquire_director(&self) -> Result<OwnedSemaphorePermit, String> {
        self.director_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "AI 导演调度器已关闭".to_string())
    }

    pub async fn acquire_subtitle_ocr(&self) -> Result<OwnedSemaphorePermit, String> {
        self.subtitle_ocr_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "字幕识别调度器已关闭".to_string())
    }

    pub async fn acquire_render(&self) -> Result<OwnedSemaphorePermit, String> {
        self.render_slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "渲染调度器已关闭".to_string())
    }

    fn is_dir_writable(path: &str) -> bool {
        let dir = Path::new(path);
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
        let probe = dir.join(".write_test");
        match std::fs::File::create(&probe).and_then(|mut f| f.write_all(b"ok")) {
            Ok(()) => {
                let _ = std::fs::remove_file(probe);
                true
            }
            Err(_) => false,
        }
    }

    fn is_unsafe_output_dir(path: &str) -> bool {
        if let Some(downloads) = dirs::download_dir() {
            if Path::new(path) == downloads.as_path() {
                return true;
            }
        }
        false
    }

    pub fn sanitize_output_dir(config: &mut AppConfig) {
        // v2 stores every task under the repository inbox. Keep the setting for
        // reading/migrating old `<output>/jobs` directories, but never use it as
        // the destination for new task resources.
        if config.output_dir.trim().is_empty()
            || Self::is_unsafe_output_dir(&config.output_dir)
            || is_legacy_output_dir(&config.output_dir)
            || !Self::is_dir_writable(&config.output_dir)
        {
            config.output_dir = default_output_dir();
        }
    }

    fn sanitize_director(config: &mut AppConfig) {
        // Every AI stage is local-only and the settings schema no longer contains
        // cloud provider fields. Deserializing and saving an old record purges them.
        let base = config.ollama_base_url.trim().trim_end_matches('/');
        config.ollama_base_url =
            if base == "http://localhost:11434" || base == "http://127.0.0.1:11434" {
                base.to_string()
            } else {
                "http://127.0.0.1:11434".to_string()
            };
        config.ollama_model = config.ollama_model.trim().to_string();
        config.ollama_vision_model = config.ollama_vision_model.trim().to_string();
    }

    pub async fn get_config(&self) -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
        let store = self.app.store(STORE_PATH)?;
        let (mut config, had_cloud_settings) = if let Some(value) = store.get(CONFIG_KEY) {
            let had_cloud = value.get("ai_provider").is_some()
                || value.get("aliyun_base_url").is_some()
                || value.get("aliyun_api_key").is_some()
                || value.get("aliyun_model").is_some();
            (
                serde_json::from_value::<AppConfig>(value.clone())?,
                had_cloud,
            )
        } else {
            (AppConfig::default(), false)
        };

        let before = config.output_dir.clone();
        let cookies_before = config.cookies_browser.clone();
        let _ = crate::services::jobs::migrate_all_legacy_jobs(&before);
        Self::sanitize_output_dir(&mut config);
        // Perform the one-time v1 migration before making inbox the fixed root.
        let _ = crate::services::jobs::migrate_all_legacy_jobs(&config.output_dir);
        let _ = crate::services::jobs::migrate_all_legacy_jobs(crate::services::jobs::repo_root());
        config.output_dir = default_output_dir();
        Self::sanitize_director(&mut config);
        if config.cookies_browser == "safari" {
            config.cookies_browser.clear();
        }
        if config.output_dir != before
            || config.cookies_browser != cookies_before
            || had_cloud_settings
        {
            self.save_config(config.clone()).await?;
        }
        Ok(config)
    }

    pub async fn save_config(
        &self,
        mut config: AppConfig,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if config.cookies_browser.eq_ignore_ascii_case("safari") {
            config.cookies_browser.clear();
        }
        Self::sanitize_output_dir(&mut config);
        config.output_dir = default_output_dir();
        Self::sanitize_director(&mut config);
        let store = self.app.store(STORE_PATH)?;
        store.set(CONFIG_KEY, serde_json::to_value(&config)?);
        store.save()?;
        Ok(())
    }
}
