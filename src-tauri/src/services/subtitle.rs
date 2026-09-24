use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

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

pub fn build_transcript(entries: &[SubtitleEntry]) -> String {
    entries
        .iter()
        .map(|e| {
            let start = time_to_secs(&e.start);
            let end = time_to_secs(&e.end);
            format!("[{start:.1}s - {end:.1}s] {}", e.text.trim())
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    let adjacent = entries.windows(2).filter(|pair| {
        let gap = time_to_secs(&pair[1].start) - time_to_secs(&pair[0].end);
        let brief = |entry: &SubtitleEntry| {
            time_to_secs(&entry.end) - time_to_secs(&entry.start) <= 0.8
        };
        (-0.2..=2.6).contains(&gap)
            && brief(&pair[0])
            && brief(&pair[1])
            && ocr_texts_match(&pair[0].text, &pair[1].text)
            && normalize_ocr_text(&pair[0].text) != normalize_ocr_text(&pair[1].text)
    }).count();
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

/// Assemble sampled OCR frames into dialogue cues.
///
/// This only merges repeated samples and removes logos or credit rolls.
/// Character correction is a later local-model pass, not a fixed word list.
/// Platform and embedded subtitles must not pass through this function.
pub fn clean_ocr_entries(entries: &[SubtitleEntry]) -> Vec<SubtitleEntry> {
    let collapsed = entries
        .iter()
        .filter_map(|entry| {
            let text = collapse_repeated_phrase(entry.text.trim());
            (!text.is_empty()).then(|| SubtitleEntry {
                index: entry.index,
                start: entry.start.clone(),
                end: entry.end.clone(),
                text,
            })
        })
        .collect::<Vec<_>>();
    let merged = merge_similar_ocr_cues(&collapsed);
    reindex_entries(trim_ocr_overlays(&merged))
}

fn collapse_repeated_phrase(text: &str) -> String {
    let parts = text
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() == 2 && normalize_ocr_text(parts[0]) == normalize_ocr_text(parts[1]) {
        return parts[0].to_string();
    }
    text.to_string()
}

pub(crate) fn ocr_edit_is_conservative(original: &str, corrected: &str) -> bool {
    let left = normalize_ocr_text(original);
    let right = normalize_ocr_text(corrected);
    if left == right {
        return !right.is_empty() || left.is_empty();
    }
    if right.chars().count() < 2 {
        return false;
    }
    let distance = ocr_edit_distance(&left, &right);
    let longest = left.chars().count().max(right.chars().count());
    if longest <= 12 {
        return distance <= 2;
    }
    distance <= 6 && distance * 4 <= longest
}

pub(crate) fn is_disposable_overlay(text: &str) -> bool {
    is_ocr_overlay(text) || is_credit_line(text)
}

fn normalize_ocr_text(text: &str) -> String {
    text.chars()
        .filter(|character| is_east_asian(*character) || character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

fn ocr_edit_distance(left: &str, right: &str) -> usize {
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

fn ocr_texts_match(left: &str, right: &str) -> bool {
    let left = normalize_ocr_text(left);
    let right = normalize_ocr_text(right);
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
    let distance = ocr_edit_distance(&left, &right);
    if longest <= 12 {
        return distance <= 1;
    }
    distance * 3 <= longest
}

fn merge_similar_ocr_cues(entries: &[SubtitleEntry]) -> Vec<SubtitleEntry> {
    let mut clusters: Vec<Vec<SubtitleEntry>> = Vec::new();
    for entry in entries {
        let can_extend = clusters.last().is_some_and(|cluster| {
            let active = cluster.last().expect("cluster is not empty");
            let gap = time_to_secs(&entry.start) - time_to_secs(&active.end);
            (-0.2..=2.6).contains(&gap) && ocr_texts_match(&active.text, &entry.text)
        });
        if can_extend {
            clusters
                .last_mut()
                .expect("cluster exists")
                .push(entry.clone());
        } else {
            clusters.push(vec![entry.clone()]);
        }
    }
    clusters.into_iter().filter_map(vote_text_cluster).collect()
}

fn vote_text_cluster(cluster: Vec<SubtitleEntry>) -> Option<SubtitleEntry> {
    let mut counts: HashMap<String, (usize, usize, String)> = HashMap::new();
    for (offset, entry) in cluster.iter().enumerate() {
        let key = normalize_ocr_text(&entry.text);
        let record = counts.entry(key).or_insert((0, offset, entry.text.clone()));
        record.0 += 1;
        record.1 = offset;
        record.2 = entry.text.clone();
    }
    let winner = counts.into_values().max_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
    })?;
    let mut chosen = cluster.first()?.clone();
    chosen.text = winner.2;
    chosen.end = cluster.last()?.end.clone();
    Some(chosen)
}

fn is_ocr_overlay(text: &str) -> bool {
    let east_asian = text.chars().filter(|character| is_east_asian(*character)).count();
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
    is_ocr_overlay(text) && (text.contains('-') || uppercase >= 4)
}

fn trim_ocr_overlays(entries: &[SubtitleEntry]) -> Vec<SubtitleEntry> {
    let dialogue_start = entries
        .iter()
        .position(|entry| !is_ocr_overlay(&entry.text) || time_to_secs(&entry.start) > 180.0)
        .unwrap_or(entries.len());
    let mut kept = entries[dialogue_start..].to_vec();
    let end_time = kept
        .last()
        .map(|entry| time_to_secs(&entry.end))
        .unwrap_or(0.0);
    let credit_roll = kept.iter().enumerate().skip(1).find(|(index, entry)| {
        let previous_end = time_to_secs(&kept[index - 1].end);
        let gap = time_to_secs(&entry.start) - previous_end;
        time_to_secs(&entry.start) >= end_time * 0.8
            && gap >= 15.0
            && is_credit_line(&entry.text)
            && kept.len() - index >= 4
    });
    if let Some((index, _)) = credit_roll {
        kept.truncate(index);
    }
    kept
}

#[derive(Debug, Deserialize)]
struct OcrAltJson {
    text: String,
    confidence: f32,
}

#[derive(Debug, Deserialize)]
struct OcrLineJson {
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
    #[serde(default)]
    w: f32,
    #[serde(default)]
    h: f32,
    #[serde(default)]
    candidates: Vec<OcrAltJson>,
}

#[derive(Debug, Deserialize)]
struct OcrObservationJson {
    t: f64,
    #[serde(default)]
    text: String,
    #[serde(default)]
    confidence: f32,
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
    #[serde(default)]
    w: f32,
    #[serde(default)]
    h: f32,
    #[serde(default)]
    alts: Vec<OcrAltJson>,
    #[serde(default)]
    lines: Vec<OcrLineJson>,
}

struct ResolvedFrame {
    t: f64,
    /// Each inner list is one visual line. Every candidate from that line is kept
    /// so later frames can outvote a weak first choice.
    line_candidates: Vec<Vec<(String, f32)>>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

pub fn ocr_observations_path(srt: &std::path::Path) -> std::path::PathBuf {
    srt.with_extension("observations.json")
}

pub fn entries_from_ocr_observations(
    raw: &str,
    interval: f64,
) -> Result<Vec<SubtitleEntry>, String> {
    let frames: Vec<OcrObservationJson> =
        serde_json::from_str(raw).map_err(|error| format!("无法解析画面采样：{error}"))?;
    Ok(vote_ocr_frames(&frames, interval))
}

struct OpenTrack {
    segments: Vec<Vec<ResolvedFrame>>,
    last_t: f64,
    last_text: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn vote_ocr_frames(frames: &[OcrObservationJson], interval: f64) -> Vec<SubtitleEntry> {
    let mut atoms = Vec::new();
    for frame in frames {
        atoms.extend(atoms_from_observation(frame));
    }
    atoms.retain(|atom| in_subtitle_band_box(atom.y, atom.h) && !preview_text(atom).is_empty());
    atoms.sort_by(|left, right| {
        left.t
            .partial_cmp(&right.t)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                left.x
                    .partial_cmp(&right.x)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let sample = interval.max(0.5);
    let mut tracks: Vec<OpenTrack> = Vec::new();
    for atom in atoms {
        let preview = preview_text(&atom);
        let track_index = tracks
            .iter()
            .enumerate()
            .filter_map(|(index, track)| {
                let gap = atom.t - track.last_t;
                if gap < -0.05 || gap > sample * 2.25 {
                    return None;
                }
                let missed = if gap <= sample * 1.25 {
                    0
                } else {
                    ((gap / sample).round() as u32).saturating_sub(1)
                };
                if missed >= 2 || !track_matches(track, &atom) {
                    return None;
                }
                Some((index, (track.y - atom.y).abs()))
            })
            .min_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index);

        if let Some(index) = track_index {
            let track = &mut tracks[index];
            if ocr_texts_match(&track.last_text, &preview) {
                track.segments.last_mut().expect("track has a segment").push(atom);
            } else {
                track.segments.push(vec![atom]);
            }
            let current = track.segments.last().expect("segment exists");
            let latest = current.last().expect("segment is not empty");
            track.last_t = latest.t;
            track.last_text = preview_text(latest);
            track.x = latest.x;
            track.y = latest.y;
            track.w = latest.w;
            track.h = latest.h;
        } else {
            tracks.push(OpenTrack {
                last_t: atom.t,
                last_text: preview,
                x: atom.x,
                y: atom.y,
                w: atom.w,
                h: atom.h,
                segments: vec![vec![atom]],
            });
        }
    }

    let entries = tracks
        .into_iter()
        .filter(|track| !track_is_watermark(track))
        .flat_map(|track| track.segments)
        .filter_map(|segment| vote_frame_cluster(&segment, interval))
        .collect::<Vec<_>>();
    reindex_entries(trim_ocr_overlays(&entries))
}

fn atoms_from_observation(frame: &OcrObservationJson) -> Vec<ResolvedFrame> {
    if !frame.lines.is_empty() {
        return group_stacked_lines(&frame.lines)
            .into_iter()
            .filter_map(|group| resolved_from_lines(frame.t, &group))
            .collect();
    }
    if frame.text.trim().is_empty() {
        return Vec::new();
    }
    let mut candidates = vec![(frame.text.trim().to_string(), frame.confidence)];
    for alternate in &frame.alts {
        let alternate_text = alternate.text.trim();
        if alternate_text.is_empty() {
            continue;
        }
        let primary_len = normalize_ocr_text(&frame.text).chars().count();
        let alternate_len = normalize_ocr_text(alternate_text).chars().count();
        if primary_len.abs_diff(alternate_len) <= 2 {
            candidates.push((alternate_text.to_string(), alternate.confidence));
        }
    }
    vec![ResolvedFrame {
        t: frame.t,
        line_candidates: vec![candidates],
        x: frame.x,
        y: frame.y,
        w: frame.w,
        h: frame.h,
    }]
}

fn group_stacked_lines(lines: &[OcrLineJson]) -> Vec<Vec<&OcrLineJson>> {
    let mut ordered = lines.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.y
            .partial_cmp(&right.y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut groups: Vec<Vec<&OcrLineJson>> = Vec::new();
    for line in ordered {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.iter().any(|existing| lines_are_stacked(existing, line)))
        {
            group.push(line);
        } else {
            groups.push(vec![line]);
        }
    }
    groups
}

fn lines_are_stacked(left: &OcrLineJson, right: &OcrLineJson) -> bool {
    if left.w <= 0.0 || right.w <= 0.0 || left.h <= 0.0 || right.h <= 0.0 {
        return true;
    }
    let y_delta = (left.y + left.h / 2.0) - (right.y + right.h / 2.0);
    let x_delta = (left.x + left.w / 2.0) - (right.x + right.w / 2.0);
    y_delta.abs() <= 0.08 && x_delta.abs() <= 0.22
}

fn resolved_from_lines(time: f64, lines: &[&OcrLineJson]) -> Option<ResolvedFrame> {
    let mut line_candidates = Vec::new();
    let mut box_left = f32::MAX;
    let mut box_right = 0.0_f32;
    let mut box_bottom = f32::MAX;
    let mut box_top = 0.0_f32;
    for line in lines {
        let candidates = line
            .candidates
            .iter()
            .filter(|candidate| !candidate.text.trim().is_empty())
            .map(|candidate| (candidate.text.trim().to_string(), candidate.confidence))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            continue;
        }
        line_candidates.push(candidates);
        if line.w > 0.0 && line.h > 0.0 {
            box_left = box_left.min(line.x);
            box_right = box_right.max(line.x + line.w);
            box_bottom = box_bottom.min(line.y);
            box_top = box_top.max(line.y + line.h);
        }
    }
    if line_candidates.is_empty() {
        return None;
    }
    Some(ResolvedFrame {
        t: time,
        line_candidates,
        x: if box_left == f32::MAX { 0.0 } else { box_left },
        y: if box_bottom == f32::MAX {
            0.0
        } else {
            box_bottom
        },
        w: if box_left == f32::MAX {
            0.0
        } else {
            box_right - box_left
        },
        h: if box_bottom == f32::MAX {
            0.0
        } else {
            box_top - box_bottom
        },
    })
}

fn preview_text(frame: &ResolvedFrame) -> String {
    frame
        .line_candidates
        .iter()
        .filter_map(|candidates| {
            candidates.iter().max_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        })
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn track_matches(track: &OpenTrack, atom: &ResolvedFrame) -> bool {
    same_resolved_band(
        &ResolvedFrame {
            t: track.last_t,
            line_candidates: Vec::new(),
            x: track.x,
            y: track.y,
            w: track.w,
            h: track.h,
        },
        atom,
    )
}

fn track_is_watermark(track: &OpenTrack) -> bool {
    let mut texts = std::collections::HashSet::new();
    let mut count = 0_usize;
    let mut first = f64::MAX;
    let mut last = 0.0_f64;
    for segment in &track.segments {
        for frame in segment {
            texts.insert(normalize_ocr_text(&preview_text(frame)));
            count += 1;
            first = first.min(frame.t);
            last = last.max(frame.t);
        }
    }
    texts.len() == 1 && count >= 8 && last - first >= 20.0
}

fn in_subtitle_band_box(y: f32, h: f32) -> bool {
    if h <= 0.0 {
        return true;
    }
    y + h / 2.0 <= 0.28
}

fn same_resolved_band(left: &ResolvedFrame, right: &ResolvedFrame) -> bool {
    if left.w <= 0.0 || right.w <= 0.0 || left.h <= 0.0 || right.h <= 0.0 {
        return true;
    }
    let y_delta = (left.y - right.y).abs();
    let height_ratio = left.h.max(right.h) / left.h.min(right.h).max(0.001);
    let overlap = (left.x + left.w).min(right.x + right.w) - left.x.max(right.x);
    y_delta <= 0.06 && height_ratio <= 1.8 && overlap > 0.0
}

fn vote_frame_cluster(cluster: &[ResolvedFrame], interval: f64) -> Option<SubtitleEntry> {
    let line_count = cluster
        .iter()
        .map(|frame| frame.line_candidates.len())
        .max()
        .unwrap_or(0);
    let mut parts = Vec::new();
    for index in 0..line_count {
        let mut scores: HashMap<String, (f32, String)> = HashMap::new();
        for frame in cluster {
            let Some(candidates) = frame.line_candidates.get(index) else {
                continue;
            };
            for (text, confidence) in candidates {
                let key = normalize_ocr_text(text);
                if key.is_empty() {
                    continue;
                }
                let record = scores.entry(key).or_insert((0.0, text.clone()));
                record.0 += confidence;
            }
        }
        let winner = scores.into_values().max_by(|left, right| {
            left.0
                .partial_cmp(&right.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        parts.push(winner.1);
    }
    if parts.is_empty() {
        return None;
    }
    let start = cluster.first()?.t;
    let end = cluster.last()?.t + interval.max(0.4);
    Some(SubtitleEntry {
        index: 1,
        start: secs_to_time(start),
        end: secs_to_time(end),
        text: parts.join(" "),
    })
}

fn reindex_entries(entries: Vec<SubtitleEntry>) -> Vec<SubtitleEntry> {
    entries
        .into_iter()
        .enumerate()
        .map(|(offset, mut entry)| {
            entry.index = offset as u32 + 1;
            entry
        })
        .collect()
}

pub fn compact_transcript(entries: &[SubtitleEntry], max_chars: usize) -> String {
    let full = build_transcript(entries);
    if full.len() <= max_chars {
        return full;
    }
    if entries.is_empty() {
        return full.chars().take(max_chars).collect();
    }
    let mut step = full.len().div_ceil(max_chars.max(1)).max(2);
    while step <= entries.len().max(1) {
        let sampled = entries
            .iter()
            .enumerate()
            .filter(|(index, _)| index % step == 0 || *index == entries.len() - 1)
            .map(|(_, entry)| {
                let start = time_to_secs(&entry.start);
                let end = time_to_secs(&entry.end);
                format!("[{start:.1}s - {end:.1}s] {}", entry.text.trim())
            })
            .collect::<Vec<_>>()
            .join("\n");
        if sampled.len() <= max_chars {
            return sampled;
        }
        step += 1;
    }
    full.chars().take(max_chars).collect()
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
    fn compact_transcript_samples_across_the_whole_transcript() {
        let entries = (0..1_200)
            .map(|index| SubtitleEntry {
                index,
                start: secs_to_time(index as f64 * 2.0),
                end: secs_to_time(index as f64 * 2.0 + 1.5),
                text: format!("第{index}条字幕包含人物行动、阻力和结果，用于覆盖完整剧情。"),
            })
            .collect::<Vec<_>>();
        let compact = compact_transcript(&entries, 60_000);
        assert!(compact.len() <= 60_000);
        assert!(compact.lines().count() > 100);
        assert!(compact.contains("第0条字幕"));
        assert!(compact.contains("第1199条字幕"));
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

    fn cue(start: f64, end: f64, text: &str) -> SubtitleEntry {
        SubtitleEntry {
            index: 1,
            start: secs_to_time(start),
            end: secs_to_time(end),
            text: text.to_string(),
        }
    }

    #[test]
    fn ocr_clean_merges_lookalike_frames_and_drops_overlays() {
        let entries = vec![
            cue(2.25, 3.0, "TUCK"),
            cue(3.0, 7.5, "TUCKER FILM"),
            cue(72.0, 77.25, "孩提时感受到的冬季 并没有这么寒冷"),
            cue(1760.25, 1761.0, "入验的时候"),
            cue(1761.0, 1761.75, "入殓的时候"),
            cue(1872.5, 1873.25, "霝要细致地进行"),
            cue(1875.5, 1876.25, "需要细致地进行"),
            cue(7145.25, 7146.0, "我丈夫是入殓师"),
            cue(7147.5, 7151.25, "我文夫是入殓师"),
            cue(7464.25, 7465.0, "老爸"),
            cue(7588.5, 7589.25, "Daigo KOBAYASHI - Masahiro MOTOKI"),
            cue(7590.0, 7593.0, "本木雅弘 Daigo KOBAYASHI -Masahiro MOTOKI"),
            cue(7593.75, 7594.5, "Mika KOBAYASHI - RyOKO HIROSUE"),
            cue(7595.0, 7596.0, "Yuriko UEMURA - Kimiko YO"),
            cue(7596.5, 7597.5, "TSUYAKO YAMASHITA - Kazuko YOSHIYUKI"),
            cue(7598.0, 7599.0, "SHOHEI SASAKI - Tsutomu YAMAZAKI"),
            cue(7599.5, 7600.5, "监督 滝田洋二郎"),
            cue(7601.0, 7602.0, "Regia: Yojiro TAKITA"),
        ];
        let cleaned = clean_ocr_entries(&entries);
        let text = cleaned
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("TUCKER"));
        assert!(!text.contains("Daigo"));
        assert!(text.contains("孩提时感受到的冬季"));
        assert!(text.contains("老爸"));
        assert_eq!(text.lines().filter(|line| line.contains("的时候")).count(), 1);
        assert_eq!(
            text.lines()
                .filter(|line| line.contains("细致地进行"))
                .count(),
            1
        );
        assert_eq!(
            text.lines().filter(|line| line.contains("入殓师")).count(),
            1,
            "the two husband readings must collapse to one cue; confidence, not order, decides the character"
        );
    }

    #[test]
    fn observation_vote_prefers_repeated_high_confidence_reading() {
        let raw = r#"[
            {"t":29.25,"text":"入验的时候","confidence":0.72},
            {"t":30.0,"text":"入殓的时候","confidence":0.84},
            {"t":30.75,"text":"入殓的时候","confidence":0.89},
            {"t":31.5,"text":"入殓的时候","confidence":0.87},
            {"t":100.0,"text":"我丈夫是入殓师","confidence":0.91,"x":0.31,"y":0.05,"w":0.38,"h":0.06},
            {"t":100.75,"text":"我文夫是入殓师","confidence":0.62,"x":0.31,"y":0.05,"w":0.38,"h":0.06},
            {"t":101.5,"text":"我文夫是入殓师","confidence":0.9,"x":0.2,"y":0.72,"w":0.4,"h":0.05}
        ]"#;
        let entries = entries_from_ocr_observations(raw, 0.75).unwrap();
        let text = entries
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text.matches("入殓的时候").count(), 1);
        assert!(!text.contains("入验"));
        assert!(text.contains("我丈夫是入殓师"));
        assert!(!text.contains("文夫"));
    }

    #[test]
    fn blank_frame_splits_repeated_dialogue() {
        let raw = r#"[
            {"t":10.5,"text":"我知道了","confidence":0.9},
            {"t":11.25,"text":"我知道了","confidence":0.9},
            {"t":12.0,"text":"","confidence":0},
            {"t":12.75,"text":"","confidence":0},
            {"t":13.5,"text":"我知道了","confidence":0.9}
        ]"#;
        let entries = entries_from_ocr_observations(raw, 0.75).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.text == "我知道了"));
    }

    #[test]
    fn second_choice_can_win_across_frames() {
        let raw = r#"[
            {"t":1.0,"lines":[{"x":0.3,"y":0.05,"w":0.3,"h":0.05,"candidates":[
                {"text":"入验的时候","confidence":0.62},
                {"text":"入殓的时候","confidence":0.60}
            ]}]},
            {"t":1.75,"lines":[{"x":0.3,"y":0.05,"w":0.3,"h":0.05,"candidates":[
                {"text":"入验的时候","confidence":0.63},
                {"text":"入殓的时候","confidence":0.61}
            ]}]},
            {"t":2.5,"lines":[{"x":0.3,"y":0.05,"w":0.3,"h":0.05,"candidates":[
                {"text":"入验的时候","confidence":0.40},
                {"text":"入殓的时候","confidence":0.95}
            ]}]}
        ]"#;
        let entries = entries_from_ocr_observations(raw, 0.75).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "入殓的时候");
    }

    #[test]
    fn watermark_stays_off_the_dialogue_line() {
        let raw = r#"[
            {"t":1.0,"lines":[
                {"x":0.02,"y":0.02,"w":0.12,"h":0.04,"candidates":[{"text":"爱壹帆","confidence":0.95}]},
                {"x":0.28,"y":0.05,"w":0.46,"h":0.05,"candidates":[{"text":"我今天一定会回来","confidence":0.9}]}
            ]},
            {"t":1.75,"lines":[
                {"x":0.02,"y":0.02,"w":0.12,"h":0.04,"candidates":[{"text":"爱壹帆","confidence":0.95}]},
                {"x":0.28,"y":0.05,"w":0.46,"h":0.05,"candidates":[{"text":"走吧","confidence":0.9}]}
            ]}
        ]"#;
        let text = entries_from_ocr_observations(raw, 0.75)
            .unwrap()
            .into_iter()
            .map(|entry| entry.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("我今天一定会回来"));
        assert!(text.contains("走吧"));
        assert!(!text.contains("爱壹帆 我今天"));
        assert!(!text.contains("爱壹帆 走吧"));
    }

    #[test]
    fn one_missed_frame_does_not_split_caption() {
        let raw = r#"[
            {"t":10.5,"text":"我知道了","confidence":0.9,"x":0.3,"y":0.05,"w":0.3,"h":0.05},
            {"t":11.25,"text":"","confidence":0},
            {"t":12.0,"text":"我知道了","confidence":0.88,"x":0.3,"y":0.05,"w":0.3,"h":0.05}
        ]"#;
        let entries = entries_from_ocr_observations(raw, 0.75).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "我知道了");
        assert!(entries[0].end.starts_with("00:00:12"));
    }

    #[test]
    fn line_candidate_can_replace_weaker_primary() {
        let raw = r#"[
            {"t":1.0,"lines":[{"x":0.3,"y":0.05,"w":0.3,"h":0.05,"candidates":[
                {"text":"入验的时候","confidence":0.55},
                {"text":"入殓的时候","confidence":0.95}
            ]}]}
        ]"#;
        let entries = entries_from_ocr_observations(raw, 0.75).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "入殓的时候");
    }

    #[test]
    fn keeps_english_and_single_cjk_and_kana_dialogue() {
        let entries = vec![
            cue(10.0, 12.0, "I love you"),
            cue(14.0, 17.0, "Where are you going?"),
            cue(20.0, 21.0, "不"),
            cue(24.0, 26.0, "ありがとう"),
        ];
        let text = clean_ocr_entries(&entries)
            .into_iter()
            .map(|entry| entry.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("I love you"));
        assert!(text.contains("Where are you going?"));
        assert!(text.contains("不"));
        assert!(text.contains("ありがとう"));
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
}
