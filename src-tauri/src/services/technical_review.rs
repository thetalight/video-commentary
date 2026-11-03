//! Deterministic local render checks. These diagnostics do not infer story meaning.
use super::subtitle::{self, SubtitleEntry};
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
            "本地技术检查失败：{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let log = String::from_utf8_lossy(&output.stderr);
    Ok(TechnicalReview {
        duration: super::ffmpeg::get_duration(path).map_err(|e| e.to_string())?,
        freezes: spans(&log, "freeze_start", "freeze_end"),
        black_frames: spans(&log, "black_start", "black_end"),
        silences: spans(&log, "silence_start", "silence_end"),
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
    fn parses_completed_spans_only() {
        let s = "freeze_start: 1.2 freeze_end: 3.8 black_start:0 black_end:1 silence_start: 8";
        assert_eq!(spans(s, "freeze_start", "freeze_end")[0].end, 3.8);
        assert_eq!(spans(s, "silence_start", "silence_end").len(), 0);
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
