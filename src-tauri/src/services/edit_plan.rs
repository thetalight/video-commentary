//! Schema-v2 evidence and picture decisions, compiled against measured narration.
//! Candidate IDs currently index subtitle-backed windows, NOT detected visual shots.
use super::{
    jobs,
    script::{self, Shot},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
    pub start: f64,
    pub end: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VisualSelection {
    pub candidate_id: usize,
    pub start: f64,
    pub end: f64,
    pub must_keep_ranges: Vec<TimeRange>,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AudioOwner {
    #[default]
    Narration,
    OriginalDialogue,
    OriginalAction,
    Silence,
}
impl AudioOwner {
    pub fn is_narration(&self) -> bool {
        *self == Self::Narration
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paragraph {
    pub narration: String,
    pub evidence_quote: String,
    pub evidence_refs: Vec<usize>,
    pub visual_selections: Vec<VisualSelection>,
    #[serde(default, skip_serializing_if = "AudioOwner::is_narration")]
    pub audio_owner: AudioOwner,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub title: String,
    pub paragraphs: Vec<Paragraph>,
}
pub fn plan_revision(plan: &Script) -> String {
    jobs::stable_hash(&[
        b"episode-plan-v2",
        &serde_json::to_vec(plan).unwrap_or_default(),
    ])
}
pub fn from_legacy(plan: script::Script, shots: &[Shot]) -> Result<Script, String> {
    let mut paragraphs = Vec::new();
    for p in plan.paragraphs {
        let mut visuals = Vec::new();
        for id in &p.shot_ids {
            let s = shots
                .get(*id)
                .filter(|s| s.id == *id)
                .ok_or("旧稿包含无效字幕窗ID，不能迁移")?;
            visuals.push(VisualSelection {
                candidate_id: *id,
                start: s.start,
                end: s.end,
                must_keep_ranges: vec![],
            });
        }
        paragraphs.push(Paragraph {
            narration: p.narration,
            evidence_quote: p.evidence_quote,
            evidence_refs: p.shot_ids,
            visual_selections: visuals,
            audio_owner: AudioOwner::Narration,
        });
    }
    Ok(Script {
        title: plan.title,
        paragraphs,
    })
}
#[derive(Debug, Serialize, Deserialize)]
pub struct EvidenceLinkRepair {
    pub paragraph_index: usize,
    pub paragraph_number: usize,
    pub added_evidence_id: usize,
    pub reason: String,
}
/// Repair a broken pointer, not the quoted words or the narration. Only a unique
/// raw substring match of >=4 meaningful characters is safe to link mechanically.
/// No fuzzy matching, translated quotes, choosing among repeated dialogue or
/// removing existing references. Every repaired candidate still needs AI review.
pub fn repair_unique_evidence_links(plan: &mut Script, shots: &[Shot]) -> Vec<EvidenceLinkRepair> {
    let mut repaired = Vec::new();
    for (i, p) in plan.paragraphs.iter_mut().enumerate() {
        if !p.audio_owner.is_narration()
            || p.evidence_refs
                .iter()
                .any(|id| shots.get(*id).is_none_or(|s| s.id != *id))
        {
            continue;
        }
        let evidence = script::Script {
            title: String::new(),
            paragraphs: vec![script::Paragraph {
                narration: p.narration.clone(),
                evidence_quote: p.evidence_quote.clone(),
                shot_ids: p.evidence_refs.clone(),
            }],
        };
        if script::validate_evidence(&evidence, shots).is_ok() {
            continue;
        }
        if p.evidence_quote
            .chars()
            .filter(|c| c.is_alphanumeric())
            .count()
            < 4
        {
            continue;
        }
        let hits: Vec<_> = shots
            .iter()
            .filter(|s| s.text.contains(&p.evidence_quote))
            .collect();
        if hits.len() != 1 {
            continue;
        }
        let hit = hits[0];
        if shots.get(hit.id).is_none_or(|s| s.id != hit.id)
            || !hit.start.is_finite()
            || !hit.end.is_finite()
            || hit.start < 0.0
            || hit.end <= hit.start
            || p.evidence_refs.contains(&hit.id)
        {
            continue;
        }
        let before = p.evidence_refs.clone();
        p.evidence_refs.push(hit.id);
        p.evidence_refs.sort_unstable();
        let check = script::Script {
            title: String::new(),
            paragraphs: vec![script::Paragraph {
                narration: p.narration.clone(),
                evidence_quote: p.evidence_quote.clone(),
                shot_ids: p.evidence_refs.clone(),
            }],
        };
        if script::validate_evidence(&check, shots).is_err() {
            p.evidence_refs = before;
            continue;
        }
        repaired.push(EvidenceLinkRepair{paragraph_index:i,paragraph_number:i+1,added_evidence_id:hit.id,reason:"引文在全片原文中唯一逐字匹配，仅补齐漏引ID；未改正文、引文、画面或必保点，仍待完整事实评审".into()});
    }
    repaired
}

#[derive(Debug, Serialize)]
pub struct RuleIssue {
    pub paragraph_index: usize,
    pub paragraph_number: usize,
    pub code: String,
    pub visual_index: Option<usize>,
    pub candidate_id: Option<usize>,
    pub detail: String,
}
/// All mechanically detectable failures, not only the first one. Textual
/// evidence is kept separate from picture corrections; numbers are actual source
/// bounds, not permission to invent a longer candidate.
pub fn rule_issues(plan: &Script, shots: &[Shot]) -> Vec<RuleIssue> {
    let mut issues = Vec::new();
    for (i, p) in plan.paragraphs.iter().enumerate() {
        let mut add = |code: &str, vi: Option<usize>, id: Option<usize>, detail: String| {
            issues.push(RuleIssue {
                paragraph_index: i,
                paragraph_number: i + 1,
                code: code.into(),
                visual_index: vi,
                candidate_id: id,
                detail,
            })
        };
        let evidence = script::Script {
            title: String::new(),
            paragraphs: vec![script::Paragraph {
                narration: if p.audio_owner.is_narration() {
                    p.narration.clone()
                } else {
                    "原声/留白".into()
                },
                evidence_quote: p.evidence_quote.clone(),
                shot_ids: p.evidence_refs.clone(),
            }],
        };
        if let Err(e) = script::validate_evidence(&evidence, shots) {
            add(
                "evidence_invalid",
                None,
                None,
                e.strip_prefix("第1段").unwrap_or(&e).to_string(),
            );
        }
        if !p.audio_owner.is_narration() {
            if let Err(e) = validate_paragraph(p, shots) {
                add("source_owner_invalid", None, None, e);
            }
        }
        if p.visual_selections.is_empty() {
            add(
                "picture_missing",
                None,
                None,
                "缺少播放画面；不能把证据引用当画面".into(),
            );
        }
        let mut previous = 0.0;
        let mut picture = 0.0;
        for (vi, v) in p.visual_selections.iter().enumerate() {
            let Some(s) = shots.get(v.candidate_id).filter(|s| s.id == v.candidate_id) else {
                add(
                    "candidate_unknown",
                    Some(vi),
                    Some(v.candidate_id),
                    "不存在的候选ID".into(),
                );
                continue;
            };
            if !valid_range(v.start, v.end) {
                add(
                    "picture_time_invalid",
                    Some(vi),
                    Some(v.candidate_id),
                    "起止时间必须有限且end>start".into(),
                );
                continue;
            }
            if v.start < s.start - 1e-7 || v.end > s.end + 1e-7 {
                add(
                    "picture_outside_candidate",
                    Some(vi),
                    Some(v.candidate_id),
                    format!(
                        "所选{:.6}–{:.6}越过候选{:.6}–{:.6}秒",
                        v.start, v.end, s.start, s.end
                    ),
                );
            }
            if v.end - v.start < 1.0 - 1e-7 {
                add("picture_too_short",Some(vi),Some(v.candidate_id),format!("画面仅{:.6}秒，最短1秒；证据仍可保留此ID，另选有依据的同情节完整候选，不能拉长源时间",v.end-v.start));
            }
            if v.start < previous - 1e-7 {
                add(
                    "picture_order",
                    Some(vi),
                    Some(v.candidate_id),
                    "段内画面倒序或重叠，请排序/调整画面而非删事实证据".into(),
                );
            }
            previous = v.end;
            if v.start >= s.start - 1e-7 && v.end <= s.end + 1e-7 && v.end - v.start >= 1.0 - 1e-7 {
                picture += v.end - v.start;
            }
            for keep in &v.must_keep_ranges {
                if !valid_range(keep.start, keep.end)
                    || keep.start < v.start - 1e-7
                    || keep.end > v.end + 1e-7
                {
                    add(
                        "protection_invalid",
                        Some(vi),
                        Some(v.candidate_id),
                        "已有必保超出画面或无效，不可删除保护骗过检查；需人工调整".into(),
                    );
                }
            }
        }
        let required = p.narration.chars().count() as f64 * 0.3;
        if picture + 1e-7 < required {
            add("picture_reserve",None,None,format!("有效≥1秒画面储备{picture:.3}秒，至少需要{required:.3}秒，缺少{:.3}秒；增同情节画面或在事实完整前提下缩稿，不改速/定格",required-picture));
        }
    }
    for group in picture_conflicts(plan) {
        let first = group.paragraph_indices[0];
        issues.push(RuleIssue {
            paragraph_index: first,
            paragraph_number: first + 1,
            code: "picture_overlap_group".into(),
            visual_index: None,
            candidate_id: None,
            detail: format!(
                "段落{:?}共用画面：并集{:.3}秒，合计需{:.3}秒，缺口{:.3}秒。{}",
                group.paragraph_numbers,
                group.unique_selected_seconds,
                group.required_seconds,
                group.deficit_seconds,
                group.resolution
            ),
        });
    }
    issues
}

#[derive(Debug, Serialize)]
pub struct PictureConflictGroup {
    pub paragraph_indices: Vec<usize>,
    pub paragraph_numbers: Vec<usize>,
    pub unique_selected_seconds: f64,
    pub required_seconds: f64,
    pub deficit_seconds: f64,
    pub has_protected_or_original_audio: bool,
    pub suggested_text_budgets: Vec<PictureTextBudget>,
    pub resolution: String,
}
#[derive(Debug, Serialize)]
pub struct PictureTextBudget {
    pub paragraph_index: usize,
    pub paragraph_number: usize,
    pub current_narration_chars: usize,
    pub suggested_max_chars_using_existing_picture: usize,
    pub note: String,
}
fn conflict_components(plan: &Script) -> Vec<Vec<usize>> {
    let n = plan.paragraphs.len();
    let mut seen = vec![false; n];
    let mut groups = Vec::new();
    for start in 0..n {
        if seen[start] {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let mut group = Vec::new();
        while let Some(i) = stack.pop() {
            group.push(i);
            for (j, visited) in seen.iter_mut().enumerate() {
                if !*visited
                    && plan.paragraphs[i].visual_selections.iter().any(|a| {
                        plan.paragraphs[j]
                            .visual_selections
                            .iter()
                            .any(|b| overlap(a.start, a.end, b.start, b.end))
                    })
                {
                    *visited = true;
                    stack.push(j);
                }
            }
        }
        if group.len() > 1 {
            group.sort_unstable();
            groups.push(group);
        }
    }
    groups
}
fn unique_seconds(plan: &Script, group: &[usize]) -> f64 {
    let mut ranges: Vec<_> = group
        .iter()
        .flat_map(|i| &plan.paragraphs[*i].visual_selections)
        .filter(|v| valid_range(v.start, v.end))
        .map(|v| (v.start, v.end))
        .collect();
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0;
    let mut active: Option<(f64, f64)> = None;
    for (a, b) in ranges {
        match active {
            Some((x, y)) if a <= y => active = Some((x, y.max(b))),
            Some((x, y)) => {
                total += y - x;
                active = Some((a, b));
            }
            None => active = Some((a, b)),
        }
    }
    if let Some((a, b)) = active {
        total += b - a;
    }
    total
}
fn picture_requirement(p: &Paragraph) -> f64 {
    if p.audio_owner.is_narration() {
        (p.narration.chars().count() as f64 * 0.3).max(1.0)
    } else {
        p.visual_selections.iter().map(|v| v.end - v.start).sum()
    }
}
pub fn picture_conflicts(plan: &Script) -> Vec<PictureConflictGroup> {
    conflict_components(plan).iter().map(|group|{
        let available=unique_seconds(plan,group);let required=group.iter().map(|i|picture_requirement(&plan.paragraphs[*i])).sum::<f64>();
        let protected=group.iter().any(|i|!plan.paragraphs[*i].audio_owner.is_narration()||plan.paragraphs[*i].visual_selections.iter().any(|v|!v.must_keep_ranges.is_empty()));
        PictureConflictGroup{paragraph_indices:group.clone(),paragraph_numbers:group.iter().map(|i|i+1).collect(),unique_selected_seconds:available,required_seconds:required,deficit_seconds:(required-available).max(0.0),has_protected_or_original_audio:protected,
            suggested_text_budgets:if required>available+1e-7 && !protected {group.iter().map(|i|{
                let p=&plan.paragraphs[*i];
                PictureTextBudget{paragraph_index:*i,paragraph_number:i+1,current_narration_chars:p.narration.chars().count(),suggested_max_chars_using_existing_picture:((available*0.98*picture_requirement(p)/required)/0.3).floor() as usize,note:"仅按现有画面并集比例估计，非保证可分配或必须删至该字数；保事实与接续，可有据补选同情节画面，不能逐段重复占用全组素材".into()}
            }).collect()}else{vec![]},
            resolution:if required>available+1e-7{"现有画面并集不足，单纯重新分配无解；须据原文收窄/合并重复叙述或为整组补选相关画面，不逐段重复领取同一素材"}else if protected{"有必保/原声决定，禁止机械删除保护；在保留保护和连续原声的前提下人工或AI调整非保护选择"}else{"画面并集可能足够；可在各段原已选区间内分配互斥子区间，仍需校验每段和全组，不保证贪心可解"}.into()}
    }).collect()
}
#[derive(Debug, Serialize)]
pub struct PictureAllocationRepair {
    pub paragraph_numbers: Vec<usize>,
    pub before_revision: String,
    pub after_revision: String,
    pub method: String,
}
fn subtract_ranges(start: f64, end: f64, used: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut free = vec![(start, end)];
    for (a, b) in used {
        free = free
            .into_iter()
            .flat_map(|(x, y)| {
                if !overlap(x, y, *a, *b) {
                    vec![(x, y)]
                } else {
                    let mut pieces = Vec::new();
                    if *a > x {
                        pieces.push((x, a.min(y)));
                    }
                    if *b < y {
                        pieces.push((b.max(x), y));
                    }
                    pieces
                }
            })
            .collect();
    }
    free
}
/// A conflicting paragraph may have copied another paragraph's picture while
/// its own cited subtitle windows remain unused. Replacing only unprotected
/// selections with those cited windows is deterministic and keeps the narration,
/// evidence, and manual anchors intact. A full AI review still follows.
fn replace_conflicting_picture_with_own_evidence(
    plan: &Script,
    shots: &[Shot],
    group: &[usize],
) -> Option<(Script, usize)> {
    let demand: f64 = group.iter().map(|i| picture_requirement(&plan.paragraphs[*i])).sum();
    if demand <= unique_seconds(plan, group) + 1e-7 {
        return None;
    }
    for &index in group {
        let paragraph = &plan.paragraphs[index];
        if !paragraph.audio_owner.is_narration()
            || paragraph.visual_selections.is_empty()
            || paragraph.visual_selections.iter().any(|v| !v.must_keep_ranges.is_empty())
            || paragraph.visual_selections.iter().any(|v| paragraph.evidence_refs.contains(&v.candidate_id))
        {
            continue;
        }
        let mut used: Vec<(f64, f64)> = plan.paragraphs.iter().enumerate()
            .filter(|(i, _)| *i != index)
            .flat_map(|(_, p)| p.visual_selections.iter())
            .filter(|v| valid_range(v.start, v.end))
            .map(|v| (v.start, v.end))
            .collect();
        let mut ids = paragraph.evidence_refs.clone();
        ids.sort_unstable();
        ids.dedup();
        ids.retain(|id| shots.get(*id).is_some_and(|s| s.id == *id && valid_range(s.start, s.end)));
        ids.sort_by(|a, b| shots[*a].start.total_cmp(&shots[*b].start));
        let mut need = picture_requirement(paragraph);
        let mut selections = Vec::new();
        for id in ids {
            let shot = &shots[id];
            for (start, end) in subtract_ranges(shot.start, shot.end, &used) {
                let available = end - start;
                if available < 1.0 - 1e-7 || need <= 1e-7 {
                    continue;
                }
                let length = need.min(available).max(1.0);
                selections.push(VisualSelection {
                    candidate_id: id,
                    start,
                    end: start + length,
                    must_keep_ranges: vec![],
                });
                used.push((start, start + length));
                need = (need - length).max(0.0);
            }
            if need <= 1e-7 {
                break;
            }
        }
        if need > 1e-7 {
            continue;
        }
        selections.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut candidate = plan.clone();
        candidate.paragraphs[index].visual_selections = selections;
        if validate_paragraph(&candidate.paragraphs[index], shots).is_ok() {
            return Some((candidate, index));
        }
    }
    None
}
/// Fill a picture-reserve deficit from this paragraph's own cited, unplayed
/// subtitle windows. Existing selections and anchors are left untouched.
/// Commit only when the whole deficit is covered with legal non-overlapping
/// clips; a partial fill would make the next AI budget harder to reason about.
pub fn repair_picture_reserves(plan: &mut Script, shots: &[Shot]) -> Vec<PictureAllocationRepair> {
    let mut repairs = Vec::new();
    for index in 0..plan.paragraphs.len() {
        let paragraph = &plan.paragraphs[index];
        if !paragraph.audio_owner.is_narration() {
            continue;
        }
        let issues: Vec<_> = rule_issues(plan, shots).into_iter()
            .filter(|issue| issue.paragraph_index == index).collect();
        if issues.is_empty() || issues.iter().any(|issue| issue.code != "picture_reserve") {
            continue;
        }
        let picture: f64 = paragraph.visual_selections.iter().map(|v| v.end - v.start).sum();
        let mut need = picture_requirement(paragraph) - picture;
        if need <= 1e-7 {
            continue;
        }
        let mut used: Vec<(f64, f64)> = plan.paragraphs.iter()
            .flat_map(|p| p.visual_selections.iter())
            .filter(|v| valid_range(v.start, v.end))
            .map(|v| (v.start, v.end)).collect();
        let mut ids = paragraph.evidence_refs.clone();
        ids.sort_unstable();
        ids.dedup();
        ids.retain(|id| shots.get(*id).is_some_and(|s| s.id == *id && valid_range(s.start, s.end)));
        ids.sort_by(|a, b| shots[*a].start.total_cmp(&shots[*b].start));
        let mut added = Vec::new();
        for id in ids {
            let shot = &shots[id];
            for (start, end) in subtract_ranges(shot.start, shot.end, &used) {
                let available = end - start;
                if available < 1.0 - 1e-7 || need <= 1e-7 {
                    continue;
                }
                let length = need.min(available).max(1.0);
                added.push(VisualSelection { candidate_id: id, start, end: start + length, must_keep_ranges: vec![] });
                used.push((start, start + length));
                need = (need - length).max(0.0);
            }
            if need <= 1e-7 {
                break;
            }
        }
        if need > 1e-7 {
            continue;
        }
        let mut candidate = plan.clone();
        candidate.paragraphs[index].visual_selections.extend(added);
        candidate.paragraphs[index].visual_selections.sort_by(|a, b| a.start.total_cmp(&b.start));
        if validate_paragraph(&candidate.paragraphs[index], shots).is_err() {
            continue;
        }
        let before = plan_revision(plan);
        *plan = candidate;
        repairs.push(PictureAllocationRepair {
            paragraph_numbers: vec![index + 1],
            before_revision: before,
            after_revision: plan_revision(plan),
            method: "append-unused-cited-picture-windows-for-reserve-v1-pending-ai-review".into(),
        });
    }
    repairs
}
/// First replace copied, unprotected picture with unused windows cited by the
/// same paragraph; then allocate exclusive subranges inside existing selections.
/// Never changes text/evidence, removes anchors or changes original-audio
/// timing. The selected budgets are reserves, not actual TTS cuts. Lack of a
/// feasible allocation remains a conflict, not permission to repeat.
pub fn repair_picture_overlaps(plan: &mut Script, shots: &[Shot]) -> Vec<PictureAllocationRepair> {
    let mut repairs = Vec::new();
    // First try unused evidence-linked windows when a group lacks total picture
    // time. Recompute components after each accepted change.
    for _ in 0..plan.paragraphs.len() {
        let mut changed = false;
        for group in conflict_components(plan) {
            if let Some((candidate, index)) = replace_conflicting_picture_with_own_evidence(plan, shots, &group) {
                let before = plan_revision(plan);
                *plan = candidate;
                repairs.push(PictureAllocationRepair {
                    paragraph_numbers: vec![index + 1],
                    before_revision: before,
                    after_revision: plan_revision(plan),
                    method: "replace-conflicted-unprotected-picture-with-own-evidence-window-v1-pending-ai-review".into(),
                });
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    for group in conflict_components(plan) {
        if group.iter().any(|i| {
            !plan.paragraphs[*i].audio_owner.is_narration()
                || plan.paragraphs[*i]
                    .visual_selections
                    .iter()
                    .any(|v| !v.must_keep_ranges.is_empty())
                || validate_paragraph(&plan.paragraphs[*i], shots).is_err()
        }) {
            continue;
        }
        let demand: f64 = group
            .iter()
            .map(|i| picture_requirement(&plan.paragraphs[*i]))
            .sum();
        if demand > unique_seconds(plan, &group) + 1e-7 {
            continue;
        }
        let before = plan_revision(plan);
        let mut accepted = None;
        // Try constrained paragraphs first, then rotations/reversal, bounded by
        // group size. No exponential solver or repeated provider calls.
        let mut order = group.clone();
        order.sort_by(|a, b| {
            let spare = |i: usize| {
                plan.paragraphs[i]
                    .visual_selections
                    .iter()
                    .map(|v| v.end - v.start)
                    .sum::<f64>()
                    - picture_requirement(&plan.paragraphs[i])
            };
            spare(*a).total_cmp(&spare(*b)).then(a.cmp(b))
        });
        for attempt in 0..(group.len() * 2).min(64) {
            let mut sequence = order.clone();
            if attempt >= group.len() {
                sequence.reverse();
            }
            sequence.rotate_left(attempt % group.len());
            let mut candidate = plan.clone();
            let mut used: Vec<(f64, f64)> = Vec::new();
            let mut feasible = true;
            for i in &sequence {
                let p = &plan.paragraphs[*i];
                let mut need = picture_requirement(p);
                let mut selections = Vec::new();
                for visual in &p.visual_selections {
                    for (a, b) in subtract_ranges(visual.start, visual.end, &used) {
                        if need <= 1e-7 {
                            break;
                        }
                        let available = b - a;
                        if available < 1.0 - 1e-7 {
                            continue;
                        }
                        let length = need.min(available).max(1.0);
                        selections.push(VisualSelection {
                            candidate_id: visual.candidate_id,
                            start: a,
                            end: a + length,
                            must_keep_ranges: vec![],
                        });
                        used.push((a, a + length));
                        need = (need - length).max(0.0);
                    }
                    if need <= 1e-7 {
                        break;
                    }
                }
                if need > 1e-7 {
                    feasible = false;
                    break;
                }
                selections.sort_by(|a, b| a.start.total_cmp(&b.start));
                candidate.paragraphs[*i].visual_selections = selections;
                if validate_paragraph(&candidate.paragraphs[*i], shots).is_err() {
                    feasible = false;
                    break;
                }
            }
            if feasible {
                let no_overlap = group.iter().enumerate().all(|(a, i)| {
                    group.iter().skip(a + 1).all(|j| {
                        !candidate.paragraphs[*i].visual_selections.iter().any(|x| {
                            candidate.paragraphs[*j]
                                .visual_selections
                                .iter()
                                .any(|y| overlap(x.start, x.end, y.start, y.end))
                        })
                    })
                });
                if no_overlap {
                    accepted = Some(candidate);
                    break;
                }
            }
        }
        if let Some(candidate) = accepted {
            *plan = candidate;
            repairs.push(PictureAllocationRepair{paragraph_numbers:group.iter().map(|i|i+1).collect(),before_revision:before,after_revision:plan_revision(plan),method:"allocate-exclusive-subranges-within-existing-unprotected-selections-v1-pending-ai-review".into()});
        }
    }
    repairs
}

fn valid_range(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && a >= 0.0 && b > a
}
fn overlap(a: f64, b: f64, c: f64, d: f64) -> bool {
    a < d - 1e-7 && b > c + 1e-7
}
pub fn validate_paragraph(p: &Paragraph, shots: &[Shot]) -> Result<(), String> {
    // Evidence may be arbitrarily short and shared with other paragraphs.
    let evidence = script::Script {
        title: String::new(),
        paragraphs: vec![script::Paragraph {
            narration: if p.audio_owner.is_narration() {
                p.narration.clone()
            } else {
                "原声或留白节点".into()
            },
            evidence_quote: p.evidence_quote.clone(),
            shot_ids: p.evidence_refs.clone(),
        }],
    };
    script::validate_evidence(&evidence, shots)
        .map_err(|e| e.strip_prefix("第1段").unwrap_or(&e).to_string())?;
    if !p.audio_owner.is_narration() {
        if !p.narration.trim().is_empty() {
            return Err("原声/留白段不能同时有配音文案，请清空文案并用证据说明节点".into());
        }
        if p.visual_selections.len() != 1 {
            return Err("原声/留白段第一版仅支持单一连续源区间，避免切断对白".into());
        }
        if p.audio_owner == AudioOwner::OriginalDialogue {
            let v = &p.visual_selections[0];
            if !p.evidence_refs.iter().all(|id| {
                shots
                    .get(*id)
                    .is_some_and(|s| s.start >= v.start && s.end <= v.end)
            }) {
                return Err("原声对白必须完整覆盖所引字幕区间，不能截半句".into());
            }
        }
    }
    if p.visual_selections.is_empty() {
        return Err("缺少独立画面选择，证据引用不等于播放画面".into());
    }
    let mut previous_end = 0.0;
    let mut picture = 0.0;
    for v in &p.visual_selections {
        let s = shots
            .get(v.candidate_id)
            .filter(|s| s.id == v.candidate_id)
            .ok_or("画面候选ID不存在")?;
        if !valid_range(v.start, v.end)
            || v.start < s.start - 1e-7
            || v.end > s.end + 1e-7
            || v.end - v.start < 1.0 - 1e-7
            || v.start < previous_end - 1e-7
        {
            return Err(
                "画面选择越过候选范围、短于1秒或段内倒序/重叠；请保留短证据并另选相关完整画面"
                    .into(),
            );
        }
        previous_end = v.end;
        picture += v.end - v.start;
        for keep in &v.must_keep_ranges {
            if !valid_range(keep.start, keep.end)
                || keep.start < v.start - 1e-7
                || keep.end > v.end + 1e-7
            {
                return Err("必保区间无效或不在所选画面内".into());
            }
        }
    }
    if picture + 1e-7 < p.narration.chars().count() as f64 * 0.3 {
        return Err("独立画面储备不足文案字符数×0.3秒；不能用证据时长或无关片段凑数".into());
    }
    Ok(())
}
pub fn validate(plan: &Script, shots: &[Shot]) -> Result<(), String> {
    if plan.title.trim().is_empty() || plan.paragraphs.is_empty() {
        return Err("整集标题或段落为空".into());
    }
    let mut selected: Vec<(f64, f64)> = vec![];
    for (i, p) in plan.paragraphs.iter().enumerate() {
        validate_paragraph(p, shots).map_err(|e| format!("第{}段：{e}", i + 1))?;
        for v in &p.visual_selections {
            if selected
                .iter()
                .any(|(a, b)| overlap(*a, *b, v.start, v.end))
            {
                return Err("跨段画面选择重叠；证据可重复引用，但不重复播放画面".into());
            }
            selected.push((v.start, v.end));
        }
    }
    Ok(())
}
/// AI can protect exact subtitle windows, never invent sub-cue action timestamps.
/// Existing manual anchors are immutable during automatic repair; UI edits re-review.
pub fn validate_ai_anchors(
    next: &Paragraph,
    previous: Option<&Paragraph>,
    shots: &[Shot],
) -> Result<(), String> {
    let old: Vec<_> = previous
        .into_iter()
        .flat_map(|p| &p.visual_selections)
        .flat_map(|v| &v.must_keep_ranges)
        .collect();
    let new: Vec<_> = next
        .visual_selections
        .iter()
        .flat_map(|v| &v.must_keep_ranges)
        .collect();
    if old.iter().any(|k| !new.contains(k)) {
        return Err("AI修稿移除了已有必保区间，已拒绝".into());
    }
    for k in new {
        if old.contains(&k) {
            continue;
        }
        if !next.evidence_refs.iter().any(|id| {
            shots
                .get(*id)
                .is_some_and(|s| s.start == k.start && s.end == k.end)
        }) {
            return Err(
                "AI不能猜测动作秒点；新增必保区间必须对应完整原字幕窗，精确动作区间由人工标记"
                    .into(),
            );
        }
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub candidate_id: usize,
    pub source_start: f64,
    pub source_end: f64,
    pub must_keep_ranges: Vec<TimeRange>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParagraphEdit {
    pub audio_owner: AudioOwner,
    pub paragraph_index: usize,
    pub audio_duration: f64,
    pub output_start: f64,
    pub output_end: f64,
    pub clips: Vec<Clip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted_candidate_ids: Vec<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPlan {
    pub schema_version: u32,
    pub algorithm: String,
    pub script_revision: String,
    pub source_revision: String,
    pub transcript_revision: String,
    pub total_duration: f64,
    pub paragraphs: Vec<ParagraphEdit>,
}
fn envelope(v: &VisualSelection) -> Option<(f64, f64)> {
    if v.must_keep_ranges.is_empty() {
        None
    } else {
        Some((
            v.must_keep_ranges
                .iter()
                .map(|k| k.start)
                .fold(f64::INFINITY, f64::min),
            v.must_keep_ranges.iter().map(|k| k.end).fold(0.0, f64::max),
        ))
    }
}
fn allocate(p: &Paragraph, duration: f64) -> Result<Vec<Clip>, String> {
    if !duration.is_finite() || duration <= 0.0 {
        return Err("实测配音时长无效".into());
    }
    let all_minimum: Vec<f64> = p
        .visual_selections
        .iter()
        .map(|v| envelope(v).map_or(1.0, |(a, b)| (b - a).max(1.0)))
        .collect();
    let pool_max: f64 = p.visual_selections.iter().map(|v| v.end - v.start).sum();
    if duration > pool_max + 1e-7 {
        return Err("实测配音长于独立画面储备，请缩稿或增加同情节画面；不定格慢放".into());
    }
    let mut selected:Vec<usize>=(0..p.visual_selections.len()).collect();
    if duration < all_minimum.iter().sum::<f64>() - 1e-7 {
        // Unprotected selections are reserves, not a mandate to play every
        // candidate. Keep all protected clips and find the smallest feasible
        // optional subset, preferring earlier reviewed selections when possible.
        selected.retain(|i|envelope(&p.visual_selections[*i]).is_some());
        let protected_min:f64=selected.iter().map(|i|all_minimum[*i]).sum();
        if duration < protected_min - 1e-7 {
            return Err(format!("配音{duration:.2}秒短于必保区间需要的{protected_min:.2}秒；请调整画面或人工安排，不截必保内容"));
        }
        let mut capacity:f64=selected.iter().map(|i|p.visual_selections[*i].end-p.visual_selections[*i].start).sum();
        let optional:Vec<_>=(0..p.visual_selections.len()).filter(|i|envelope(&p.visual_selections[*i]).is_none()).collect();
        let lengths=|indices:&[usize]|{
            let mut values:Vec<f64>=indices.iter().map(|i|p.visual_selections[*i].end-p.visual_selections[*i].start).collect();
            values.sort_by(|a,b|b.total_cmp(a));values
        };
        let mut best_capacity=capacity;let mut count=0;
        for length in lengths(&optional){
            if best_capacity+1e-7>=duration{break;}
            best_capacity+=length;count+=1;
        }
        let feasible_min=protected_min+count as f64;
        if duration<feasible_min-1e-7 || best_capacity+1e-7<duration {
            return Err(format!("配音{duration:.2}秒不足以容纳必保内容及最少可用镜头（至少{feasible_min:.2}秒）；不降低1秒镜头下限或截断保护"));
        }
        for (position,index) in optional.iter().enumerate(){
            if count==0{break;}
            let v=&p.visual_selections[*index];let available=v.end-v.start;
            let future:f64=lengths(&optional[position+1..]).into_iter().take(count-1).sum();
            if capacity+available+future+1e-7>=duration {
                selected.push(*index);capacity+=available;count-=1;
            }
        }
        if count!=0{return Err("无法在配音预算内选择合法的储备画面子集".into());}
        selected.sort_unstable();
    }
    let min_total:f64=selected.iter().map(|i|all_minimum[*i]).sum();
    let max_total:f64=selected.iter().map(|i|p.visual_selections[*i].end-p.visual_selections[*i].start).sum();
    let extra = (duration - min_total).max(0.0);
    let capacity = (max_total - min_total).max(0.0);
    let mut out = Vec::new();
    for index in selected {
        let v=&p.visual_selections[index];let min=all_minimum[index];
        let length = min
            + if capacity > 1e-9 {
                extra * ((v.end - v.start - min).max(0.0) / capacity)
            } else {
                0.0
            };
        let center = envelope(v).map_or((v.start + v.end) / 2.0, |(a, b)| (a + b) / 2.0);
        let start = (center - length / 2.0).max(v.start).min(v.end - length);
        let end = start + length;
        if length < 1.0 - 1e-6
            || v.must_keep_ranges
                .iter()
                .any(|k| k.start < start - 1e-6 || k.end > end + 1e-6)
        {
            return Err("无法在配音预算内完整保留必保区间，未输出裁剪计划".into());
        }
        out.push(Clip {
            candidate_id: v.candidate_id,
            source_start: start,
            source_end: end,
            must_keep_ranges: v.must_keep_ranges.clone(),
        });
    }
    Ok(out)
}
pub fn compile(
    plan: &Script,
    shots: &[Shot],
    durations: &[f64],
    source_revision: &str,
    transcript_revision: &str,
) -> Result<EditPlan, String> {
    validate(plan, shots)?;
    if durations.len() != plan.paragraphs.len() {
        return Err("实测配音时长未覆盖所有段落".into());
    }
    let mut cursor = 0.0;
    let mut paragraphs = vec![];
    let mut ranges = vec![];
    for (i, (p, d)) in plan.paragraphs.iter().zip(durations).enumerate() {
        let clips = if p.audio_owner.is_narration() {
            allocate(p, *d).map_err(|e| format!("第{}段剪辑冲突：{e}", i + 1))?
        } else {
            let v = &p.visual_selections[0];
            if !d.is_finite() || (*d - (v.end - v.start)).abs() > 1e-6 {
                return Err("原声/留白时钟必须与连续源区间等长，不变速或裁尾".into());
            }
            vec![Clip {
                candidate_id: v.candidate_id,
                source_start: v.start,
                source_end: v.end,
                must_keep_ranges: v.must_keep_ranges.clone(),
            }]
        };
        for clip in &clips {
            if ranges
                .iter()
                .any(|(a, b)| overlap(*a, *b, clip.source_start, clip.source_end))
            {
                return Err("实际裁剪画面跨段重叠，未输出计划".into());
            }
            ranges.push((clip.source_start, clip.source_end));
        }
        paragraphs.push(ParagraphEdit {
            audio_owner: p.audio_owner,
            paragraph_index: i,
            audio_duration: *d,
            output_start: cursor,
            output_end: cursor + d,
            omitted_candidate_ids: p.visual_selections.iter().filter(|v|!clips.iter().any(|c|c.candidate_id==v.candidate_id)).map(|v|v.candidate_id).collect(),
            clips,
        });
        cursor += d;
    }
    Ok(EditPlan {
        schema_version: 1,
        algorithm: "anchor-envelope-optional-reserve-v2-seconds-not-frame-snapped".into(),
        script_revision: plan_revision(plan),
        source_revision: source_revision.into(),
        transcript_revision: transcript_revision.into(),
        total_duration: cursor,
        paragraphs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p() -> Paragraph {
        Paragraph {
            narration: "关键内容需要保留".into(),
            evidence_quote: "不是".into(),
            evidence_refs: vec![0],
            audio_owner: AudioOwner::Narration,
            visual_selections: vec![VisualSelection {
                candidate_id: 1,
                start: 100.0,
                end: 110.0,
                must_keep_ranges: vec![TimeRange {
                    start: 107.0,
                    end: 109.0,
                }],
            }],
        }
    }
    fn shots() -> Vec<Shot> {
        vec![
            Shot {
                id: 0,
                start: 24.0,
                end: 24.5,
                text: "不是".into(),
            },
            Shot {
                id: 1,
                start: 100.0,
                end: 110.0,
                text: "同一情节的画面候选".into(),
            },
            Shot {
                id: 2,
                start: 200.0,
                end: 210.0,
                text: "后续相关画面".into(),
            },
        ]
    }
    fn reserve_fixture(lengths:&[f64])->(Script,Vec<Shot>){
        let shots:Vec<_>=lengths.iter().enumerate().map(|(id,length)|Shot{id,start:id as f64*20.0,end:id as f64*20.0+length,text:format!("节点证据{id}")}).collect();
        let paragraph=Paragraph{narration:"事实有据".into(),evidence_quote:shots[0].text.clone(),evidence_refs:vec![0],audio_owner:AudioOwner::Narration,
            visual_selections:shots.iter().map(|s|VisualSelection{candidate_id:s.id,start:s.start,end:s.end,must_keep_ranges:vec![]}).collect()};
        (Script{title:"画面储备".into(),paragraphs:vec![paragraph]},shots)
    }
    #[test]
    fn thirty_one_reserve_clips_fit_seventeen_point_seven_six_seconds() {
        let mut lengths=vec![4.0;31];lengths[0]=7.0;lengths[1]=13.55;lengths[2]=15.5;
        let (mut plan,shots)=reserve_fixture(&lengths);plan.paragraphs[0].narration="甲".repeat(77);
        let revision=plan_revision(&plan);
        let edit=compile(&plan,&shots,&[17.76],"source","transcript").unwrap();let p=&edit.paragraphs[0];
        assert_eq!(p.clips.iter().map(|c|c.candidate_id).collect::<Vec<_>>(),vec![0,1]);
        assert_eq!(p.omitted_candidate_ids.len(),29);
        assert!((p.clips.iter().map(|c|c.source_end-c.source_start).sum::<f64>()-17.76).abs()<1e-7);
        assert!(p.clips.iter().all(|c|c.source_end-c.source_start>=1.0));
        assert_eq!(plan_revision(&plan),revision);
    }
    #[test]
    fn protected_late_selection_survives_optional_reserve_pruning() {
        let (mut plan,shots)=reserve_fixture(&[10.0;31]);
        let v=&mut plan.paragraphs[0].visual_selections[30];let anchor=TimeRange{start:v.start+3.0,end:v.start+6.0};v.must_keep_ranges.push(anchor.clone());
        let edit=compile(&plan,&shots,&[5.0],"s","t").unwrap();let clips=&edit.paragraphs[0].clips;
        assert_eq!(clips.len(),1);assert_eq!(clips[0].candidate_id,30);assert_eq!(clips[0].must_keep_ranges,vec![anchor.clone()]);
        assert!(clips[0].source_start<=anchor.start && clips[0].source_end>=anchor.end);
        plan.paragraphs[0].visual_selections[2].must_keep_ranges.push(TimeRange{start:41.0,end:44.0});
        assert!(compile(&plan,&shots,&[5.0],"s","t").unwrap_err().contains("必保"));
    }
    #[test]
    fn reserve_selection_skips_tiny_earlier_candidates_when_necessary() {
        let (plan,shots)=reserve_fixture(&[1.0,1.0,1.0,10.0]);
        let edit=compile(&plan,&shots,&[1.5],"s","t").unwrap();assert_eq!(edit.paragraphs[0].clips.len(),1);assert_eq!(edit.paragraphs[0].clips[0].candidate_id,3);
        let (plan,shots)=reserve_fixture(&[1.0,1.0]);
        assert!(compile(&plan,&shots,&[1.5],"s","t").is_err());
        assert!(compile(&plan,&shots,&[3.0],"s","t").is_err());
    }
    #[test]
    fn half_second_evidence_and_late_anchor_survive_six_second_narration() {
        let plan = Script {
            title: "例子".into(),
            paragraphs: vec![p()],
        };
        let edit = compile(&plan, &shots(), &[6.0], "source", "transcript").unwrap();
        let c = &edit.paragraphs[0].clips[0];
        assert!(c.source_start <= 107.0 && c.source_end >= 109.0);
        assert!((c.source_end - c.source_start - 6.0).abs() < 1e-7);
        assert!(c.source_end > 106.0);
    }
    #[test]
    fn output_clock_matches_actual_audio_and_multiple_keeps_are_protected() {
        let mut first = p();
        first.visual_selections[0].must_keep_ranges = vec![
            TimeRange {
                start: 101.0,
                end: 102.0,
            },
            TimeRange {
                start: 108.0,
                end: 109.0,
            },
        ];
        let mut second = p();
        second.visual_selections = vec![VisualSelection {
            candidate_id: 2,
            start: 200.0,
            end: 210.0,
            must_keep_ranges: vec![],
        }];
        let plan = Script {
            title: "时钟".into(),
            paragraphs: vec![first, second],
        };
        assert!(compile(&plan, &shots(), &[6.0, 4.0], "s", "t").is_err());
        let edit = compile(&plan, &shots(), &[8.0, 4.0], "s", "t").unwrap();
        assert_eq!(edit.paragraphs[0].output_start, 0.0);
        assert_eq!(edit.paragraphs[1].output_start, 8.0);
        assert_eq!(edit.total_duration, 12.0);
        assert_eq!(edit.script_revision, plan_revision(&plan));
        let clip = &edit.paragraphs[0].clips[0];
        assert!(clip.source_start <= 101.0 && clip.source_end >= 109.0);
    }

    #[test]
    fn source_owned_beats_use_source_clock_and_no_tts_text() {
        let mut paragraph = p();
        paragraph.audio_owner = AudioOwner::OriginalDialogue;
        paragraph.narration.clear();
        // Full cited subtitle is required, unrelated source does not qualify.
        assert!(validate_paragraph(&paragraph, &shots()).is_err());
        paragraph.evidence_refs = vec![1];
        paragraph.evidence_quote = "同一情节的画面候选".into();
        let plan = Script {
            title: "原声".into(),
            paragraphs: vec![paragraph.clone()],
        };
        let edit = compile(&plan, &shots(), &[10.0], "s", "t").unwrap();
        assert_eq!(edit.paragraphs[0].audio_owner, AudioOwner::OriginalDialogue);
        assert_eq!(edit.paragraphs[0].clips[0].source_start, 100.0);
        assert!(compile(&plan, &shots(), &[6.0], "s", "t").is_err());
        paragraph.narration = "不允许同时配音".into();
        assert!(validate_paragraph(&paragraph, &shots()).is_err());
    }
    #[test]
    fn default_audio_owner_keeps_existing_v2_revision_stable() {
        let paragraph = p();
        let mut value = serde_json::to_value(&paragraph).unwrap();
        assert!(value.get("audio_owner").is_none());
        let restored: Paragraph = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(restored.audio_owner, AudioOwner::Narration);
        value["audio_owner"] = serde_json::json!("silence");
        value["narration"] = serde_json::json!("");
        let silence: Paragraph = serde_json::from_value(value).unwrap();
        assert_eq!(silence.audio_owner, AudioOwner::Silence);
    }

    #[test]
    fn unique_verbatim_quote_repairs_only_missing_reference_not_picture_or_prose() {
        let windows = vec![
            Shot {
                id: 0,
                start: 1.0,
                end: 2.0,
                text: "A unique source statement.".into(),
            },
            Shot {
                id: 1,
                start: 100.0,
                end: 110.0,
                text: "Other related dialogue.".into(),
            },
        ];
        let paragraph = Paragraph {
            narration: "有依据的中文叙事".into(),
            evidence_quote: "A unique source statement.".into(),
            evidence_refs: vec![1],
            visual_selections: vec![VisualSelection {
                candidate_id: 1,
                start: 100.0,
                end: 110.0,
                must_keep_ranges: vec![],
            }],
            audio_owner: AudioOwner::Narration,
        };
        let before_picture = paragraph.visual_selections.clone();
        let before_text = paragraph.narration.clone();
        let mut plan = Script {
            title: "例子".into(),
            paragraphs: vec![paragraph],
        };
        let repairs = repair_unique_evidence_links(&mut plan, &windows);
        assert_eq!(repairs.len(), 1);
        assert_eq!(repairs[0].added_evidence_id, 0);
        assert_eq!(plan.paragraphs[0].evidence_refs, vec![0, 1]);
        assert_eq!(plan.paragraphs[0].visual_selections, before_picture);
        assert_eq!(plan.paragraphs[0].narration, before_text);
        validate(&plan, &windows).unwrap();
        assert!(repair_unique_evidence_links(&mut plan, &windows).is_empty());
    }
    #[test]
    fn missing_ambiguous_or_translated_quotes_are_never_relinked_by_guessing() {
        let mut windows = shots();
        let mut plan = Script {
            title: "例子".into(),
            paragraphs: vec![p()],
        };
        plan.paragraphs[0].evidence_refs = vec![1];
        plan.paragraphs[0].evidence_quote = "unmatched invented quote".into();
        assert!(repair_unique_evidence_links(&mut plan, &windows).is_empty());
        windows[0].text = "The exact duplicated quote".into();
        windows[2].text = windows[0].text.clone();
        plan.paragraphs[0].evidence_quote = windows[0].text.clone();
        assert!(repair_unique_evidence_links(&mut plan, &windows).is_empty());
        plan.paragraphs[0].evidence_quote = "the exact duplicated quote".into();
        assert!(
            repair_unique_evidence_links(&mut plan, &windows).is_empty(),
            "case-folded matching is not verbatim relocation"
        );
    }

    #[test]
    fn report_lists_later_visual_errors_instead_of_hiding_them_behind_bad_quote() {
        let mut paragraph = p();
        paragraph.evidence_refs = vec![1];
        paragraph.evidence_quote = "bad quote".into();
        paragraph.visual_selections = vec![VisualSelection {
            candidate_id: 0,
            start: 24.0,
            end: 24.5,
            must_keep_ranges: vec![],
        }];
        let plan = Script {
            title: "例子".into(),
            paragraphs: vec![paragraph],
        };
        let issues = rule_issues(&plan, &shots());
        assert!(issues.iter().any(|i| i.code == "evidence_invalid"));
        assert!(issues.iter().any(|i| i.code == "picture_too_short"));
        assert!(issues.iter().any(|i| i.code == "picture_reserve"));
        assert!(issues
            .iter()
            .all(|i| i.paragraph_index == 0 && i.paragraph_number == 1));
    }

    #[test]
    fn copied_late_picture_is_replaced_with_unused_own_evidence_window() {
        let windows = vec![
            Shot { id: 0, start: 0.0, end: 15.0, text: "第一个节点".into() },
            Shot { id: 1, start: 100.0, end: 116.2, text: "最后一个节点".into() },
        ];
        let shared = VisualSelection {
            candidate_id: 1, start: 100.0, end: 116.2, must_keep_ranges: vec![],
        };
        let mut plan = Script {
            title: "两段主线".into(),
            paragraphs: vec![
                Paragraph {
                    narration: "甲".repeat(27), evidence_quote: windows[0].text.clone(),
                    evidence_refs: vec![0], audio_owner: AudioOwner::Narration,
                    visual_selections: vec![shared.clone()],
                },
                Paragraph {
                    narration: "乙".repeat(54), evidence_quote: windows[1].text.clone(),
                    evidence_refs: vec![1], audio_owner: AudioOwner::Narration,
                    visual_selections: vec![shared.clone()],
                },
            ],
        };
        assert!((picture_conflicts(&plan)[0].deficit_seconds - 8.1).abs() < 1e-7);
        let before_narration: Vec<_> = plan.paragraphs.iter().map(|p| p.narration.clone()).collect();
        let repairs = repair_picture_overlaps(&mut plan, &windows);
        assert_eq!(repairs.len(), 1);
        assert_eq!(plan.paragraphs[0].visual_selections[0].candidate_id, 0);
        assert_eq!(plan.paragraphs[1].visual_selections, vec![shared]);
        assert_eq!(plan.paragraphs.iter().map(|p| p.narration.clone()).collect::<Vec<_>>(), before_narration);
        validate(&plan, &windows).unwrap();
    }

    #[test]
    fn picture_reserve_uses_remaining_cited_window_without_touching_existing_clip() {
        let windows = vec![Shot { id: 0, start: 0.0, end: 16.0, text: "节点事实".into() }];
        let original = VisualSelection { candidate_id: 0, start: 0.0, end: 8.1, must_keep_ranges: vec![TimeRange { start: 1.0, end: 2.0 }] };
        let mut plan = Script {
            title: "储备".into(),
            paragraphs: vec![Paragraph {
                narration: "甲".repeat(40), evidence_quote: windows[0].text.clone(),
                evidence_refs: vec![0], audio_owner: AudioOwner::Narration,
                visual_selections: vec![original.clone()],
            }],
        };
        assert!(validate(&plan, &windows).is_err());
        let repairs = repair_picture_reserves(&mut plan, &windows);
        assert_eq!(repairs.len(), 1);
        assert_eq!(plan.paragraphs[0].visual_selections[0], original);
        assert_eq!(plan.paragraphs[0].visual_selections.len(), 2);
        validate(&plan, &windows).unwrap();
        assert!(repair_picture_reserves(&mut plan, &windows).is_empty());
    }

    #[test]
    fn duplicate_picture_with_surplus_is_partitioned_without_changing_evidence() {
        let mut first = p();
        first.visual_selections[0].must_keep_ranges.clear();
        first.narration = "旁白甲".into();
        let mut second = first.clone();
        second.narration = "旁白乙".into();
        let mut plan = Script {
            title: "足够画面".into(),
            paragraphs: vec![first, second],
        };
        let before_quote = plan.paragraphs[0].evidence_quote.clone();
        let before_refs = plan.paragraphs[0].evidence_refs.clone();
        let repaired = repair_picture_overlaps(&mut plan, &shots());
        assert_eq!(repaired.len(), 1);
        validate(&plan, &shots()).unwrap();
        assert!(picture_conflicts(&plan).is_empty());
        assert_eq!(plan.paragraphs[0].evidence_quote, before_quote);
        assert_eq!(plan.paragraphs[0].evidence_refs, before_refs);
        assert_eq!(plan.paragraphs[0].narration, "旁白甲");
        for paragraph in &plan.paragraphs {
            for v in &paragraph.visual_selections {
                assert_eq!(v.candidate_id, 1);
                assert!(v.start >= 100.0 && v.end <= 110.0);
            }
        }
        assert!(repair_picture_overlaps(&mut plan, &shots()).is_empty());
    }
    #[test]
    fn combined_deficit_is_reported_once_without_inventing_new_picture_or_lowering_budget() {
        let mut first = p();
        first.visual_selections[0].must_keep_ranges.clear();
        first.narration = "字".repeat(30);
        let mut plan = Script {
            title: "不足".into(),
            paragraphs: vec![first.clone(), first],
        };
        let before = plan_revision(&plan);
        assert!(repair_picture_overlaps(&mut plan, &shots()).is_empty());
        assert_eq!(plan_revision(&plan), before);
        let groups = picture_conflicts(&plan);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].paragraph_numbers, vec![1, 2]);
        assert!((groups[0].deficit_seconds - 8.0).abs() < 1e-7);
        let issues = rule_issues(&plan, &shots());
        assert_eq!(
            issues
                .iter()
                .filter(|i| i.code == "picture_overlap_group")
                .count(),
            1
        );
        assert!(validate(&plan, &shots()).is_err());
    }
    #[test]
    fn overlapping_protected_or_original_beats_are_not_reallocated() {
        let first = p();
        let mut plan = Script {
            title: "保护".into(),
            paragraphs: vec![first.clone(), first],
        };
        let before = plan_revision(&plan);
        assert!(repair_picture_overlaps(&mut plan, &shots()).is_empty());
        assert_eq!(plan_revision(&plan), before);
        assert!(picture_conflicts(&plan)[0].has_protected_or_original_audio);
        for paragraph in &mut plan.paragraphs {
            paragraph.visual_selections[0].must_keep_ranges.clear();
            paragraph.audio_owner = AudioOwner::Silence;
            paragraph.narration.clear();
        }
        let before = plan_revision(&plan);
        assert!(repair_picture_overlaps(&mut plan, &shots()).is_empty());
        assert_eq!(plan_revision(&plan), before);
    }

    #[test]
    fn evidence_can_repeat_but_picture_cannot() {
        let mut other = p();
        other.visual_selections[0] = VisualSelection {
            candidate_id: 2,
            start: 200.0,
            end: 210.0,
            must_keep_ranges: vec![],
        };
        let mut plan = Script {
            title: "例子".into(),
            paragraphs: vec![p(), other],
        };
        validate(&plan, &shots()).unwrap();
        plan.paragraphs[1].visual_selections = plan.paragraphs[0].visual_selections.clone();
        assert!(validate(&plan, &shots()).is_err());
    }
    #[test]
    fn protection_budget_and_invalid_timestamps_fail_closed() {
        let plan = Script {
            title: "例子".into(),
            paragraphs: vec![p()],
        };
        assert!(compile(&plan, &shots(), &[1.5], "s", "t").is_err());
        assert!(compile(&plan, &shots(), &[11.0], "s", "t").is_err());
        let mut plan = plan;
        plan.paragraphs[0].visual_selections[0].must_keep_ranges[0].end = 111.0;
        assert!(validate(&plan, &shots()).is_err());
        plan.paragraphs[0].visual_selections[0].end = f64::NAN;
        assert!(validate(&plan, &shots()).is_err());
    }
    #[test]
    fn ai_cannot_invent_action_seconds_or_remove_manual_protection() {
        let paragraph = p();
        assert!(validate_ai_anchors(&paragraph, None, &shots()).is_err());
        validate_ai_anchors(&paragraph, Some(&paragraph), &shots()).unwrap();
        let mut next = paragraph.clone();
        next.visual_selections[0].must_keep_ranges.clear();
        assert!(validate_ai_anchors(&next, Some(&paragraph), &shots()).is_err());
    }
    #[test]
    fn legacy_conversion_is_explicit_and_never_invents_anchors() {
        let old = script::Script {
            title: "旧稿".into(),
            paragraphs: vec![script::Paragraph {
                narration: "旧稿内容".into(),
                evidence_quote: "同一情节的画面候选".into(),
                shot_ids: vec![1],
            }],
        };
        let next = from_legacy(old, &shots()).unwrap();
        assert_eq!(next.paragraphs[0].evidence_refs, vec![1]);
        assert!(next.paragraphs[0].visual_selections[0]
            .must_keep_ranges
            .is_empty());
    }
}
