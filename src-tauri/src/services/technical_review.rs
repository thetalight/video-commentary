//! Deterministic local render checks. These diagnostics do not infer story meaning.
use super::subtitle::{self, SubtitleEntry};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Command};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TechnicalReview {
    pub duration: f64,
    pub freezes: Vec<Span>,
    pub black_frames: Vec<Span>,
    pub silences: Vec<Span>,
}

impl TechnicalReview {
    /// Silence while a caption says narration is playing is a broken render and
    /// blocks publishing. Black or static picture can be genuine source footage,
    /// so it is reported for the editor to judge.
    pub fn assess(&self, narration: &[SubtitleEntry]) -> (Vec<String>, Vec<String>) {
        let blocking = self
            .silences
            .iter()
            .filter(|span| overlap_seconds(span, narration) >= 1.0)
            .map(|span| format!("{:.1}–{:.1} 秒解说应在播放却没有声音", span.start, span.end))
            .collect();
        let mut notes: Vec<String> = self
            .black_frames
            .iter()
            .filter(|span| span.end - span.start >= 1.0 && overlap_seconds(span, narration) >= 0.5)
            .map(|span| {
                format!(
                    "{:.1}–{:.1} 秒画面接近全黑，可取消勾选对应镜头",
                    span.start, span.end
                )
            })
            .collect();
        notes.extend(
            self.freezes
                .iter()
                .filter(|span| span.end - span.start >= 3.0)
                .map(|span| format!("{:.1}–{:.1} 秒画面几乎静止", span.start, span.end)),
        );
        (blocking, notes)
    }
}

fn overlap_seconds(span: &Span, narration: &[SubtitleEntry]) -> f64 {
    let mut intervals = narration
        .iter()
        .filter_map(|entry| {
            let start = subtitle::time_to_secs(&entry.start).max(span.start);
            let end = subtitle::time_to_secs(&entry.end).min(span.end);
            (end > start).then_some((start, end))
        })
        .collect::<Vec<_>>();
    intervals.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut total = 0.0;
    let mut active: Option<(f64, f64)> = None;
    for (start, end) in intervals {
        match active {
            Some((active_start, active_end)) if start <= active_end => {
                active = Some((active_start, active_end.max(end)));
            }
            Some((active_start, active_end)) => {
                total += active_end - active_start;
                active = Some((start, end));
            }
            None => active = Some((start, end)),
        }
    }
    if let Some((start, end)) = active {
        total += end - start;
    }
    total
}

