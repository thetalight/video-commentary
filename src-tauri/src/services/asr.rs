//! 本机语音转写兜底：平台字幕与内封字幕都没有时，用 whisper.cpp 从音轨生成逐字稿。
//!
//! 全程在本机完成，只依赖两样外部资源：
//! - `whisper-cli`（PATH 中，`brew install whisper-cpp`）
//! - 一个 ggml 模型（放在应用数据目录的 `models/` 下）
//!
//! 时间戳策略：字幕行边界取 whisper 的段级时间，段内按字数比例切分，
//! 再用音频能量包络把每行起点吸附到真实的语音起点（实测可把 1 秒级离群值拉回 0.2 秒量级）。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tauri::AppHandle;

use crate::services::ffmpeg;

/// 字幕行最长 4 秒，超过就在段内切分，避免出现十几秒一条的长字幕。
const MAX_CUE_SECS: f64 = 4.0;
/// 字幕行最多 40 个字符（西文按字符算，中文等价 40 字）。
const MAX_CUE_CHARS: usize = 40;
/// 起点吸附的最大距离：超过这个范围说明 whisper 的段边界本身可信。
const SNAP_WINDOW_SECS: f64 = 0.6;

#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error("找不到 whisper-cli，请先安装：brew install whisper-cpp")]
    BinaryMissing,
    #[error("找不到语音模型，请把 ggml-*.bin 放到应用数据目录的 models/ 下")]
    ModelMissing,
    #[error("语音转写失败：{0}")]
    Failed(String),
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct TranscribeOutput {
    pub subtitle_path: PathBuf,
    /// whisper 自己判断出的语言代码（如 zh / es / en）。
    pub language: String,
    pub segments: usize,
}

pub fn whisper_bin() -> Option<PathBuf> {
    let output = Command::new("which").arg("whisper-cli").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

pub fn models_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("com.esun.video-commentary").join("models"))
}

/// 优先 turbo，其次 large/medium/small，避免误用 brew 自带的玩具模型。
pub fn model_path(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        return path.is_file().then_some(path);
    }

    let dir = models_dir()?;
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"))
        })
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("ggml-") && !name.contains("for-tests")
        })
        .collect();
    let score = |path: &PathBuf| {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if name.contains("turbo") {
            0
        } else if name.contains("large") {
            1
        } else if name.contains("medium") {
            2
        } else if name.contains("small") {
            3
        } else {
            4
        }
    };
    candidates.sort_by_key(score);
    candidates.into_iter().next()
}

pub fn available(configured_model: &str) -> bool {
    whisper_bin().is_some() && model_path(configured_model).is_some()
}

