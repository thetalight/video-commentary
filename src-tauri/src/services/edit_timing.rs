//! Pure timeline planning. Uses only director-selected source intervals.
use super::subtitle::{secs_to_time, SubtitleEntry};
use crate::models::edit::{EditPlan, EditSegment};

/// Speech rate used everywhere a spoken duration has to be estimated before TTS runs.
const CHARS_PER_SECOND: f64 = 4.0;
/// Floor for very short paragraphs; measured Edge TTS rarely runs faster than this.
const MIN_SPOKEN_SECONDS: f64 = 2.5;
/// Pictures must outlast the estimated voice track by this factor, because the real
/// TTS duration is only known after synthesis and `pictures` never pads with a freeze.
const PICTURE_RESERVE_RATIO: f64 = 1.2;
/// Sub-microsecond differences are floating-point noise, not real shortfalls.
const RESERVE_EPSILON: f64 = 1e-6;

/// Estimated spoken duration of one paragraph, in seconds.
pub fn spoken_seconds(narration: &str) -> f64 {
    (narration.chars().count() as f64 / CHARS_PER_SECOND).max(MIN_SPOKEN_SECONDS)
}

/// Seconds of picture this segment can actually play at native speed.
pub fn picture_seconds(segment: &EditSegment) -> f64 {
    if segment.shots.is_empty() {
        (segment.src_end - segment.src_start).max(0.0)
    } else {
        segment
            .shots
            .iter()
            .map(|range| (range[1] - range[0]).max(0.0))
            .sum()
    }
}

/// Seconds of picture this segment should reserve before its voice track is measured.
pub fn reserve_seconds(segment: &EditSegment) -> f64 {
    spoken_seconds(&segment.narration) * PICTURE_RESERVE_RATIO
}

/// How badly one paragraph's selected pictures fall short of its own voice track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveSeverity {
    /// The estimated voice track already exceeds the available pictures: rendering
    /// this paragraph is certain to fail.
    Blocking,
    /// Pictures cover the estimate but not the 1.2x reserve, so a slower-than-expected
    /// TTS take would fail. Worth surfacing, not worth blocking a render on its own.
    Marginal,
}

#[derive(Debug, Clone)]
pub struct ReserveIssue {
    /// Zero-based position in `EditPlan::segments`.
    pub index: usize,
    pub available: f64,
    pub spoken: f64,
    pub reserve: f64,
    pub severity: ReserveSeverity,
}

impl ReserveIssue {
    /// Characters that would have to go for the paragraph to fit the pictures it has.
    pub fn excess_chars(&self) -> usize {
        let seconds = match self.severity {
            ReserveSeverity::Blocking => self.spoken - self.available,
            ReserveSeverity::Marginal => self.reserve - self.available,
        };
        (seconds.max(0.0) * CHARS_PER_SECOND).ceil() as usize
    }
}

/// Every paragraph whose pictures cannot safely carry its voice track, in plan order.
pub fn reserve_issues(plan: &EditPlan) -> Vec<ReserveIssue> {
    plan.segments
        .iter()
        .enumerate()
        .filter_map(|(index, segment)| {
            // Original-audio handoff paragraphs play the whole window and are governed
            // by `dialogue_handoff`, which needs four seconds of source dialogue left.
            if segment.keep_original_audio {
                return None;
            }
            let available = picture_seconds(segment);
            let spoken = spoken_seconds(&segment.narration);
            let reserve = spoken * PICTURE_RESERVE_RATIO;
            let severity = if spoken > available + RESERVE_EPSILON {
                ReserveSeverity::Blocking
            } else if reserve > available + RESERVE_EPSILON {
                ReserveSeverity::Marginal
            } else {
                return None;
            };
            Some(ReserveIssue {
                index,
                available,
                spoken,
                reserve,
                severity,
            })
        })
        .collect()
}