/// Pair events in log order (not by zipping two independent lists). freezedetect
/// can leave an open start at EOF; silence/black detectors may emit an end there.
fn spans(stderr: &str, start_key: &str, end_key: &str, duration: f64) -> Vec<Span> {
    if !duration.is_finite() || duration <= 0.0 {
        return vec![];
    }
    let pattern = regex::Regex::new(&format!(
        r"\b({}|{})\s*:?\s*(-?\d+(?:\.\d+)?)",
        regex::escape(start_key),
        regex::escape(end_key)
    ))
    .unwrap();
    let mut active: Option<f64> = None;
    let mut result = Vec::new();
    for capture in pattern.captures_iter(stderr) {
        let Ok(value) = capture[2].parse::<f64>() else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        let value = value.clamp(0.0, duration);
        if &capture[1] == start_key {
            active = Some(active.map_or(value, |old| old.min(value)));
        } else if let Some(start) = active.take() {
            if value > start {
                result.push(Span { start, end: value });
            }
        }
    }
    if let Some(start) = active {
        if duration > start {
            result.push(Span {
                start,
                end: duration,
            });
        }
    }
    result
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NarrationReview {
    pub method: String,
    pub duration: f64,
    pub peak_db: Option<f64>,
    pub silences: Vec<Span>,
    pub audible_seconds: f64,
    pub blocking: Vec<String>,
    pub semantic_coverage_verified: bool,
}
fn narration_report(log: &str, duration: f64) -> NarrationReview {
    let pattern = regex::Regex::new(r"max_volume:\s*(-?\d+(?:\.\d+)?|-inf)\s*dB").unwrap();
    let peak = pattern
        .captures_iter(log)
        .last()
        .and_then(|c| c[1].parse::<f64>().ok())
        .filter(|n| n.is_finite());
    let silence = spans(log, "silence_start", "silence_end", duration);
    let silent_seconds: f64 = silence.iter().map(|s| s.end - s.start).sum();
    let audible = (duration - silent_seconds).max(0.0);
    let mut blocking = Vec::new();
    if !duration.is_finite() || duration <= 0.0 {
        blocking.push("独立旁白时长无效".into());
    }
    if peak.is_none_or(|db| db <= -50.0) || audible < 0.08 || silent_seconds >= duration * 0.95 {
        blocking.push("独立旁白没有足够可检测声音，禁止依靠原声或BGM掩盖".into());
    }
    for span in &silence {
        let edge = span.start <= 0.05 || span.end >= duration - 0.05;
        if span.end - span.start >= if edge { 2.0 } else { 3.0 } {
            blocking.push(format!(
                "独立旁白 {:.2}–{:.2} 秒异常静音，请试听核对",
                span.start, span.end
            ));
        }
    }
    NarrationReview {
        method: "decode+volumedetect+silencedetect(-50dB,0.2s)-v1".into(),
        duration,
        peak_db: peak,
        silences: silence,
        audible_seconds: audible,
        blocking,
        semantic_coverage_verified: false,
    }
}

/// Inspect the original TTS track only, before adding source audio or music.
/// Sound energy is NOT proof of speech recognition or complete text reading.
pub fn review_narration(path: &Path) -> Result<NarrationReview, String> {
    let duration = super::ffmpeg::get_duration(path).map_err(|e| e.to_string())?;
    let ffmpeg = super::ffmpeg::resolve_ffmpeg_bin().ok_or("未找到可运行的FFmpeg")?;
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-xerror", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-af",
            "silencedetect=n=-50dB:d=0.2,volumedetect",
            "-f",
            "null",
            "-",
        ])
        .output()
        .map_err(|_| "无法启动独立旁白解码检查".to_string())?;
    if !output.status.success() {
        return Err("独立旁白解码检查失败，不能继续混音".into());
    }
    Ok(narration_report(
        &String::from_utf8_lossy(&output.stderr),
        duration,
    ))
}

