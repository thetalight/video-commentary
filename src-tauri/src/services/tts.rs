use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("TTS 失败: {0}")]
    Failed(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
}

fn command_ok(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn is_edge_tts_available() -> bool {
    command_ok("edge-tts", &["--help"]) || command_ok("python3", &["-m", "edge_tts", "--help"])
}

pub fn validate_voice(voice: &str) -> Result<&str, String> {
    let voice = voice.trim();
    if voice.len() > 100
        || !voice.starts_with(|c: char| c.is_ascii_alphabetic())
        || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("请输入有效的 Edge TTS 音色名称，例如 zh-CN-XiaoxiaoNeural".into());
    }
    Ok(voice)
}

/// Preview only the selected Edge voice, never the system fallback or its cache.
pub fn preview_voice(voice: &str) -> Result<PathBuf, TtsError> {
    let voice = validate_voice(voice).map_err(TtsError::Failed)?;
    let text = "你好，这是电影解说音色试听。同样的一段故事，不同的声音，会带来不同的感受。";
    let root = crate::services::jobs::inbox_root().join(".voice-previews");
    let key =
        crate::services::jobs::stable_hash(&[b"edge-only-v1", voice.as_bytes(), text.as_bytes()]);
    let output = root.join(format!("{key}.mp3"));
    if validate_audio(&output).is_ok() {
        return Ok(output);
    }
    let pending = PendingAudio::new(&root)?;
    let staged = pending.0.join("preview.mp3");
    run_edge_tts(text, voice, &staged)?;
    validate_audio(&staged)?;
    std::fs::rename(staged, &output)?;
    Ok(output)
}

fn run_edge_tts(text: &str, voice: &str, output: &Path) -> Result<(), TtsError> {
    let out = output.to_str().unwrap_or_default();
    let attempts = [
        (
            "edge-tts",
            vec!["--voice", voice, "--text", text, "--write-media", out],
        ),
        (
            "python3",
            vec![
                "-m",
                "edge_tts",
                "--voice",
                voice,
                "--text",
                text,
                "--write-media",
                out,
            ],
        ),
    ];

    let mut last = String::new();
    for (bin, args) in attempts {
        // Each command must produce its own fresh staged result.
        if Path::new(out).exists() {
            std::fs::remove_file(out)?;
        }
        let output = Command::new(bin).args(&args).output();
        match output {
            Ok(o) if o.status.success() => match validate_audio(Path::new(out)) {
                Ok(()) => return Ok(()),
                Err(error) => last = format!("{bin} 已结束，但音频校验失败：{error}"),
            },
            Ok(o) => {
                last = format!(
                    "配音命令失败或输出音频无效：{} {}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(TtsError::Failed(last))
}

fn validate_audio(path: &Path) -> Result<(), TtsError> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(TtsError::Failed("配音文件为空或不是普通文件".into()));
    }
    let duration = crate::services::ffmpeg::get_duration(path)
        .map_err(|error| TtsError::Failed(format!("配音文件不可读：{error}")))?;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(TtsError::Failed("配音文件没有有效时长".into()));
    }
    Ok(())
}

pub fn cached_or_synthesize(text: &str, voice: &str, output: &Path) -> Result<PathBuf, TtsError> {
    let target = output.with_extension("mp3");
    if validate_audio(&target).is_ok() && read_word_boundaries(&target).is_ok() {
        return Ok(target);
    }
    let voice = validate_voice(voice).map_err(TtsError::Failed)?;
    let pending = PendingAudio::new(output.parent().unwrap_or_else(|| Path::new(".")))?;
    let staged = pending.0.join("aligned.mp3");
    let result = Command::new("python3")
        .args(["-c", include_str!("edge_aligned.py"), text, voice])
        .arg(&staged)
        .output()?;
    if !result.status.success() {
        return Err(TtsError::Failed(format!(
            "逐词对齐配音失败：{}；未回退为估算字幕",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    validate_audio(&staged)?;
    let words = read_word_boundaries(&staged)?;
    let duration = crate::services::ffmpeg::get_duration(&staged)
        .map_err(|e| TtsError::Failed(e.to_string()))?;
    if words.last().is_none_or(|w| w.end > duration + 0.1) {
        return Err(TtsError::Failed("配音词边界超出音频时长".into()));
    }
    // Remove the old pair's timing first so an interruption never accepts mismatched metadata.
    let sidecar = word_path(&target);
    if sidecar.exists() {
        std::fs::remove_file(&sidecar)?;
    }
    std::fs::rename(&staged, &target)?;
    std::fs::rename(word_path(&staged), sidecar)?;
    Ok(target)
}

#[derive(Debug, serde::Deserialize)]
pub struct WordBoundary {
    pub start: f64,
    pub end: f64,
    pub text: String,
}
fn word_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.words.json", path.to_string_lossy()))
}
pub fn read_word_boundaries(audio: &Path) -> Result<Vec<WordBoundary>, TtsError> {
    let words: Vec<WordBoundary> = serde_json::from_slice(&std::fs::read(word_path(audio))?)
        .map_err(|e| TtsError::Failed(e.to_string()))?;
    let mut end = 0.0;
    for word in &words {
        if !word.start.is_finite()
            || !word.end.is_finite()
            || word.start < end
            || word.end <= word.start
            || word.text.trim().is_empty()
        {
            return Err(TtsError::Failed("配音词边界无效".into()));
        }
        end = word.end;
    }
    if words.is_empty() {
        return Err(TtsError::Failed("缺少配音词边界".into()));
    }
    Ok(words)
}

// Only our own unique staging directory is cleaned up. Existing cache files
// are untouched until a replacement has been generated and validated.
struct PendingAudio(PathBuf);

impl PendingAudio {
    fn new(parent: &Path) -> Result<Self, TtsError> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(parent)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = parent.join(format!(
            ".tts-pending-{}-{stamp}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for PendingAudio {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_selected_and_custom_voice_ids() {
        assert_eq!(
            validate_voice(" zh-CN-YunxiNeural ").unwrap(),
            "zh-CN-YunxiNeural"
        );
        assert!(validate_voice("en-US-JennyNeural").is_ok());
        for bad in ["", "--help", "../../secret", "voice\n--text", "voice name"] {
            assert!(validate_voice(bad).is_err());
        }
    }

    #[test]
    fn empty_mp3_is_rejected_without_probing() {
        let root = PendingAudio::new(&std::env::temp_dir()).unwrap();
        let path = root.0.join("empty.mp3");
        std::fs::write(&path, b"").unwrap();
        assert!(validate_audio(&path)
            .unwrap_err()
            .to_string()
            .contains("为空"));
    }
}