/// 抽 16k 单声道 WAV：whisper-cli 只吃带头的音频文件。
fn extract_audio(video: &Path, wav_path: &Path) -> Result<(), AsrError> {
    let ffmpeg_bin =
        ffmpeg::resolve_ffmpeg().ok_or_else(|| AsrError::Failed("找不到 ffmpeg".to_string()))?;
    let output = Command::new(ffmpeg_bin)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(video)
        .args([
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-f",
            "wav",
        ])
        .arg(wav_path)
        .output()?;
    if !output.status.success() {
        return Err(AsrError::Failed(format!(
            "抽取音轨失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn run_whisper(
    model: &Path,
    audio: &Path,
    prefix: &Path,
    language: &str,
) -> Result<String, AsrError> {
    let bin = whisper_bin().ok_or(AsrError::BinaryMissing)?;
    let mut command = Command::new(bin);
    command
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(audio)
        // 完整 JSON 里带 token 级 offsets，段级时间也从这里取
        .args(["-ojf", "-ml", "0", "-sow", "-of"])
        .arg(prefix)
        .args([
            "-l",
            if language.trim().is_empty() {
                "auto"
            } else {
                language.trim()
            },
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = command.output()?;
    if !output.status.success() {
        return Err(AsrError::Failed(format!(
            "whisper-cli 退出异常：{}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .filter(|line| !line.starts_with("ggml_") && !line.starts_with("load_backend"))
                .last()
                .unwrap_or("未知错误")
        )));
    }
    Ok(String::from_utf8_lossy(&output.stderr).into_owned())
}

/// whisper 在 stderr 里以 `... auto-detected language: es (p = 0.99)` 的形式报告语种。
fn parse_detected_language(log: &str, fallback: &str) -> String {
    log.lines()
        .find_map(|line| {
            line.split("auto-detected language:")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .filter(|code| !code.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            if fallback.trim().is_empty() {
                "auto".to_string()
            } else {
                fallback.trim().to_string()
            }
        })
}

fn read_json(path: &Path) -> Result<serde_json::Value, AsrError> {
    let mut file = std::fs::File::open(path)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    serde_json::from_str(&text)
        .map_err(|error| AsrError::Failed(format!("解析转写结果失败：{error}")))
}

/// 从 WAV 里取 PCM 采样：跳过 RIFF 头，定位 `data` 块。
fn wav_samples(bytes: &[u8]) -> Vec<i16> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" {
        return Vec::new();
    }
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let body = offset + 8;
        if id == b"data" {
            let end = (body + size).min(bytes.len());
            return bytes[body..end]
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                .collect();
        }
        offset = body + size + (size & 1);
    }
    Vec::new()
}

/// 每 50ms 一帧的 RMS 包络，用来找出真实的语音起点。
fn speech_onsets(audio: &Path) -> Vec<f64> {
    let Ok(bytes) = std::fs::read(audio) else {
        return Vec::new();
    };
    let samples = wav_samples(&bytes);
    let hop = 800; // 16kHz * 50ms
    if samples.len() < hop * 4 {
        return Vec::new();
    }
    let rms: Vec<f64> = samples
        .chunks(hop)
        .map(|chunk| {
            let sum: f64 = chunk.iter().map(|s| (*s as f64) * (*s as f64)).sum();
            (sum / chunk.len() as f64).sqrt()
        })
        .collect();
    let mean = rms.iter().sum::<f64>() / rms.len() as f64;
    if mean <= 1.0 {
        return Vec::new();
    }
    let loud = mean * 1.4;
    let quiet = loud * 0.6;
    let mut onsets = Vec::new();
    let mut was_quiet = true;
    for (index, value) in rms.iter().enumerate() {
        let time = index as f64 * 0.05;
        if was_quiet && *value > loud {
            onsets.push(time);
            was_quiet = false;
        } else if !was_quiet && *value < quiet {
            was_quiet = true;
        }
    }
    onsets
}

fn snap_to_speech(start: f64, onsets: &[f64]) -> f64 {
    onsets
        .iter()
        .copied()
        .filter(|onset| (onset - start).abs() <= SNAP_WINDOW_SECS)
        .min_by(|left, right| {
            (left - start)
                .abs()
                .partial_cmp(&(right - start).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(start)
}

fn clean_token_text(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '[' => {
                // 形如 [_BEG_] / [_TT_123] 的控制标记，整段丢掉
                depth += 1;
            }
            ']' if depth > 0 => {
                depth -= 1;
            }
            _ if depth > 0 => {}
            other => out.push(other),
        }
    }
    out
}

fn has_text(text: &str) -> bool {
    // 汉字属于 Unicode 的 Lo 类，`is_alphabetic()` 也返回 true，中英文都覆盖
    text.chars().any(char::is_alphabetic)
}

/// whisper 在音乐/静音段会背出训练语料里的字幕署名或口播套话，必须拦掉。
const ARTIFACT_LINES: [&str; 11] = [
    "transcriptionbycastingwords",
    "subtitlesbytheamaraorgcommunity",
    "subtitledby",
    "amaraorg",
    "thanksforwatching",
    "pleasesubscribe",
    "likesubscribe",
    "字幕由",
    "字幕提供",
    "谢谢观看",
    "请不吝点赞",
];

fn normalize_for_match(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

fn is_artifact_line(text: &str) -> bool {
    let normalized = normalize_for_match(text);
    if normalized.is_empty() {
        return true;
    }
    ARTIFACT_LINES
        .iter()
        .any(|pattern| normalized.contains(pattern))
}

#[derive(Debug, Clone)]
struct Cue {
    start: f64,
    end: f64,
    text: String,
}

fn build_cues(value: &serde_json::Value, onsets: &[f64]) -> Vec<Cue> {
    let Some(segments) = value.get("transcription").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut cues = Vec::new();
    for segment in segments {
        let text = segment
            .get("text")
            .and_then(|v| v.as_str())
            .map(clean_token_text)
            .unwrap_or_default();
        let text = text.trim();
        let start = segment
            .pointer("/offsets/from")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            / 1000.0;
        let end = segment
            .pointer("/offsets/to")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
            / 1000.0;
        let span = end - start;
        // 丢掉退化段（0 长度）和没有文字的段落（纯音乐/噪声幻觉）
        if !has_text(text) || span < 0.2 {
            continue;
        }
        // 幻觉整段都不可信，必须在切分前判断（否则会被切成 "Transcription"/"by" 之类）
        if is_artifact_line(text) {
            continue;
        }

        let words: Vec<&str> = text.split_whitespace().collect();
        let words = if words.is_empty() { vec![text] } else { words };
        let slices = ((span / MAX_CUE_SECS).ceil() as usize)
            .min(words.len())
            .max(1);
        let per_slice = words.len().div_ceil(slices);
        let step = span / slices as f64;
        for (index, chunk) in words.chunks(per_slice).enumerate() {
            let mut line = chunk.join(" ");
            if is_artifact_line(&line) {
                continue;
            }
            if line.chars().count() > MAX_CUE_CHARS * 2 {
                line = line.chars().take(MAX_CUE_CHARS * 2).collect();
            }
            let cue_start = snap_to_speech(start + index as f64 * step, onsets).max(0.0);
            let raw_end = (start + (index as f64 + 1.0) * step).min(end);
            // 字幕行不能被 whisper 的时间拉伸拖成十几秒
            let cue_end = raw_end.min(cue_start + MAX_CUE_SECS).max(cue_start + 0.4);
            cues.push(Cue {
                start: cue_start,
                end: cue_end,
                text: line,
            });
        }
    }
    cues.sort_by(|left, right| {
        left.start
            .partial_cmp(&right.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // 吸附可能把相邻两行挤到一起，按时间顺序裁掉重叠部分
    for index in 0..cues.len().saturating_sub(1) {
        let next_start = cues[index + 1].start;
        if cues[index].end > next_start {
            cues[index].end = next_start.max(cues[index].start + 0.2);
        }
    }
    // whisper 偶尔对同一句重复输出，连续的完全重复行只留一条
    let mut deduped: Vec<Cue> = Vec::with_capacity(cues.len());
    for cue in cues {
        let repeated = deduped.last().is_some_and(|previous: &Cue| {
            normalize_for_match(&previous.text) == normalize_for_match(&cue.text)
                && cue.start - previous.end < 1.5
        });
        if !repeated {
            deduped.push(cue);
        }
    }
    deduped
}

fn format_time(seconds: f64) -> String {
    let total_ms = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        total_ms / 3_600_000,
        total_ms / 60_000 % 60,
        total_ms / 1000 % 60,
        total_ms % 1000
    )
}

fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (index, cue) in cues.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            index + 1,
            format_time(cue.start),
            format_time(cue.end),
            cue.text.replace('\n', " ")
        ));
    }
    out
}

/// 抽取音轨 → 本机语音转写 → 归一化成 SRT。产物写在 `job_dir/source.asr.srt`。
pub fn transcribe(
    view: &AppHandle,
    task_id: &str,
    video: &Path,
    job_dir: &Path,
    model: &str,
    language: &str,
    progress: impl Fn(f64, &str),
) -> Result<TranscribeOutput, AsrError> {
    let output = transcribe_core(video, job_dir, model, language, progress)?;
    crate::services::task_events::emit_progress(
        view,
        Some(task_id),
        "transcribe",
        92.0,
        format!(
            "语音转写完成：{} 条对白（{}）",
            output.segments, output.language
        ),
    );
    Ok(output)
}

/// 不依赖 AppHandle 的核心流程，便于直接测试。
pub fn transcribe_core(
    video: &Path,
    job_dir: &Path,
    model: &str,
    language: &str,
    progress: impl Fn(f64, &str),
) -> Result<TranscribeOutput, AsrError> {
    let model_path = model_path(model).ok_or(AsrError::ModelMissing)?;
    if whisper_bin().is_none() {
        return Err(AsrError::BinaryMissing);
    }

    let audio = job_dir.join("ASR.audio.wav");
    progress(12.0, "正在抽取音轨...");
    extract_audio(video, &audio)?;

    progress(28.0, "正在用本机语音转写（长片约需 1–2 分钟）...");
    let prefix = job_dir.join("ASR.out");
    // 上一次失败可能留下旧结果，先清掉，否则会读到过期文本
    for stale in [
        PathBuf::from(format!("{}.json", prefix.to_string_lossy())),
        PathBuf::from(format!("{}.srt", prefix.to_string_lossy())),
    ] {
        let _ = std::fs::remove_file(stale);
    }
    let log = run_whisper(&model_path, &audio, &prefix, language)?;
    let detected = parse_detected_language(&log, language);

    progress(78.0, "正在整理字幕时间轴...");
    // whisper-cli 的 -of 只是前缀，实际文件是 <prefix>.json
    let json_path = PathBuf::from(format!("{}.json", prefix.to_string_lossy()));
    let value = read_json(&json_path)?;
    let onsets = speech_onsets(&audio);
    let cues = build_cues(&value, &onsets);
    if cues.is_empty() {
        let _ = std::fs::remove_file(&audio);
        return Err(AsrError::Failed("没有识别出对白".to_string()));
    }

    let subtitle_path = job_dir.join("source.asr.srt");
    crate::services::jobs::atomic_write(&subtitle_path, to_srt(&cues).as_bytes())?;
    let _ = std::fs::remove_file(&audio);

    Ok(TranscribeOutput {
        subtitle_path,
        language: detected,
        segments: cues.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(segments: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "transcription": segments })
    }

    #[test]
    fn control_tokens_never_reach_the_transcript() {
        assert_eq!(clean_token_text("primos.[_TT_584]"), "primos.");
        assert_eq!(clean_token_text("[_BEG_] 你好"), " 你好");
    }

    #[test]
    fn degenerate_and_wordless_segments_are_dropped() {
        let value = json(serde_json::json!([
            { "text": "你好世界。", "offsets": { "from": 0, "to": 0 } },
            { "text": "[音乐]", "offsets": { "from": 1000, "to": 3000 } },
            { "text": "真的对白。", "offsets": { "from": 5000, "to": 7000 } }
        ]));

        let cues = build_cues(&value, &[]);

        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "真的对白。");
        assert!((cues[0].start - 5.0).abs() < 0.001);
    }

    #[test]
    fn long_segments_are_split_into_subtitle_sized_lines() {
        let value = json(serde_json::json!([{
            "text": "第一句话在这里 第二句接着往下说 第三句把话讲完",
            "offsets": { "from": 0, "to": 12000 }
        }]));

        let cues = build_cues(&value, &[]);

        assert!(cues.len() >= 3, "12 秒的段应被切成多条");
        assert!(cues
            .iter()
            .all(|cue| cue.end - cue.start <= MAX_CUE_SECS + 0.01));
    }

    #[test]
    fn cue_starts_snap_to_a_nearby_speech_onset() {
        // 段边界比真实开口晚了 0.4 秒，吸附后应回到 10.0
        assert!((snap_to_speech(10.4, &[10.0, 20.0]) - 10.0).abs() < 0.001);
        // 超出吸附窗口就保留原值
        assert!((snap_to_speech(10.4, &[30.0]) - 10.4).abs() < 0.001);
    }

    #[test]
    fn detected_language_is_read_from_whisper_log() {
        let log = "whisper_full_with_state: auto-detected language: es (p = 0.997815)\n";
        assert_eq!(parse_detected_language(log, ""), "es");
        // 没有自动检测时用配置值；配置也为空就记 auto
        assert_eq!(parse_detected_language("nothing here", "zh"), "zh");
        assert_eq!(parse_detected_language("", ""), "auto");
    }

    #[test]
    #[ignore = "需要真实视频 + whisper-cli + 模型，用 VC_ASR_TEST_VIDEO 指定"]
    fn transcribes_a_real_video_end_to_end() {
        let Ok(video) = std::env::var("VC_ASR_TEST_VIDEO") else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("vc-asr-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let output = transcribe_core(Path::new(&video), &dir, "", "auto", |percent, message| {
            println!("[{percent:.0}%] {message}");
        })
        .unwrap();

        let srt = std::fs::read_to_string(&output.subtitle_path).unwrap();
        assert!(srt.contains("-->"));
        assert!(output.segments > 10, "只识别出 {} 条", output.segments);
        println!("语言={} 条数={}", output.language, output.segments);
        println!("{}", srt.lines().take(16).collect::<Vec<_>>().join("\n"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whisper_credit_hallucinations_are_dropped() {
        assert!(is_artifact_line("Transcription by CastingWords"));
        assert!(is_artifact_line("字幕由 XX 字幕组提供"));
        assert!(is_artifact_line("Thanks for watching!"));
        assert!(!is_artifact_line("我们明天就出发。"));
        assert!(!is_artifact_line("Casting a wide net tonight."));
    }

    #[test]
    fn hallucinated_credit_segments_never_reach_the_transcript() {
        let value = json(serde_json::json!([
            { "text": "Transcription by CastingWords", "offsets": { "from": 0, "to": 9990 } },
            { "text": "真正的第一句对白。", "offsets": { "from": 12000, "to": 15000 } }
        ]));

        let cues = build_cues(&value, &[]);

        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "真正的第一句对白。");
    }

    #[test]
    fn a_single_word_segment_cannot_stay_on_screen_for_ten_seconds() {
        let value = json(serde_json::json!([{
            "text": "Transcription",
            "offsets": { "from": 0, "to": 9990 }
        }]));

        let cues = build_cues(&value, &[]);

        assert_eq!(cues.len(), 1);
        assert!(cues[0].end - cues[0].start <= MAX_CUE_SECS + 0.01);
    }

    #[test]
    fn srt_output_uses_standard_timestamps() {
        let srt = to_srt(&[Cue {
            start: 61.5,
            end: 63.25,
            text: "对白".to_string(),
        }]);

        assert!(srt.contains("00:01:01,500 --> 00:01:03,250"));
    }
}
