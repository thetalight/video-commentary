use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub output_dir: String,
    /// Browser for --cookies-from-browser (e.g. "chrome"). Empty = disabled.
    #[serde(default)]
    pub cookies_browser: String,
    /// "best" | "1080" | "720"
    #[serde(default = "default_video_quality")]
    pub video_quality: String,
    #[serde(default = "default_tts_voice")]
    pub tts_voice: String,
    /// "mute" | "duck"
    #[serde(default = "default_original_audio_mode")]
    pub original_audio_mode: String,
    /// Used when original_audio_mode is duck (e.g. -20).
    #[serde(default = "default_duck_db")]
    pub duck_db: f64,
    #[serde(default = "default_burn_captions")]
    pub burn_captions: bool,
    /// Bottom picture area covered before commentary captions, 0 disables masking.
    #[serde(default = "default_source_caption_mask")]
    pub source_caption_mask_percent: u32,
    #[serde(default = "default_subtitle_font_size")]
    pub subtitle_font_size: u32,
    /// Optional local music file. Empty keeps music disabled.
    #[serde(default)]
    pub background_music_path: String,
    #[serde(default = "default_background_music_db")]
    pub background_music_db: f64,
    #[serde(default = "default_target_duration")]
    pub default_target_duration_secs: u32,
    #[serde(default = "default_style")]
    pub default_style: String,
    #[serde(default = "default_ollama_base_url")]
    pub ollama_base_url: String,
    /// Empty = automatically choose a locally installed model.
    #[serde(default)]
    pub ollama_model: String,
    /// Optional local multimodal model for rough-cut frames. Empty = auto-detect,
    /// then fall back to the director model if it also supports images.
    #[serde(default)]
    pub ollama_vision_model: String,
    #[serde(default = "default_true")]
    pub skip_intro_outro: bool,
    #[serde(default = "default_true")]
    pub skip_ads: bool,
}

fn default_video_quality() -> String {
    "best".to_string()
}

fn default_source_caption_mask() -> u32 {
    18
}

fn default_tts_voice() -> String {
    "zh-CN-XiaoxiaoNeural".to_string()
}

fn default_original_audio_mode() -> String {
    "mute".to_string()
}

fn default_duck_db() -> f64 {
    -20.0
}

fn default_burn_captions() -> bool {
    true
}

fn default_subtitle_font_size() -> u32 {
    24
}

fn default_target_duration() -> u32 {
    90
}
fn default_background_music_db() -> f64 {
    -24.0
}

fn default_style() -> String {
    "剧情解说".to_string()
}

fn default_ollama_base_url() -> String {
    "http://127.0.0.1:11434".to_string()
}

fn default_true() -> bool {
    true
}

pub fn default_output_dir() -> String {
    crate::services::jobs::inbox_root()
        .to_string_lossy()
        .into_owned()
}

pub fn is_legacy_output_dir(path: &str) -> bool {
    let path = path.trim();
    path == "/Users/Shared/Video Commentary"
        || path.ends_with("/Video Commentary") && path.contains("/Shared/")
        || dirs::home_dir()
            .is_some_and(|home| home.join("Video Commentary") == std::path::Path::new(path))
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            output_dir: default_output_dir(),
            cookies_browser: String::new(),
            video_quality: default_video_quality(),
            tts_voice: default_tts_voice(),
            original_audio_mode: default_original_audio_mode(),
            duck_db: default_duck_db(),
            burn_captions: default_burn_captions(),
            source_caption_mask_percent: default_source_caption_mask(),
            subtitle_font_size: default_subtitle_font_size(),
            background_music_path: String::new(),
            background_music_db: default_background_music_db(),
            default_target_duration_secs: default_target_duration(),
            default_style: default_style(),
            ollama_base_url: default_ollama_base_url(),
            ollama_model: String::new(),
            ollama_vision_model: String::new(),
            skip_intro_outro: true,
            skip_ads: true,
        }
    }
}

impl AppConfig {
    pub fn original_volume(&self) -> f64 {
        if self.original_audio_mode == "duck" {
            10f64.powf(self.duck_db / 20.0)
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub step: String,
    pub percent: f64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCompletePayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub output_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyStatus {
    pub ytdlp: bool,
    pub ffmpeg: bool,
    pub ffmpeg_burn: bool,
    pub hard_subtitle_ocr: bool,
    pub edge_tts: bool,
    pub ytdlp_version: Option<String>,
    pub ffmpeg_version: Option<String>,
    pub ffmpeg_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaStatus {
    pub available: bool,
    pub base_url: String,
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExcludedRange {
    pub start: f64,
    pub end: f64,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareResult {
    pub task_id: String,
    pub job_dir: String,
    pub inbox_dir: String,
    pub source_path: String,
    pub transcript_path: String,
    #[serde(default)]
    pub source_available: bool,
    #[serde(default)]
    pub transcript_available: bool,
    #[serde(default)]
    pub subtitle_source: String,
    pub title: String,
    pub duration_secs: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<crate::models::edit::EditPlan>,
    #[serde(default)]
    pub commentary: String,
    #[serde(default)]
    pub waiting_for_director: bool,
    #[serde(default)]
    pub excluded_ranges: Vec<ExcludedRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub director_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderResult {
    pub task_id: String,
    pub output_path: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistingJob {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub prepare: PrepareResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub updated_at_ms: u64,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_url: Option<String>,
}
