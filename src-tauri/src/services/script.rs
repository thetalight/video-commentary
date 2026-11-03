//! Subtitle-anchored shots, narration scripts and their deterministic contracts.
use super::{
    jobs,
    subtitle::{self, secs_to_time, SubtitleEntry},
    tts::WordBoundary,
};
use crate::models::config::ExcludedRange;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Picture needed per narrated character at native speed (4 chars/s plus 20% reserve).
const PICTURE_PER_CHAR: f64 = 1.2 / 4.0;
const MIN_CUT_SECS: f64 = 1.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shot {
    pub id: usize,
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paragraph {
    pub narration: String,
    pub evidence_quote: String,
    pub shot_ids: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Script {
    pub title: String,
    pub paragraphs: Vec<Paragraph>,
}

impl Script {
    pub fn chars(&self) -> usize {
        self.paragraphs
            .iter()
            .map(|p| p.narration.chars().count())
            .sum()
    }
}

pub fn transcript_revision(entries: &[SubtitleEntry]) -> String {
    jobs::stable_hash(&[&serde_json::to_vec(entries).unwrap_or_default()])
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

// Derived text only. Original subtitles remain byte-for-byte untouched.
fn clean_text(text: &str) -> String {
    let labels = regex::Regex::new(r"说话人\s*\d+\s*[:：]?").expect("static regex");
    let text = labels.replace_all(text, "");
    text.split_whitespace()
        .filter(|word| {
            let token = normalized(word);
            !token.is_empty()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every subtitle-anchored picture window in the film outside intro, outro and
/// ad ranges. Adjacent cues merge into windows of at most 16 seconds; ids equal
/// positions.
pub fn film_shots(
    entries: &[SubtitleEntry],
    duration: f64,
    excluded: &[ExcludedRange],
) -> Vec<Shot> {
    subtitle_windows(entries, duration, excluded, 4.0, 2)
}

/// Whole-episode indexing keeps short replies: a brief refusal can be a crucial cause.
pub fn episode_shots(
    entries: &[SubtitleEntry],
    duration: f64,
    excluded: &[ExcludedRange],
) -> Vec<Shot> {
    subtitle_windows(entries, duration, excluded, 0.0, 1)
}

fn subtitle_windows(
    entries: &[SubtitleEntry],
    duration: f64,
    excluded: &[ExcludedRange],
    min_secs: f64,
    min_chars: usize,
) -> Vec<Shot> {
    let mut shots: Vec<Shot> = Vec::new();
    for entry in entries {
        let start = subtitle::time_to_secs(&entry.start);
        let end = subtitle::time_to_secs(&entry.end);
        if !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end > duration
            || end <= start
            || end - start > 20.0
            || excluded.iter().any(|r| start < r.end && end > r.start)
        {
            continue;
        }
        let text = clean_text(&entry.text);
        if normalized(&text).chars().count() < min_chars {
            continue;
        }
        if let Some(last) = shots.last_mut() {
            if start >= last.end
                && start - last.end <= 2.0
                && end - last.start <= 16.0
                && !excluded.iter().any(|r| last.start < r.end && end > r.start)
            {
                last.end = end;
                last.text.push_str(&format!(" {text}"));
                continue;
            }
        }
        shots.push(Shot {
            id: shots.len(),
            start,
            end,
            text,
        });
    }
    shots.retain(|s| s.end - s.start >= min_secs);
    for (id, shot) in shots.iter_mut().enumerate() {
        shot.id = id;
    }
    shots
}

/// Text evidence has no minimum shot length. A 0.5s reply can establish a fact;
/// this does not make it a usable 1s visual clip. Until the v2 edit plan splits
/// evidence_refs from visual selections, validate() below still checks both.
pub fn validate_evidence(script: &Script, shots: &[Shot]) -> Result<(), String> {
    if script.paragraphs.is_empty() {
        return Err("文案没有解说段落".into());
    }
    for (i, paragraph) in script.paragraphs.iter().enumerate() {
        if paragraph.narration.trim().is_empty() || paragraph.shot_ids.is_empty() {
            return Err(format!("第{}段缺少文案或证据", i + 1));
        }
        let mut text = String::new();
        for id in &paragraph.shot_ids {
            let shot = shots
                .get(*id)
                .filter(|s| s.id == *id)
                .ok_or("文案引用了不存在的字幕证据")?;
            if !shot.start.is_finite()
                || !shot.end.is_finite()
                || shot.start < 0.0
                || shot.end <= shot.start
            {
                return Err("字幕证据时间无效".into());
            }
            text.push_str(&shot.text);
        }
        let quote = normalized(&paragraph.evidence_quote);
        // Short replies can be referenced alongside a full quote; they must not
        // be discarded from evidence merely to satisfy rendering constraints.
        // Full short utterances such as “不是” are useful evidence. Permit a
        // <4-character quote only when it equals an entire referenced cue/window,
        // not an arbitrary substring of a long line. Render validate keeps its
        // legacy >=4 rule until evidence and visuals have separate schema fields.
        let full_short_cue = !quote.is_empty()
            && paragraph
                .shot_ids
                .iter()
                .any(|id| normalized(&shots[*id].text) == quote);
        if (quote.chars().count() < 4 && !full_short_cue) || !normalized(&text).contains(&quote) {
            return Err(format!("第{}段证据引文无法在所选字幕中找到", i + 1));
        }
    }
    Ok(())
}

pub fn validate(script: &Script, shots: &[Shot]) -> Result<(), String> {
    validate_evidence(script, shots)?;
    if script.paragraphs.is_empty() {
        return Err("文案没有解说段落".into());
    }
    let mut used = HashSet::new();
    let mut used_ranges: Vec<(f64, f64)> = Vec::new();
    for (i, p) in script.paragraphs.iter().enumerate() {
        if p.narration.trim().is_empty() || p.shot_ids.is_empty() {
            return Err(format!("第 {} 段缺少文案或画面", i + 1));
        }
        let mut evidence = String::new();
        let mut duration = 0.0;
        let mut previous_end = 0.0;
        for id in &p.shot_ids {
            let shot = shots
                .get(*id)
                .filter(|s| s.id == *id)
                .ok_or("文案引用了不存在的字幕镜头")?;
            if !shot.start.is_finite()
                || !shot.end.is_finite()
                || shot.start < 0.0
                || shot.end - shot.start < 0.8
                || used_ranges
                    .iter()
                    .any(|(a, b)| shot.start < *b && shot.end > *a)
            {
                return Err("画面窗口短于0.8秒、时间无效或重叠；短字幕可作证据，但当前旧版共用shot_ids，需在后续独立剪辑计划中配置相关完整画面，不能丢弃证据或放宽剪辑门槛".into());
            }
            used_ranges.push((shot.start, shot.end));
            if !used.insert(*id) || shot.start < previous_end {
                return Err("镜头重复或段内时间倒序，请重新选片".into());
            }
            previous_end = shot.end;
            duration += shot.end - shot.start;
            evidence.push_str(&shot.text);
        }
        let quote = normalized(&p.evidence_quote);
        if quote.chars().count() < 4 || !normalized(&evidence).contains(&quote) {
            return Err(format!("第 {} 段证据引文无法在所选字幕中找到", i + 1));
        }
        if duration < p.narration.chars().count() as f64 * PICTURE_PER_CHAR {
            return Err(format!(
                "第 {} 段画面储备不足，请增加同一情节的镜头，不能靠定格补时",
                i + 1
            ));
        }
    }
    Ok(())
}

/// Remove a reused shot only when its paragraph still has valid evidence and
/// enough native-speed picture. This handles a common mechanical model error
/// without asking the model to rewrite otherwise valid narration.
pub fn drop_redundant_reused_shots(plan: &mut Script, shots: &[Shot]) {
    loop {
        let mut changed = false;
        let ids: Vec<usize> = plan
            .paragraphs
            .iter()
            .flat_map(|p| p.shot_ids.iter().copied())
            .collect();
        for id in ids {
            let uses = plan
                .paragraphs
                .iter()
                .map(|p| p.shot_ids.iter().filter(|shot_id| **shot_id == id).count())
                .sum::<usize>();
            if uses < 2 {
                continue;
            }
            for index in 0..plan.paragraphs.len() {
                let Some(position) = plan.paragraphs[index]
                    .shot_ids
                    .iter()
                    .position(|shot_id| *shot_id == id)
                else {
                    continue;
                };
                let mut paragraph = plan.paragraphs[index].clone();
                paragraph.shot_ids.remove(position);
                let isolated = Script {
                    title: plan.title.clone(),
                    paragraphs: vec![paragraph.clone()],
                };
                if validate(&isolated, shots).is_ok() {
                    plan.paragraphs[index] = paragraph;
                    changed = true;
                    break;
                }
            }
            if changed {
                break;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Distribute actual voice duration over selected shots at native speed; never pad/freeze.
pub fn timing(shots: &[Shot], duration: f64) -> Result<Vec<(f64, f64)>, String> {
    let available: f64 = shots.iter().map(|s| s.end - s.start).sum();
    if !duration.is_finite() || duration <= 0.0 || duration > available {
        return Err("实测配音长于所选画面，请补选镜头或缩短这一段文案；不使用定格或慢放".into());
    }
    let ratio = duration / available;
    let ranges: Vec<_> = shots
        .iter()
        .map(|s| (s.start, s.start + (s.end - s.start) * ratio))
        .collect();
    if ranges.iter().any(|(a, b)| b - a < MIN_CUT_SECS) {
        return Err("镜头过碎，请减少这一段的选片数量".into());
    }
    Ok(ranges)
}

/// Captions follow estimated word boundaries: a new line starts at a pause or
/// before `max_chars` would be exceeded.
pub fn captions(
    words: &[WordBoundary],
    offset: f64,
    first: u32,
    max_chars: usize,
) -> Vec<SubtitleEntry> {
    let mut result = Vec::new();
    let mut text = String::new();
    let mut start = 0.0;
    let mut end = 0.0;
    let flush = |text: &mut String, start: f64, end: f64, result: &mut Vec<SubtitleEntry>| {
        result.push(SubtitleEntry {
            index: first + result.len() as u32,
            start: secs_to_time(offset + start),
            end: secs_to_time(offset + end),
            text: std::mem::take(text),
        });
    };
    for word in words {
        if !text.is_empty()
            && (text.chars().count() + word.text.chars().count() > max_chars
                || word.start - end > 0.45)
        {
            flush(&mut text, start, end, &mut result);
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
        flush(&mut text, start, end, &mut result);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shot(id: usize, start: f64, end: f64) -> Shot {
        Shot {
            id,
            start,
            end,
            text: "父亲离开以后没有回来".into(),
        }
    }

    #[test]
    fn episode_index_keeps_short_replies_that_legacy_short_selection_discards() {
        let entries = vec![SubtitleEntry {
            index: 1,
            start: "00:00:10,000".into(),
            end: "00:00:11,000".into(),
            text: "不".into(),
        }];
        assert!(film_shots(&entries, 60.0, &[]).is_empty());
        let shots = episode_shots(&entries, 60.0, &[]);
        assert_eq!(shots.len(), 1);
        assert_eq!(shots[0].id, 0);
        assert_eq!(shots[0].text, "不");
    }

    #[test]
    fn redundant_reused_shot_is_removed_without_losing_evidence() {
        let shots = vec![shot(0, 0.0, 16.0), shot(1, 16.0, 32.0), shot(2, 32.0, 48.0)];
        let mut plan = Script {
            title: "例子".into(),
            paragraphs: vec![
                Paragraph {
                    narration: "故事开始了，父亲离开以后没有回来".into(),
                    evidence_quote: "父亲离开以后没有回来".into(),
                    shot_ids: vec![0, 1],
                },
                Paragraph {
                    narration: "第二段又提到父亲离开以后没有回来".into(),
                    evidence_quote: "父亲离开以后没有回来".into(),
                    shot_ids: vec![1, 2],
                },
            ],
        };
        drop_redundant_reused_shots(&mut plan, &shots);
        assert!(validate(&plan, &shots).is_ok());
        assert_eq!(plan.paragraphs[0].shot_ids, vec![0]);
    }

    fn word(text: &str, start: f64, end: f64) -> WordBoundary {
        WordBoundary {
            text: text.into(),
            start,
            end,
        }
    }

    #[test]
    fn montage_never_extends_source_or_freezes() {
        let shots = vec![shot(0, 10.0, 20.0), shot(1, 30.0, 40.0)];
        assert_eq!(
            timing(&shots, 12.0).unwrap(),
            vec![(10.0, 16.0), (30.0, 36.0)]
        );
        assert!(timing(&shots, 21.0).is_err());
        assert!(timing(&shots, f64::NAN).is_err());
    }

    #[test]
    fn rejects_missing_evidence_and_duplicate_shots() {
        let shots = vec![shot(0, 0.0, 10.0)];
        let mut script = Script {
            title: "test".into(),
            paragraphs: vec![Paragraph {
                narration: "父亲离开后，再也没有回来。".into(),
                evidence_quote: "父亲离开以后".into(),
                shot_ids: vec![0],
            }],
        };
        assert!(validate(&script, &shots).is_ok());
        script.paragraphs[0].shot_ids.push(0);
        assert!(validate(&script, &shots).is_err());
        script.paragraphs[0].shot_ids = vec![0];
        script.paragraphs[0].evidence_quote = "登山者折断双腿".into();
        assert!(validate(&script, &shots).is_err());
    }

    #[test]
    fn half_second_denial_is_valid_evidence_but_not_a_valid_visual_cut() {
        let shots = vec![
            Shot {
                id: 0,
                start: 10.0,
                end: 20.0,
                text: "他问她是不是要离开".into(),
            },
            Shot {
                id: 1,
                start: 24.0,
                end: 24.5,
                text: "不是".into(),
            },
        ];
        let plan = Script {
            title: "否定回答".into(),
            paragraphs: vec![Paragraph {
                narration: "她否认自己要离开。".into(),
                evidence_quote: "她是不是要离开".into(),
                shot_ids: vec![0, 1],
            }],
        };
        validate_evidence(&plan, &shots).unwrap();
        let mut short_quote = plan.clone();
        short_quote.paragraphs[0].evidence_quote = "不是".into();
        validate_evidence(&short_quote, &shots).unwrap();
        short_quote.paragraphs[0].evidence_quote = "她是".into();
        assert!(validate_evidence(&short_quote, &shots).is_err());
        assert!(validate(&plan, &shots).unwrap_err().contains("画面窗口"));
        assert!(timing(&[shots[1].clone()], 0.4).is_err());
        let cue = SubtitleEntry {
            index: 1,
            start: "00:00:24,000".into(),
            end: "00:00:24,500".into(),
            text: "不是".into(),
        };
        assert_eq!(episode_shots(&[cue], 30.0, &[])[0].text, "不是");
    }

    #[test]
    fn noise_cleanup_is_derived_and_keeps_english_words() {
        assert_eq!(
            clean_text("说话人001 The. 父亲没有回来。"),
            "The. 父亲没有回来。"
        );
        assert_eq!(clean_text("说话人001：父亲没有回来。"), "父亲没有回来。");
        assert_eq!(
            clean_text("The key is in the drawer."),
            "The key is in the drawer."
        );
    }

    #[test]
    fn chinese_narration_uses_original_english_evidence_without_translation() {
        let entries = vec![SubtitleEntry {
            index: 1,
            start: "00:00:01,000".into(),
            end: "00:00:15,000".into(),
            text: "The key is in the drawer.".into(),
        }];
        let original = entries[0].text.clone();
        let shots = episode_shots(&entries, 30.0, &[]);
        let mut plan = Script {
            title: "寻找钥匙".into(),
            paragraphs: vec![Paragraph {
                narration: "钥匙就在抽屉里，他终于有了寻找的方向。".into(),
                evidence_quote: "The key is in the drawer.".into(),
                shot_ids: vec![0],
            }],
        };
        validate(&plan, &shots).unwrap();
        assert_eq!(entries[0].text, original);
        plan.paragraphs[0].evidence_quote = "钥匙在抽屉里".into();
        assert!(
            validate(&plan, &shots).is_err(),
            "a Chinese paraphrase is not a quote of the English source"
        );
    }

    #[test]
    fn excluded_ranges_and_short_cues_never_become_shots() {
        let cue = |i: u32, s: f64, e: f64| SubtitleEntry {
            index: i,
            start: secs_to_time(s),
            end: secs_to_time(e),
            text: "她把钥匙藏进了鞋里".into(),
        };
        let entries = vec![cue(1, 0.0, 5.0), cue(2, 10.0, 15.0), cue(3, 30.0, 31.0)];
        let excluded = vec![ExcludedRange {
            start: 0.0,
            end: 6.0,
            kind: "片头".into(),
        }];
        let shots = film_shots(&entries, 60.0, &excluded);
        assert_eq!(shots.len(), 1);
        assert_eq!((shots[0].start, shots[0].id), (10.0, 0));
    }

    #[test]
    fn captions_break_at_width_and_pauses() {
        let words = vec![
            word("她把", 0.0, 0.4),
            word("钥匙", 0.4, 0.8),
            word("藏进", 0.8, 1.2),
            word("鞋里", 2.0, 2.4),
        ];
        let lines = captions(&words, 10.0, 1, 4);
        let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["她把钥匙", "藏进", "鞋里"]);
        assert_eq!(lines[0].start, "00:00:10,000");
        assert_eq!(lines[2].index, 3);
    }
}
