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

const QWEN_PREFIX: &str = "qwen3:";
pub const DEFAULT_INSTRUCTION:&str="用沉稳自然的中文电影解说口吻朗读，吐字清楚，节奏连贯，有克制的情绪变化，不喊叫，不添加台词或背景声。";
fn qwen_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.esun.video-commentary")
}
fn qwen_python() -> PathBuf {
    qwen_root().join("qwen-tts/venv/bin/python")
}
fn qwen_model() -> PathBuf {
    qwen_root().join("models/Qwen3-TTS-12Hz-1.7B-CustomVoice-bf16")
}
fn is_qwen(voice: &str) -> bool {
    voice.starts_with(QWEN_PREFIX)
}
fn qwen_speaker(voice: &str) -> Result<&str, TtsError> {
    let name = voice
        .strip_prefix(QWEN_PREFIX)
        .ok_or_else(|| TtsError::Failed("不是Qwen音色".into()))?;
    if ["Uncle_Fu", "Serena", "Vivian", "Dylan", "Eric"].contains(&name) {
        Ok(name)
    } else {
        Err(TtsError::Failed("未知Qwen中文预设音色".into()))
    }
}
pub fn qwen_available() -> bool {
    qwen_python().is_file()
        && [
            ("model.safetensors", 3833402589u64),
            ("speech_tokenizer/model.safetensors", 682293092u64),
        ]
        .iter()
        .all(|(p, n)| std::fs::metadata(qwen_model().join(p)).is_ok_and(|m| m.len() == *n))
}
fn voice_catalog() -> Vec<LocalVoice> {
    ["Uncle_Fu", "Serena", "Vivian", "Dylan", "Eric"]
        .iter()
        .map(|name| LocalVoice {
            name: format!("{QWEN_PREFIX}{name}"),
            language: "中文 · Qwen3-TTS/MLX（本机情绪指令）".into(),
        })
        .collect()
}

pub fn local_voices() -> Vec<LocalVoice> {
    let mut voices = voice_catalog();
    voices.extend(say_voices());
    voices
}
fn say_voices() -> Vec<LocalVoice> {
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
    qwen_available() || !say_voices().is_empty()
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

#[allow(dead_code)] // Legacy explicit say preview retained for compatibility.
pub fn preview_voice(voice: &str) -> Result<PathBuf, TtsError> {
    let voice = validate_voice(voice).map_err(TtsError::Failed)?;
    let text = "你好，这是电影解说的本机配音试听。同样的一段故事，不同的声音，会带来不同的感受。";
    let root = crate::services::jobs::inbox_root().join(".voice-previews");
    if is_qwen(voice) {
        let key = crate::services::jobs::stable_hash(&[b"qwen-preview-v1", voice.as_bytes()]);
        return cached_or_synthesize(text, voice, &root.join(key));
    }
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

fn synthesize_qwen(text: &str, voice: &str, instruct: &str, output: &Path) -> Result<(), TtsError> {
    qwen_speaker(voice)?;
    if !qwen_available() {
        return Err(TtsError::Failed(
            "Qwen3-TTS/MLX环境或4.52GB模型权重尚未完整安装，禁止回退say".into(),
        ));
    }
    let parent = output
        .parent()
        .ok_or_else(|| TtsError::Failed("音频路径无父目录".into()))?;
    let adapter = parent.join("qwen_adapter.py");
    let request = parent.join("qwen_request.json");
    std::fs::write(&adapter, include_str!("qwen_tts.py"))?;
    std::fs::write(
        &request,
        serde_json::to_vec(
            &serde_json::json!({"text":text,"speaker":qwen_speaker(voice)?,"instruct":instruct}),
        )
        .map_err(|_| TtsError::Failed("无法构造配音请求".into()))?,
    )?;
    let mut child = Command::new(qwen_python())
        .arg(&adapter)
        .arg("--model")
        .arg(qwen_model())
        .arg("--request")
        .arg(&request)
        .arg("--output")
        .arg(output)
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&request);
            let _ = std::fs::remove_file(&adapter);
            return Err(TtsError::Failed(
                "Qwen本机配音超过15分钟已停止，不发布不完整音频".into(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let _ = std::fs::remove_file(request);
    let _ = std::fs::remove_file(adapter);
    if !status.success() {
        return Err(TtsError::Failed(
            "Qwen3-TTS生成失败，请检查本机依赖、完整权重和段落长度；没有回退say，原始响应已隐藏"
                .into(),
        ));
    }
    Ok(())
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
    cached_with_instruction(text, voice, DEFAULT_INSTRUCTION, output)
}
pub fn cached_with_instruction(
    text: &str,
    voice: &str,
    instruct: &str,
    output: &Path,
) -> Result<PathBuf, TtsError> {
    if instruct.chars().count() > 500 {
        return Err(TtsError::Failed("配音指令不能超过500字符".into()));
    }
    let identity = if is_qwen(voice) {
        if !qwen_available() {
            return Err(TtsError::Failed(
                "Qwen3-TTS尚未就绪：权重下载未完成或隔离环境缺失，不回退say".into(),
            ));
        }
        [
            ("model.safetensors"),
            ("speech_tokenizer/model.safetensors"),
        ]
        .iter()
        .map(|p| {
            crate::services::stage_manifest::file_revision(&qwen_model().join(p))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("|")
    } else {
        "say-v1".into()
    };
    let key = crate::services::jobs::stable_hash(&[
        b"tts-provider-cache-v2",
        identity.as_bytes(),
        voice.as_bytes(),
        text.as_bytes(),
        instruct.as_bytes(),
        b"mlx-audio-0.6.4-temp0.7-seed42-budget2048",
    ]);
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let output = parent.join(key);
    let target = output.with_extension(if is_qwen(voice) { "wav" } else { "aiff" });
    let report_path = output.with_extension("audio-review.json");
    if inspect_audio(&target, Some(&report_path)).is_ok() && read_word_boundaries(&target).is_ok() {
        return Ok(target);
    }
    let voice = validate_voice(voice).map_err(TtsError::Failed)?;
    let pending = PendingAudio::new(output.parent().unwrap_or_else(|| Path::new(".")))?;
    let staged = pending.0.join(if is_qwen(voice) {
        "narration.wav"
    } else {
        "narration.aiff"
    });
    if is_qwen(voice) {
        synthesize_qwen(text, voice, instruct, &staged)?;
    } else {
        synthesize(text, voice, &staged)?;
    }
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
    fn qwen_uses_only_approved_presets_and_offline_adapter() {
        assert_eq!(qwen_speaker("qwen3:Uncle_Fu").unwrap(), "Uncle_Fu");
        assert!(qwen_speaker("qwen3:arbitrary-clone").is_err());
        assert_eq!(voice_catalog().len(), 5);
        let adapter = include_str!("qwen_tts.py");
        assert!(adapter.contains("HF_HUB_OFFLINE"));
        assert!(!adapter.contains("snapshot_download"));
        assert!(adapter.contains("generate_custom_voice"));
    }

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