/// Render-time wording for the paragraphs that cannot be built at all. Keeps the
/// operator out of a half-finished render that dies on the first short paragraph.
pub fn blocking_reserve_message(issues: &[ReserveIssue]) -> String {
    const LISTED: usize = 8;
    let mut message = String::new();
    for issue in issues.iter().take(LISTED) {
        if !message.is_empty() {
            message.push_str("；");
        }
        message.push_str(&format!(
            "第 {} 段画面 {:.2} 秒，口播约 {:.2} 秒，差 {:.2} 秒（约 {} 字）",
            issue.index + 1,
            issue.available,
            issue.spoken,
            (issue.spoken - issue.available).max(0.0),
            issue.excess_chars()
        ));
    }
    if issues.len() > LISTED {
        message.push_str(&format!("；另有 {} 段同类问题", issues.len() - LISTED));
    }
    format!(
        "{message}。请在审稿台扩选本段画面或缩短文案，也可重新生成导演稿自动扩选；渲染按原速播放，不用定格或慢放补时。"
    )
}

pub fn dialogue_handoff(
    entries: &[SubtitleEntry],
    start: f64,
    end: f64,
    spoken: f64,
) -> Result<f64, String> {
    entries
        .iter()
        .filter_map(|e| {
            let a = super::subtitle::time_to_secs(&e.start);
            let b = super::subtitle::time_to_secs(&e.end);
            (a >= start + spoken
                && end - a >= 4.0
                && b <= end
                && b > a
                && !e.text.trim().is_empty())
            .then_some(a)
        })
        .min_by(f64::total_cmp)
        .ok_or_else(|| "引导结束后没有可容纳的原字幕对白起点，请补选原声区间或缩短引导".into())
}

pub fn aligned_captions(
    words: &[super::tts::WordBoundary],
    offset: f64,
    first: u32,
) -> Vec<SubtitleEntry> {
    let mut result = Vec::new();
    let mut text = String::new();
    let mut start = 0.0;
    let mut end = 0.0;
    for word in words {
        if !text.is_empty()
            && (text.chars().count() + word.text.chars().count() > 18 || word.start - end > 0.45)
        {
            result.push(SubtitleEntry {
                index: first + result.len() as u32,
                start: secs_to_time(offset + start),
                end: secs_to_time(offset + end),
                text: std::mem::take(&mut text),
            });
        }
        if text.is_empty() {
            start = word.start;
        } else if text.ends_with(|c: char| c.is_ascii_alphanumeric())
            && word.text.starts_with(|c: char| c.is_ascii_alphanumeric())
        {
            text.push(' ');
        }
        text.push_str(&word.text);
        end = word.end;
    }
    if !text.is_empty() {
        result.push(SubtitleEntry {
            index: first + result.len() as u32,
            start: secs_to_time(offset + start),
            end: secs_to_time(offset + end),
            text,
        });
    }
    result
}

