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
    /// Bottom share of the source picture cropped away to hide its burned-in subtitles.
    #[serde(default = "default_source_caption_mask")]
    pub source_caption_mask_percent: u32,
    /// Optional local music file. Empty keeps music disabled.
    #[serde(default)]
    pub background_music_path: String,
    #[serde(default = "default_background_music_db")]
    pub background_music_db: f64,
    #[serde(default = "default_style")]
    pub default_style: String,
    /// Legacy fields retained for old settings only; DeepSeek reads backend .env.
    #[serde(default = "default_ollama_base_url")]
    pub ollama_base_url: String,
    /// Ignored by AI requests; kept for deserialization compatibility.
    #[serde(default)]
    pub ollama_model: String,
    #[serde(default = "default_true")]
    pub skip_intro_outro: bool,
    #[serde(default = "default_true")]
    pub skip_ads: bool,
    /// 平台字幕与内封字幕都没有时，用本机 whisper 从音轨转写（不联网）。
    #[serde(default = "default_true")]
    pub asr_fallback: bool,
    /// 空 = 自动在 models/ 里挑一个 ggml 模型。
    #[serde(default)]
    pub asr_model_path: String,
    /// 空 = auto（让 whisper 自己判断语种）。
    #[serde(default)]
    pub asr_language: String,
}

fn default_video_quality() -> String {
    "best".to_string()
}

fn default_source_caption_mask() -> u32 {
    18
}

fn default_tts_voice() -> String {
    "Tingting".to_string()
}

fn default_original_audio_mode() -> String {
    "duck".to_string()
}

fn default_duck_db() -> f64 {
    -20.0
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
            source_caption_mask_percent: default_source_caption_mask(),
            background_music_path: String::new(),
            background_music_db: default_background_music_db(),
            default_style: default_style(),
            ollama_base_url: default_ollama_base_url(),
            ollama_model: String::new(),
            skip_intro_outro: true,
            skip_ads: true,
            asr_fallback: true,
            asr_model_path: String::new(),
            asr_language: String::new(),
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
pub struct DependencyStatus {
    pub ytdlp: bool,
    pub ffmpeg: bool,
    pub ffmpeg_burn: bool,
    pub local_tts: bool,
    /// 本机语音转写：whisper-cli 与 ggml 模型是否都就位。
    #[serde(default)]
    pub whisper: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whisper_model: Option<String>,
    pub ytdlp_version: Option<String>,
    pub ffmpeg_version: Option<String>,
    pub ffmpeg_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiStatus {
    pub available: bool,
    pub model: String,
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
    #[serde(default)]
    pub excluded_ranges: Vec<ExcludedRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistingJob {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub prepare: PrepareResult,
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
    #[serde(default)]
    pub shorts_total: usize,
    #[serde(default)]
    pub shorts_rendered: usize,
}
