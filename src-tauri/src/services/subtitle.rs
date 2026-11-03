use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::models::config::ExcludedRange;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleEntry {
    pub index: u32,
    pub start: String,
    pub end: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleQuality {
    pub needs_retranscription: bool,
    pub score: u8,
    pub reasons: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SubtitleError {
    #[error("Failed to read subtitle file: {0}")]
    Read(#[from] std::io::Error),
    #[error("Failed to parse subtitle: {0}")]
    Parse(String),
}

pub fn parse_file(path: &Path) -> Result<Vec<SubtitleEntry>, SubtitleError> {
    let content = std::fs::read_to_string(path)?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if ext == "vtt" {
        parse_vtt(&content)
    } else {
        parse_srt(&content)
    }
}

pub fn parse_srt(content: &str) -> Result<Vec<SubtitleEntry>, SubtitleError> {
    let mut entries = Vec::new();
    let blocks: Vec<&str> = content.trim().split("\n\n").collect();

    for block in blocks {
        let lines: Vec<&str> = block.lines().collect();
        if lines.len() < 2 {
            continue;
        }

        let (index_line, time_line, text_start) = if lines[0].contains("-->") {
            (None, lines[0], 1)
        } else if lines.len() >= 3 && lines[1].contains("-->") {
            (lines[0].parse::<u32>().ok(), lines[1], 2)
        } else {
            continue;
        };

        let times: Vec<&str> = time_line.split("-->").collect();
        if times.len() != 2 {
            continue;
        }

        let start = normalize_time(times[0].trim());
        let end = normalize_time(times[1].trim());
        let text = lines[text_start..].join("\n").trim().to_string();

        if text.is_empty() {
            continue;
        }

        entries.push(SubtitleEntry {
            index: index_line.unwrap_or(entries.len() as u32 + 1),
            start,
            end,
            text,
        });
    }

    if entries.is_empty() {
        return Err(SubtitleError::Parse("No subtitle entries found".into()));
    }

    Ok(entries)
}

pub fn parse_vtt(content: &str) -> Result<Vec<SubtitleEntry>, SubtitleError> {
    let mut srt_like = String::new();
    let mut index = 1u32;

    for block in content.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let time_line = lines.iter().find(|l| l.contains("-->"));
        if let Some(time_line) = time_line {
            let pos = lines.iter().position(|l| l == time_line).unwrap();
            let text = lines[(pos + 1)..].join("\n").trim().to_string();
            if !text.is_empty() && !text.starts_with("NOTE") {
                srt_like.push_str(&format!("{index}\n{time_line}\n{text}\n\n"));
                index += 1;
            }
        }
    }

    parse_srt(&srt_like)
}

fn normalize_time(t: &str) -> String {
    let t = t.trim().replace(',', ".");
    t.replace('.', ",")
}

pub fn to_srt(entries: &[SubtitleEntry]) -> String {
    entries
        .iter()
        .map(|e| format!("{}\n{} --> {}\n{}\n", e.index, e.start, e.end, e.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn write_srt(entries: &[SubtitleEntry], path: &Path) -> Result<(), SubtitleError> {
    std::fs::write(path, to_srt(entries))?;
    Ok(())
}

pub fn time_to_secs(t: &str) -> f64 {
    let t = t.trim().replace(',', ".");
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() != 3 {
        return 0.0;
    }
    let h: f64 = parts[0].parse().unwrap_or(0.0);
    let m: f64 = parts[1].parse().unwrap_or(0.0);
    let s: f64 = parts[2].parse().unwrap_or(0.0);
    h * 3600.0 + m * 60.0 + s
}

pub fn secs_to_time(secs: f64) -> String {
    let total_ms = (secs.max(0.0) * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

pub fn assess_quality(entries: &[SubtitleEntry]) -> SubtitleQuality {
    let leaked_prompt = entries.iter().any(|entry| {
        let text = entry.text.replace(['，', ',', '。', ' '], "");
        text.contains("请准确识别人名地名和完整句子") || text.contains("以下是影视剧对白")
    });
    if entries.len() < 20 {
        return SubtitleQuality {
            needs_retranscription: leaked_prompt,
            score: if leaked_prompt { 10 } else { 70 },
            reasons: if leaked_prompt {
                vec!["识别提示词被错误写入字幕，存在静音幻觉".to_string()]
            } else {
                Vec::new()
            },
        };
    }

    let mut meaningful_chars = 0usize;
    let mut unexpected_script_chars = 0usize;
    let mut low_information_lines = 0usize;
    let mut frequencies: HashMap<String, usize> = HashMap::new();

    for entry in entries {
        let normalized = entry
            .text
            .to_lowercase()
            .chars()
            .filter(|character| character.is_alphanumeric() || is_cjk(*character))
            .collect::<String>();
        if normalized.chars().count() < 2 {
            low_information_lines += 1;
        }
        if !normalized.is_empty() {
            *frequencies.entry(normalized).or_default() += 1;
        }
        for character in entry
            .text
            .chars()
            .filter(|character| character.is_alphanumeric() || is_cjk(*character))
        {
            meaningful_chars += 1;
            if is_unexpected_script(character) {
                unexpected_script_chars += 1;
            }
        }
    }

    let duplicate_lines: usize = frequencies
        .values()
        .map(|count| count.saturating_sub(2))
        .sum();
    let line_count = entries.len() as f64;
    let duplicate_ratio = duplicate_lines as f64 / line_count;
    let low_information_ratio = low_information_lines as f64 / line_count;
    let unexpected_ratio = unexpected_script_chars as f64 / meaningful_chars.max(1) as f64;

    let mut reasons = Vec::new();
    let mut penalty = 0u8;
    if leaked_prompt {
        reasons.push("识别提示词被错误写入字幕，存在静音幻觉".to_string());
        penalty = penalty.saturating_add(60);
    }
    if unexpected_ratio > 0.006 {
        reasons.push("字幕混入大量异常语种字符或乱码".to_string());
        penalty = penalty.saturating_add(45);
    }
    if duplicate_ratio > 0.22 {
        reasons.push("字幕存在大面积机械重复".to_string());
        penalty = penalty.saturating_add(35);
    }
    if low_information_ratio > 0.38 {
        reasons.push("过多字幕行缺少有效对白信息".to_string());
        penalty = penalty.saturating_add(30);
    }
    let adjacent = entries
        .windows(2)
        .filter(|pair| {
            let gap = time_to_secs(&pair[1].start) - time_to_secs(&pair[0].end);
            let brief = |entry: &SubtitleEntry| {
                time_to_secs(&entry.end) - time_to_secs(&entry.start) <= 0.8
            };
            (-0.2..=2.6).contains(&gap)
                && brief(&pair[0])
                && brief(&pair[1])
                && cue_texts_match(&pair[0].text, &pair[1].text)
                && normalize_cue_text(&pair[0].text) != normalize_cue_text(&pair[1].text)
        })
        .count();
    if entries.len() > 1 && adjacent as f64 / (entries.len() - 1) as f64 > 0.12 {
        reasons.push("相邻字幕仍是同一句的不同识别结果".to_string());
        penalty = penalty.saturating_add(30);
    }
    let overlays = entries
        .iter()
        .filter(|entry| is_disposable_overlay(&entry.text))
        .count();
    let cjk_lines = entries
        .iter()
        .filter(|entry| entry.text.chars().any(is_cjk))
        .count();
    if cjk_lines * 2 > entries.len() && overlays >= 8 && overlays as f64 / line_count > 0.12 {
        reasons.push("字幕混入片名、画面文字或演职员表".to_string());
        penalty = penalty.saturating_add(25);
    }

    let score = 100u8.saturating_sub(penalty);
    let severe_garble = unexpected_ratio > 0.006;
    SubtitleQuality {
        needs_retranscription: leaked_prompt || severe_garble || score < 50,
        score,
        reasons,
    }
}

fn is_cjk(character: char) -> bool {
    is_east_asian(character)
}

fn is_east_asian(character: char) -> bool {
    matches!(
        character as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF
    )
}

fn is_unexpected_script(character: char) -> bool {
    matches!(character as u32, 0x0400..=0x052F | 0x0600..=0x06FF | 0x0750..=0x077F)
}

fn is_disposable_overlay(text: &str) -> bool {
    is_visual_overlay(text) || is_credit_line(text)
}

fn normalize_cue_text(text: &str) -> String {
    text.chars()
        .filter(|character| is_east_asian(*character) || character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (row, left_char) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != right_char);
            current[column + 1] = substitution
                .min(current[column] + 1)
                .min(previous[column + 1] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn cue_texts_match(left: &str, right: &str) -> bool {
    let left = normalize_cue_text(left);
    let right = normalize_cue_text(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    let longest = left.chars().count().max(right.chars().count());
    if longest <= 2 {
        return false;
    }
    let distance = edit_distance(&left, &right);
    if longest <= 12 {
        return distance <= 1;
    }
    distance * 3 <= longest
}

fn is_visual_overlay(text: &str) -> bool {
    let east_asian = text
        .chars()
        .filter(|character| is_east_asian(*character))
        .count();
    let latin = text
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .count();
    if east_asian >= 4 && latin * 2 < east_asian {
        return false;
    }
    let uppercase = text
        .chars()
        .filter(|character| character.is_ascii_uppercase())
        .count();
    let credit_name = latin >= 6 && text.contains('-') && uppercase >= 4;
    let title_card = east_asian == 0 && latin >= 4 && uppercase * 2 >= latin;
    credit_name || title_card
}

fn is_credit_line(text: &str) -> bool {
    let uppercase = text
        .chars()
        .filter(|character| character.is_ascii_uppercase())
        .count();
    is_visual_overlay(text) && (text.contains('-') || uppercase >= 4)
}

pub fn detect_excluded_ranges(
    entries: &[SubtitleEntry],
    duration: f64,
    skip_intro_outro: bool,
    skip_ads: bool,
) -> Vec<ExcludedRange> {
    let mut ranges = Vec::new();
    if skip_intro_outro && duration >= 1_200.0 {
        let intro_markers: Vec<&SubtitleEntry> = entries
            .iter()
            .filter(|entry| time_to_secs(&entry.start) <= 180.0 && is_intro_marker(&entry.text))
            .collect();
        let intro_end = intro_markers
            .iter()
            .map(|entry| time_to_secs(&entry.end) + 4.0)
            .chain(entries.windows(2).filter_map(|pair| {
                let left = pair[0].text.trim().to_lowercase();
                let right = pair[1].text.trim().to_lowercase();
                let end = time_to_secs(&pair[1].end);
                (end <= 180.0 && left.len() >= 6 && left == right).then_some(end + 2.0)
            }))
            .fold(60.0_f64, f64::max)
            .clamp(30.0, 180.0);
        ranges.push(ExcludedRange {
            start: 0.0,
            end: intro_end,
            kind: "片头".into(),
        });

        let outro_search_start = (duration - 240.0).max(duration * 0.7);
        let marker_start = entries
            .iter()
            .filter(|entry| {
                time_to_secs(&entry.start) >= outro_search_start && is_outro_marker(&entry.text)
            })
            .map(|entry| time_to_secs(&entry.start))
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        ranges.push(ExcludedRange {
            start: marker_start.unwrap_or((duration - 90.0).max(0.0)),
            end: duration,
            kind: "片尾".into(),
        });
    }

    if skip_ads {
        for entry in entries.iter().filter(|entry| is_ad_marker(&entry.text)) {
            ranges.push(ExcludedRange {
                start: (time_to_secs(&entry.start) - 3.0).max(0.0),
                end: (time_to_secs(&entry.end) + 5.0).min(duration),
                kind: "广告".into(),
            });
        }
    }
    merge_ranges(ranges)
}

fn is_intro_marker(text: &str) -> bool {
    let normalized = text.to_lowercase();
    [
        "片头曲",
        "主题曲",
        "主题歌",
        "作词",
        "作曲",
        "演唱",
        "领衔主演",
        "♪",
        "♫",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
}

fn is_outro_marker(text: &str) -> bool {
    let normalized = text.to_lowercase();
    [
        "片尾曲",
        "下集预告",
        "演职员",
        "鸣谢",
        "制作人员",
        "本集完",
        "未完待续",
        "♪",
        "♫",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
}

fn is_ad_marker(text: &str) -> bool {
    let normalized = text.to_lowercase().replace(' ', "");
    [
        "本节目由",
        "广告之后",
        "马上回来",
        "扫描二维码",
        "扫码下载",
        "点击下载",
        "立即下载",
        "打开app",
        "下载app",
        "会员专享",
        "开通会员",
        "赞助播出",
        "品牌赞助",
        "购买链接",
        "关注公众号",
        "长按识别",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
}

fn merge_ranges(mut ranges: Vec<ExcludedRange>) -> Vec<ExcludedRange> {
    ranges.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut merged: Vec<ExcludedRange> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut() {
            if range.start <= previous.end + 5.0 && range.kind == previous.kind {
                previous.end = previous.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_srt() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello world\n\n2\n00:00:05,000 --> 00:00:08,000\nSecond line\n";
        let entries = parse_srt(srt).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "Hello world");
    }

    #[test]
    fn accepts_clean_chinese_and_english_subtitles() {
        let chinese = (0..60)
            .map(|index| SubtitleEntry {
                index,
                start: secs_to_time(index as f64),
                end: secs_to_time(index as f64 + 1.0),
                text: format!("人物在第{index}个场景做出新的选择"),
            })
            .collect::<Vec<_>>();
        let english = (0..60)
            .map(|index| SubtitleEntry {
                index,
                start: secs_to_time(index as f64),
                end: secs_to_time(index as f64 + 1.0),
                text: format!("The character makes decision number {index}"),
            })
            .collect::<Vec<_>>();
        assert!(!assess_quality(&chinese).needs_retranscription);
        assert!(!assess_quality(&english).needs_retranscription);
    }

    #[test]
    fn detects_garbled_multiscript_subtitles() {
        let entries = (0..80)
            .map(|index| SubtitleEntry {
                index,
                start: secs_to_time(index as f64),
                end: secs_to_time(index as f64 + 1.0),
                text: if index % 2 == 0 {
                    "اتطبي убк mixed garble".to_string()
                } else {
                    "ah".to_string()
                },
            })
            .collect::<Vec<_>>();
        let quality = assess_quality(&entries);
        assert!(quality.needs_retranscription);
        assert!(quality.score < 60);
    }

    #[test]
    fn detects_recognition_prompt_leak_even_in_short_subtitles() {
        let entries = vec![SubtitleEntry {
            index: 1,
            start: secs_to_time(0.0),
            end: secs_to_time(5.0),
            text: "请准确识别人名，地名和完整句子。".to_string(),
        }];
        let quality = assess_quality(&entries);
        assert!(quality.needs_retranscription);
        assert!(quality.reasons[0].contains("提示词"));
    }
    fn entry(index: u32, start: f64, end: f64, text: &str) -> SubtitleEntry {
        SubtitleEntry {
            index,
            start: secs_to_time(start),
            end: secs_to_time(end),
            text: text.into(),
        }
    }

    #[test]
    fn excludes_long_form_intro_outro_and_ads() {
        let entries = vec![
            entry(1, 4.0, 8.0, "主题曲 演唱"),
            entry(2, 600.0, 604.0, "本节目由某品牌赞助播出"),
            entry(3, 2_520.0, 2_524.0, "下集预告"),
        ];
        let ranges = detect_excluded_ranges(&entries, 2_600.0, true, true);
        assert!(ranges
            .iter()
            .any(|range| range.kind == "片头" && range.start == 0.0));
        assert!(ranges.iter().any(|range| range.kind == "广告"));
        assert!(ranges
            .iter()
            .any(|range| range.kind == "片尾" && range.end == 2_600.0));
    }

    #[test]
    fn short_videos_do_not_get_forced_intro_outro_trim() {
        let entries = vec![entry(1, 0.0, 3.0, "开场")];
        assert!(detect_excluded_ranges(&entries, 120.0, true, true).is_empty());
    }

    #[test]
    fn repeated_early_lyrics_extend_intro_range() {
        let entries = vec![
            entry(1, 150.0, 160.0, "O'er the world's great"),
            entry(2, 160.0, 176.0, "O'er the world's great"),
            entry(3, 220.0, 224.0, "人物开始对话"),
        ];
        let ranges = detect_excluded_ranges(&entries, 2_600.0, true, false);
        let intro = ranges.iter().find(|range| range.kind == "片头").unwrap();
        assert!(intro.end >= 178.0);
    }
}