pub fn pictures(segment: &EditSegment, duration: f64) -> Result<Vec<(f64, f64)>, String> {
    let mut shots = if segment.shots.is_empty() {
        vec![[segment.src_start, segment.src_end]]
    } else {
        segment.shots.clone()
    };
    let mut available: f64 = shots.iter().map(|r| r[1] - r[0]).sum();
    if duration.is_finite() && duration > available && !segment.shots.is_empty() {
        // The AI may identify several strong shots but underestimate real TTS duration.
        // Extend those shots into adjacent unused time inside the authorized window,
        // preserving every selected shot and source order.
        let mut missing = duration - available;
        for i in 0..shots.len() {
            let limit = shots
                .get(i + 1)
                .map(|next| next[0])
                .unwrap_or(segment.src_end);
            let extension = (limit - shots[i][1]).max(0.0).min(missing);
            shots[i][1] += extension;
            missing -= extension;
            if missing <= 0.000001 {
                break;
            }
        }
        if missing > 0.000001 {
            let extension = (shots[0][0] - segment.src_start).max(0.0).min(missing);
            shots[0][0] -= extension;
        }
        available = shots.iter().map(|r| r[1] - r[0]).sum();
    }
    if !duration.is_finite()
        || duration < 0.8
        || duration > available
        || shots
            .iter()
            .any(|r| !r[0].is_finite() || !r[1].is_finite() || r[1] - r[0] < 0.8)
    {
        return Err(format!("实测配音 {duration:.2} 秒，已选画面 {available:.2} 秒；请让 AI 调整本段文案或多镜头选片，不会定格补时"));
    }
    // Preserve source order and complete selected pictures; only the final used picture is trimmed.
    let mut remaining = duration;
    let mut ranges: Vec<(f64, f64)> = vec![];
    for [start, end] in shots {
        if remaining < 0.000001 {
            break;
        }
        let take = remaining.min(end - start);
        if take < 0.8 {
            // Avoid a tiny last flash by borrowing time from the preceding picture.
            let prev = ranges.last_mut().ok_or("没有可用镜头")?;
            let borrow = 0.8 - take;
            if prev.1 - prev.0 - borrow < 0.8 {
                return Err("镜头过碎，请重新安排选片".into());
            }
            prev.1 -= borrow;
            ranges.push((start, start + 0.8));
        } else {
            ranges.push((start, start + take));
        }
        remaining -= take;
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consumes_only_selected_pictures_at_native_speed() {
        let s: EditSegment = serde_json::from_str(
            r#"{"src_start":0,"src_end":20,"narration":"测试","shots":[[0,4],[10,15]]}"#,
        )
        .unwrap();
        assert_eq!(pictures(&s, 7.0).unwrap(), vec![(0.0, 4.0), (10.0, 13.0)]);
        assert_eq!(
            pictures(&s, 4.2).unwrap(),
            vec![(0.0, 3.4000000000000004), (10.0, 10.8)]
        );
        assert_eq!(pictures(&s, 10.0).unwrap(), vec![(0.0, 5.0), (10.0, 15.0)]);
        assert!(pictures(&s, 21.0).is_err());
    }
    #[test]
    fn invalid_multishot_plans_are_rejected() {
        for shots in ["[[0,4],[3,7]]", "[[0,30]]", "[[0,0.4]]"] {
            let raw = format!(
                r#"{{"title":"测试","segments":[{{"src_start":0,"src_end":20,"narration":"测试内容","shots":{shots}}}]}}"#
            );
            let mut plan = crate::models::edit::EditPlan::from_json_str(&raw).unwrap();
            assert!(plan.validate_and_clamp(20.0).is_err());
        }
    }
    #[test]
    fn reserve_issues_classify_blocking_and_marginal() {
        let plan: EditPlan = serde_json::from_str(
            r#"{"title":"t","segments":[
                {"src_start":0,"src_end":3.8,"shots":[[0,3.8]],"narration":"现场有人指出逝者看起来还像活着。"},
                {"src_start":10,"src_end":15,"narration":"十个字以上的叙述内容用于测试余量。"},
                {"src_start":20,"src_end":30,"keep_original_audio":true,"narration":"原声接力段不按多镜头储备规则校验。"}
            ]}"#,
        )
        .unwrap();
        let issues = reserve_issues(&plan);
        // 16 chars → spoken 4.0s over 3.8s of picture: the render cannot succeed.
        assert_eq!(issues[0].index, 0);
        assert_eq!(issues[0].severity, ReserveSeverity::Blocking);
        assert_eq!(issues[0].excess_chars(), 1);
        // 17 chars → spoken 4.25s fits 5.0s of picture but misses the 5.1s reserve.
        assert_eq!(issues[1].index, 1);
        assert_eq!(issues[1].severity, ReserveSeverity::Marginal);
        // Handoff paragraphs are governed by dialogue_handoff instead.
        assert_eq!(issues.len(), 2);
    }
    #[test]
    fn spoken_estimate_never_drops_below_the_floor() {
        assert_eq!(spoken_seconds("六个字"), 2.5);
        assert_eq!(
            spoken_seconds("一二三四五六七八九十一二三四五六七八九十"),
            5.0
        );
    }
}