pub fn review(path: &Path) -> Result<TechnicalReview, String> {
    let ffmpeg = super::ffmpeg::resolve_ffmpeg_bin().ok_or("未找到可运行的 FFmpeg")?;
    let duration = super::ffmpeg::get_duration(path).map_err(|e| e.to_string())?;
    let output = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-xerror",
            "-i",
            path.to_str().unwrap_or_default(),
            "-vf",
            "freezedetect=n=-50dB:d=1.5,blackdetect=d=0.5:pix_th=0.10",
            "-af",
            "silencedetect=n=-45dB:d=1.5",
            "-f",
            "null",
            "-",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "本地技术检查失败：{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let log = String::from_utf8_lossy(&output.stderr);
    Ok(TechnicalReview {
        duration,
        freezes: spans(&log, "freeze_start", "freeze_end", duration),
        black_frames: spans(&log, "black_start", "black_end", duration),
        silences: spans(&log, "silence_start", "silence_end", duration),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caption(start: &str, end: &str) -> SubtitleEntry {
        SubtitleEntry {
            index: 1,
            start: start.into(),
            end: end.into(),
            text: "解说".into(),
        }
    }

    fn review(freezes: Vec<Span>, black_frames: Vec<Span>, silences: Vec<Span>) -> TechnicalReview {
        TechnicalReview {
            duration: 60.0,
            freezes,
            black_frames,
            silences,
        }
    }

    #[test]
    fn closes_open_anomalies_at_eof_without_mispairing_orphans() {
        let s="freeze_end: 0.5 freeze_start: 1.2 freeze_end: 3.8 freeze_start: 7 black_start:0 black_end:1 silence_start: 8";
        let freezes = spans(s, "freeze_start", "freeze_end", 10.0);
        assert_eq!(freezes.len(), 2);
        assert_eq!(freezes[0].end, 3.8);
        assert_eq!(freezes[1].end, 10.0);
        assert_eq!(spans(s, "silence_start", "silence_end", 10.0)[0].end, 10.0);
        assert_eq!(
            spans("black_start:8", "black_start", "black_end", 10.0)[0].end,
            10.0
        );
        assert!(spans(s, "silence_start", "silence_end", f64::NAN).is_empty());
    }

    #[test]
    fn independent_narration_blocks_silence_even_if_other_tracks_have_sound() {
        let silent = narration_report("silence_start:0\nmax_volume: -inf dB", 8.0);
        assert!(!silent.blocking.is_empty());
        assert_eq!(silent.audible_seconds, 0.0);
        assert!(!silent.semantic_coverage_verified);
        let tail = narration_report("max_volume: -8.0 dB\nsilence_start:5", 8.0);
        assert!(tail.blocking.iter().any(|s| s.contains("异常静音")));
        let healthy = narration_report("silence_start:0 silence_end:0.1 max_volume: -10.0 dB", 8.0);
        assert!(healthy.blocking.is_empty());
        assert!(!healthy.semantic_coverage_verified);
        assert!(!narration_report("unknown detector output", 8.0)
            .blocking
            .is_empty());
    }

    #[test]
    fn trailing_silence_is_blocking_only_when_narration_is_expected() {
        let spans = spans("silence_start:8", "silence_start", "silence_end", 10.0);
        let report = review(vec![], vec![], spans);
        assert!(!report
            .assess(&[caption("00:00:08,000", "00:00:10,000")])
            .0
            .is_empty());
        assert!(report
            .assess(&[caption("00:00:00,000", "00:00:07,000")])
            .0
            .is_empty());
    }

    /// Synthetic audio only; no say, source video, network, model or task writes.
    #[test]
    fn decodes_synthetic_pcm_and_rejects_all_silence_before_any_mix() {
        if super::super::ffmpeg::resolve_ffmpeg_bin().is_none() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "vc-audio-qc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let wav = |tone: bool| {
            let rate = 16000u32;
            let count = rate as usize;
            let data: Vec<u8> = (0..count)
                .flat_map(|i| {
                    let sample = if tone {
                        (8000.0 * (i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin())
                            as i16
                    } else {
                        0
                    };
                    sample.to_le_bytes()
                })
                .collect();
            let mut bytes = Vec::new();
            bytes.extend_from_slice(b"RIFF");
            bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&rate.to_le_bytes());
            bytes.extend_from_slice(&(rate * 2).to_le_bytes());
            bytes.extend_from_slice(&2u16.to_le_bytes());
            bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
            bytes.extend(data);
            bytes
        };
        let silence = root.join("silence.wav");
        let tone = root.join("tone.wav");
        std::fs::write(&silence, wav(false)).unwrap();
        std::fs::write(&tone, wav(true)).unwrap();
        let silent = review_narration(&silence).unwrap();
        assert!(!silent.blocking.is_empty());
        let audible = review_narration(&tone).unwrap();
        assert!(audible.blocking.is_empty());
        assert!(
            !audible.semantic_coverage_verified,
            "a tone is not verified speech"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn silence_blocks_only_under_narration() {
        let captions = vec![caption("00:00:10,000", "00:00:14,000")];
        let silent = |start, end| vec![Span { start, end }];
        let (blocking, _) = review(vec![], vec![], silent(11.0, 13.5)).assess(&captions);
        assert_eq!(blocking.len(), 1);
        let (blocking, _) = review(vec![], vec![], silent(14.2, 16.0)).assess(&captions);
        assert!(blocking.is_empty());
    }

    #[test]
    fn black_and_static_picture_are_notes_not_failures() {
        let captions = vec![caption("00:00:10,000", "00:00:14,000")];
        let (blocking, notes) = review(
            vec![Span {
                start: 20.0,
                end: 24.0,
            }],
            vec![Span {
                start: 9.0,
                end: 12.0,
            }],
            vec![],
        )
        .assess(&captions);
        assert!(blocking.is_empty());
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("全黑"));
    }
}
