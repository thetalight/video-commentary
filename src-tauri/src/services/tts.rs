//! Offline macOS speech synthesis. Caption boundaries are estimated from the
//! measured audio duration; no narration text leaves this computer.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    #[error("本机配音失败: {0}")]
    Failed(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalVoice {
    pub name: String,
    pub language: String,
}

pub fn local_voices() -> Vec<LocalVoice> {
    let Ok(output) = Command::new("say").args(["-v", "?"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let pattern = regex::Regex::new(r"^(.+?)\s{2,}(zh_(?:CN|TW|HK))\s+#").expect("voice regex");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let captures = pattern.captures(line)?;
            Some(LocalVoice {
                name: captures[1].trim().to_string(),
                language: captures[2].to_string(),
            })
        })
        .collect()
}

pub fn is_local_tts_available() -> bool {
    !local_voices().is_empty()
}

pub fn validate_voice(voice: &str) -> Result<&str, String> {
    let voice = voice.trim();
    if local_voices()
        .iter()
        .any(|candidate| candidate.name == voice)
    {
        Ok(voice)
    } else {
        Err(format!(
            "本机没有「{voice}」中文音色，请从设置中的已安装音色选择"
        ))
    }
}

pub fn preview_voice(voice: &str) -> Result<PathBuf, TtsError> {
    let voice = validate_voice(voice).map_err(TtsError::Failed)?;
    let text = "你好，这是电影解说的本机配音试听。同样的一段故事，不同的声音，会带来不同的感受。";
    let root = crate::services::jobs::inbox_root().join(".voice-previews");
    let key =
        crate::services::jobs::stable_hash(&[b"local-say-v1", voice.as_bytes(), text.as_bytes()]);
    let output = root.join(format!("{key}.wav"));
    if validate_audio(&output).is_ok() {
        return Ok(output);
    }
    let pending = PendingAudio::new(&root)?;
    let staged = pending.0.join("preview.aiff");
    synthesize(text, voice, &staged)?;
    validate_audio(&staged)?;
    let web_audio = pending.0.join("preview.wav");
    let ffmpeg = crate::services::ffmpeg::resolve_ffmpeg_bin()
        .ok_or_else(|| TtsError::Failed("未找到可运行的 FFmpeg".into()))?;
    let result = Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(&staged)
        .args(["-ac", "1", "-ar", "44100", "-c:a", "pcm_s16le"])
        .arg(&web_audio)
        .output()?;
    if !result.status.success() {
        return Err(TtsError::Failed(
            String::from_utf8_lossy(&result.stderr).trim().to_string(),
        ));
    }
    validate_audio(&web_audio)?;
    std::fs::rename(web_audio, &output)?;
    Ok(output)
}

fn synthesize(text: &str, voice: &str, output: &Path) -> Result<(), TtsError> {
    let script = output.with_extension("txt");
    std::fs::write(&script, text)?;
    let result = Command::new("say")
        .arg("-v")
        .arg(voice)
        .arg("-o")
        .arg(output)
        .arg("-f")
        .arg(&script)
        .output()?;
    let _ = std::fs::remove_file(script);
    if !result.status.success() {
        return Err(TtsError::Failed(
            String::from_utf8_lossy(&result.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

fn validate_audio(path: &Path) -> Result<f64, TtsError> {
    inspect_audio(path, None)
}
fn inspect_audio(path: &Path, report_path: Option<&Path>) -> Result<f64, TtsError> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(TtsError::Failed("配音文件为空或不是普通文件".into()));
    }
    let review = crate::services::technical_review::review_narration(path)
        .map_err(|error| TtsError::Failed(format!("独立配音检查失败：{error}")))?;
    if let Some(report_path) = report_path {
        crate::services::jobs::atomic_write(
            report_path,
            serde_json::to_vec_pretty(&review)
                .map_err(|_| TtsError::Failed("无法序列化独立配音检查报告".into()))?,
        )?;
    }
    if !review.blocking.is_empty() {
        return Err(TtsError::Failed(review.blocking.join("；")));
    }
    Ok(review.duration)
}

pub fn cached_or_synthesize(text: &str, voice: &str, output: &Path) -> Result<PathBuf, TtsError> {
    let target = output.with_extension("aiff");
    let report_path = output.with_extension("audio-review.json");
    if inspect_audio(&target, Some(&report_path)).is_ok() && read_word_boundaries(&target).is_ok() {
        return Ok(target);
    }
    let voice = validate_voice(voice).map_err(TtsError::Failed)?;
    let pending = PendingAudio::new(output.parent().unwrap_or_else(|| Path::new(".")))?;
    let staged = pending.0.join("narration.aiff");
    synthesize(text, voice, &staged)?;
    let duration = inspect_audio(&staged, Some(&report_path))?;
    let words = approximate_boundaries(text, duration);
    if words.is_empty() {
        return Err(TtsError::Failed("文案没有可配音的文字".into()));
    }
    let timing = serde_json::to_vec(&words).map_err(|e| TtsError::Failed(e.to_string()))?;
    std::fs::write(word_path(&staged), timing)?;
    let sidecar = word_path(&target);
    if sidecar.exists() {
        std::fs::remove_file(&sidecar)?;
    }
    std::fs::rename(&staged, &target)?;
    std::fs::rename(word_path(&staged), sidecar)?;
    Ok(target)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordBoundary {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

fn approximate_boundaries(text: &str, duration: f64) -> Vec<WordBoundary> {
    let mut tokens: Vec<(String, f64, f64)> = Vec::new();
    let mut ascii = String::new();
    let flush_ascii = |ascii: &mut String, tokens: &mut Vec<(String, f64, f64)>| {
        if !ascii.is_empty() {
            let weight = (ascii.len() as f64 * 0.55).max(1.0);
            tokens.push((std::mem::take(ascii), weight, 0.0));
        }
    };
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            ascii.push(ch);
            continue;
        }
        flush_ascii(&mut ascii, &mut tokens);
        if "，,、；;：:".contains(ch) {
            if let Some(last) = tokens.last_mut() {
                last.0.push(ch);
                last.2 += 0.6;
            }
        } else if "。.!！?？…".contains(ch) {
            if let Some(last) = tokens.last_mut() {
                last.0.push(ch);
                last.2 += 1.0;
            }
        } else if !ch.is_whitespace() {
            tokens.push((ch.to_string(), 1.0, 0.0));
        }
    }
    flush_ascii(&mut ascii, &mut tokens);
    if tokens.is_empty() || !duration.is_finite() || duration <= 0.0 {
        return Vec::new();
    }
    let weight: f64 = tokens.iter().map(|(_, speech, pause)| speech + pause).sum();
    let lead = duration.min(0.04);
    let budget = (duration - lead).max(0.0);
    let mut cursor = lead;
    tokens
        .into_iter()
        .map(|(text, speech, pause)| {
            let start = cursor;
            let end = (start + budget * speech / weight).min(duration);
            cursor = (end + budget * pause / weight).min(duration);
            WordBoundary { start, end, text }
        })
        .collect()
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
            return Err(TtsError::Failed("配音字幕时间轴无效".into()));
        }
        end = word.end;
    }
    if words.is_empty() {
        return Err(TtsError::Failed("缺少配音字幕时间轴".into()));
    }
    Ok(words)
}

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
    fn estimated_boundaries_are_ordered_and_cover_the_narration() {
        let words = approximate_boundaries("他先停下。然后说 Hello。", 6.0);
        assert!(!words.is_empty());
        assert_eq!(
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>(),
            "他先停下。然后说Hello。"
        );
        let mut previous = 0.0;
        for word in &words {
            assert!(word.start >= previous);
            assert!(word.end > word.start);
            previous = word.end;
        }
        assert!(words.last().unwrap().end <= 6.0);
    }

    #[test]
    fn no_text_produces_no_boundaries() {
        assert!(approximate_boundaries("  ，。  ", 3.0).is_empty());
    }

    #[test]
    #[ignore]
    fn local_voice_creates_audio_and_caption_timing() {
        let voice = local_voices()
            .into_iter()
            .find(|voice| voice.language == "zh_CN")
            .expect("install a Chinese macOS voice");
        let root = PendingAudio::new(&std::env::temp_dir()).unwrap();
        let path = cached_or_synthesize(
            "一个人先站了出来。第二个人跟上，队伍慢慢变大。",
            &voice.name,
            &root.0.join("local-smoke"),
        )
        .unwrap();
        assert!(validate_audio(&path).unwrap() > 0.0);
        assert!(!read_word_boundaries(&path).unwrap().is_empty());
    }
}
