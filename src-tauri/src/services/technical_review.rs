//! Deterministic local rough-cut checks. These diagnostics do not infer story meaning.
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Command};

#[derive(Debug, Serialize, Deserialize)]
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
    pub warnings: Vec<String>,
}

impl TechnicalReview {
    #[cfg(test)]
    pub fn blocking_reasons(&self) -> Vec<String> {
        self.blocking_reasons_for_narration(&[])
    }

    /// Silence or black frames are deterministic render failures only when they
    /// overlap commentary speech. Quiet or black source footage after an
    /// original-audio handoff can be an authored pause/fade and remains a warning.
    pub fn blocking_reasons_for_narration(
        &self,
        narration: &[super::subtitle::SubtitleEntry],
    ) -> Vec<String> {
        let mut reasons = Vec::new();
        if let Some(span) = self.freezes.iter().find(|s| s.end - s.start >= 5.0) {
            reasons.push(format!(
                "{:.1}–{:.1} 秒出现超过5秒的近似静止画面",
                span.start, span.end
            ));
        }
        let black = self.black_frames.iter().find(|span| {
            span.end - span.start >= 1.5
                && (narration.is_empty() || narration_overlap_seconds(span, narration) >= 1.0)
        });
        if let Some(span) = black {
            reasons.push(format!("{:.1}–{:.1} 秒出现持续黑场", span.start, span.end));
        }
        let silence = self.silences.iter().find(|span| {
            span.end - span.start >= 5.0
                && (narration.is_empty() || narration_overlap_seconds(span, narration) >= 2.0)
        });
        if let Some(span) = silence {
            reasons.push(format!(
                "{:.1}–{:.1} 秒的解说区间出现超过5秒静音",
                span.start, span.end
            ));
        }
        reasons
    }
}

fn narration_overlap_seconds(span: &Span, narration: &[super::subtitle::SubtitleEntry]) -> f64 {
    let mut intervals = narration
        .iter()
        .filter_map(|entry| {
            let start = super::subtitle::time_to_secs(&entry.start).max(span.start);
            let end = super::subtitle::time_to_secs(&entry.end).min(span.end);
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

fn spans(stderr: &str, start_key: &str, end_key: &str) -> Vec<Span> {
    let number =
        regex::Regex::new(&format!(r"{}\s*:?\s*([0-9.]+)", regex::escape(start_key))).unwrap();
    let end = regex::Regex::new(&format!(r"{}\s*:?\s*([0-9.]+)", regex::escape(end_key))).unwrap();
    let starts = number
        .captures_iter(stderr)
        .filter_map(|c| c[1].parse().ok())
        .collect::<Vec<_>>();
    let ends = end
        .captures_iter(stderr)
        .filter_map(|c| c[1].parse().ok())
        .collect::<Vec<_>>();
    starts
        .into_iter()
        .zip(ends)
        .filter_map(|(start, end)| (end > start).then_some(Span { start, end }))
        .collect()
}

pub fn review(path: &Path) -> Result<TechnicalReview, String> {
    let ffmpeg = super::ffmpeg::resolve_ffmpeg_bin().ok_or("未找到可运行的 FFmpeg")?;
    let output = Command::new(ffmpeg)
        .args([
            "-hide_banner",
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
            "本地粗剪技术检查失败：{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let log = String::from_utf8_lossy(&output.stderr);
    let freezes = spans(&log, "freeze_start", "freeze_end");
    let black_frames = spans(&log, "black_start", "black_end");
    let silences = spans(&log, "silence_start", "silence_end");
    let duration = super::ffmpeg::get_duration(path).map_err(|e| e.to_string())?;
    let mut warnings = vec![];
    if freezes.iter().any(|s| s.end - s.start >= 3.0) {
        warnings.push("检测到超过3秒的近似静止画面；可能是原片静态构图，需结合视觉评审确认".into());
    }
    if black_frames.iter().any(|s| s.end - s.start >= 1.0) {
        warnings.push("检测到超过1秒的黑场".into());
    }
    if silences.iter().any(|s| s.end - s.start >= 3.0) {
        warnings.push("检测到超过3秒的静音".into());
    }
    Ok(TechnicalReview {
        duration,
        freezes,
        black_frames,
        silences,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_completed_spans_only() {
        let s = "freeze_start: 1.2 freeze_end: 3.8 black_start:0 black_end:1 silence_start: 8";
        assert_eq!(spans(s, "freeze_start", "freeze_end")[0].end, 3.8);
        assert_eq!(spans(s, "silence_start", "silence_end").len(), 0);
    }

    #[test]
    fn long_freeze_black_or_silence_blocks_publish() {
        let review = TechnicalReview {
            duration: 30.0,
            freezes: vec![Span {
                start: 1.0,
                end: 7.0,
            }],
            black_frames: vec![],
            silences: vec![],
            warnings: vec![],
        };
        assert!(review.blocking_reasons()[0].contains("静止画面"));
    }

    #[test]
    fn silence_only_blocks_when_commentary_should_be_audible() {
        let review = TechnicalReview {
            duration: 120.0,
            freezes: vec![],
            black_frames: vec![],
            silences: vec![Span {
                start: 78.1,
                end: 103.8,
            }],
            warnings: vec![],
        };
        let captions = vec![
            super::super::subtitle::SubtitleEntry {
                index: 1,
                start: "00:01:20,546".into(),
                end: "00:01:24,534".into(),
                text: "朋友指出老屋能免房租".into(),
            },
            super::super::subtitle::SubtitleEntry {
                index: 2,
                start: "00:01:25,546".into(),
                end: "00:01:31,821".into(),
                text: "小林以为这是人生最大的分岔点".into(),
            },
        ];
        assert!(review
            .blocking_reasons_for_narration(&captions)
            .first()
            .is_some_and(|reason| reason.contains("解说区间")));
        assert!(review.blocking_reasons_for_narration(&[]).len() == 1);
        let outside = vec![super::super::subtitle::SubtitleEntry {
            index: 3,
            start: "00:00:01,000".into(),
            end: "00:00:03,000".into(),
            text: "不在静音区间".into(),
        }];
        assert!(review.blocking_reasons_for_narration(&outside).is_empty());
    }

    #[test]
    fn black_only_blocks_when_commentary_should_be_visible() {
        let review = TechnicalReview {
            duration: 120.0,
            freezes: vec![],
            black_frames: vec![Span {
                start: 106.2,
                end: 110.5,
            }],
            silences: vec![],
            warnings: vec![],
        };
        let commentary_before_black = vec![super::super::subtitle::SubtitleEntry {
            index: 1,
            start: "00:01:14,209".into(),
            end: "00:01:18,134".into(),
            text: "引导结束，随后保留原片声音和画面".into(),
        }];
        assert!(review
            .blocking_reasons_for_narration(&commentary_before_black)
            .is_empty());

        let commentary_over_black = vec![super::super::subtitle::SubtitleEntry {
            index: 2,
            start: "00:01:46,700".into(),
            end: "00:01:49,900".into(),
            text: "此时解说仍在继续".into(),
        }];
        assert!(review
            .blocking_reasons_for_narration(&commentary_over_black)
            .first()
            .is_some_and(|reason| reason.contains("持续黑场")));
        assert!(review.blocking_reasons_for_narration(&[])[0].contains("持续黑场"));
    }
}
