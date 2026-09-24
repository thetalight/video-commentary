use std::cmp::Ordering;
use std::path::Path;
use std::time::Duration;

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::models::config::{ExcludedRange, OllamaStatus};
use crate::models::edit::{EditPlan, EditSegment};
use crate::services::subtitle::{self, SubtitleEntry};
use crate::services::{director_checkpoint::DirectorCheckpoint, task_events};

const STYLE_CATALOG: &str = include_str!("../../../styles/STYLES.md");
const SPEAKER_RULE: &str = "请结合字幕上下文理解人物身份与关系，自主选择自然、清楚且前后一致的人物称呼，让观众容易听懂；对无法确认的身份保持审慎。";
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";
const OLLAMA_GENERATION_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Shared provider transport; samples have their own evidence and validation contract.
pub async fn sample_json<T: DeserializeOwned>(
    options: &DirectorOptions,
    system: &str,
    user: &str,
    schema: Value,
) -> Result<T, String> {
    crate::services::llm::structured(options, &format!("{system}\n{SPEAKER_RULE}"), user, schema)
        .await
}

pub(crate) fn director_request_timeout(_provider: &str) -> Duration {
    OLLAMA_GENERATION_TIMEOUT
}

pub(crate) fn director_client(
    _provider: &str,
    timeout: Duration,
) -> Result<Client, reqwest::Error> {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .no_proxy()
        .build()
}

fn ollama_request_error(error: &reqwest::Error, timeout: Duration) -> String {
    if error.is_timeout() {
        return format!(
            "Ollama 请求超时（等待上限 {} 秒）：本地模型尚未完成响应，或连接等待超时。已完成的导演检查点会保留，可重试 AI 导演；若反复超时，可选择更小的模型或减少同时运行的任务。",
            timeout.as_secs()
        );
    }
    if error.is_connect() {
        return "无法连接本地 Ollama：请确认 Ollama 应用已启动，且设置中的服务地址可访问，再重试 AI 导演。".to_string();
    }
    let mut detail = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        detail.push_str(&format!("；{cause}"));
        source = cause.source();
    }
    format!("Ollama 请求中断：{detail}。请检查 Ollama 服务日志后重试 AI 导演。")
}

#[derive(Debug, Clone)]
pub struct DirectorOptions {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub title: String,
    pub style: String,
    pub duration: f64,
    pub skip_intro_outro: bool,
    pub skip_ads: bool,
}

#[derive(Debug, Clone)]
enum DirectorBackend {
    Ollama { base_url: String, model: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoryBeat {
    #[serde(default)]
    id: String,
    start: f64,
    end: f64,
    summary: String,
    importance: u8,
    #[serde(default)]
    quote: String,
    #[serde(default)]
    kind: String,
    #[serde(default = "default_narrative_layer")]
    narrative_layer: String,
    #[serde(default)]
    evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoryClaim {
    claim: String,
    beat_ids: Vec<String>,
    #[serde(default)]
    evidence_quotes: Vec<String>,
    confidence: f64,
    #[serde(default)]
    uncertainty: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CharacterCard {
    id: String,
    canonical_name: String,
    #[serde(default)]
    aliases: Vec<String>,
    role: String,
    facts: Vec<StoryClaim>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelationshipCard {
    from_character_id: String,
    to_character_id: String,
    changes: Vec<StoryClaim>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoryThread {
    id: String,
    question: String,
    beat_ids: Vec<String>,
    resolution: String,
    importance: u8,
}

fn default_narrative_layer() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BeatResponse {
    beats: Vec<StoryBeat>,
}

/// A beat-level dialogue attribution is deliberately separate from the source
/// subtitle. It records the model's contextual reading without rewriting the
/// original line, and it may explicitly remain unresolved.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DialogueAttribution {
    beat_id: String,
    evidence_quote: String,
    speaker: String,
    addressee: String,
    confidence: f64,
    uncertainty: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DialogueAttributionResponse {
    attributions: Vec<DialogueAttribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NarrativeBlueprint {
    protagonist: String,
    desire: String,
    core_conflict: String,
    stakes: String,
    hook: String,
    #[serde(default)]
    central_question: String,
    #[serde(default)]
    interpretive_thesis: String,
    #[serde(default)]
    character_arc: Vec<String>,
    #[serde(default)]
    relationship_arc: Vec<String>,
    #[serde(default)]
    recurring_evidence: Vec<String>,
    causal_chain: Vec<String>,
    payoff: String,
    #[serde(default)]
    ending_echo: String,
    character_bible: Vec<CharacterCard>,
    relationships: Vec<RelationshipCard>,
    story_threads: Vec<StoryThread>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NarrativeUnit {
    id: String,
    title: String,
    purpose: String,
    beat_ids: Vec<String>,
    entry_state: String,
    turn: String,
    exit_state: String,
    narration_goal: String,
    audio_strategy: String,
    original_audio_beat_id: String,
    importance: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoryPlan {
    opening_unit_id: String,
    ending_unit_id: String,
    units: Vec<NarrativeUnit>,
    omitted_beat_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct StoryChapter {
    units: Vec<NarrativeUnit>,
    beats: Vec<StoryBeat>,
}

/// The Writer delivers prose only. Picture selection is derived afterwards from
/// the Story Planner's already-grounded beat ids, so a fluent paragraph cannot
/// silently authorize an unrelated or cross-chapter source range.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChapterScript {
    lines: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FactCheckVerdict {
    segment_index: usize,
    supported: bool,
    evidence_ids: Vec<String>,
    corrected_narration: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct FactCheckResponse {
    verdicts: Vec<FactCheckVerdict>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PictureFitRepair {
    segment_index: usize,
    narration: String,
    evidence_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PictureFitResponse {
    repairs: Vec<PictureFitRepair>,
}

#[derive(Debug, Clone, Serialize)]
struct FactEvidenceCandidate {
    id: String,
    start: f64,
    end: f64,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct SegmentFactEvidence {
    segment_index: usize,
    candidates: Vec<FactEvidenceCandidate>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    thinking: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatMessage,
    #[serde(default)]
    done_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
struct TagModel {
    #[serde(default)]
    name: String,
    #[serde(default)]
    model: String,
}

pub async fn check_ollama(base_url: &str) -> OllamaStatus {
    let base_url = sanitize_base_url(base_url);
    let client = match director_client("ollama", Duration::from_secs(4)) {
        Ok(client) => client,
        Err(error) => {
            return OllamaStatus {
                available: false,
                base_url,
                models: Vec::new(),
                error: Some(error.to_string()),
            }
        }
    };

    match fetch_models(&client, &base_url).await {
        Ok(models) => OllamaStatus {
            available: true,
            base_url,
            models,
            error: None,
        },
        Err(error) => OllamaStatus {
            available: false,
            base_url,
            models: Vec::new(),
            error: Some(error),
        },
    }
}

pub async fn generate_plan(
    app: &AppHandle,
    task_id: &str,
    entries: &[SubtitleEntry],
    options: &DirectorOptions,
    director_root: &Path,
) -> Result<(EditPlan, Vec<ExcludedRange>), String> {
    let client = director_client(
        &options.provider,
        director_request_timeout(&options.provider),
    )
    .map_err(|error| error.to_string())?;
    let backend = resolve_backend(&client, options).await?;
    let mut transcript_revision_input = entries
        .iter()
        .map(|entry| {
            format!(
                "{}|{}|{}|{}",
                entry.index, entry.start, entry.end, entry.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    transcript_revision_input.push_str(&format!(
        "\nOPTIONS|{}|{:.3}|{}|{}",
        options.title, options.duration, options.skip_intro_outro, options.skip_ads
    ));
    let resolved_model = match &backend {
        DirectorBackend::Ollama { model, .. } => model,
    };
    let checkpoint = DirectorCheckpoint::open(
        task_id,
        director_root,
        transcript_revision_input.as_bytes(),
        &options.provider,
        resolved_model,
        &options.style,
    )?;
    let mut excluded = detect_excluded_ranges(
        entries,
        options.duration,
        options.skip_intro_outro,
        options.skip_ads,
    );
    let mut content_entries: Vec<SubtitleEntry> = entries
        .iter()
        .filter(|entry| {
            let start = subtitle::time_to_secs(&entry.start);
            let end = subtitle::time_to_secs(&entry.end);
            !overlaps_any(start, end, &excluded)
        })
        .cloned()
        .collect();
    if content_entries.is_empty() && !entries.is_empty() {
        // Detection is heuristic. Never let an over-eager intro/ad range erase
        // the whole task; fall back to the original transcript and let the AI
        // judge which beats are useful.
        excluded.clear();
        content_entries = entries.to_vec();
        task_events::emit_progress(
            app,
            Some(task_id),
            "director",
            71.0,
            "跳过规则覆盖了全部字幕，已自动退回原字幕继续导演",
        );
    }
    if content_entries.is_empty() {
        return Err("字幕文件没有可用对白，请检查 transcript.srt".into());
    }

    // Keep MAP requests small enough that a detailed beat list can still close
    // its JSON object before the model output limit.
    let chunks = transcript_chunks(&content_entries, 12_000);
    let mut beats: Vec<StoryBeat> = if let Some(beats) = checkpoint.read("beats.json") {
        task_events::emit_progress(app, Some(task_id), "director", 84.0, "已恢复剧情节拍检查点");
        beats
    } else {
        let mut mapped = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            task_events::emit_progress(
                app,
                Some(task_id),
                "director",
                72.0 + (index as f64 / chunks.len().max(1) as f64) * 12.0,
                format!("AI 正在梳理剧情节拍 {}/{}", index + 1, chunks.len()),
            );
            let relative = format!("map/chunk-{:03}.json", index + 1);
            let response: BeatResponse = if let Some(response) = checkpoint.read(&relative) {
                response
            } else {
                let response: BeatResponse = chat_json(
                    &client,
                    &backend,
                    beat_system_prompt(),
                    &format!(
                        "请提取 CORE 区间 {:.1}s–{:.1}s 中所有真正推动故事的节拍，不限制数量。CONTEXT 仅用于确认场景、说话对象和省略主语，不得从 CONTEXT 单独输出节拍。人物目标、阻力、选择、关系变化或后果每发生一次实质变化，就保留一个节拍；不要把寒暄和重复对白算作节拍。\n\n{}",
                        chunk.core_start,
                        chunk.core_end,
                        chunk.text
                    ),
                    beat_schema(),
                    0.15,
                    4_096,
                    "story_beats",
                )
                .await?;
                checkpoint.write("director_map_chunk", &relative, &response)?;
                response
            };
            mapped.extend(response.beats.into_iter().filter(|beat| {
                beat.start >= chunk.core_start - 0.5 && beat.start < chunk.core_end + 0.5
            }));
        }
        normalize_beats(&mut mapped, options.duration, &excluded);
        mapped
    };
    normalize_beats(&mut beats, options.duration, &excluded);
    attach_beat_evidence(&mut beats, &content_entries);
    checkpoint.write("director_beats", "beats.json", &beats)?;
    if beats.is_empty() {
        return Err("AI 模型没有提取出足够的剧情节拍；请换用更强的中文模型后重试".into());
    }

    task_events::emit_progress(
        app,
        Some(task_id),
        "director",
        84.5,
        "AI 正在结合上下文梳理关键台词的说话人与听话对象...",
    );
    let dialogue_attributions =
        build_dialogue_attributions(&client, &backend, &beats, &checkpoint).await?;

    let excluded_note = if excluded.is_empty() {
        "无".to_string()
    } else {
        excluded
            .iter()
            .map(|range| format!("{} {:.1}s–{:.1}s", range.kind, range.start, range.end))
            .collect::<Vec<_>>()
            .join("；")
    };
    let story_evidence_json = serde_json::to_string_pretty(&json!({
        "story_beats": &beats,
        "dialogue_attribution": &dialogue_attributions
    }))
    .map_err(|error| error.to_string())?;
    let (min_narration_chars, max_narration_chars) =
        narration_char_range(&content_entries, options.duration);
    task_events::emit_progress(
        app,
        Some(task_id),
        "director",
        85.0,
        "AI 正在建立全片人物、关系、线索与因果模型...",
    );
    let mut blueprint: NarrativeBlueprint = if let Some(blueprint) =
        checkpoint.read("story-model.json")
    {
        task_events::emit_progress(
            app,
            Some(task_id),
            "director",
            86.0,
            "已恢复全片 Story Model",
        );
        blueprint
    } else {
        let mut initial: NarrativeBlueprint = chat_json(
            &client,
            &backend,
            blueprint_system_prompt(),
            &format!(
                "片名：{}\n风格：{}\n原片时长：{:.1} 秒\n解说总字数建议：{}–{} 字\n\n根据局部事实和对白归属表建立全片 Story Model。不要写旁白，不要决定剪辑段落；人物事实、关系和主题判断必须引用 beat_id：\n{}",
                options.title,
                options.style,
                options.duration,
                min_narration_chars,
                max_narration_chars,
                story_evidence_json
            ),
            blueprint_schema(),
            0.15,
            8_192,
            "story_model",
        )
        .await?;
        normalize_story_model(&mut initial, &beats);
        checkpoint.write(
            "director_story_model_initial",
            "story-model.initial.json",
            &initial,
        )?;
        task_events::emit_progress(
            app,
            Some(task_id),
            "director",
            86.0,
            "AI 正在反证人物身份、亲属关系和跨场连续性...",
        );
        let audit_input = json!({
            "draft_story_model": &initial,
            "story_beats_with_original_evidence": &beats,
            "dialogue_attribution": &dialogue_attributions
        });
        match chat_json::<NarrativeBlueprint>(
            &client,
            &backend,
            continuity_audit_system_prompt(),
            &serde_json::to_string_pretty(&audit_input).map_err(|error| error.to_string())?,
            blueprint_schema(),
            0.0,
            8_192,
            "story_model_continuity_audit",
        )
        .await
        {
            Ok(mut audited) => {
                normalize_story_model(&mut audited, &beats);
                checkpoint.write(
                    "director_story_model_audit",
                    "story-model.audit.json",
                    &audited,
                )?;
                audited
            }
            Err(error) => {
                checkpoint.write(
                    "director_story_model_audit_warning",
                    "story-model.audit-warning.json",
                    &json!({
                        "warning": "人物连续性复审不可用，拒绝使用未经反证审校的初始 Story Model",
                        "detail": error
                    }),
                )?;
                return Err(format!(
                    "全片人物连续性复审不可用。为避免人物串线，本次没有继续生成旁白；初始 Story Model 和错误详情已保留，可直接重试 AI 导演：{error}"
                ));
            }
        }
    };
    normalize_story_model(&mut blueprint, &beats);
    checkpoint.write("director_story_model", "story-model.json", &blueprint)?;
    let trusted_blueprint = trusted_story_model(&blueprint);
    let trusted_fact_count = trusted_blueprint
        .character_bible
        .iter()
        .map(|character| character.facts.len())
        .sum::<usize>();
    let required_trusted_facts = if options.duration >= 600.0 { 3 } else { 1 };
    if trusted_fact_count < required_trusted_facts {
        checkpoint.write(
            "director_story_model_trust_failure",
            "story-model.trust-failure.json",
            &json!({
                "trusted_fact_count": trusted_fact_count,
                "required_trusted_facts": required_trusted_facts,
                "reason": "人物事实缺少可逐字匹配的字幕引句，或仍含身份不确定性"
            }),
        )?;
        return Err(format!(
            "全片人物连续性审校未通过：只形成 {trusted_fact_count} 条带原字幕引句的可信人物事实，至少需要 {required_trusted_facts} 条。为避免人物串线，本次没有继续写旁白；请检查字幕质量或更换理解能力更强的本地模型后重试"
        ));
    }
    checkpoint.write(
        "director_story_model_trusted",
        "story-model.trusted.json",
        &trusted_blueprint,
    )?;
    let blueprint_json =
        serde_json::to_string_pretty(&trusted_blueprint).map_err(|error| error.to_string())?;
    let (suggested_units_min, suggested_units_max) =
        story_unit_count_hint(options.duration, beats.len());
    let essential_beat_count = beats.iter().filter(|beat| beat.importance >= 4).count();
    let hard_units_min = suggested_units_min
        .saturating_mul(2)
        .div_ceil(3)
        .max(12)
        .min(beats.len())
        .max(1);
    task_events::emit_progress(
        app,
        Some(task_id),
        "director",
        87.0,
        "AI 正在取舍剧情并合并成全片 Narrative Units...",
    );
    let mut story_plan: StoryPlan = if let Some(plan) = checkpoint.read("story-plan.json") {
        task_events::emit_progress(
            app,
            Some(task_id),
            "director",
            87.5,
            "已恢复全片 Story Plan",
        );
        plan
    } else {
        chat_json(
            &client,
            &backend,
            story_plan_system_prompt(),
            &format!(
                "片名：{}\n风格：{}\n原片时长：{:.1} 秒\n本片共有 {} 个 importance 4–5 的关键节拍。请输出 {}–{} 个 Narrative Unit：下限防止把完整电影压成剧情梗概，上限防止退化为一事件一句；只能依据实际节拍组织，不得用重复或空洞单元凑数。\n\n全片 Story Model：\n{}\n\n全部局部事实节拍与对白归属：\n{}\n\n请先取舍，再把服务同一戏剧目的的局部事件合成 Narrative Unit。不要写最终旁白。",
                options.title,
                options.style,
                options.duration,
                essential_beat_count,
                suggested_units_min,
                suggested_units_max,
                blueprint_json,
                story_evidence_json
            ),
            story_plan_schema(1, suggested_units_max),
            0.2,
            8_192,
            "story_planner",
        )
        .await?
    };
    normalize_story_plan(&mut story_plan, &beats, hard_units_min, suggested_units_max);
    if story_plan.units.is_empty() {
        return Err("Story Planner 没有生成可用叙事单元，请保留字幕后重试 AI 导演".into());
    }
    checkpoint.write("director_story_plan", "story-plan.json", &story_plan)?;
    let story_plan_json =
        serde_json::to_string_pretty(&story_plan).map_err(|error| error.to_string())?;
    let editorial_context_json = serde_json::to_string_pretty(&json!({
        "story_model": &trusted_blueprint,
        "story_plan": &story_plan,
        "dialogue_attribution": &dialogue_attributions
    }))
    .map_err(|error| error.to_string())?;
    // Fact checking and final review necessarily remove unsupported wording. Give
    // the first draft a bounded editorial headroom, while staying inside the
    // evidence-derived maximum, so the release floor is a planning input rather
    // than a surprise after all expensive stages have completed.
    let writer_target_chars = ((min_narration_chars as f64 * 1.30).ceil() as usize)
        .min(max_narration_chars)
        .max(min_narration_chars);
    let unit_budgets = story_unit_budgets(&story_plan.units, writer_target_chars);
    checkpoint.write(
        "director_narration_budget",
        "narration-budget.json",
        &json!({
            "release_floor_chars": min_narration_chars,
            "writer_target_chars": writer_target_chars,
            "evidence_ceiling_chars": max_narration_chars,
            "unit_budgets": &unit_budgets
        }),
    )?;
    task_events::emit_progress(
        app,
        Some(task_id),
        "director",
        87.8,
        format!(
            "Story Plan 已整理为 {} 个连续叙事单元，旁白预算已分配",
            story_plan.units.len()
        ),
    );
    let chapters = build_story_chapters(&beats, &story_plan);
    let mut plan = EditPlan {
        title: format!("{}｜{}", options.title, options.style),
        style: options.style.clone(),
        target_duration_secs: 1.0,
        segments: Vec::new(),
    };
    let mut previous_tail = trusted_blueprint.hook.clone();

    for (chapter_index, chapter) in chapters.iter().enumerate() {
        let chapter_beats = &chapter.beats;
        let chapter_units_json =
            serde_json::to_string_pretty(&chapter.units).map_err(|error| error.to_string())?;
        let chapter_budgets = chapter
            .units
            .iter()
            .filter_map(|unit| {
                unit_budgets
                    .get(&unit.id)
                    .copied()
                    .map(|budget| (unit.id.clone(), budget))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let chapter_target_chars = chapter_budgets.values().sum::<usize>();
        let checked_relative = format!("chapters/chapter-{:03}.checked.json", chapter_index + 1);
        if let Some(chapter_plan) = checkpoint.read::<EditPlan>(&checked_relative) {
            task_events::emit_progress(
                app,
                Some(task_id),
                "director",
                88.0 + (chapter_index as f64 / chapters.len().max(1) as f64) * 10.0,
                format!("已恢复第 {}/{} 章检查点", chapter_index + 1, chapters.len()),
            );
            if let Some(last) = chapter_plan.segments.last() {
                previous_tail = last.narration.clone();
            }
            plan.segments.extend(chapter_plan.segments);
            continue;
        }
        let chapter_evidence = evidence_for_beats(entries, chapter_beats);
        let chapter_beat_ids = chapter_beats
            .iter()
            .map(|beat| beat.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let chapter_attributions = dialogue_attributions
            .iter()
            .filter(|item| chapter_beat_ids.contains(item.beat_id.as_str()))
            .collect::<Vec<_>>();
        let chapter_attribution_json = serde_json::to_string_pretty(&chapter_attributions)
            .map_err(|error| error.to_string())?;
        let understanding_source = format!(
            "原字幕证据：\n{}\n\n已核验格式的对白归属派生表（低置信项仍视为身份未确认）：\n{}",
            chapter_evidence, chapter_attribution_json
        );
        task_events::emit_progress(
            app,
            Some(task_id),
            "director",
            88.0,
            format!("正在理解第 {} 章字幕歧义与人物事件关系", chapter_index + 1),
        );
        let understanding_path = format!("understanding/chapter-{:03}.json", chapter_index + 1);
        let (understanding, mut understanding_warnings): (
            super::understanding::Understanding,
            Vec<String>,
        ) = if let Some(cached) = checkpoint.read(&understanding_path) {
            (cached, vec![])
        } else {
            match super::understanding::analyze(options, &understanding_source).await {
                Ok(result) => {
                    checkpoint.write("subtitle_understanding", &understanding_path, &result)?;
                    (result, vec![])
                }
                Err(error) => (
                    super::understanding::Understanding::default(),
                    vec![format!(
                        "字幕理解派生层本次不可用，已忽略并继续使用原字幕：{error}"
                    )],
                ),
            }
        };
        let mut understanding = understanding;
        understanding_warnings.extend(understanding.sanitize(&chapter_evidence));
        if !understanding_warnings.is_empty() {
            checkpoint.write(
                "subtitle_understanding_warnings",
                format!(
                    "understanding/chapter-{:03}.warnings.json",
                    chapter_index + 1
                ),
                &understanding_warnings,
            )?;
        }
        let understanding_json =
            serde_json::to_string(&understanding).map_err(|e| e.to_string())?;
        let chapter_beat_json =
            serde_json::to_string_pretty(chapter_beats).map_err(|error| error.to_string())?;
        task_events::emit_progress(
            app,
            Some(task_id),
            "director",
            88.0 + (chapter_index as f64 / chapters.len().max(1) as f64) * 10.0,
            format!(
                "AI 正在按叙事预算写完整旁白 {}/{}...",
                chapter_index + 1,
                chapters.len()
            ),
        );
        let chapter_script: ChapterScript = chat_json(
            &client,
            &backend,
            &chapter_system_prompt(&options.style, chapter_index == 0),
            &format!(
                "片名：{}\n章节：{}/{}\n必须避开的区间：{}\n上一章结尾：{}\n本章交付预算：约 {} 字。每个 unit_id 的最低交付量已写进 JSON Schema：这是第一次写作的完整交付，不允许先交梗概再补写。\n各单元预算：{}\n\n全片 Story Model（人物身份合同）：\n{}\n\n全片 Story Plan（只用于理解前后文）：\n{}\n\n当前必须写成完整意群的 Narrative Units：\n{}\n\n这些 Units 引用的局部事实节拍：\n{}\n\n本章关键台词归属：\n{}\n\n本章字幕证据：\n{}\n\n字幕理解建议（仅解释歧义，不是身份合同；与 Story Model 冲突时不得采用）：{understanding_json}\n只生成本章 lines。",
                options.title,
                chapter_index + 1,
                chapters.len(),
                excluded_note,
                previous_tail,
                chapter_target_chars,
                serde_json::to_string(&chapter_budgets).map_err(|error| error.to_string())?,
                blueprint_json,
                story_plan_json,
                chapter_units_json,
                chapter_beat_json,
                chapter_attribution_json,
                chapter_evidence,
            ),
            chapter_script_schema(&chapter.units, &chapter_budgets),
            0.3,
            8_192,
            "director_chapter_script",
        )
        .await?;
        checkpoint.write(
            "director_chapter_script",
            format!("chapters/chapter-{:03}.script.json", chapter_index + 1),
            &chapter_script,
        )?;
        let mut chapter_plan =
            chapter_script_to_edit_plan(chapter_script, chapter, &options.title, &options.style)?;
        let next_chapter_start = chapters
            .get(chapter_index + 1)
            .and_then(|next| next.beats.first())
            .map(|beat| beat.start);
        let previous_chapter_end = chapter_index
            .checked_sub(1)
            .and_then(|previous| chapters.get(previous))
            .and_then(|previous| previous.beats.last())
            .map(|beat| beat.end);
        finalize_plan(
            &mut chapter_plan,
            entries,
            chapter_beats,
            &excluded,
            &options.title,
            &options.style,
            previous_chapter_end,
            next_chapter_start,
            options.duration,
        )?;
        checkpoint.write(
            "director_chapter_draft",
            format!("chapters/chapter-{:03}.draft.json", chapter_index + 1),
            &chapter_plan,
        )?;
        let safe_chapter_draft = chapter_plan.clone();

        // A short, grounded chapter is valid. Retry only if fact checking
        // removed every segment, never to fill a word-count deficit.
        for audit_round in 0..2 {
            let fact_evidence = fact_evidence_packet(entries, &chapter_plan);
            checkpoint.write(
                "director_audit_input",
                format!(
                    "chapters/chapter-{:03}.audit-input-{}.json",
                    chapter_index + 1,
                    audit_round + 1
                ),
                &json!({"plan": &chapter_plan, "segment_evidence": &fact_evidence}),
            )?;
            task_events::emit_progress(
                app,
                Some(task_id),
                "director",
                88.0 + ((chapter_index as f64 + 0.5) / chapters.len().max(1) as f64) * 10.0,
                format!("AI 正在核对第 {} 章字幕证据...", chapter_index + 1),
            );
            let fact_check = match request_fact_check(
                &client,
                &backend,
                &chapter_plan,
                &fact_evidence,
                &blueprint_json,
            )
            .await
            {
                Ok(fact_check) => fact_check,
                Err(error) => {
                    checkpoint.write(
                        "director_fact_check_warning",
                        format!(
                            "chapters/chapter-{:03}.audit-warning-{}.json",
                            chapter_index + 1,
                            audit_round + 1
                        ),
                        &json!({
                            "warning": "本地模型事实核验不可用，拒绝发布未经核验的章节稿",
                            "detail": error,
                            "plan_preserved": false
                        }),
                    )?;
                    return Err(format!(
                        "第 {} 章字幕事实核验不可用。为避免把人物错位或字幕识别错误写进成片，本次已停止；检查点已保留，可直接重试 AI 导演：{}",
                        chapter_index + 1,
                        error
                    ));
                }
            };
            checkpoint.write(
                "director_fact_check",
                format!(
                    "chapters/chapter-{:03}.verdict-{}.json",
                    chapter_index + 1,
                    audit_round + 1
                ),
                &fact_check,
            )?;
            let rejected: Vec<String> = chapter_plan
                .segments
                .iter()
                .enumerate()
                .filter_map(|(i, _segment)| {
                    let verdicts: Vec<_> = fact_check
                        .verdicts
                        .iter()
                        .filter(|v| v.segment_index == i)
                        .collect();
                    if verdicts.len() != 1 {
                        return Some(format!("第 {} 段裁决缺失或重复", i + 1));
                    }
                    let v = verdicts[0];
                    if !fact_verdict_has_valid_evidence(&fact_evidence[i], v) {
                        Some(format!(
                            "第 {} 段没有选择本段证据编号，或选择了其他时间段的证据",
                            i + 1
                        ))
                    } else if !v.supported
                        && clean_narration(&v.corrected_narration).chars().count() < 8
                    {
                        Some(format!(
                            "第 {} 段被模型判为无依据且未提供有效修正文案",
                            i + 1
                        ))
                    } else {
                        None
                    }
                })
                .collect();
            checkpoint.write(
                "director_rejections",
                format!(
                    "chapters/chapter-{:03}.rejections-{}.json",
                    chapter_index + 1,
                    audit_round + 1
                ),
                &rejected,
            )?;
            apply_fact_check(&mut chapter_plan, fact_check, &fact_evidence);
            if !chapter_plan.segments.is_empty() {
                chapter_plan
                    .validate_and_clamp(options.duration)
                    .map_err(|error| error.to_string())?;
                break;
            }
            if audit_round == 1 {
                checkpoint.write(
                    "director_fact_check_warning",
                    format!(
                        "chapters/chapter-{:03}.audit-warning-{}.json",
                        chapter_index + 1,
                        audit_round + 1
                    ),
                    &json!({
                        "warning": "本章两次核验均建议删除全部片段，拒绝发布空章并保留最后一个完整候选",
                        "rejections": rejected,
                        "plan_preserved": true
                    }),
                )?;
                chapter_plan = safe_chapter_draft.clone();
                break;
            }
            task_events::emit_progress(
                app,
                Some(task_id),
                "director",
                88.0 + ((chapter_index as f64 + 0.5) / chapters.len().max(1) as f64) * 10.0,
                format!(
                    "第 {} 章没有通过事实核验的片段，正在依据字幕修正一次...",
                    chapter_index + 1
                ),
            );
            let repaired_script: ChapterScript = chat_json(
                &client,
                &backend,
                &format!(
                    "{}\n上一版没有任何段落通过字幕证据核验。重新写一份保守的本章旁白，不猜测姓名、身份、动机、心理或结局；预算必须通过讲清已有事实完成，不得注水。",
                    chapter_system_prompt(&options.style, chapter_index == 0)
                ),
                &format!(
                    "第 {}/{} 章。上一章结尾：{}\n本章预算约{}字，各单元预算：{}。\n全片人物与关系模型：{}\n当前 Narrative Units：{}\n本章剧情节拍：{}\n本章关键台词归属：{}\n本章字幕证据：{}",
                    chapter_index + 1,
                    chapters.len(),
                    previous_tail,
                    chapter_target_chars,
                    serde_json::to_string(&chapter_budgets).map_err(|error| error.to_string())?,
                    blueprint_json,
                    chapter_units_json,
                    chapter_beat_json,
                    chapter_attribution_json,
                    chapter_evidence
                ),
                chapter_script_schema(&chapter.units, &chapter_budgets),
                0.15,
                8_192,
                "director_grounded_chapter_rewrite",
            )
            .await?;
            chapter_plan = chapter_script_to_edit_plan(
                repaired_script,
                chapter,
                &options.title,
                &options.style,
            )?;
            finalize_plan(
                &mut chapter_plan,
                entries,
                chapter_beats,
                &excluded,
                &options.title,
                &options.style,
                previous_chapter_end,
                next_chapter_start,
                options.duration,
            )?;
        }
        checkpoint.write("director_chapter_checked", &checked_relative, &chapter_plan)?;
        if let Some(last) = chapter_plan.segments.last() {
            previous_tail = last.narration.clone();
        }
        plan.segments.extend(chapter_plan.segments);
    }

    task_events::emit_progress(
        app,
        Some(task_id),
        "director",
        99.0,
        "程序正在合并章节、校验时间轴和总解说时长...",
    );
    finalize_plan(
        &mut plan,
        entries,
        &beats,
        &excluded,
        &options.title,
        &options.style,
        None,
        None,
        options.duration,
    )?;

    task_events::emit_progress(
        app,
        Some(task_id),
        "director",
        99.0,
        "正在进行一次整片连贯性、事实与重复检查",
    );
    let pre_review_plan = plan.clone();
    let usable_seconds = usable_subtitle_duration(&content_entries, options.duration);
    plan = super::full_review::refine(
        plan,
        entries,
        options,
        &checkpoint,
        &editorial_context_json,
        min_narration_chars,
    )
    .await?;
    // Whole-script review can delete neighbours or rewrite narration, changing the
    // amount of picture available to a paragraph. Reflow once more, then let the
    // local model compress only paragraphs that remain physically boxed in.
    finalize_plan(
        &mut plan,
        entries,
        &beats,
        &excluded,
        &options.title,
        &options.style,
        None,
        None,
        options.duration,
    )?;
    task_events::emit_progress(
        app,
        Some(task_id),
        "director",
        99.5,
        "正在扩选画面并压缩超时口播...",
    );
    compile_renderable_timeline(&mut plan, entries, &excluded, options.duration);
    repair_picture_shortfalls(&client, &backend, &mut plan, entries, Some(&checkpoint)).await?;
    compile_renderable_timeline(&mut plan, entries, &excluded, options.duration);
    collapse_repeated_narration(&mut plan);
    plan.validate_and_clamp(options.duration)
        .map_err(|error| error.to_string())?;
    if let Err(reviewed_error) = validate_market_ready(&plan, min_narration_chars, usable_seconds) {
        let pre_review_is_renderable = super::edit_timing::reserve_issues(&pre_review_plan)
            .into_iter()
            .all(|issue| issue.severity != super::edit_timing::ReserveSeverity::Blocking);
        if pre_review_is_renderable
            && validate_market_ready(&pre_review_plan, min_narration_chars, usable_seconds).is_ok()
        {
            checkpoint.write(
                "full_review_rollback",
                "full-review/rollback.json",
                &json!({
                    "reason": reviewed_error,
                    "action": "整片评审结果未通过最终硬规则，已回滚到评审前安全版本"
                }),
            )?;
            plan = pre_review_plan;
        } else {
            return Err(reviewed_error);
        }
    }
    checkpoint.write("director_final_plan", "final-plan.json", &plan)?;
    Ok((plan, excluded))
}

async fn resolve_backend(
    client: &Client,
    options: &DirectorOptions,
) -> Result<DirectorBackend, String> {
    if options.provider != "ollama" {
        return Err("当前版本只允许本机 Ollama，云模型已禁用".into());
    }
    let base_url = sanitize_base_url(&options.base_url);
    let model = resolve_model(client, &base_url, &options.model).await?;
    Ok(DirectorBackend::Ollama { base_url, model })
}

fn sanitize_base_url(base_url: &str) -> String {
    match base_url.trim().trim_end_matches('/') {
        "http://localhost:11434" => "http://localhost:11434".to_string(),
        "http://127.0.0.1:11434" => DEFAULT_BASE_URL.to_string(),
        _ => DEFAULT_BASE_URL.to_string(),
    }
}

async fn fetch_models(client: &Client, base_url: &str) -> Result<Vec<String>, String> {
    let response = client
        .get(format!("{base_url}/api/tags"))
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .map_err(|error| ollama_request_error(&error, Duration::from_secs(4)))?;
    if !response.status().is_success() {
        return Err(format!("Ollama 返回 HTTP {}", response.status()));
    }
    let tags: TagsResponse = response.json().await.map_err(|error| error.to_string())?;
    Ok(tags
        .models
        .into_iter()
        .filter_map(|item| {
            let name = if item.name.is_empty() {
                item.model
            } else {
                item.name
            };
            (!name.is_empty()).then_some(name)
        })
        .collect())
}

async fn resolve_model(
    client: &Client,
    base_url: &str,
    configured: &str,
) -> Result<String, String> {
    let models = fetch_models(client, base_url).await?;
    if models.is_empty() {
        return Err("Ollama 已启动，但没有本地模型；请先执行 `ollama pull qwen3:8b`".into());
    }
    if !configured.trim().is_empty() {
        if models.iter().any(|model| model == configured.trim()) {
            return Ok(configured.trim().to_string());
        }
        return Err(format!(
            "设置中的 Ollama 模型 `{}` 未安装；当前可用：{}",
            configured.trim(),
            models.join("、")
        ));
    }
    Ok(models
        .iter()
        .find(|model| model.to_lowercase().contains("qwen"))
        .or_else(|| {
            models
                .iter()
                .find(|model| model.to_lowercase().contains("gemma"))
        })
        .unwrap_or(&models[0])
        .clone())
}

async fn request_fact_check(
    client: &Client,
    backend: &DirectorBackend,
    chapter_plan: &EditPlan,
    fact_evidence: &[SegmentFactEvidence],
    trusted_story_model: &str,
) -> Result<FactCheckResponse, String> {
    let chapter_plan_json =
        serde_json::to_string(chapter_plan).map_err(|error| error.to_string())?;
    chat_json(
        client,
        backend,
        &format!("{}\n{SPEAKER_RULE}\n逐项审查叙述中的人物、地点、物品、行为和因果，不能因选择了一个证据编号就认可整段。trusted_story_model 已由程序删除所有低置信或含 uncertainty 的人物事实：旁白中的姓名、亲属、婚姻、职业和说话对象若不在该可信模型中，就算局部字幕提到类似话题也必须判为不支持；尤其不能把‘我抛下自己的孩子’改写成‘主角母亲抛下主角’。特别检查主宾倒置、比喻被当作实物、演示被误认作节目、识别乱码被扩写成具体情节。你是事实编辑，不是摘要器：需要修正时，保留原段已被证明的叙事作用、前后承接和信息密度，corrected_narration 通常不得短于原段的80%；删除不实断言后，用同段证据中的处境、动作、选择、变化或后果补足，但绝不增加新事实。重复叙述交给整片评审处理。", fact_check_system_prompt()),
        &format!(
            "逐段核对下面导演稿。segment_index 从 0 开始，每段都必须返回 verdict。每段只能从同 segment_index 的 candidates 选择 evidence_ids，不要复制字幕原文作为引用。\n\n可信人物与关系模型：\n{}\n\n本章导演稿：\n{}\n\n逐段证据候选：\n{}",
            trusted_story_model,
            chapter_plan_json,
            serde_json::to_string(fact_evidence).map_err(|error| error.to_string())?,
        ),
        fact_check_schema(),
        0.0,
        4_096,
        "director_fact_check",
    )
    .await
}

async fn chat_json<T: DeserializeOwned>(
    client: &Client,
    backend: &DirectorBackend,
    system: &str,
    user: &str,
    schema: Value,
    temperature: f64,
    num_predict: u32,
    schema_name: &str,
) -> Result<T, String> {
    let (provider, content, finish_reason) = chat_content(
        client,
        backend,
        system,
        user,
        &schema,
        temperature,
        num_predict,
        schema_name,
    )
    .await?;
    match parse_structured_json::<T>(&content) {
        Ok(value) => Ok(value),
        Err(first_error) => {
            let retry_tokens = num_predict.saturating_mul(2).clamp(4_096, 32_768);
            let retry_system = format!(
                "{system}\n\n上一次输出在 JSON 中途截断。此次必须从第一个左花括号开始，重新输出完整 JSON；不得省略末尾字段，不要附加解释，使用紧凑 JSON 减少输出长度。"
            );
            let retry_user = format!(
                "{user}\n\n上一次结果解析失败：{first_error}。请重新从头生成完整结果，不要续写残缺片段。"
            );
            let (_, retry_content, retry_finish_reason) = chat_content(
                client,
                backend,
                &retry_system,
                &retry_user,
                &schema,
                temperature.min(0.15),
                retry_tokens,
                schema_name,
            )
            .await?;
            match parse_structured_json::<T>(&retry_content) {
                Ok(value) => Ok(value),
                Err(retry_error) => {
                    let final_tokens = retry_tokens.saturating_mul(2).clamp(8_192, 32_768);
                    let final_system = format!(
                        "{system}\n\n这是最后一次结构修复：只输出一个以 {{ 开始、以 }} 结束的紧凑 JSON 对象。根节点必须是对象，各数组字段必须用 []，数字不能代替数组；不要输出 Markdown、解释或空白响应。"
                    );
                    let final_user = format!(
                        "{user}\n\n前两次分别解析失败：{first_error}；{retry_error}。请重新独立生成完整 JSON。"
                    );
                    let (_, final_content, final_finish_reason) = chat_content(
                        client,
                        backend,
                        &final_system,
                        &final_user,
                        &schema,
                        0.0,
                        final_tokens,
                        schema_name,
                    )
                    .await?;
                    parse_structured_json::<T>(&final_content).map_err(|final_error| {
                        format!(
                            "{provider} 两次自动修复后仍未返回有效导演 JSON：{final_error}（最终结束原因：{}；第二次错误：{retry_error}，结束原因：{}；首次错误：{first_error}，结束原因：{}）",
                            final_finish_reason.as_deref().unwrap_or("未知"),
                            retry_finish_reason.as_deref().unwrap_or("未知"),
                            finish_reason.as_deref().unwrap_or("未知"),
                        )
                    })
                }
            }
        }
    }
}

fn parse_structured_json<T: DeserializeOwned>(content: &str) -> Result<T, String> {
    let payload = first_json_object(content).unwrap_or_else(|| content.trim());
    if payload.is_empty() {
        return Err("响应正文为空".to_string());
    }
    serde_json::from_str::<T>(payload).map_err(|error| error.to_string())
}

fn first_json_object(content: &str) -> Option<&str> {
    let start = content.find('{')?;
    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    for (relative_index, character) in content[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    return Some(&content[start..start + relative_index + character.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    Some(&content[start..])
}

#[cfg(test)]
fn recover_edit_plan_fragment(
    content: &str,
    fallback_title: &str,
    fallback_style: &str,
) -> Option<EditPlan> {
    let segments_key = content.find("\"segments\"")?;
    let array_offset = content[segments_key..].find('[')? + segments_key + 1;
    let mut segments = Vec::new();
    let mut object_start = None;
    let mut object_depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    for (relative_index, character) in content[array_offset..].char_indices() {
        let index = array_offset + relative_index;
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => {
                if object_depth == 0 {
                    object_start = Some(index);
                }
                object_depth += 1;
            }
            '}' if object_depth > 0 => {
                object_depth -= 1;
                if object_depth == 0 {
                    if let Some(start) = object_start.take() {
                        if let Ok(segment) =
                            serde_json::from_str::<EditSegment>(&content[start..=index])
                        {
                            segments.push(segment);
                        }
                    }
                }
            }
            ']' if object_depth == 0 => break,
            _ => {}
        }
    }
    (!segments.is_empty()).then(|| EditPlan {
        title: fallback_title.to_string(),
        style: fallback_style.to_string(),
        target_duration_secs: 1.0,
        segments,
    })
}

fn ollama_chat_body(
    model: &str,
    system: &str,
    user: &str,
    schema: &Value,
    temperature: f64,
    num_predict: u32,
) -> Value {
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "stream": false,
        "think": false,
        "format": schema,
        "keep_alive": "10m",
        "options": {
            "temperature": temperature,
            "num_ctx": 65536,
            "num_predict": num_predict
        }
    })
}

fn ollama_response_content(parsed: ChatResponse) -> Result<(String, Option<String>), String> {
    if parsed.message.content.trim().is_empty() && !parsed.message.thinking.trim().is_empty() {
        return Err(format!(
            "Ollama 只返回了思考内容，没有导演 JSON（结束原因：{}）。请求已设置 think=false，但模型仍未输出正文；请检查该模型的思考模式支持情况后重试。延长等待时间不能解决输出 token 上限问题。",
            parsed.done_reason.as_deref().unwrap_or("未知")
        ));
    }
    Ok((parsed.message.content, parsed.done_reason))
}

async fn chat_content(
    client: &Client,
    backend: &DirectorBackend,
    system: &str,
    user: &str,
    schema: &Value,
    temperature: f64,
    num_predict: u32,
    _schema_name: &str,
) -> Result<(&'static str, String, Option<String>), String> {
    let DirectorBackend::Ollama { base_url, model } = backend;
    let provider = "Ollama";
    let response = client
        .post(format!("{base_url}/api/chat"))
        .json(&ollama_chat_body(
            model,
            system,
            user,
            schema,
            temperature,
            num_predict,
        ))
        .send()
        .await
        .map_err(|error| ollama_request_error(&error, OLLAMA_GENERATION_TIMEOUT))?;
    let status = response.status();
    let raw = response
        .text()
        .await
        .map_err(|error| ollama_request_error(&error, OLLAMA_GENERATION_TIMEOUT))?;
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|value| {
                value.get("error").and_then(|error| {
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .or_else(|| error.as_str())
                        .map(str::to_string)
                })
            })
            .unwrap_or(raw);
        return Err(format!("{provider} 返回 HTTP {status}：{detail}"));
    }
    let parsed = serde_json::from_str::<ChatResponse>(&raw)
        .map_err(|error| format!("无法解析 Ollama 响应：{error}"))?;
    let (content, finish_reason) = ollama_response_content(parsed)?;
    Ok((provider, content, finish_reason))
}

fn beat_system_prompt() -> &'static str {
    "你是影视事实记录员。输入是连续场景字幕，其中 CORE 是本轮可输出区间，CONTEXT 只用于理解前后场景与省略主语。只提取字幕直接证明的局部事件，不写解说、不规划成片、不编造身份、心理、动机、因果或结局。每个节拍只说明‘谁在什么处境中做了什么→出现了什么可观察变化’；没有可观察变化的操作步骤、问候、报价、重复说明和哲学扩写不是独立节拍。旧字幕中可能存在的自动聚类编号已被程序移除，因为它们不稳定且不是人物身份。只有连续字幕中的姓名、称呼或明确问答关系能同时证明身份与动作时才能点名；日语省略主语时宁可写‘对方、同事、家属、现场的人’，不得把当前话题人物自动当作说话者。第一人称‘我留下了孩子’只属于当前说话者，绝不能自动改写成主角童年。summary 禁止‘这揭示了、这反映了、为后续埋下伏笔、迫使他重新审视、内心冲突、存在意义’等分析评论，只记录事实变化。importance 为 1–5：5只给改变主线或兑现核心伏笔的事件，1只给可删除细节。start/end 必须取自能直接证明事件的 CORE 字幕时间，通常4–20秒，单个节拍不得超过35秒；更长场景要按实际变化拆分。quote 只填写字幕中确实出现、单独听仍有情绪或叙事价值的完整关键台词，否则留空。narrative_layer 必须标为 main、framing、in_world_media、flashback 或 uncertain：演员采访、节目导语、幕后说明、预告和主线开始前播放的片中节目都属于 framing；角色正在观看且会推动主线的电视内容才属于 in_world_media。绝不能把 framing 或 in_world_media 里的人物动作写成主角亲身经历。忽略片头曲、片尾曲、预告、广告、赞助、下载引导、寒暄和重复口播。"
}

fn dialogue_attribution_system_prompt() -> &'static str {
    "你是电影对白归属编辑。输入是按时间排列、已附原字幕证据的剧情节拍。每个 beat_id 至少返回一条归属判断；同一节拍里凡会被用于判断人物身份、人物关系、行动主体或因果的关键台词，都要各自返回，最多三条。不能漏掉任何 beat_id。evidence_quote 必须逐字复制该节拍 evidence 中的单条原字幕，不得改写或跨行拼接。speaker 表示这句话是谁说的，addressee 表示主要说给谁听：只能依据相邻问答、直接称呼、第一/第二人称承接、人物出入场和本批次已出现的连续状态判断，不得调用电影常识。能由字幕明确确认时使用姓名或稳定身份；只能判断是同一现场人物但不知道身份时使用自然中性称呼；无法确认就写‘身份未确认’，绝不能把台词谈论的人当成说话者。confidence 为0到1；低于0.8或存在冲突时，speaker 必须写‘身份未确认’，并在 uncertainty 简短说明缺少什么证据。addressee 不明确时写‘未确认’。未进入本表的台词只能作为场景上下文，不能证明人物身份、关系或说话对象。这一步只建立派生理解，不修改原字幕、不写旁白。"
}

fn blueprint_system_prompt() -> &'static str {
    "你是全片 Story Builder。当前输入包含带稳定编号的局部事实节拍，以及逐节拍对白归属表；你的任务是先建立整部作品唯一的人物、关系、线索与因果模型，绝对不要写分段旁白。局部 summary 和对白归属都是待审派生理解，和原字幕 evidence 冲突时必须以 evidence 为准；speaker=身份未确认、confidence<0.8 或 uncertainty 非空的归属没有人物命名权。每条 StoryClaim 只能表达一个主语的一项原子事实，必须引用 beat_ids，并在 evidence_quotes 中逐字复制1–3条真正证明该事实的原字幕短句；只有对白归属与原文能同时证明‘是谁’和‘做了什么’才能写入人物事实。confidence 为0到1；字幕直接证明且无冲突时 uncertainty 必须为空，不要在 uncertainty 中写分析过程；主语、亲属、婚姻、职业或说话对象有任何缺口时，uncertainty 简短说明缺口且 confidence 不得超过0.65。protagonist/desire/core_conflict/stakes 说明叙事骨架；central_question 提出全片持续追问的人物问题；interpretive_thesis 必须由至少三个不同 beat_id 支撑；character_arc、relationship_arc、recurring_evidence、causal_chain 和 ending_echo 都要描述可观察的前后变化。character_bible 是全片唯一人物表：姓名不确定就使用稳定的自然身份称呼，aliases 只收录字幕明确支持的别称。relationships 的两端必须引用 character_bible 的 id，关系变化同样逐条举证。story_threads 用 beat_ids 记录问题如何建立和回收。严格区分父亲、母亲、伴侣、同事等身份；第一人称‘我留下了孩子’只证明说话者自己的孩子，不能自动变成主角童年。发现局部节拍互相冲突时保留为不同人物，不擅自合并。片名只能帮助理解任务，不是事实证据；禁止调用电影常识补剧情。"
}

fn continuity_audit_system_prompt() -> &'static str {
    "你是全片人物连续性与事实审校员。输入包含一版 Story Model、全部带原字幕 evidence 的 Story Beats 和逐节拍对白归属表。请返回一份完整修订后的 Story Model，而不是问题列表。只依据输入字幕，不调用电影常识。逐条反查每个事实所引台词是谁说、说给谁听；对白归属 confidence<0.8、uncertainty 非空或标为身份未确认时，不得据此确定姓名和亲属关系。某人若已明确死亡、离场或不在场，后续第一人称台词不能仅因话题相关就归给此人；日语常省略主语，‘我当年留下孩子’只说明当前说话者抛下自己的孩子，不能自动变成‘主角母亲抛弃主角’，除非姓名、亲属称呼、问答对象或跨段证据同时证明两者为同一人。每条 StoryClaim 只保留一个原子事实，evidence_quotes 必须逐字复制能同时证明主语和事件的原字幕；引文只证明事件却不能证明身份时，必须拆开身份与事件，身份项写 uncertainty 且 confidence≤0.65。字幕直接证明且无冲突时 uncertainty 必须为空。检查数字、专有词和同音词：局部 summary 与 evidence 冲突时以 evidence 为准；如果词语在语境中明显不成立但正确词也无法由相邻字幕证明，就改写成不依赖该词的安全事实，不猜具体术语。character_bible 不求角色少，宁可保留两个身份未定角色，也不能错误合并。人物弧线只能由可观察选择与关系变化组成，不写模型自创心理。所有事实与关系仍必须引用有效 beat_id。"
}

fn dialogue_attribution_schema(expected: usize) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "attributions": {
                "type": "array",
                "minItems": expected,
                "maxItems": expected.saturating_mul(3),
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "beat_id": {"type": "string"},
                        "evidence_quote": {"type": "string", "minLength": 2},
                        "speaker": {"type": "string", "minLength": 1},
                        "addressee": {"type": "string", "minLength": 1},
                        "confidence": {"type": "number", "minimum": 0, "maximum": 1},
                        "uncertainty": {"type": "string"}
                    },
                    "required": ["beat_id", "evidence_quote", "speaker", "addressee", "confidence", "uncertainty"]
                }
            }
        },
        "required": ["attributions"]
    })
}

fn story_plan_system_prompt() -> &'static str {
    "你是全片 Story Planner，不负责遣词造句。根据 Story Model 和带编号的事实节拍，决定这部作品到底怎么讲。units 是成片级 Narrative Unit，不是字幕事件清单：一个 unit 应合并同一戏剧目的下的多个局部节拍，完整表达人物的处境、选择、变化或后果；面试中的进门、问答、误会、解释和决定应合成一个 unit，而不是逐条拆开。优先保留改变主角目标、关系、认识、代价以及建立/回收伏笔的节拍；操作细节、报价、寒暄、重复对白和不推动主线的信息放入 omitted_beat_ids。每个有效 unit 必须引用一个或多个 beat_ids，并说明 entry_state、turn、exit_state 与 narration_goal；importance 为1到5。audio_strategy 只能是 narration、original_dialogue、transition、emotional_pause：只有被引用节拍确有完整 quote 且原声比转述更有价值时才用 original_dialogue，并把该编号写入 original_audio_beat_id，否则该字段留空。units 按原片剧情顺序排列，前后形成一条连续因果线；开头从具体人物困境进入，结尾回答中心问题。不要为了达到数量拆碎单位，也不要为了压缩而丢失关键因果。"
}

fn chapter_system_prompt(style: &str, is_first_chapter: bool) -> String {
    let opening_rule = if is_first_chapter {
        "第一段从具体人物的一次反常选择或关系困境进入故事，前45字让观众知道‘谁正在面对什么’，留下一个自然未解的问题；不夸大危险，不连续反问。"
    } else {
        "第一段承接上一章造成的关系或选择变化，再进入本章的新处境；不要重新起一个营销钩子。"
    };
    let base = format!(
        "你是成熟的中文电影解说 Writer。本次只负责写旁白，不负责选镜头或时间码；程序会把每个 Narrative Unit 映射到已经核验的字幕节拍。成稿应有统一、克制的叙述人格，但不得模仿任何具体创作者的措辞或口头禅。\n\n硬规则：\n1. 全片 Story Model 是人物身份与关系的一致性合同；当前 Narrative Unit 决定讲什么；字幕只负责事实举证。不得把局部理解建议中的身份猜测凌驾于全片人物表，不编造事实。\n2. lines 中每个 unit_id 必须且只能交付一次。一个 Narrative Unit 写成一个完整叙事段，以‘人物状态/处境 → 关键事件或选择 → 造成的变化’为骨架；不得把每条字幕或每个局部 beat 各写一句。\n3. JSON Schema 给每个 unit_id 的 minLength 是制作预算，不是鼓励复述。达到预算要靠讲清此前处境、当下选择、可观察变化和后续压力；不得重复同一事实、堆形容词、虚构心理或用空泛评论注水。{opening_rule}\n4. 旁白要像同一个人完整讲故事，不写成‘发生A、某人说B、另一人回答C、然后发生D’的字幕摘要。相邻 unit 之间要有因果承接，但不要重复上一段结尾。\n5. 全章只保留一条主推进线。每次转折回应前面的具体问题，再把注意力转向人物下一次选择；情绪已经由关键原声成立时，引导语保持克制。\n6. 人物称呼严格服从 character_bible；不确定时省略主语或使用其中的稳定身份称呼。禁止‘大家好、开场、镜头来到、故事开始、这部剧讲了、让我们看看’，避免‘殊不知、命运的齿轮、局面彻底改变、真正的考验、这意味着、不是…而是…’等AI套话。\n7. audio_strategy=original_dialogue 的 unit 只写10–24字引导，让原声完成情绪；其余 unit 写完整旁白。严禁选择片头、片尾、演职员表、广告、赞助、下载引导和预告。\n8. 只输出 {{\"lines\":{{\"unit_id\":\"旁白\"}}}} 结构，键名必须与输入 unit_id 完全一致；不要输出镜头、时间码、解释或 Markdown。\n\n风格规范：\n{STYLE_CATALOG}\n\n当前风格：{style}。只输出符合 Schema 的紧凑 JSON。"
    );
    format!(
        "{base}\n\n人物连续性附加规则：输入中的 Story Model 已经只保留 confidence≥0.8、uncertainty 为空且有原文引句的人物事实；它没有列出的姓名、亲属、婚姻或职业身份一律没有命名权限。Story Beat 的 summary 是上游待审理解，其中出现的姓名、亲属或说话对象不能绕过可信人物表；这时只能使用不虚构关系的自然称呼或省略主语。某角色若已被明确说明死亡、离开或不在场，不得把后续说话者自动认成该角色；日语省略主语时尤其禁止仅凭一句第一人称自述推断‘母亲、妻子、父亲’。当前 Unit 的 narration_goal 只约束本段，不得把相邻 Unit 的相同宏观目标重复讲一遍。"
    )
}

fn fact_check_system_prompt() -> &'static str {
    "你是独立于写稿模型的影视事实裁判。只使用程序给每段列出的字幕证据候选核对导演稿，不得调用电影常识补全。必须为每个 segment_index 返回且只返回一个 verdict。evidence_ids 只能选择同段 candidates 中确实支持裁决的编号，不允许引用其他段；不要自己抄写或拼接字幕。重点检查：1. 字幕未明确出现的人名；2. 把演员采访、节目导语、幕后说明、预告、片中节目或回忆误写成主角亲身经历；3. 字幕没有证明的伤势、身份、动机、心理、因果和结局；4. 把不同说话人的台词拼成同一事件。原稿全部受证据支持时 supported=true、corrected_narration 留空；有越界但属于有效剧情时 supported=false，并且必须在 corrected_narration 中写一条由所选证据直接支持的保守完整旁白，不得留空。只有广告、片头片尾、采访导语、重复内容或完全没有本地证据时才允许 evidence_ids 和修正文案都留空，以删除该段。禁止为了保留文采而保留未经证实的断言。只输出 JSON。"
}

fn blueprint_schema() -> Value {
    let claim = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "claim": {"type": "string"},
            "beat_ids": {"type": "array", "minItems": 1, "items": {"type": "string"}, "uniqueItems": true},
            "evidence_quotes": {"type": "array", "minItems": 1, "maxItems": 3, "items": {"type": "string"}},
            "confidence": {"type": "number", "minimum": 0, "maximum": 1},
            "uncertainty": {"type": "string"}
        },
        "required": ["claim", "beat_ids", "evidence_quotes", "confidence", "uncertainty"]
    });
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "protagonist": {"type": "string"},
            "desire": {"type": "string"},
            "core_conflict": {"type": "string"},
            "stakes": {"type": "string"},
            "hook": {"type": "string"},
            "central_question": {"type": "string"},
            "interpretive_thesis": {"type": "string"},
            "character_arc": {"type": "array", "items": {"type": "string"}},
            "relationship_arc": {"type": "array", "items": {"type": "string"}},
            "recurring_evidence": {"type": "array", "items": {"type": "string"}},
            "causal_chain": {
                "type": "array",
                "minItems": 1,
                "items": {"type": "string"}
            },
            "payoff": {"type": "string"},
            "ending_echo": {"type": "string"},
            "character_bible": {"type": "array", "minItems": 1, "items": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "id": {"type": "string"},
                    "canonical_name": {"type": "string"},
                    "aliases": {"type": "array", "items": {"type": "string"}, "uniqueItems": true},
                    "role": {"type": "string"},
                    "facts": {"type": "array", "minItems": 1, "items": claim.clone()}
                },
                "required": ["id", "canonical_name", "aliases", "role", "facts"]
            }},
            "relationships": {"type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "from_character_id": {"type": "string"},
                    "to_character_id": {"type": "string"},
                    "changes": {"type": "array", "minItems": 1, "items": claim.clone()}
                },
                "required": ["from_character_id", "to_character_id", "changes"]
            }},
            "story_threads": {"type": "array", "minItems": 1, "items": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "id": {"type": "string"},
                    "question": {"type": "string"},
                    "beat_ids": {"type": "array", "minItems": 1, "items": {"type": "string"}, "uniqueItems": true},
                    "resolution": {"type": "string"},
                    "importance": {"type": "integer", "minimum": 1, "maximum": 5}
                },
                "required": ["id", "question", "beat_ids", "resolution", "importance"]
            }}
        },
        "required": ["protagonist", "desire", "core_conflict", "stakes", "hook", "central_question", "interpretive_thesis", "character_arc", "relationship_arc", "recurring_evidence", "causal_chain", "payoff", "ending_echo", "character_bible", "relationships", "story_threads"]
    })
}

fn story_plan_schema(min_units: usize, max_units: usize) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "opening_unit_id": {"type": "string"},
            "ending_unit_id": {"type": "string"},
            "units": {"type": "array", "minItems": min_units, "maxItems": max_units.max(min_units), "items": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "id": {"type": "string"},
                    "title": {"type": "string"},
                    "purpose": {"type": "string"},
                    "beat_ids": {"type": "array", "minItems": 1, "items": {"type": "string"}, "uniqueItems": true},
                    "entry_state": {"type": "string"},
                    "turn": {"type": "string"},
                    "exit_state": {"type": "string"},
                    "narration_goal": {"type": "string"},
                    "audio_strategy": {"type": "string", "enum": ["narration", "original_dialogue", "transition", "emotional_pause"]},
                    "original_audio_beat_id": {"type": "string"},
                    "importance": {"type": "integer", "minimum": 1, "maximum": 5}
                },
                "required": ["id", "title", "purpose", "beat_ids", "entry_state", "turn", "exit_state", "narration_goal", "audio_strategy", "original_audio_beat_id", "importance"]
            }},
            "omitted_beat_ids": {"type": "array", "items": {"type": "string"}, "uniqueItems": true}
        },
        "required": ["opening_unit_id", "ending_unit_id", "units", "omitted_beat_ids"]
    })
}

fn beat_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "beats": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "start": {"type": "number"},
                        "end": {"type": "number"},
                        "summary": {"type": "string"},
                        "importance": {"type": "integer", "minimum": 1, "maximum": 5},
                        "quote": {"type": "string"},
                        "kind": {"type": "string"},
                        "narrative_layer": {
                            "type": "string",
                            "enum": ["main", "framing", "in_world_media", "flashback", "uncertain"]
                        }
                    },
                    "required": ["start", "end", "summary", "importance", "quote", "kind", "narrative_layer"]
                }
            }
        },
        "required": ["beats"]
    })
}

pub(crate) fn edit_plan_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "title": {"type": "string"},
            "style": {"type": "string"},
            "target_duration_secs": {"type": "number"},
            "segments": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "src_start": {"type": "number"},
                        "src_end": {"type": "number"},
                        "shots": {"type":"array","items":{"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2}},
                        "narration": {"type": "string"},
                        "keep_original_audio": {"type": "boolean"}
                    },
                    "required": ["src_start", "src_end", "shots", "narration", "keep_original_audio"]
                }
            }
        },
        "required": ["title", "style", "target_duration_secs", "segments"]
    })
}

fn chapter_script_schema(
    units: &[NarrativeUnit],
    budgets: &std::collections::BTreeMap<String, usize>,
) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for unit in units {
        let minimum = budgets.get(&unit.id).copied().unwrap_or(48).max(10);
        let maximum = if unit.audio_strategy == "original_dialogue" {
            24.max(minimum)
        } else {
            (minimum + 72).max((minimum as f64 * 1.45).ceil() as usize)
        };
        properties.insert(
            unit.id.clone(),
            json!({
                "type": "string",
                "minLength": minimum,
                "maxLength": maximum
            }),
        );
        required.push(Value::String(unit.id.clone()));
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "lines": {
                "type": "object",
                "additionalProperties": false,
                "properties": properties,
                "required": required
            }
        },
        "required": ["lines"]
    })
}

fn story_unit_budgets(
    units: &[NarrativeUnit],
    total_chars: usize,
) -> std::collections::BTreeMap<String, usize> {
    let mut budgets = std::collections::BTreeMap::new();
    if units.is_empty() {
        return budgets;
    }
    let average = total_chars / units.len().max(1);
    let normal_minimum = if average >= 60 { 48 } else { average.max(10) };
    let original_target = average.min(18).max(10);
    let normal_weight = |unit: &NarrativeUnit| {
        unit.importance.max(1) as usize * 10 + unit.beat_ids.len().max(1) * 3
    };
    let normal_weight_sum = units
        .iter()
        .filter(|unit| unit.audio_strategy != "original_dialogue")
        .map(normal_weight)
        .sum::<usize>();
    let normal_count = units
        .iter()
        .filter(|unit| unit.audio_strategy != "original_dialogue")
        .count();
    let original_total = units
        .iter()
        .filter(|unit| unit.audio_strategy == "original_dialogue")
        .count()
        * original_target;
    let distributable = total_chars.saturating_sub(original_total);
    // The usual editorial ceiling is 180 characters, but a renderer that ignored
    // the planner's minimum unit count may leave fewer containers. Keep the
    // allocator finite and capable of representing the requested total; the
    // deterministic unit splitter above should make this fallback uncommon.
    let unit_cap = if normal_count == 0 {
        180
    } else {
        distributable.div_ceil(normal_count).max(180)
    };
    for unit in units {
        let budget = if unit.audio_strategy == "original_dialogue" {
            original_target
        } else if normal_weight_sum == 0 {
            average.max(10)
        } else {
            ((distributable as f64 * normal_weight(unit) as f64 / normal_weight_sum as f64).round()
                as usize)
                .max(normal_minimum)
                .min(unit_cap)
        };
        budgets.insert(unit.id.clone(), budget);
    }
    let mut allocated = budgets.values().sum::<usize>();
    let adjustable = units
        .iter()
        .filter(|unit| unit.audio_strategy != "original_dialogue")
        .map(|unit| unit.id.clone())
        .collect::<Vec<_>>();
    if !adjustable.is_empty() {
        let mut cursor = 0usize;
        while allocated < total_chars {
            let next = (0..adjustable.len())
                .map(|offset| (cursor + offset) % adjustable.len())
                .find(|index| budgets[&adjustable[*index]] < unit_cap);
            let Some(index) = next else {
                break;
            };
            *budgets
                .get_mut(&adjustable[index])
                .expect("known unit budget") += 1;
            allocated += 1;
            cursor = index + 1;
        }
        cursor = 0;
        while allocated > total_chars {
            let next = (0..adjustable.len())
                .map(|offset| (cursor + offset) % adjustable.len())
                .find(|index| budgets[&adjustable[*index]] > normal_minimum);
            let Some(index) = next else {
                break;
            };
            *budgets
                .get_mut(&adjustable[index])
                .expect("known unit budget") -= 1;
            allocated -= 1;
            cursor = index + 1;
        }
    }
    budgets
}

fn chapter_script_to_edit_plan(
    script: ChapterScript,
    chapter: &StoryChapter,
    title: &str,
    style: &str,
) -> Result<EditPlan, String> {
    let by_id = chapter
        .beats
        .iter()
        .map(|beat| (beat.id.as_str(), beat))
        .collect::<std::collections::HashMap<_, _>>();
    let mut segments = Vec::with_capacity(chapter.units.len());
    for unit in &chapter.units {
        let narration = script
            .lines
            .get(&unit.id)
            .map(|line| clean_narration(line))
            .filter(|line| !line.is_empty())
            .ok_or_else(|| format!("Writer 未交付叙事单元 {} 的旁白", unit.id))?;
        let mut unit_beats = unit
            .beat_ids
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).copied())
            .collect::<Vec<_>>();
        unit_beats.sort_by(|left, right| left.start.total_cmp(&right.start));
        let first = unit_beats
            .first()
            .ok_or_else(|| format!("叙事单元 {} 没有有效字幕节拍", unit.id))?;
        let last = unit_beats.last().unwrap_or(first);
        let original = if unit.audio_strategy == "original_dialogue" {
            unit_beats
                .iter()
                .find(|beat| beat.id == unit.original_audio_beat_id && !beat.quote.is_empty())
                .copied()
        } else {
            None
        };
        let (src_start, src_end, shots, keep_original_audio) = if let Some(beat) = original {
            (beat.start, beat.end, Vec::new(), true)
        } else {
            (
                first.start,
                last.end,
                unit_beats
                    .iter()
                    .map(|beat| [beat.start, beat.end])
                    .collect(),
                false,
            )
        };
        segments.push(EditSegment {
            shots,
            src_start,
            src_end,
            narration,
            keep_original_audio,
        });
    }
    Ok(EditPlan {
        title: format!("{}｜{}", title, style),
        style: style.to_string(),
        target_duration_secs: 1.0,
        segments,
    })
}

fn fact_check_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "verdicts": {
                "type": "array",
                "minItems": 1,
                "maxItems": 16,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "segment_index": {"type": "integer", "minimum": 0},
                        "supported": {"type": "boolean"},
                        "evidence_ids": {
                            "type": "array",
                            "items": {"type": "string"},
                            "uniqueItems": true
                        },
                        "corrected_narration": {"type": "string"}
                    },
                    "required": ["segment_index", "supported", "evidence_ids", "corrected_narration"]
                }
            }
        },
        "required": ["verdicts"]
    })
}

#[derive(Debug, Clone)]
struct TranscriptChunk {
    core_start: f64,
    core_end: f64,
    text: String,
}

fn transcript_line(entry: &SubtitleEntry, scope: &str) -> String {
    format!(
        "[{scope} {:.1}s - {:.1}s] {}\n",
        subtitle::time_to_secs(&entry.start),
        subtitle::time_to_secs(&entry.end),
        evidence_text(&entry.text)
    )
}

fn transcript_chunks(entries: &[SubtitleEntry], max_chars: usize) -> Vec<TranscriptChunk> {
    if entries.is_empty() {
        return vec![];
    }
    let mut ranges = Vec::<(usize, usize)>::new();
    let mut start = 0usize;
    let mut chars = 0usize;
    for (index, entry) in entries.iter().enumerate() {
        let line_chars = transcript_line(entry, "CORE").len();
        if index > start && chars + line_chars > max_chars {
            ranges.push((start, index));
            start = index;
            chars = 0;
        }
        chars += line_chars;
    }
    ranges.push((start, entries.len()));

    ranges
        .into_iter()
        .map(|(start, end)| {
            let core_start = subtitle::time_to_secs(&entries[start].start);
            let core_end = entries
                .get(end)
                .map(|entry| subtitle::time_to_secs(&entry.start))
                .unwrap_or_else(|| subtitle::time_to_secs(&entries[end - 1].end) + 0.01);
            let context_start = entries[..start]
                .iter()
                .rposition(|entry| subtitle::time_to_secs(&entry.end) < core_start - 90.0)
                .map(|index| index + 1)
                .unwrap_or(0);
            let context_end = entries[end..]
                .iter()
                .position(|entry| subtitle::time_to_secs(&entry.start) > core_end + 45.0)
                .map(|offset| end + offset)
                .unwrap_or(entries.len());
            let mut text = String::new();
            for (index, entry) in entries[context_start..context_end].iter().enumerate() {
                let absolute = context_start + index;
                let scope = if absolute >= start && absolute < end {
                    "CORE"
                } else {
                    "CONTEXT"
                };
                text.push_str(&transcript_line(entry, scope));
            }
            TranscriptChunk {
                core_start,
                core_end,
                text,
            }
        })
        .collect()
}

fn normalize_beats(beats: &mut Vec<StoryBeat>, duration: f64, excluded: &[ExcludedRange]) {
    beats.retain_mut(|beat| {
        beat.start = beat.start.max(0.0);
        beat.end = beat.end.min(duration);
        beat.summary = beat.summary.trim().to_string();
        beat.quote = beat.quote.trim().to_string();
        beat.importance = beat.importance.clamp(1, 5);
        beat.narrative_layer = beat.narrative_layer.trim().to_lowercase();
        beat.end - beat.start >= 0.8
            && beat.end - beat.start <= 35.0
            && !beat.summary.is_empty()
            && beat.narrative_layer != "framing"
            && !overlaps_any(beat.start, beat.end, excluded)
    });
    beats.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(Ordering::Equal));
    beats.dedup_by(|a, b| (a.start - b.start).abs() < 1.0 && a.summary == b.summary);
    for (index, beat) in beats.iter_mut().enumerate() {
        beat.id = format!("B{:04}", index + 1);
    }
}

fn attach_beat_evidence(beats: &mut [StoryBeat], entries: &[SubtitleEntry]) {
    for beat in beats {
        beat.evidence = entries
            .iter()
            .filter(|entry| {
                let start = subtitle::time_to_secs(&entry.start);
                let end = subtitle::time_to_secs(&entry.end);
                end >= beat.start - 2.0 && start <= beat.end + 2.0
            })
            .filter(|entry| !entry.text.trim().is_empty())
            .take(32)
            .map(|entry| {
                format!(
                    "[{:.1}s-{:.1}s] {}",
                    subtitle::time_to_secs(&entry.start),
                    subtitle::time_to_secs(&entry.end),
                    evidence_text(&entry.text)
                )
            })
            .collect();
    }
}

fn attribution_quote_matches(beat: &StoryBeat, quote: &str) -> bool {
    let quote = normalize_for_evidence(quote);
    quote.chars().count() >= 2
        && beat
            .evidence
            .iter()
            .map(|line| normalize_for_evidence(line))
            .any(|line| line.contains(&quote))
}

async fn build_dialogue_attributions(
    client: &Client,
    backend: &DirectorBackend,
    beats: &[StoryBeat],
    checkpoint: &DirectorCheckpoint,
) -> Result<Vec<DialogueAttribution>, String> {
    if let Some(attributions) = checkpoint.read("dialogue-attribution.json") {
        return Ok(attributions);
    }

    let mut collected = Vec::<DialogueAttribution>::new();
    for (batch_index, batch) in beats.chunks(8).enumerate() {
        let relative = format!("dialogue-attribution/batch-{:03}.json", batch_index + 1);
        let response: DialogueAttributionResponse =
            if let Some(response) = checkpoint.read(&relative) {
                response
            } else {
                let established_labels = collected
                    .iter()
                    .filter(|item| item.confidence >= 0.8 && item.uncertainty.is_empty())
                    .map(|item| item.speaker.as_str())
                    .filter(|speaker| *speaker != "身份未确认")
                    .collect::<std::collections::BTreeSet<_>>();
                let input = json!({
                    "previously_established_speaker_labels": established_labels,
                    "beats": batch
                });
                let response = chat_json(
                    client,
                    backend,
                    dialogue_attribution_system_prompt(),
                    &serde_json::to_string_pretty(&input).map_err(|error| error.to_string())?,
                    dialogue_attribution_schema(batch.len()),
                    0.0,
                    8_192,
                    "dialogue_attribution",
                )
                .await?;
                checkpoint.write("dialogue_attribution_batch", &relative, &response)?;
                response
            };

        let by_id = batch
            .iter()
            .map(|beat| (beat.id.as_str(), beat))
            .collect::<std::collections::HashMap<_, _>>();
        let mut accepted = std::collections::HashMap::<String, Vec<DialogueAttribution>>::new();
        for mut attribution in response.attributions {
            let Some(beat) = by_id.get(attribution.beat_id.as_str()) else {
                continue;
            };
            if !attribution_quote_matches(beat, &attribution.evidence_quote) {
                continue;
            }
            attribution.evidence_quote = attribution.evidence_quote.trim().to_string();
            attribution.speaker = attribution.speaker.trim().to_string();
            attribution.addressee = attribution.addressee.trim().to_string();
            attribution.uncertainty = attribution.uncertainty.trim().to_string();
            attribution.confidence = if attribution.confidence.is_finite() {
                attribution.confidence.clamp(0.0, 1.0)
            } else {
                0.0
            };
            if attribution.confidence < 0.8 || !attribution.uncertainty.is_empty() {
                attribution.speaker = "身份未确认".to_string();
            }
            if attribution.addressee.is_empty() {
                attribution.addressee = "未确认".to_string();
            }
            let group = accepted.entry(attribution.beat_id.clone()).or_default();
            let normalized_quote = normalize_for_evidence(&attribution.evidence_quote);
            if group.iter().any(|existing| {
                normalize_for_evidence(&existing.evidence_quote) == normalized_quote
            }) {
                continue;
            }
            if group.len() < 3 {
                group.push(attribution);
            }
        }
        for beat in batch {
            let Some(attributions) = accepted.remove(&beat.id) else {
                return Err(format!(
                    "对白归属未通过：剧情节拍 {} 缺少可逐字匹配的说话人判断。原字幕和检查点均已保留，可直接重试 AI 导演",
                    beat.id
                ));
            };
            collected.extend(attributions);
        }
    }
    checkpoint.write(
        "director_dialogue_attribution",
        "dialogue-attribution.json",
        &collected,
    )?;
    Ok(collected)
}

fn normalize_for_evidence(value: &str) -> String {
    evidence_text(value)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn normalize_story_claim(
    claim: &mut StoryClaim,
    valid_beats: &std::collections::HashSet<String>,
    beat_by_id: &std::collections::HashMap<String, &StoryBeat>,
) {
    claim.claim = claim.claim.trim().to_string();
    claim.confidence = if claim.confidence.is_finite() {
        claim.confidence.clamp(0.0, 1.0)
    } else {
        0.0
    };
    claim.uncertainty = claim.uncertainty.trim().to_string();
    if !claim.uncertainty.is_empty() {
        claim.confidence = claim.confidence.min(0.65);
    }
    claim.beat_ids.retain(|id| valid_beats.contains(id));
    claim.beat_ids.sort();
    claim.beat_ids.dedup();
    let cited_evidence = claim
        .beat_ids
        .iter()
        .filter_map(|id| beat_by_id.get(id))
        .flat_map(|beat| beat.evidence.iter())
        .map(|line| normalize_for_evidence(line))
        .collect::<Vec<_>>();
    claim.evidence_quotes = std::mem::take(&mut claim.evidence_quotes)
        .into_iter()
        .map(|quote| quote.trim().to_string())
        .filter(|quote| {
            let normalized = normalize_for_evidence(quote);
            normalized.chars().count() >= 4
                && cited_evidence
                    .iter()
                    .any(|evidence| evidence.contains(&normalized))
        })
        .collect();
    claim.evidence_quotes.sort();
    claim.evidence_quotes.dedup();
    if claim.evidence_quotes.is_empty() {
        claim.confidence = 0.0;
    }
}

fn normalize_story_model(model: &mut NarrativeBlueprint, beats: &[StoryBeat]) {
    let valid_beats = beats
        .iter()
        .map(|beat| beat.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let beat_by_id = beats
        .iter()
        .map(|beat| (beat.id.clone(), beat))
        .collect::<std::collections::HashMap<_, _>>();
    let mut character_ids = std::collections::HashSet::new();
    for character in &mut model.character_bible {
        character.id = character.id.trim().to_string();
        character.canonical_name = character.canonical_name.trim().to_string();
        character.role = character.role.trim().to_string();
        character.aliases = std::mem::take(&mut character.aliases)
            .into_iter()
            .map(|alias| alias.trim().to_string())
            .filter(|alias| {
                !alias.is_empty()
                    && alias != &character.canonical_name
                    && !contains_speaker_metadata(alias)
            })
            .collect();
        character.aliases.sort();
        character.aliases.dedup();
        for fact in &mut character.facts {
            normalize_story_claim(fact, &valid_beats, &beat_by_id);
        }
        character.facts.retain(|fact| {
            !fact.claim.is_empty() && !fact.beat_ids.is_empty() && !fact.evidence_quotes.is_empty()
        });
    }
    model.character_bible.retain(|character| {
        !character.id.is_empty()
            && !character.canonical_name.is_empty()
            && !character.facts.is_empty()
            && character_ids.insert(character.id.clone())
    });
    let character_ids = model
        .character_bible
        .iter()
        .map(|character| character.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for relationship in &mut model.relationships {
        for change in &mut relationship.changes {
            normalize_story_claim(change, &valid_beats, &beat_by_id);
        }
        relationship.changes.retain(|change| {
            !change.claim.is_empty()
                && !change.beat_ids.is_empty()
                && !change.evidence_quotes.is_empty()
        });
    }
    model.relationships.retain(|relationship| {
        relationship.from_character_id != relationship.to_character_id
            && character_ids.contains(relationship.from_character_id.as_str())
            && character_ids.contains(relationship.to_character_id.as_str())
            && !relationship.changes.is_empty()
    });
    let mut thread_ids = std::collections::HashSet::new();
    for thread in &mut model.story_threads {
        thread.id = thread.id.trim().to_string();
        thread.question = thread.question.trim().to_string();
        thread.resolution = thread.resolution.trim().to_string();
        thread.importance = thread.importance.clamp(1, 5);
        thread.beat_ids.retain(|id| valid_beats.contains(id));
        thread.beat_ids.sort();
        thread.beat_ids.dedup();
    }
    model.story_threads.retain(|thread| {
        !thread.id.is_empty()
            && !thread.question.is_empty()
            && !thread.beat_ids.is_empty()
            && thread_ids.insert(thread.id.clone())
    });
}

fn trusted_story_model(model: &NarrativeBlueprint) -> NarrativeBlueprint {
    let mut trusted = model.clone();
    for character in &mut trusted.character_bible {
        character
            .facts
            .retain(|fact| fact.confidence >= 0.8 && fact.uncertainty.is_empty());
    }
    trusted
        .character_bible
        .retain(|character| !character.facts.is_empty());
    let trusted_ids = trusted
        .character_bible
        .iter()
        .map(|character| character.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    for relationship in &mut trusted.relationships {
        relationship
            .changes
            .retain(|change| change.confidence >= 0.8 && change.uncertainty.is_empty());
    }
    trusted.relationships.retain(|relationship| {
        trusted_ids.contains(relationship.from_character_id.as_str())
            && trusted_ids.contains(relationship.to_character_id.as_str())
            && !relationship.changes.is_empty()
    });
    if !trusted
        .character_bible
        .iter()
        .any(|character| character.canonical_name == trusted.protagonist)
    {
        trusted.protagonist = "主角".into();
    }
    trusted
}

fn story_unit_count_hint(duration: f64, beat_count: usize) -> (usize, usize) {
    let duration_target = ((duration.max(60.0) / 120.0).round() as usize).clamp(8, 60);
    let evidence_target = ((beat_count as f64 * 0.75).ceil() as usize).max(4);
    let target = duration_target.min(evidence_target).min(beat_count.max(1));
    (
        target.saturating_sub(8).max(6).min(beat_count.max(1)),
        (target + 10).min(72).max(6).min(beat_count.max(1)),
    )
}

fn merge_story_units(left: &mut NarrativeUnit, right: NarrativeUnit) {
    left.title = format!("{} / {}", left.title.trim(), right.title.trim());
    left.purpose = format!("{}；{}", left.purpose.trim(), right.purpose.trim());
    left.turn = format!("{}；{}", left.turn.trim(), right.turn.trim());
    left.exit_state = right.exit_state;
    left.narration_goal = format!(
        "{}；{}",
        left.narration_goal.trim(),
        right.narration_goal.trim()
    );
    left.importance = left.importance.max(right.importance);
    left.beat_ids.extend(right.beat_ids);
    if left.original_audio_beat_id.is_empty() {
        left.original_audio_beat_id = right.original_audio_beat_id;
        if !left.original_audio_beat_id.is_empty() {
            left.audio_strategy = "original_dialogue".into();
        }
    }
}

fn refresh_unit_from_own_beats(
    unit: &mut NarrativeUnit,
    beat_by_id: &std::collections::HashMap<String, &StoryBeat>,
) {
    let selected = unit
        .beat_ids
        .iter()
        .filter_map(|id| beat_by_id.get(id).copied())
        .collect::<Vec<_>>();
    let Some(first) = selected.first() else {
        return;
    };
    let last = selected.last().unwrap_or(first);
    let base_title = unit
        .title
        .split('｜')
        .next()
        .unwrap_or(unit.title.as_str())
        .trim();
    let focus = first.summary.chars().take(24).collect::<String>();
    unit.title = format!("{base_title}｜{focus}");
    unit.entry_state = first.summary.clone();
    unit.turn = selected
        .iter()
        .map(|beat| beat.summary.as_str())
        .collect::<Vec<_>>()
        .join("；");
    unit.exit_state = last.summary.clone();
    unit.narration_goal = format!(
        "只讲清本单元 {} 个节拍形成的具体处境、事件与变化；不要复述同一宏观章节中其他单元的事实：{}",
        selected.len(), unit.turn
    );
}

fn normalize_story_plan(
    plan: &mut StoryPlan,
    beats: &[StoryBeat],
    min_units: usize,
    max_units: usize,
) {
    let beat_by_id = beats
        .iter()
        .map(|beat| (beat.id.clone(), beat))
        .collect::<std::collections::HashMap<_, _>>();
    let beat_order = beats
        .iter()
        .enumerate()
        .map(|(index, beat)| (beat.id.clone(), index))
        .collect::<std::collections::HashMap<_, _>>();
    let mut globally_selected = std::collections::HashSet::new();
    for unit in &mut plan.units {
        unit.title = unit.title.trim().to_string();
        unit.purpose = unit.purpose.trim().to_string();
        unit.importance = unit.importance.clamp(1, 5);
        unit.beat_ids
            .retain(|id| beat_by_id.contains_key(id) && globally_selected.insert(id.clone()));
        unit.beat_ids
            .sort_by_key(|id| beat_order.get(id).copied().unwrap_or(usize::MAX));
        let original_is_valid = unit.audio_strategy == "original_dialogue"
            && unit.beat_ids.contains(&unit.original_audio_beat_id)
            && beat_by_id
                .get(&unit.original_audio_beat_id)
                .is_some_and(|beat| !beat.quote.trim().is_empty());
        if !original_is_valid {
            unit.original_audio_beat_id.clear();
            if unit.audio_strategy == "original_dialogue" {
                unit.audio_strategy = "narration".into();
            }
        }
        if !matches!(
            unit.audio_strategy.as_str(),
            "narration" | "original_dialogue" | "transition" | "emotional_pause"
        ) {
            unit.audio_strategy = "narration".into();
        }
    }
    plan.units
        .retain(|unit| !unit.title.is_empty() && !unit.beat_ids.is_empty());
    plan.units.sort_by_key(|unit| {
        unit.beat_ids
            .first()
            .and_then(|id| beat_order.get(id))
            .copied()
            .unwrap_or(usize::MAX)
    });

    // The planner may accidentally omit a central turn. Attach every importance
    // 4/5 beat to the closest existing unit instead of creating another fragment.
    let missing_essential = beats
        .iter()
        .filter(|beat| beat.importance >= 4 && !globally_selected.contains(&beat.id))
        .cloned()
        .collect::<Vec<_>>();
    for beat in missing_essential {
        if let Some(unit) = plan.units.iter_mut().min_by_key(|unit| {
            unit.beat_ids
                .iter()
                .filter_map(|id| beat_by_id.get(id))
                .map(|candidate| (candidate.start - beat.start).abs() as u64)
                .min()
                .unwrap_or(u64::MAX)
        }) {
            unit.beat_ids.push(beat.id.clone());
            unit.beat_ids
                .sort_by_key(|id| beat_order.get(id).copied().unwrap_or(usize::MAX));
            globally_selected.insert(beat.id);
        }
    }

    // A Narrative Unit may skip unimportant beats, but it must never reach past
    // another unit and then return later. Such interleaving creates source windows
    // hundreds of seconds wide; chapter-local validation accepts them, while the
    // whole-film merge later has to discard the enclosed paragraphs. Split those
    // assignments into monotonic runs before any writing begins.
    if !plan.units.is_empty() {
        let mut assigned = plan
            .units
            .iter()
            .enumerate()
            .flat_map(|(owner, unit)| {
                let beat_order = &beat_order;
                unit.beat_ids.iter().filter_map(move |id| {
                    beat_order
                        .get(id)
                        .copied()
                        .map(|order| (order, owner, id.clone()))
                })
            })
            .collect::<Vec<_>>();
        assigned.sort_by_key(|(order, _, _)| *order);
        let source_units = plan.units.clone();
        let mut linear = Vec::<NarrativeUnit>::new();
        let mut last_owner = None;
        for (_, owner, beat_id) in assigned {
            if last_owner == Some(owner) {
                linear
                    .last_mut()
                    .expect("owner run has a unit")
                    .beat_ids
                    .push(beat_id);
                continue;
            }
            let mut unit = source_units[owner].clone();
            unit.beat_ids = vec![beat_id.clone()];
            if unit.original_audio_beat_id != beat_id {
                unit.original_audio_beat_id.clear();
                if unit.audio_strategy == "original_dialogue" {
                    unit.audio_strategy = "narration".into();
                }
            }
            linear.push(unit);
            last_owner = Some(owner);
        }
        plan.units = linear;
    }

    if plan.units.is_empty() {
        let important = beats
            .iter()
            .filter(|beat| beat.importance >= 3)
            .cloned()
            .collect::<Vec<_>>();
        let fallback_beats = if important.is_empty() {
            beats.to_vec()
        } else {
            important
        };
        for chunk in fallback_beats.chunks(3) {
            let Some(first) = chunk.first() else { continue };
            let last = chunk.last().unwrap_or(first);
            plan.units.push(NarrativeUnit {
                id: String::new(),
                title: first.summary.clone(),
                purpose: "保留关键因果".into(),
                beat_ids: chunk.iter().map(|beat| beat.id.clone()).collect(),
                entry_state: String::new(),
                turn: chunk
                    .iter()
                    .map(|beat| beat.summary.as_str())
                    .collect::<Vec<_>>()
                    .join("；"),
                exit_state: last.summary.clone(),
                narration_goal: "把相邻关键事件组织成一个完整意群".into(),
                audio_strategy: "narration".into(),
                original_audio_beat_id: String::new(),
                importance: chunk.iter().map(|beat| beat.importance).max().unwrap_or(3),
            });
        }
    }

    // Some local renderers accept the schema but ignore `minItems`, returning a
    // dozen macro chapters for a feature-length film. A macro chapter is useful
    // planning, but it cannot carry several minutes of narration as one editable
    // paragraph. Split the largest multi-beat units into chronological runs until
    // the requested floor is met. We never invent beats or split a single beat.
    let unit_floor = min_units.min(
        plan.units
            .iter()
            .map(|unit| unit.beat_ids.len())
            .sum::<usize>(),
    );
    while plan.units.len() < unit_floor {
        let Some(split_at) = plan
            .units
            .iter()
            .enumerate()
            .filter(|(_, unit)| unit.beat_ids.len() > 1)
            .max_by_key(|(_, unit)| unit.beat_ids.len())
            .map(|(index, _)| index)
        else {
            break;
        };
        let mut right = plan.units[split_at].clone();
        let midpoint = plan.units[split_at].beat_ids.len().div_ceil(2);
        right.beat_ids = plan.units[split_at].beat_ids.split_off(midpoint);
        let original_audio_id = plan.units[split_at].original_audio_beat_id.clone();
        if !original_audio_id.is_empty() {
            if right.beat_ids.contains(&original_audio_id) {
                plan.units[split_at].original_audio_beat_id.clear();
                plan.units[split_at].audio_strategy = "narration".into();
            } else {
                right.original_audio_beat_id.clear();
                right.audio_strategy = "narration".into();
            }
        }
        plan.units.insert(split_at + 1, right);
    }

    // Linearisation and minimum-unit recovery may clone planner metadata. Rebuild
    // each unit's writing goal from only its own beat ids, otherwise neighbouring
    // split units ask the Writer to narrate the same macro chapter repeatedly.
    for unit in &mut plan.units {
        refresh_unit_from_own_beats(unit, &beat_by_id);
    }

    // A pathological one-event-per-unit response recreates the original failure.
    // Consolidate adjacent low-level units to a duration-scaled ceiling.
    while plan.units.len() > max_units.max(1) {
        let merge_at = (0..plan.units.len() - 1)
            .min_by_key(|index| {
                let left = &plan.units[*index];
                let right = &plan.units[*index + 1];
                (
                    left.importance.max(right.importance),
                    left.beat_ids.len() + right.beat_ids.len(),
                )
            })
            .unwrap_or(0);
        let right = plan.units.remove(merge_at + 1);
        merge_story_units(&mut plan.units[merge_at], right);
    }
    for (index, unit) in plan.units.iter_mut().enumerate() {
        unit.id = format!("N{:03}", index + 1);
        unit.beat_ids
            .sort_by_key(|id| beat_order.get(id).copied().unwrap_or(usize::MAX));
        unit.beat_ids.dedup();
    }
    plan.opening_unit_id = plan
        .units
        .first()
        .map(|unit| unit.id.clone())
        .unwrap_or_default();
    plan.ending_unit_id = plan
        .units
        .last()
        .map(|unit| unit.id.clone())
        .unwrap_or_default();
    let selected = plan
        .units
        .iter()
        .flat_map(|unit| unit.beat_ids.iter().cloned())
        .collect::<std::collections::HashSet<_>>();
    plan.omitted_beat_ids = beats
        .iter()
        .filter_map(|beat| (!selected.contains(&beat.id)).then_some(beat.id.clone()))
        .collect();
}

fn evidence_for_beats(entries: &[SubtitleEntry], beats: &[StoryBeat]) -> String {
    let mut lines = Vec::new();
    for entry in entries {
        let start = subtitle::time_to_secs(&entry.start);
        let end = subtitle::time_to_secs(&entry.end);
        if beats
            .iter()
            .any(|beat| end >= beat.start - 12.0 && start <= beat.end + 12.0)
        {
            lines.push(format!(
                "[{start:.1}s - {end:.1}s] {}",
                evidence_text(&entry.text)
            ));
        }
    }
    let mut evidence = lines.join("\n");
    if evidence.len() > 42_000 {
        truncate_utf8(&mut evidence, 42_000);
    }
    evidence
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

fn finalize_plan(
    plan: &mut EditPlan,
    entries: &[SubtitleEntry],
    beats: &[StoryBeat],
    excluded: &[ExcludedRange],
    source_title: &str,
    style: &str,
    head_limit: Option<f64>,
    tail_limit: Option<f64>,
    duration: f64,
) -> Result<(), String> {
    plan.style = style.to_string();
    if plan.title.trim().is_empty() {
        plan.title = format!("{}｜{}", source_title, style);
    }
    plan.segments.sort_by(|a, b| {
        a.src_start
            .partial_cmp(&b.src_start)
            .unwrap_or(Ordering::Equal)
    });
    let candidates = std::mem::take(&mut plan.segments);
    // Raw start of the next paragraph bounds how far this one may grow forward
    // without stealing a neighbour's evidence.
    let upcoming_starts: Vec<f64> = candidates.iter().map(|s| s.src_start).collect();
    let mut normalized = Vec::new();
    for (index, mut segment) in candidates.into_iter().enumerate() {
        segment.narration = clean_narration(&segment.narration);
        if segment.narration.chars().count() < 10
            || overlaps_any(segment.src_start, segment.src_end, excluded)
        {
            continue;
        }
        normalize_segment_shots(&mut segment, duration);
        if segment.shots.is_empty() {
            snap_segment(&mut segment, entries);
        }
        if segment
            .shots
            .iter()
            .any(|r| overlaps_any(r[0], r[1], excluded))
        {
            continue;
        }
        if segment.keep_original_audio
            && !beats.iter().any(|beat| {
                !beat.quote.trim().is_empty()
                    && beat.end > segment.src_start
                    && beat.start < segment.src_end
            })
        {
            segment.keep_original_audio = false;
        }
        let spoken_secs = (segment.narration.chars().count() as f64 / 4.0).max(2.5);
        // The 1.2x picture reserve is a hard render constraint (pictures must outlast
        // the voice track at native speed), so it overrides the style cap.
        let max_visual_secs = if segment.keep_original_audio {
            12.0
        } else {
            (spoken_secs * 1.6)
                .clamp(4.0, 20.0)
                .max(super::edit_timing::reserve_seconds(&segment))
        };
        if segment.shots.is_empty() && segment.src_end - segment.src_start > max_visual_secs {
            segment.src_end = (segment.src_start + max_visual_secs).min(duration);
        }
        let previous_end = normalized
            .last()
            .map(|previous: &EditSegment| previous.src_end)
            .unwrap_or_else(|| head_limit.unwrap_or(0.0));
        if let Some(previous) = normalized.last() {
            let previous: &EditSegment = previous;
            if segment.src_start < previous.src_end {
                segment.src_start = previous.src_end;
            }
        }
        // Moving the paragraph past its predecessor can invalidate an otherwise
        // valid shot. Re-clip after the window changes instead of rejecting the
        // complete chapter at the final schema gate.
        normalize_segment_shots(&mut segment, duration);
        if segment.shots.is_empty() && segment.src_end - segment.src_start > max_visual_secs {
            segment.src_end = (segment.src_start + max_visual_secs).min(duration);
        }
        // The window (and any explicitly selected shots inside it) must carry the
        // paragraph's own voice track; grow it into free, non-excluded time here so
        // the plan is renderable instead of dying on the first short paragraph.
        ensure_picture_reserve(
            &mut segment,
            excluded,
            duration,
            previous_end,
            upcoming_starts.get(index + 1).copied().or(tail_limit),
        );
        if segment.src_end - segment.src_start >= 0.8 {
            normalized.push(segment);
        }
    }
    if normalized.is_empty() {
        return Err("AI 脚本通过片头片尾/广告过滤后没有可用片段，请重试".into());
    }
    // Continuity rules adapted from NarratoAI's MIT-licensed narration validator:
    // open with narration, and never let original-audio inserts replace the story spine.
    if let Some(first) = normalized.first_mut() {
        first.keep_original_audio = false;
    }
    let mut consecutive_original = 0;
    for segment in &mut normalized {
        if segment.keep_original_audio {
            consecutive_original += 1;
            if consecutive_original > 2 {
                segment.keep_original_audio = false;
                consecutive_original = 0;
            }
        } else {
            consecutive_original = 0;
        }
    }
    plan.segments = normalized;
    plan.validate_and_clamp(duration)
        .map_err(|error| error.to_string())
}

/// Canonicalize model-proposed pictures without expanding their authorized
/// paragraph window. Ordering and overlap are representation errors, not reasons
/// to discard an otherwise usable chapter.
fn normalize_segment_shots(segment: &mut EditSegment, duration: f64) {
    if segment.keep_original_audio {
        // Original-audio handoff needs one continuous source interval.
        segment.shots.clear();
        return;
    }
    let window_start = segment.src_start.max(0.0);
    let window_end = if duration > 0.0 {
        segment.src_end.min(duration)
    } else {
        segment.src_end
    };
    let mut shots = std::mem::take(&mut segment.shots)
        .into_iter()
        .filter_map(|shot| {
            if !shot[0].is_finite() || !shot[1].is_finite() {
                return None;
            }
            let start = shot[0].max(window_start);
            let end = shot[1].min(window_end);
            (end - start >= 0.8).then_some([start, end])
        })
        .collect::<Vec<_>>();
    shots.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(Ordering::Equal));
    let mut merged = Vec::<[f64; 2]>::new();
    for shot in shots {
        if let Some(previous) = merged.last_mut() {
            if shot[0] <= previous[1] {
                previous[1] = previous[1].max(shot[1]);
                continue;
            }
        }
        merged.push(shot);
    }
    segment.shots = merged;
}

/// Repair only picture/voice fit for an already approved plan. This is used by
/// the review UI so a red reserve warning does not force a fresh multi-hour
/// creative pass or discard the user's current script.
pub async fn repair_existing_picture_fit(
    mut plan: EditPlan,
    entries: &[SubtitleEntry],
    options: &DirectorOptions,
    excluded: &[ExcludedRange],
) -> Result<EditPlan, String> {
    let client = director_client(
        &options.provider,
        director_request_timeout(&options.provider),
    )
    .map_err(|error| error.to_string())?;
    let backend = resolve_backend(&client, options).await?;
    compile_renderable_timeline(&mut plan, entries, excluded, options.duration);
    repair_picture_shortfalls(&client, &backend, &mut plan, entries, None).await?;
    compile_renderable_timeline(&mut plan, entries, excluded, options.duration);
    plan.validate_and_clamp(options.duration)
        .map_err(|error| error.to_string())?;
    Ok(plan)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TimelineCompileReport {
    /// Original-audio handoffs moved to leave a narration lead-in before dialogue.
    pub realigned_original_audio: Vec<usize>,
    /// Original-audio handoffs that could not be placed safely and became narration.
    pub downgraded_original_audio: Vec<usize>,
    /// Narration paragraphs shortened deterministically to fit native-speed pictures.
    pub shortened_narration: Vec<usize>,
    /// Sub-three-second fragments that cannot carry even the shortest narration.
    pub dropped_unrenderable: Vec<usize>,
}

impl TimelineCompileReport {
    pub fn changed(&self) -> bool {
        !self.realigned_original_audio.is_empty()
            || !self.downgraded_original_audio.is_empty()
            || !self.shortened_narration.is_empty()
            || !self.dropped_unrenderable.is_empty()
    }
}

/// Compile an editable director plan into a physically renderable timeline.
///
/// This is intentionally deterministic and local. Every entry point (AI output,
/// restored jobs, manual review and render) can therefore enforce the same rules
/// without asking the model to repair an already approved script.
pub fn compile_renderable_timeline(
    plan: &mut EditPlan,
    entries: &[SubtitleEntry],
    excluded: &[ExcludedRange],
    duration: f64,
) -> TimelineCompileReport {
    plan.segments.sort_by(|left, right| {
        left.src_start
            .partial_cmp(&right.src_start)
            .unwrap_or(Ordering::Equal)
    });
    let original_handoffs = plan
        .segments
        .iter()
        .map(|segment| {
            (
                segment.src_start,
                segment.src_end,
                segment.keep_original_audio,
            )
        })
        .collect::<Vec<_>>();
    let downgraded_original_audio =
        normalize_original_audio_handoffs(plan, entries, excluded, duration);
    let realigned_original_audio = plan
        .segments
        .iter()
        .enumerate()
        .filter_map(|(index, segment)| {
            let (start, end, kept) = original_handoffs[index];
            (kept
                && segment.keep_original_audio
                && ((segment.src_start - start).abs() > 0.000_001
                    || (segment.src_end - end).abs() > 0.000_001))
                .then_some(index)
        })
        .collect();
    reflow_picture_reserve(plan, excluded, duration);

    // A paragraph needs at least ten Chinese characters' three-second reserve.
    // If neighbouring segments and exclusions leave less than that, no wording can
    // make the fragment renderable at native speed. Drop the fragment instead of
    // freezing a frame or failing the entire film late in the pipeline.
    let dropped_unrenderable = super::edit_timing::reserve_issues(plan)
        .into_iter()
        .filter(|issue| {
            issue.severity == super::edit_timing::ReserveSeverity::Blocking
                && ((issue.available * 4.0) / 1.2).floor() < 10.0
        })
        .map(|issue| issue.index)
        .collect::<Vec<_>>();
    if !dropped_unrenderable.is_empty() {
        let dropped = dropped_unrenderable
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        plan.segments = std::mem::take(&mut plan.segments)
            .into_iter()
            .enumerate()
            .filter_map(|(index, segment)| (!dropped.contains(&index)).then_some(segment))
            .collect();
        reflow_picture_reserve(plan, excluded, duration);
    }

    let mut shortened_narration = Vec::new();
    let blocking = super::edit_timing::reserve_issues(plan)
        .into_iter()
        .filter(|issue| issue.severity == super::edit_timing::ReserveSeverity::Blocking)
        .collect::<Vec<_>>();
    for issue in blocking {
        // Keep the same 1.2x headroom used by the renderer's preflight so the
        // measured TTS take can vary slightly without reopening the edit.
        let max_chars = ((issue.available * 4.0) / 1.2).floor() as usize;
        if max_chars < 8 {
            continue;
        }
        let shortened =
            shorten_narration_by_deletion(&plan.segments[issue.index].narration, max_chars);
        if shortened != plan.segments[issue.index].narration {
            plan.segments[issue.index].narration = shortened;
            shortened_narration.push(issue.index);
        }
    }
    // Shortening changes the reserve target. Reflow again so explicit shot lists
    // and segment windows describe exactly what the renderer will consume.
    reflow_picture_reserve(plan, excluded, duration);

    TimelineCompileReport {
        realigned_original_audio,
        downgraded_original_audio,
        shortened_narration,
        dropped_unrenderable,
    }
}

fn reflow_picture_reserve(plan: &mut EditPlan, excluded: &[ExcludedRange], duration: f64) {
    plan.segments.sort_by(|a, b| {
        a.src_start
            .partial_cmp(&b.src_start)
            .unwrap_or(Ordering::Equal)
    });
    let starts = plan
        .segments
        .iter()
        .map(|segment| segment.src_start)
        .collect::<Vec<_>>();
    let mut previous_end = 0.0;
    for (index, segment) in plan.segments.iter_mut().enumerate() {
        normalize_segment_shots(segment, duration);
        ensure_picture_reserve(
            segment,
            excluded,
            duration,
            previous_end,
            starts.get(index + 1).copied(),
        );
        previous_end = segment.src_end;
    }
}

/// Make every original-audio handoff executable before publishing the plan.
/// Expand only into free forward time; if no complete dialogue can fit, keep the
/// paragraph as normal narration instead of failing during TTS/render.
pub fn normalize_original_audio_handoffs(
    plan: &mut EditPlan,
    entries: &[SubtitleEntry],
    excluded: &[ExcludedRange],
    duration: f64,
) -> Vec<usize> {
    let starts = plan
        .segments
        .iter()
        .map(|segment| segment.src_start)
        .collect::<Vec<_>>();
    let ends = plan
        .segments
        .iter()
        .map(|segment| segment.src_end)
        .collect::<Vec<_>>();
    let mut downgraded = Vec::new();
    for (index, segment) in plan.segments.iter_mut().enumerate() {
        if !segment.keep_original_audio {
            continue;
        }
        let spoken = super::edit_timing::spoken_seconds(&segment.narration);
        if super::edit_timing::dialogue_handoff(entries, segment.src_start, segment.src_end, spoken)
            .is_ok()
        {
            continue;
        }
        let previous_end = index
            .checked_sub(1)
            .and_then(|previous| ends.get(previous).copied())
            .unwrap_or(0.0);
        let next_start = starts
            .get(index + 1)
            .copied()
            .unwrap_or(duration)
            .min(duration);
        let forward_limit = free_forward(segment.src_start, next_start, excluded);
        let lead_seconds = super::edit_timing::reserve_seconds(segment);
        let candidate = entries
            .iter()
            .filter_map(|entry| {
                let start = subtitle::time_to_secs(&entry.start);
                let end = subtitle::time_to_secs(&entry.end);
                let backward_limit = free_backward(start, previous_end, excluded);
                let lead_start = start - lead_seconds;
                (lead_start >= backward_limit
                    && end > start
                    && !entry.text.trim().is_empty()
                    && end <= forward_limit
                    && start + 4.0 <= forward_limit)
                    .then_some((
                        // Prefer dialogue already selected by the director. If
                        // several cues overlap, choose the one nearest the raw start.
                        !(start < segment.src_end && end > segment.src_start),
                        (start - segment.src_start).abs(),
                        lead_start,
                        end.max(start + 4.0),
                    ))
            })
            .min_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.total_cmp(&right.1))
            });
        if let Some((_, _, lead_start, target_end)) = candidate {
            segment.src_start = segment.src_start.min(lead_start);
            segment.src_end = segment.src_end.max(target_end);
            segment.shots.clear();
            // A candidate should satisfy the estimator by construction. Keep the
            // fallback local in case floating-point boundaries make it fail.
            if super::edit_timing::dialogue_handoff(
                entries,
                segment.src_start,
                segment.src_end,
                spoken,
            )
            .is_err()
            {
                segment.keep_original_audio = false;
                downgraded.push(index);
            }
        } else {
            segment.keep_original_audio = false;
            downgraded.push(index);
        }
    }
    downgraded
}

async fn repair_picture_shortfalls(
    client: &Client,
    backend: &DirectorBackend,
    plan: &mut EditPlan,
    entries: &[SubtitleEntry],
    checkpoint: Option<&DirectorCheckpoint>,
) -> Result<(), String> {
    let blocking = super::edit_timing::reserve_issues(plan)
        .into_iter()
        .filter(|issue| issue.severity == super::edit_timing::ReserveSeverity::Blocking)
        .collect::<Vec<_>>();
    if blocking.is_empty() {
        return Ok(());
    }
    let evidence = fact_evidence_packet(entries, plan);
    let requests = blocking
        .iter()
        .map(|issue| {
            let max_chars = ((issue.available * 4.0) / 1.2).floor().max(8.0) as usize;
            json!({
                "segment_index": issue.index,
                "max_chars": max_chars,
                "available_picture_seconds": issue.available,
                "original_narration": plan.segments[issue.index].narration,
                "evidence_candidates": evidence[issue.index].candidates
            })
        })
        .collect::<Vec<_>>();
    if let Some(checkpoint) = checkpoint {
        checkpoint.write("picture_fit_input", "picture-fit/input.json", &requests)?;
    }
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "repairs": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "segment_index": {"type": "integer"},
                        "narration": {"type": "string"},
                        "evidence_ids": {"type": "array", "items": {"type": "string"}, "uniqueItems": true}
                    },
                    "required": ["segment_index", "narration", "evidence_ids"]
                }
            }
        },
        "required": ["repairs"]
    });
    let response = chat_json::<PictureFitResponse>(
        client,
        backend,
        &format!(
            "你是影视解说压缩编辑。只处理列出的超时段落；在不增加任何人物、动作、心理、因果或结局的前提下，把原旁白压缩到 max_chars 以内，保留最关键的动作与结果。每段必须且只能返回一次。evidence_ids 只能选择同段 evidence_candidates 的编号，至少一个。不要出现字幕里的说话人编号。只输出 JSON。\n{SPEAKER_RULE}"
        ),
        &serde_json::to_string(&requests).map_err(|error| error.to_string())?,
        schema,
        0.1,
        2_048,
        "picture_fit_repairs",
    )
    .await;
    let mut repairs = std::collections::HashMap::new();
    if let Ok(response) = response {
        if let Some(checkpoint) = checkpoint {
            checkpoint.write(
                "picture_fit_response",
                "picture-fit/response.json",
                &response,
            )?;
        }
        for repair in response.repairs {
            if repairs.contains_key(&repair.segment_index) {
                continue;
            }
            let Some(issue) = blocking
                .iter()
                .find(|issue| issue.index == repair.segment_index)
            else {
                continue;
            };
            let narration = clean_narration(&repair.narration);
            let max_chars = ((issue.available * 4.0) / 1.2).floor().max(8.0) as usize;
            let verdict = FactCheckVerdict {
                segment_index: repair.segment_index,
                supported: true,
                evidence_ids: repair.evidence_ids,
                corrected_narration: String::new(),
            };
            if narration.chars().count() >= 8
                && narration.chars().count() <= max_chars
                && fact_verdict_has_valid_evidence(&evidence[repair.segment_index], &verdict)
            {
                repairs.insert(repair.segment_index, narration);
            }
        }
    }
    let mut applied = Vec::new();
    for issue in blocking {
        let max_chars = ((issue.available * 4.0) / 1.2).floor().max(8.0) as usize;
        let narration = repairs.remove(&issue.index).unwrap_or_else(|| {
            shorten_narration_by_deletion(&plan.segments[issue.index].narration, max_chars)
        });
        plan.segments[issue.index].narration = narration;
        applied.push(json!({
            "segment_index": issue.index,
            "max_chars": max_chars,
            "final_narration": plan.segments[issue.index].narration
        }));
    }
    if let Some(checkpoint) = checkpoint {
        checkpoint.write("picture_fit_applied", "picture-fit/applied.json", &applied)?;
    }
    let remaining = super::edit_timing::reserve_issues(plan)
        .into_iter()
        .filter(|issue| issue.severity == super::edit_timing::ReserveSeverity::Blocking)
        .collect::<Vec<_>>();
    if remaining.is_empty() {
        Ok(())
    } else {
        Err(super::edit_timing::blocking_reserve_message(&remaining))
    }
}

fn contains_speaker_metadata(narration: &str) -> bool {
    regex::Regex::new(r"(?i)(?:说话人|说话者)\s*\d+|speaker[\s_：:\-]*\d+")
        .expect("static speaker metadata regex")
        .is_match(narration)
}

fn shorten_narration_by_deletion(narration: &str, max_chars: usize) -> String {
    if narration.chars().count() <= max_chars {
        return narration.trim().to_string();
    }
    let mut kept = String::new();
    for clause in
        narration.split_inclusive(|character| matches!(character, '，' | '。' | '；' | '！' | '？'))
    {
        if kept.chars().count() + clause.chars().count() <= max_chars {
            kept.push_str(clause);
        } else {
            break;
        }
    }
    if kept.chars().count() >= 8 {
        return kept.trim_end_matches('，').to_string();
    }
    let take = max_chars.saturating_sub(1).max(7);
    let mut fallback = narration.chars().take(take).collect::<String>();
    fallback = fallback
        .trim_end_matches(['，', '；', '：', '、', ' '])
        .to_string();
    fallback.push('。');
    fallback
}

/// Grow one paragraph's window until its pictures can carry the voice track at
/// native speed. Growth is bounded by the next paragraph's raw start, the previous
/// paragraph's end, excluded ranges and the media duration. Returns whatever
/// shortfall remains so callers can report it instead of freezing or slowing.
fn ensure_picture_reserve(
    segment: &mut EditSegment,
    excluded: &[ExcludedRange],
    duration: f64,
    previous_end: f64,
    next_start: Option<f64>,
) -> f64 {
    if segment.keep_original_audio {
        // Handoff paragraphs play the whole continuous window and are governed by
        // edit_timing::dialogue_handoff, not by the multishot reserve rule.
        return 0.0;
    }
    let required = super::edit_timing::reserve_seconds(segment);
    let mut missing = required - super::edit_timing::picture_seconds(segment);
    if missing <= 0.0 {
        return 0.0;
    }
    // Consume gaps between the explicitly selected shots before growing anything:
    // the renderer can use that time already.
    if !segment.shots.is_empty() {
        close_shot_gaps(segment);
        missing = required - super::edit_timing::picture_seconds(segment);
        if missing <= 0.0 {
            return 0.0;
        }
    }
    // Forward first: appending evidence keeps the paragraph's own chronology intact.
    let forward_limit = next_start
        .filter(|value| value.is_finite())
        .unwrap_or(duration)
        .min(duration)
        .max(segment.src_end);
    let forward = (free_forward(segment.src_end, forward_limit, excluded) - segment.src_end)
        .clamp(0.0, missing);
    segment.src_end += forward;
    missing -= forward;
    if missing > 0.0 {
        let backward_limit = previous_end.max(0.0).min(segment.src_start);
        let backward = (segment.src_start
            - free_backward(segment.src_start, backward_limit, excluded))
        .clamp(0.0, missing);
        segment.src_start -= backward;
    }
    // Pull the explicit shots out to the grown window so the reserve is real.
    close_shot_gaps(segment);
    (required - super::edit_timing::picture_seconds(segment)).max(0.0)
}

/// Absorb the window's unused time into the selected shots so they tile it exactly:
/// the first shot opens at `src_start`, the last closes at `src_end`, and interior
/// gaps join the preceding shot. Mirrors the extension `edit_timing::pictures`
/// performs at render time, so reserve computed here equals reserve available there.
/// Only called for paragraphs that are actually short, never eagerly.
fn close_shot_gaps(segment: &mut EditSegment) {
    if segment.shots.is_empty() {
        return;
    }
    segment.shots[0][0] = segment.src_start.min(segment.shots[0][0]);
    for index in 0..segment.shots.len() - 1 {
        let next_start = segment.shots[index + 1][0];
        if segment.shots[index][1] < next_start {
            segment.shots[index][1] = next_start;
        }
    }
    let last = segment.shots.len() - 1;
    segment.shots[last][1] = segment.shots[last][1].max(segment.src_end);
}

/// Furthest end reachable from `start` without entering an excluded range.
fn free_forward(start: f64, limit: f64, excluded: &[ExcludedRange]) -> f64 {
    let limit = limit.max(start);
    excluded
        .iter()
        .filter(|range| range.end > start)
        .fold(limit, |best, range| best.min(range.start.max(start)))
}

/// Earliest start reachable from `end` without entering an excluded range.
fn free_backward(end: f64, limit: f64, excluded: &[ExcludedRange]) -> f64 {
    let limit = limit.min(end);
    excluded
        .iter()
        .filter(|range| range.start < end)
        .fold(limit, |best, range| best.max(range.end.min(end)))
}

pub fn validate_existing_plan(
    plan: &EditPlan,
    duration: f64,
    entries: &[SubtitleEntry],
    excluded: &[ExcludedRange],
) -> Result<(), String> {
    let mut checked = plan.clone();
    checked
        .validate_and_clamp(duration)
        .map_err(|error| error.to_string())?;
    let content_entries = entries
        .iter()
        .filter(|entry| {
            let start = subtitle::time_to_secs(&entry.start);
            let end = subtitle::time_to_secs(&entry.end);
            !overlaps_any(start, end, excluded)
        })
        .cloned()
        .collect::<Vec<_>>();
    let (min_chars, _) = narration_char_range(&content_entries, duration);
    validate_market_ready(
        &checked,
        min_chars,
        usable_subtitle_duration(&content_entries, duration),
    )
}

fn narration_char_range(entries: &[SubtitleEntry], duration: f64) -> (usize, usize) {
    let evidence_seconds = usable_subtitle_duration(entries, duration);
    // Narration density follows the actual usable subtitle coverage, not the
    // media container duration. Long silent scenes, music and subtitle-free
    // gaps therefore do not create an artificial word-count debt.
    let min_chars = (evidence_seconds * 1.2).ceil().max(24.0) as usize;
    let max_chars = (evidence_seconds * 1.8).ceil().max(min_chars as f64) as usize;
    (min_chars, max_chars)
}

fn usable_subtitle_duration(entries: &[SubtitleEntry], duration: f64) -> f64 {
    let duration = duration.max(0.0);
    let mut intervals = entries
        .iter()
        .filter(|entry| !entry.text.trim().is_empty())
        .filter_map(|entry| {
            let start = subtitle::time_to_secs(&entry.start).clamp(0.0, duration);
            let end = subtitle::time_to_secs(&entry.end).clamp(0.0, duration);
            (end > start).then_some((start, end))
        })
        .collect::<Vec<_>>();
    intervals.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
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

fn build_beat_chapters(beats: &[StoryBeat]) -> Vec<Vec<StoryBeat>> {
    // Bound each request by actual beats, not by a desired word count.
    // A large subtitle-time gap is a useful boundary, not a claim that we
    // have detected a semantic scene transition.
    let mut chapters = Vec::new();
    let mut current: Vec<StoryBeat> = Vec::new();
    for beat in beats {
        let boundary = current.len() >= 12
            || current
                .last()
                .is_some_and(|last| beat.start - last.end >= 45.0);
        if boundary {
            chapters.push(std::mem::take(&mut current));
        }
        current.push(beat.clone());
    }
    if !current.is_empty() {
        chapters.push(current);
    }
    chapters
}

fn build_story_chapters(beats: &[StoryBeat], story_plan: &StoryPlan) -> Vec<StoryChapter> {
    let by_id = beats
        .iter()
        .map(|beat| (beat.id.as_str(), beat))
        .collect::<std::collections::HashMap<_, _>>();
    let mut chapters = Vec::<StoryChapter>::new();
    let mut current_units = Vec::<NarrativeUnit>::new();
    let mut current_beats = Vec::<StoryBeat>::new();
    let mut current_ids = std::collections::HashSet::<String>::new();
    for unit in &story_plan.units {
        let unit_beats = unit
            .beat_ids
            .iter()
            .filter_map(|id| by_id.get(id.as_str()).copied().cloned())
            .collect::<Vec<_>>();
        if unit_beats.is_empty() {
            continue;
        }
        let boundary = !current_units.is_empty()
            && (current_units.len() >= 4 || current_beats.len() + unit_beats.len() > 14);
        if boundary {
            current_beats.sort_by(|left, right| left.start.total_cmp(&right.start));
            chapters.push(StoryChapter {
                units: std::mem::take(&mut current_units),
                beats: std::mem::take(&mut current_beats),
            });
            current_ids.clear();
        }
        current_units.push(unit.clone());
        for beat in unit_beats {
            if current_ids.insert(beat.id.clone()) {
                current_beats.push(beat);
            }
        }
    }
    if !current_units.is_empty() {
        current_beats.sort_by(|left, right| left.start.total_cmp(&right.start));
        chapters.push(StoryChapter {
            units: current_units,
            beats: current_beats,
        });
    }
    if chapters.is_empty() {
        return build_beat_chapters(beats)
            .into_iter()
            .enumerate()
            .map(|(index, beats)| StoryChapter {
                units: vec![NarrativeUnit {
                    id: format!("N{:03}", index + 1),
                    title: "关键剧情".into(),
                    purpose: "保留关键因果".into(),
                    beat_ids: beats.iter().map(|beat| beat.id.clone()).collect(),
                    entry_state: String::new(),
                    turn: String::new(),
                    exit_state: String::new(),
                    narration_goal: "把相邻事件写成完整意群".into(),
                    audio_strategy: "narration".into(),
                    original_audio_beat_id: String::new(),
                    importance: 3,
                }],
                beats,
            })
            .collect();
    }
    chapters
}

fn narration_chars(plan: &EditPlan) -> usize {
    plan.segments
        .iter()
        .map(|segment| segment.narration.chars().count())
        .sum()
}

#[cfg(test)]
fn normalized_evidence(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

// Legacy diarization labels are unstable clustering metadata. Keep the
// original transcript untouched, but never expose those labels to an LLM as if
// they were character identities.
pub(crate) fn evidence_text(value: &str) -> &str {
    let value = value.trim();
    if let Some((label, text)) = value.split_once(['：', ':']) {
        let label = label.trim();
        if ["说话人", "说话者", "speaker", "SPEAKER"]
            .iter()
            .any(|prefix| {
                label.strip_prefix(prefix).is_some_and(|suffix| {
                    let suffix = suffix.trim().trim_start_matches('_');
                    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
                })
            })
        {
            return text.trim();
        }
    }
    value
}

#[cfg(test)]
pub(crate) fn evidence_quote_matches_segment(
    entries: &[SubtitleEntry],
    segment: &EditSegment,
    quote: &str,
) -> bool {
    let normalized_quote = normalized_evidence(quote.trim());
    if normalized_quote.chars().count() < 4 {
        return false;
    }
    let local_evidence = entries
        .iter()
        .filter(|entry| {
            let start = subtitle::time_to_secs(&entry.start);
            let end = subtitle::time_to_secs(&entry.end);
            if segment.shots.is_empty() {
                end >= segment.src_start - 12.0 && start <= segment.src_end + 12.0
            } else {
                segment.shots.iter().any(|r| end > r[0] && start < r[1])
            }
        })
        .map(|entry| evidence_text(&entry.text))
        .collect::<Vec<_>>()
        .join(" ");
    normalized_evidence(&local_evidence).contains(&normalized_quote)
}

fn fact_evidence_packet(entries: &[SubtitleEntry], plan: &EditPlan) -> Vec<SegmentFactEvidence> {
    plan.segments
        .iter()
        .enumerate()
        .map(|(segment_index, segment)| {
            let candidates = entries
                .iter()
                .filter(|entry| {
                    let start = subtitle::time_to_secs(&entry.start);
                    let end = subtitle::time_to_secs(&entry.end);
                    if segment.shots.is_empty() {
                        end >= segment.src_start - 12.0 && start <= segment.src_end + 12.0
                    } else {
                        segment.shots.iter().any(|r| end > r[0] && start < r[1])
                    }
                })
                .map(|entry| FactEvidenceCandidate {
                    id: format!("S{segment_index}-E{}", entry.index),
                    start: subtitle::time_to_secs(&entry.start),
                    end: subtitle::time_to_secs(&entry.end),
                    text: evidence_text(&entry.text).to_string(),
                })
                .collect();
            SegmentFactEvidence {
                segment_index,
                candidates,
            }
        })
        .collect()
}

fn fact_verdict_has_valid_evidence(
    evidence: &SegmentFactEvidence,
    verdict: &FactCheckVerdict,
) -> bool {
    let valid = evidence
        .candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let selected = verdict
        .evidence_ids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let needs_evidence = verdict.supported
        || clean_narration(&verdict.corrected_narration)
            .chars()
            .count()
            >= 8;
    (!needs_evidence || !selected.is_empty())
        && selected.len() == verdict.evidence_ids.len()
        && selected.iter().all(|id| valid.contains(id))
}

fn apply_fact_check(
    plan: &mut EditPlan,
    fact_check: FactCheckResponse,
    evidence: &[SegmentFactEvidence],
) {
    let mut corrections = (0..plan.segments.len()).map(|_| None).collect::<Vec<_>>();
    let mut counts = vec![0; plan.segments.len()];
    for verdict in fact_check.verdicts {
        let index = verdict.segment_index;
        if index < corrections.len() {
            counts[index] += 1;
            corrections[index] = Some(verdict);
        }
    }
    plan.segments = std::mem::take(&mut plan.segments)
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut segment)| {
            if counts[index] != 1 {
                return Some(segment);
            }
            let Some(verdict) = corrections[index].take() else {
                return Some(segment);
            };
            if !evidence
                .get(index)
                .is_some_and(|local| fact_verdict_has_valid_evidence(local, &verdict))
            {
                return Some(segment);
            }
            if verdict.supported {
                return Some(segment);
            }
            let correction = clean_narration(&verdict.corrected_narration);
            if correction.chars().count() >= 8 {
                segment.narration = correction;
                Some(segment)
            } else {
                None
            }
        })
        .collect();
}

fn validate_market_ready(
    plan: &EditPlan,
    min_chars: usize,
    usable_subtitle_seconds: f64,
) -> Result<(), String> {
    let Some(first) = plan.segments.first() else {
        return Err("AI 导演稿没有可用片段".into());
    };
    let opening = first.narration.trim();
    let forbidden_openings = [
        "大家好",
        "开场",
        "镜头",
        "故事开始",
        "这部剧",
        "这部电影",
        "让我们",
        "有人",
    ];
    if forbidden_openings
        .iter()
        .any(|phrase| opening.starts_with(phrase) || opening.contains(&format!("{phrase}先")))
    {
        return Err(format!(
            "AI 终审后仍使用低质量开场“{}”，请点击重新生成",
            opening.chars().take(24).collect::<String>()
        ));
    }
    if let Some((index, segment)) = plan
        .segments
        .iter()
        .enumerate()
        .find(|(_, segment)| contains_speaker_metadata(&segment.narration))
    {
        return Err(format!(
            "AI 终审后第 {} 段仍含来源字幕中的自动说话人编号“{}”，不能作为成片人物称呼，请重新生成 AI 导演稿",
            index + 1,
            segment.narration.chars().take(24).collect::<String>()
        ));
    }
    let low_information_phrases = [
        "更具体的转折",
        "反应并不一致",
        "在前行中前行",
        "在承受中前行",
        "命运的齿轮",
    ];
    for (index, segment) in plan.segments.iter().enumerate() {
        if low_information_phrases
            .iter()
            .any(|phrase| segment.narration.contains(phrase))
        {
            return Err(format!(
                "AI 终审后第 {} 段仍含无有效剧情信息的套话，请重新生成 AI 导演稿",
                index + 1
            ));
        }
        let mut sentences = std::collections::HashSet::new();
        for sentence in segment
            .narration
            .split(['。', '！', '？', '；'])
            .map(str::trim)
            .filter(|sentence| sentence.chars().count() >= 8)
        {
            if !sentences.insert(sentence) {
                return Err(format!(
                    "AI 终审后第 {} 段重复同一句内容，请重新生成 AI 导演稿",
                    index + 1
                ));
            }
        }
    }
    let narration_chars = narration_chars(plan);
    if narration_chars < min_chars {
        return Err(format!(
            "AI 终审后解说不足有效字幕覆盖时长的 30%：当前约 {:.1} 秒，至少需要 {:.1} 秒（{min_chars} 字），请点击重新生成",
            narration_chars as f64 / 4.0,
            usable_subtitle_seconds * 0.3,
        ));
    }
    Ok(())
}

fn collapse_repeated_narration(plan: &mut EditPlan) {
    for segment in &mut plan.segments {
        segment.narration = collapse_repeated_sentences(&segment.narration);
    }
    plan.segments
        .retain(|segment| segment.narration.chars().count() >= 8);
}

fn collapse_repeated_sentences(narration: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut output = String::new();
    let mut sentence = String::new();
    for character in narration.chars() {
        sentence.push(character);
        if matches!(character, '。' | '！' | '？' | '；') {
            push_unique_sentence(&mut output, &mut seen, &sentence);
            sentence.clear();
        }
    }
    if !sentence.trim().is_empty() {
        push_unique_sentence(&mut output, &mut seen, &sentence);
    }
    output
}

fn push_unique_sentence(
    output: &mut String,
    seen: &mut std::collections::HashSet<String>,
    sentence: &str,
) {
    let trimmed = sentence.trim();
    if trimmed.is_empty() {
        return;
    }
    let key = trimmed
        .trim_end_matches(['。', '！', '？', '；'])
        .trim()
        .to_string();
    if key.chars().count() >= 8 && !seen.insert(key) {
        return;
    }
    output.push_str(trimmed);
}

fn clean_narration(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_bullet = trimmed
        .trim_start_matches(|character: char| {
            character.is_ascii_digit()
                || matches!(character, '.' | '、' | '-' | '—' | '*' | '#' | ' ')
        })
        .trim();
    without_bullet.replace('\n', " ")
}

fn snap_segment(segment: &mut EditSegment, entries: &[SubtitleEntry]) {
    if let Some(entry) = entries.iter().min_by(|a, b| {
        let a_delta = (subtitle::time_to_secs(&a.start) - segment.src_start).abs();
        let b_delta = (subtitle::time_to_secs(&b.start) - segment.src_start).abs();
        a_delta.partial_cmp(&b_delta).unwrap_or(Ordering::Equal)
    }) {
        let candidate = subtitle::time_to_secs(&entry.start);
        if (candidate - segment.src_start).abs() <= 6.0 {
            segment.src_start = candidate;
        }
    }
    if let Some(entry) = entries.iter().min_by(|a, b| {
        let a_delta = (subtitle::time_to_secs(&a.end) - segment.src_end).abs();
        let b_delta = (subtitle::time_to_secs(&b.end) - segment.src_end).abs();
        a_delta.partial_cmp(&b_delta).unwrap_or(Ordering::Equal)
    }) {
        let candidate = subtitle::time_to_secs(&entry.end);
        if (candidate - segment.src_end).abs() <= 6.0 {
            segment.src_end = candidate;
        }
    }
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
            .filter(|entry| {
                subtitle::time_to_secs(&entry.start) <= 180.0 && is_intro_marker(&entry.text)
            })
            .collect();
        let intro_end = intro_markers
            .iter()
            .map(|entry| subtitle::time_to_secs(&entry.end) + 4.0)
            .chain(entries.windows(2).filter_map(|pair| {
                let left = pair[0].text.trim().to_lowercase();
                let right = pair[1].text.trim().to_lowercase();
                let end = subtitle::time_to_secs(&pair[1].end);
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
                subtitle::time_to_secs(&entry.start) >= outro_search_start
                    && is_outro_marker(&entry.text)
            })
            .map(|entry| subtitle::time_to_secs(&entry.start))
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        ranges.push(ExcludedRange {
            start: marker_start.unwrap_or((duration - 90.0).max(0.0)),
            end: duration,
            kind: "片尾".into(),
        });
    }

    if skip_ads {
        for entry in entries.iter().filter(|entry| is_ad_marker(&entry.text)) {
            ranges.push(ExcludedRange {
                start: (subtitle::time_to_secs(&entry.start) - 3.0).max(0.0),
                end: (subtitle::time_to_secs(&entry.end) + 5.0).min(duration),
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
    ranges.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(Ordering::Equal));
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

fn overlaps_any(start: f64, end: f64, ranges: &[ExcludedRange]) -> bool {
    ranges
        .iter()
        .any(|range| end > range.start && start < range.end)
}

#[cfg(test)]
mod tests {
    #[test]
    fn adjacent_quotes_ignore_only_legacy_speaker_metadata() {
        let segment: EditSegment = serde_json::from_str(r#"{"src_start":1211.7,"src_end":1218.3,"shots":[[1211.7,1218.3]],"narration":"面试者自我介绍"}"#).unwrap();
        let entries = vec![
            entry(1, 1213.3, 1214.68, "说话人28：面接の人。"),
            entry(
                2,
                1214.99,
                1218.25,
                "说话人45：はじめまして午前中にお電話した箱林です。",
            ),
        ];
        assert!(evidence_quote_matches_segment(
            &entries,
            &segment,
            "面接の人。はじめまして午前中にお電話した箱林です。"
        ));
        assert!(!evidence_quote_matches_segment(
            &entries,
            &segment,
            "面接の人。小林进入工作群。"
        ));
        assert_eq!(evidence_text("他说：说话人87来了"), "他说：说话人87来了");
    }
    use super::*;

    #[test]
    fn ollama_director_disables_thinking_and_keeps_schema_and_budget() {
        let schema = json!({"type": "object"});
        let body = ollama_chat_body("qwen3.8:27b-mlx", "system", "user", &schema, 0.2, 8192);
        assert_eq!(body["model"], "qwen3.8:27b-mlx");
        assert_eq!(body["think"], false);
        assert_eq!(body["format"], schema);
        assert_eq!(body["options"]["num_predict"], 8192);
    }

    #[test]
    fn thinking_only_response_is_not_treated_as_a_director_script() {
        let response = serde_json::from_value::<ChatResponse>(json!({
            "message": {"content": "", "thinking": "private reasoning"},
            "done_reason": "length"
        }))
        .unwrap();
        let error = ollama_response_content(response).unwrap_err();
        assert!(error.contains("只返回了思考内容"));
        assert!(error.contains("length"));
        assert!(!error.contains("private reasoning"));
    }

    #[test]
    fn normal_ollama_response_does_not_require_thinking_field() {
        let response = serde_json::from_value::<ChatResponse>(json!({
            "message": {"content": "{\"ok\":true}"}, "done_reason": "stop"
        }))
        .unwrap();
        assert_eq!(
            ollama_response_content(response).unwrap().0,
            "{\"ok\":true}"
        );
    }

    #[test]
    fn local_generation_has_its_own_bounded_timeout() {
        assert_eq!(director_request_timeout("ollama").as_secs(), 3600);
    }

    #[tokio::test]
    async fn local_timeout_is_reported_as_timeout_not_connection_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let client = director_client("ollama", Duration::from_millis(100)).unwrap();
        let error = client
            .post(format!("http://{address}/api/chat"))
            .send()
            .await
            .unwrap_err();
        server.abort();
        assert!(error.is_timeout(), "{error:?}");
        let message = ollama_request_error(&error, OLLAMA_GENERATION_TIMEOUT);
        assert!(message.contains("超时"));
        assert!(message.contains("3600 秒"));
        assert!(!message.contains("无法连接"));
    }

    #[tokio::test]
    async fn local_client_waits_for_a_delayed_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            socket.read(&mut request).await.unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        });
        let client = director_client("ollama", director_request_timeout("ollama")).unwrap();
        let response = client
            .post(format!("http://{address}/api/chat"))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(response.text().await.unwrap(), "{}");
        server.await.unwrap();
    }

    fn entry(index: u32, start: f64, end: f64, text: &str) -> SubtitleEntry {
        SubtitleEntry {
            index,
            start: subtitle::secs_to_time(start),
            end: subtitle::secs_to_time(end),
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
        let ranges = detect_excluded_ranges(&entries, 120.0, true, true);
        assert!(ranges.is_empty());
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

    #[test]
    fn utf8_truncation_stays_on_character_boundary() {
        let mut value = "剧情转折".repeat(10_000);
        truncate_utf8(&mut value, 42_000);
        assert!(value.len() <= 42_000);
        assert!(value.is_char_boundary(value.len()));
    }

    #[test]
    fn structured_json_parser_accepts_fenced_or_explained_object() {
        let response: BeatResponse =
            parse_structured_json("下面是结果：```json\n{\"beats\":[]}\n```不要再补充文字")
                .unwrap();
        assert!(response.beats.is_empty());
    }

    #[test]
    fn structured_json_parser_rejects_number_in_place_of_array() {
        let error = parse_structured_json::<BeatResponse>("{\"beats\":63.4}").unwrap_err();
        assert!(error.contains("expected a sequence"));
    }

    #[test]
    fn continuity_rules_keep_narration_as_story_spine() {
        let entries = (0..8)
            .map(|index| {
                let start = index as f64 * 10.0;
                entry(index + 1, start, start + 8.0, "人物做出关键选择")
            })
            .collect::<Vec<_>>();
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 1.0,
            segments: (0..5)
                .map(|index| EditSegment {
                    shots: vec![],
                    src_start: index as f64 * 10.0,
                    src_end: index as f64 * 10.0 + 8.0,
                    narration: "人物因为现实压力被迫作出关键选择，可这个决定马上伤害了最信任他的人，也让原本能解决的问题变得更加危险。".into(),
                    keep_original_audio: true,
                })
                .collect(),
        };

        let beats = vec![StoryBeat {
            id: "B0001".into(),
            start: 0.0,
            end: 50.0,
            summary: "人物做出关键选择".into(),
            importance: 5,
            quote: "关键台词".into(),
            kind: "转折".into(),
            narrative_layer: "main".into(),
            evidence: vec![],
        }];
        finalize_plan(
            &mut plan,
            &entries,
            &beats,
            &[],
            "测试",
            "剧情解说",
            None,
            None,
            80.0,
        )
        .unwrap();
        assert!(!plan.segments[0].keep_original_audio);
        assert!(plan
            .segments
            .windows(3)
            .all(|window| !window.iter().all(|segment| segment.keep_original_audio)));
    }

    #[test]
    fn picture_reserve_grows_short_windows_into_free_time() {
        // Regression from job-1788396413406-sehcf1 segment 2: a 3.80s window whose
        // single shot already fills it, 16 chars of narration (4.0s spoken, 4.8s
        // reserve). Forward growth is capped by the next paragraph starting 0.1s
        // later, so the rest has to come from the free time before the paragraph.
        let entries = vec![
            entry(1, 124.0, 136.0, "上一段对白"),
            entry(2, 165.8, 169.6, "现场有人指出逝者看起来还像活着。"),
            entry(3, 169.7, 179.0, "有人判断逝者可能是自杀"),
        ];
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 1.0,
            segments: vec![
                EditSegment {
                    shots: vec![],
                    src_start: 124.0,
                    src_end: 137.2,
                    narration: "主角在前一段做了充分的选择，这里的叙述只需要占住时间位置，不参与本段判定的任何结果。".into(),
                    keep_original_audio: false,
                },
                EditSegment {
                    shots: vec![[165.8, 169.6]],
                    src_start: 165.8,
                    src_end: 169.6,
                    narration: "现场有人指出逝者看起来还像活着。".into(),
                    keep_original_audio: false,
                },
                EditSegment {
                    shots: vec![],
                    src_start: 169.7,
                    src_end: 179.6,
                    narration: "有人判断逝者可能是自杀，并解释在寒冷车内死亡且发现及时会出现这种状态。".into(),
                    keep_original_audio: false,
                },
            ],
        };
        finalize_plan(
            &mut plan,
            &entries,
            &[],
            &[],
            "测试",
            "剧情解说",
            None,
            None,
            200.0,
        )
        .unwrap();
        let second = &plan.segments[1];
        let available = second
            .shots
            .iter()
            .map(|range| range[1] - range[0])
            .sum::<f64>();
        assert!(
            available >= 4.8 - 1e-6,
            "available {available:.2}s must cover the 4.8s reserve"
        );
        assert_eq!(second.shots.len(), 1);
        assert_eq!(second.shots[0][0], second.src_start);
        assert_eq!(second.shots[0][1], second.src_end);
        assert!(
            second.src_end <= 169.7 + 1e-6,
            "must not steal the next paragraph's evidence"
        );
        // The whole plan is renderable now, not just the repaired paragraph.
        assert!(super::super::edit_timing::reserve_issues(&plan).is_empty());
    }

    #[test]
    fn chapter_tail_growth_stops_at_the_next_chapter() {
        let entries = vec![entry(1, 10.0, 14.0, "人物做出选择")];
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 1.0,
            segments: vec![EditSegment {
                shots: vec![[10.0, 14.0]],
                src_start: 10.0,
                src_end: 14.0,
                narration: "人物在压力下做出选择，这个决定改变了他与家人的关系，也把下一步行动推到了无法回避的位置。".into(),
                keep_original_audio: false,
            }],
        };
        finalize_plan(
            &mut plan,
            &entries,
            &[],
            &[],
            "测试",
            "剧情解说",
            None,
            Some(18.0),
            120.0,
        )
        .unwrap();
        assert!(plan.segments[0].src_end <= 18.0);
    }

    #[test]
    fn boxed_in_reserve_passes_through_for_reporting() {
        // Two adjacent paragraphs pinned to the media bounds: no free time exists,
        // so both stay short and must surface as blocking issues instead of being
        // silently frozen or slowed at render time.
        let entries = vec![entry(1, 10.0, 12.0, "远处的对白")];
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 1.0,
            segments: vec![
                EditSegment {
                    shots: vec![],
                    src_start: 0.0,
                    src_end: 1.0,
                    narration: "第一段有十个字符以上的叙述内容，画面被完全卡住没有空间。".into(),
                    keep_original_audio: false,
                },
                EditSegment {
                    shots: vec![],
                    src_start: 1.0,
                    src_end: 2.0,
                    narration: "第二段同样有十个字符以上的叙述内容，画面也被完全卡住了。".into(),
                    keep_original_audio: false,
                },
            ],
        };
        finalize_plan(
            &mut plan,
            &entries,
            &[],
            &[],
            "测试",
            "剧情解说",
            None,
            None,
            2.0,
        )
        .unwrap();
        let issues = super::super::edit_timing::reserve_issues(&plan);
        assert_eq!(issues.len(), 2);
        assert!(issues
            .iter()
            .all(|issue| issue.severity == super::super::edit_timing::ReserveSeverity::Blocking));
    }

    #[test]
    fn market_validator_rejects_meta_openings() {
        let plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 20.0,
            segments: vec![EditSegment {
                src_start: 1.0,
                src_end: 8.0,
                narration: "开场先把故事背景交代一下，接下来局面发生变化。".into(),
                shots: vec![],
                keep_original_audio: false,
            }],
        };
        assert!(validate_market_ready(&plan, 96, 80.0).is_err());
    }

    #[test]
    fn market_validator_rejects_repeated_and_empty_ai_prose() {
        let make_plan = |narration: &str| EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 20.0,
            segments: vec![EditSegment {
                src_start: 1.0,
                src_end: 18.0,
                narration: narration.into(),
                shots: vec![],
                keep_original_audio: false,
            }],
        };
        assert!(validate_market_ready(
            &make_plan("大悟在偏见中坚持。大悟在偏见中坚持。"),
            10,
            20.0
        )
        .is_err());
        let collapsed = collapse_repeated_sentences(
            "他最终选择前往，标志着他从被动接受到主动参与的转变。他最终选择前往，标志着他从被动接受到主动参与的转变。后面还有新的决定。",
        );
        assert_eq!(collapsed.matches("他最终选择前往").count(), 1);
        assert!(collapsed.contains("后面还有新的决定"));
        assert!(validate_market_ready(
            &make_plan("周围人的对话里，却冒出一个更具体的转折。"),
            10,
            20.0
        )
        .is_err());
    }

    #[test]
    fn audience_narration_rejects_legacy_speaker_metadata() {
        assert!(contains_speaker_metadata("说话人87纠正了他的理解。"));
        assert!(contains_speaker_metadata("speaker_12 asks him to leave"));
        assert!(!contains_speaker_metadata("社长纠正了小林的理解。"));
        let plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 10.0,
            segments: vec![EditSegment {
                src_start: 1.0,
                src_end: 12.0,
                narration: "小林以为这是一份旅行工作，说话人87却告诉他，这份工作真正面对的是死亡。"
                    .into(),
                shots: vec![],
                keep_original_audio: false,
            }],
        };
        let error = validate_market_ready(&plan, 1, 1.0).unwrap_err();
        assert!(error.contains("说话人编号"));
    }

    #[test]
    fn dialogue_attribution_must_quote_the_same_beat_evidence() {
        let beat = StoryBeat {
            id: "B0001".into(),
            start: 10.0,
            end: 18.0,
            summary: "妻子追问丈夫的新工作".into(),
            importance: 4,
            quote: "你到底在做什么工作".into(),
            kind: String::new(),
            narrative_layer: "main".into(),
            evidence: vec![
                "[10.0s-12.0s] 你到底在做什么工作？".into(),
                "[12.1s-14.0s] 我只是暂时帮忙。".into(),
            ],
        };
        assert!(attribution_quote_matches(&beat, "你到底在做什么工作？"));
        assert!(!attribution_quote_matches(&beat, "她已经知道全部真相"));
    }

    #[test]
    fn short_videos_get_proportional_narration_targets() {
        let short = vec![entry(1, 0.0, 20.0, "有效对白")];
        assert_eq!(narration_char_range(&short, 120.0), (24, 36));
        let episode = vec![entry(1, 0.0, 2_609.0, "持续有效对白")];
        let (episode_min, episode_max) = narration_char_range(&episode, 7_862.0);
        assert_eq!(episode_min, 3_131);
        assert_eq!(episode_max, 4_697);
    }

    #[test]
    fn narration_target_uses_subtitle_coverage_instead_of_container_duration() {
        let entries = vec![
            entry(1, 10.0, 20.0, "第一段对白"),
            entry(2, 15.0, 25.0, "与第一段重叠的对白"),
            entry(3, 1_000.0, 1_010.0, "第二段对白"),
        ];
        assert_eq!(usable_subtitle_duration(&entries, 7_862.0), 25.0);
        assert_eq!(narration_char_range(&entries, 7_862.0), (30, 45));
    }

    #[test]
    fn chapters_follow_beats_without_losing_order_or_content() {
        let beats = (0..37)
            .map(|index| StoryBeat {
                id: format!("B{:04}", index + 1),
                start: index as f64 * 10.0,
                end: index as f64 * 10.0 + 8.0,
                summary: format!("剧情节拍 {index}"),
                importance: 3,
                quote: String::new(),
                kind: "推进".into(),
                narrative_layer: "main".into(),
                evidence: vec![],
            })
            .collect::<Vec<_>>();
        let chapters = build_beat_chapters(&beats);
        assert_eq!(
            chapters.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![12, 12, 12, 1]
        );
        let starts: Vec<_> = chapters.iter().flatten().map(|beat| beat.start).collect();
        assert_eq!(
            starts,
            beats.iter().map(|beat| beat.start).collect::<Vec<_>>()
        );
        let mut with_gap = beats[..3].to_vec();
        with_gap[2].start = 100.0;
        with_gap[2].end = 108.0;
        assert_eq!(
            build_beat_chapters(&with_gap)
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert!(build_beat_chapters(&[]).is_empty());
    }

    #[test]
    fn map_chunks_keep_scene_context_but_hide_unstable_speaker_ids() {
        let entries = vec![
            entry(1, 0.0, 2.0, "说话人87：第一场前文需要保留"),
            entry(2, 70.0, 72.0, "说话人12：第一场核心内容足够长以触发分块"),
            entry(3, 100.0, 102.0, "说话人99：第二场核心内容"),
        ];
        let chunks = transcript_chunks(&entries, 80);
        assert!(chunks.len() >= 2);
        assert!(chunks[1].text.contains("[CONTEXT"));
        assert!(chunks[1].text.contains("第一场"));
        assert!(chunks[1].text.contains("第二场核心内容"));
        assert!(!chunks[1].text.contains("说话人"));
        assert!(!chunks[1].text.contains("speaker_"));
    }

    #[test]
    fn story_planner_consolidates_fragments_and_keeps_essential_turns() {
        assert_eq!(story_unit_count_hint(7_862.0, 60), (37, 55));
        let mut beats = (0..8)
            .map(|index| StoryBeat {
                id: String::new(),
                start: index as f64 * 10.0,
                end: index as f64 * 10.0 + 8.0,
                summary: format!("局部事件 {index}"),
                importance: if index == 7 { 5 } else { 2 },
                quote: if index == 1 {
                    "值得保留的完整台词".into()
                } else {
                    String::new()
                },
                kind: "推进".into(),
                narrative_layer: "main".into(),
                evidence: vec![],
            })
            .collect::<Vec<_>>();
        normalize_beats(&mut beats, 100.0, &[]);
        let unit = |id: &str, beat_id: &str, audio: &str| NarrativeUnit {
            id: id.into(),
            title: format!("单元 {id}"),
            purpose: "推进剧情".into(),
            beat_ids: vec![beat_id.into()],
            entry_state: "之前".into(),
            turn: "发生变化".into(),
            exit_state: "之后".into(),
            narration_goal: "讲清选择与后果".into(),
            audio_strategy: audio.into(),
            original_audio_beat_id: beat_id.into(),
            importance: 2,
        };
        let mut plan = StoryPlan {
            opening_unit_id: "wrong".into(),
            ending_unit_id: "wrong".into(),
            units: vec![
                unit("raw-1", "B0001", "original_dialogue"),
                unit("raw-2", "B0002", "original_dialogue"),
                unit("raw-3", "INVALID", "narration"),
            ],
            omitted_beat_ids: vec![],
        };
        normalize_story_plan(&mut plan, &beats, 1, 1);
        assert_eq!(plan.units.len(), 1);
        assert_eq!(plan.opening_unit_id, "N001");
        assert_eq!(plan.ending_unit_id, "N001");
        assert!(plan.units[0].beat_ids.contains(&"B0008".to_string()));
        assert!(!plan.units[0].beat_ids.contains(&"INVALID".to_string()));
        assert_eq!(plan.units[0].audio_strategy, "original_dialogue");
        assert_eq!(plan.units[0].original_audio_beat_id, "B0002");
        let chapters = build_story_chapters(&beats, &plan);
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].units.len(), 1);
        assert_eq!(chapters[0].beats.len(), 3);
    }

    #[test]
    fn interleaved_story_units_are_split_into_monotonic_runs() {
        let mut beats = (0..4)
            .map(|index| StoryBeat {
                id: String::new(),
                start: index as f64 * 10.0,
                end: index as f64 * 10.0 + 8.0,
                summary: format!("事件 {index}"),
                importance: 3,
                quote: String::new(),
                kind: "推进".into(),
                narrative_layer: "main".into(),
                evidence: vec![],
            })
            .collect::<Vec<_>>();
        normalize_beats(&mut beats, 60.0, &[]);
        let make_unit = |title: &str, ids: &[&str]| NarrativeUnit {
            id: title.into(),
            title: title.into(),
            purpose: "推进".into(),
            beat_ids: ids.iter().map(|id| (*id).to_string()).collect(),
            entry_state: String::new(),
            turn: String::new(),
            exit_state: String::new(),
            narration_goal: "完整讲述".into(),
            audio_strategy: "narration".into(),
            original_audio_beat_id: String::new(),
            importance: 3,
        };
        let mut plan = StoryPlan {
            opening_unit_id: String::new(),
            ending_unit_id: String::new(),
            units: vec![
                make_unit("A", &["B0001", "B0003"]),
                make_unit("B", &["B0002", "B0004"]),
            ],
            omitted_beat_ids: vec![],
        };
        normalize_story_plan(&mut plan, &beats, 1, 10);
        assert_eq!(plan.units.len(), 4);
        assert_eq!(
            plan.units
                .iter()
                .map(|unit| unit.beat_ids[0].clone())
                .collect::<Vec<_>>(),
            vec!["B0001", "B0002", "B0003", "B0004"]
        );
    }

    #[test]
    fn story_unit_budgets_deliver_the_whole_writer_target() {
        let units = (0..5)
            .map(|index| NarrativeUnit {
                id: format!("N{:03}", index + 1),
                title: format!("单元 {index}"),
                purpose: "推进".into(),
                beat_ids: vec![format!("B{:04}", index + 1)],
                entry_state: String::new(),
                turn: String::new(),
                exit_state: String::new(),
                narration_goal: "完整讲述".into(),
                audio_strategy: "narration".into(),
                original_audio_beat_id: String::new(),
                importance: (index + 1) as u8,
            })
            .collect::<Vec<_>>();
        let budgets = story_unit_budgets(&units, 500);
        assert_eq!(budgets.values().sum::<usize>(), 500);
        assert!(budgets["N005"] > budgets["N001"]);
        let schema = chapter_script_schema(&units, &budgets);
        assert_eq!(
            schema["properties"]["lines"]["properties"]["N005"]["minLength"],
            budgets["N005"]
        );
    }

    #[test]
    fn feature_budget_with_original_audio_units_always_terminates() {
        let units = (0..12)
            .map(|index| NarrativeUnit {
                id: format!("N{:03}", index + 1),
                title: format!("单元 {index}"),
                purpose: "推进".into(),
                beat_ids: vec![format!("B{:04}", index + 1)],
                entry_state: String::new(),
                turn: String::new(),
                exit_state: String::new(),
                narration_goal: "完整讲述".into(),
                audio_strategy: if index >= 10 {
                    "original_dialogue".into()
                } else {
                    "narration".into()
                },
                original_audio_beat_id: if index >= 10 {
                    format!("B{:04}", index + 1)
                } else {
                    String::new()
                },
                importance: 4,
            })
            .collect::<Vec<_>>();
        let budgets = story_unit_budgets(&units, 3_909);
        assert_eq!(budgets.values().sum::<usize>(), 3_909);
        assert_eq!(budgets["N011"], 18);
        assert_eq!(budgets["N012"], 18);
    }

    #[test]
    fn undersized_macro_plan_is_split_to_the_requested_floor() {
        let mut beats = (0..30)
            .map(|index| StoryBeat {
                id: String::new(),
                start: index as f64 * 10.0,
                end: index as f64 * 10.0 + 8.0,
                summary: format!("事件 {index}"),
                importance: 4,
                quote: String::new(),
                kind: "推进".into(),
                narrative_layer: "main".into(),
                evidence: vec![],
            })
            .collect::<Vec<_>>();
        normalize_beats(&mut beats, 400.0, &[]);
        let mut plan = StoryPlan {
            opening_unit_id: String::new(),
            ending_unit_id: String::new(),
            units: (0..10)
                .map(|index| NarrativeUnit {
                    id: format!("raw-{index}"),
                    title: format!("宏观单元 {index}"),
                    purpose: "推进".into(),
                    beat_ids: (0..3)
                        .map(|offset| format!("B{:04}", index * 3 + offset + 1))
                        .collect(),
                    entry_state: String::new(),
                    turn: String::new(),
                    exit_state: String::new(),
                    narration_goal: "完整讲述".into(),
                    audio_strategy: "narration".into(),
                    original_audio_beat_id: String::new(),
                    importance: 4,
                })
                .collect(),
            omitted_beat_ids: vec![],
        };
        normalize_story_plan(&mut plan, &beats, 27, 41);
        assert_eq!(plan.units.len(), 27);
        assert!(plan.units.iter().all(|unit| !unit.beat_ids.is_empty()));
        assert_ne!(plan.units[0].narration_goal, plan.units[1].narration_goal);
        assert!(plan.units[0].narration_goal.contains("事件 0"));
        assert!(!plan.units[0].narration_goal.contains("事件 2"));
        assert_eq!(
            plan.units
                .iter()
                .flat_map(|unit| unit.beat_ids.iter())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            30
        );
    }

    #[test]
    fn uncertain_character_claims_are_downgraded_and_speaker_aliases_removed() {
        let beats = vec![StoryBeat {
            id: "B0001".into(),
            start: 1.0,
            end: 5.0,
            summary: "有人谈起过去".into(),
            importance: 4,
            quote: String::new(),
            kind: "关系揭示".into(),
            narrative_layer: "main".into(),
            evidence: vec!["[1.0s-5.0s] 我当年离开了孩子".into()],
        }];
        let mut model = NarrativeBlueprint {
            protagonist: "主角".into(),
            desire: String::new(),
            core_conflict: String::new(),
            stakes: String::new(),
            hook: String::new(),
            central_question: String::new(),
            interpretive_thesis: String::new(),
            character_arc: vec![],
            relationship_arc: vec![],
            recurring_evidence: vec![],
            causal_chain: vec![],
            payoff: String::new(),
            ending_echo: String::new(),
            character_bible: vec![CharacterCard {
                id: "C001".into(),
                canonical_name: "身份未确认的人物".into(),
                aliases: vec!["说话人87".into(), "speaker_12".into(), "来访者".into()],
                role: "相关人物".into(),
                facts: vec![StoryClaim {
                    claim: "此人是主角母亲".into(),
                    beat_ids: vec!["B0001".into()],
                    evidence_quotes: vec!["我当年离开了孩子".into()],
                    confidence: 0.98,
                    uncertainty: "字幕省略主语，无法确认亲属身份".into(),
                }],
            }],
            relationships: vec![],
            story_threads: vec![],
        };
        normalize_story_model(&mut model, &beats);
        assert_eq!(model.character_bible[0].aliases, vec!["来访者"]);
        assert_eq!(model.character_bible[0].facts[0].confidence, 0.65);
        let trusted = trusted_story_model(&model);
        assert!(trusted.character_bible.is_empty());
        assert_eq!(trusted.protagonist, "主角");
    }

    #[test]
    fn story_model_beats_carry_traceable_original_subtitle_evidence() {
        let mut beats = vec![StoryBeat {
            id: "B0001".into(),
            start: 20.0,
            end: 28.0,
            summary: "局部模型可能写错人物关系".into(),
            importance: 5,
            quote: String::new(),
            kind: "关系揭示".into(),
            narrative_layer: "main".into(),
            evidence: vec!["旧缓存".into()],
        }];
        let entries = vec![
            entry(1, 18.5, 20.5, "父亲在他六岁时离开了家。"),
            entry(2, 40.0, 42.0, "与这个节拍无关的字幕。"),
        ];
        attach_beat_evidence(&mut beats, &entries);
        assert_eq!(beats[0].evidence.len(), 1);
        assert!(beats[0].evidence[0].contains("父亲在他六岁时离开了家"));
        assert!(!beats[0].evidence[0].contains("旧缓存"));
    }

    #[test]
    fn chapter_prompt_separates_script_writing_from_timeline_selection() {
        let prompt = chapter_system_prompt("剧情解说", true);
        assert!(prompt.contains("只负责写旁白，不负责选镜头或时间码"));
        assert!(prompt.contains("一个 Narrative Unit 写成一个完整叙事段"));
        assert!(prompt.contains("minLength 是制作预算"));
        assert!(prompt.contains("不要输出镜头、时间码"));
        assert!(prompt.contains("confidence≥0.8"));
        assert!(prompt.contains("不得把后续说话者自动认成该角色"));
        assert!(!prompt.contains("target_duration_secs 可填任意正数"));
    }

    #[test]
    fn grounded_chapter_accepts_361_characters_without_a_368_gate() {
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 1.0,
            segments: vec![EditSegment {
                src_start: 1.0,
                src_end: 8.0,
                narration: "钥".repeat(361),
                shots: vec![],
                keep_original_audio: false,
            }],
        };
        let verdict = FactCheckResponse {
            verdicts: vec![FactCheckVerdict {
                segment_index: 0,
                supported: true,
                evidence_ids: vec!["S0-E1".into()],
                corrected_narration: String::new(),
            }],
        };
        let entries = vec![entry(1, 1.0, 8.0, "钥匙已经丢失")];
        let evidence = fact_evidence_packet(&entries, &plan);
        apply_fact_check(&mut plan, verdict, &evidence);
        assert_eq!(narration_chars(&plan), 361);
        assert!(plan.validate_and_clamp(20.0).is_ok());
    }

    #[test]
    fn model_shots_are_clipped_sorted_and_merged_before_validation() {
        let mut segment = EditSegment {
            src_start: 10.0,
            src_end: 20.0,
            shots: vec![
                [16.0, 19.0],
                [11.0, 15.0],
                [14.0, 17.0],
                [25.0, 30.0],
                [12.0, 12.4],
            ],
            narration: "镜头归一化测试".into(),
            keep_original_audio: false,
        };
        normalize_segment_shots(&mut segment, 100.0);
        assert_eq!(segment.shots, vec![[11.0, 19.0]]);
        let mut plan = EditPlan {
            title: "测试".into(),
            style: String::new(),
            target_duration_secs: 0.0,
            segments: vec![segment],
        };
        assert!(plan.validate_and_clamp(100.0).is_ok());
    }

    #[test]
    fn original_audio_paragraph_uses_a_continuous_window() {
        let mut segment = EditSegment {
            src_start: 10.0,
            src_end: 20.0,
            shots: vec![[11.0, 15.0], [16.0, 19.0]],
            narration: "原声接力测试".into(),
            keep_original_audio: true,
        };
        normalize_segment_shots(&mut segment, 100.0);
        assert!(segment.shots.is_empty());
    }

    #[test]
    fn picture_fit_fallback_only_deletes_and_obeys_the_limit() {
        let original = "小林接到消息后追问详情，同事告诉他父亲已经去世。";
        let shortened = shorten_narration_by_deletion(original, 16);
        assert!(shortened.chars().count() <= 16);
        assert!(shortened.chars().count() >= 8);
        assert!(original.starts_with(shortened.trim_end_matches('。')));
    }

    #[test]
    fn impossible_original_audio_handoff_becomes_normal_narration() {
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 0.0,
            segments: vec![EditSegment {
                src_start: 467.06,
                src_end: 469.9,
                shots: vec![],
                narration: "家属说，请按女性来。".into(),
                keep_original_audio: true,
            }],
        };
        let downgraded = normalize_original_audio_handoffs(&mut plan, &[], &[], 469.9);
        assert_eq!(downgraded, vec![0]);
        assert!(!plan.segments[0].keep_original_audio);
    }

    #[test]
    fn original_audio_handoff_expands_to_a_complete_dialogue_tail() {
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 0.0,
            segments: vec![EditSegment {
                src_start: 10.0,
                src_end: 14.0,
                shots: vec![],
                narration: "简短引导旁白。".into(),
                keep_original_audio: true,
            }],
        };
        let entries = vec![entry(1, 13.0, 14.0, "值得保留的原声对白")];
        let downgraded = normalize_original_audio_handoffs(&mut plan, &entries, &[], 30.0);
        assert!(downgraded.is_empty());
        assert!(plan.segments[0].keep_original_audio);
        assert_eq!(plan.segments[0].src_end, 17.0);
        assert!(super::super::edit_timing::dialogue_handoff(
            &entries,
            plan.segments[0].src_start,
            plan.segments[0].src_end,
            super::super::edit_timing::spoken_seconds(&plan.segments[0].narration)
        )
        .is_ok());
    }

    #[test]
    fn handoff_starting_on_dialogue_gets_a_real_narration_lead_in() {
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 0.0,
            segments: vec![
                EditSegment {
                    src_start: 430.0,
                    src_end: 445.0,
                    shots: vec![],
                    narration: "上一段解说已经结束。".into(),
                    keep_original_audio: false,
                },
                EditSegment {
                    src_start: 467.06,
                    src_end: 469.9,
                    shots: vec![],
                    narration: "家属说，请按女性来。".into(),
                    keep_original_audio: true,
                },
                EditSegment {
                    src_start: 745.5,
                    src_end: 762.1,
                    shots: vec![],
                    narration: "下一段剧情继续。".into(),
                    keep_original_audio: false,
                },
            ],
        };
        let entries = vec![entry(49, 467.06, 469.9, "女でお願いします。")];
        let report = compile_renderable_timeline(&mut plan, &entries, &[], 800.0);
        assert_eq!(report.realigned_original_audio, vec![1]);
        assert!(report.downgraded_original_audio.is_empty());
        assert!(plan.segments[1].keep_original_audio);
        assert!(plan.segments[1].src_start <= 464.06);
        assert!(plan.segments[1].src_end >= 471.06);
        assert!(super::super::edit_timing::dialogue_handoff(
            &entries,
            plan.segments[1].src_start,
            plan.segments[1].src_end,
            super::super::edit_timing::spoken_seconds(&plan.segments[1].narration)
        )
        .is_ok());
    }

    #[test]
    fn boxed_in_narration_is_compiled_to_native_speed_before_render() {
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 0.0,
            segments: vec![
                EditSegment {
                    src_start: 1619.0,
                    src_end: 1641.9,
                    shots: vec![],
                    narration: "前一段剧情已经完整讲清。".into(),
                    keep_original_audio: false,
                },
                EditSegment {
                    src_start: 1642.19,
                    src_end: 1645.48,
                    shots: vec![],
                    narration: "小林感慨人生最后的消费由他人决定。".into(),
                    keep_original_audio: false,
                },
                EditSegment {
                    src_start: 1645.8,
                    src_end: 1660.2,
                    shots: vec![],
                    narration: "下一段剧情继续推进。".into(),
                    keep_original_audio: false,
                },
            ],
        };
        let report = compile_renderable_timeline(&mut plan, &[], &[], 1700.0);
        assert_eq!(report.shortened_narration, vec![1]);
        assert!(super::super::edit_timing::reserve_issues(&plan)
            .into_iter()
            .all(|issue| issue.severity != super::super::edit_timing::ReserveSeverity::Blocking));
    }

    #[test]
    fn sub_three_second_fragment_cannot_block_the_whole_film() {
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 0.0,
            segments: vec![
                EditSegment {
                    src_start: 0.0,
                    src_end: 10.0,
                    shots: vec![],
                    narration: "完整的前一段解说内容。".into(),
                    keep_original_audio: false,
                },
                EditSegment {
                    src_start: 10.0,
                    src_end: 12.0,
                    shots: vec![],
                    narration: "这个碎片无论如何都放不下最短口播。".into(),
                    keep_original_audio: false,
                },
                EditSegment {
                    src_start: 12.0,
                    src_end: 22.0,
                    shots: vec![],
                    narration: "完整的后一段解说内容。".into(),
                    keep_original_audio: false,
                },
            ],
        };
        let report = compile_renderable_timeline(&mut plan, &[], &[], 22.0);
        assert_eq!(report.dropped_unrenderable, vec![1]);
        assert_eq!(plan.segments.len(), 2);
        assert!(super::super::edit_timing::reserve_issues(&plan)
            .into_iter()
            .all(|issue| issue.severity != super::super::edit_timing::ReserveSeverity::Blocking));
    }

    #[test]
    fn recovers_complete_segments_from_truncated_director_json() {
        let fragment = r#"{
            "title":"测试",
            "style":"剧情解说",
            "target_duration_secs":100,
            "segments":[
                {"src_start":1.0,"src_end":9.0,"narration":"第一个完整片段保留人物动作和后果。","keep_original_audio":false},
                {"src_start":10.0,"src_end":18.0,"narration":"第二段在这里被截断
        "#;

        let recovered = recover_edit_plan_fragment(fragment, "兜底标题", "剧情解说").unwrap();
        assert_eq!(recovered.title, "兜底标题");
        assert_eq!(recovered.segments.len(), 1);
        assert_eq!(recovered.segments[0].src_start, 1.0);
        assert!(recovered.segments[0].narration.contains("人物动作和后果"));
    }

    #[test]
    fn framing_interviews_and_inserts_are_removed_from_story_beats() {
        let mut beats = vec![
            StoryBeat {
                id: String::new(),
                start: 63.0,
                end: 75.0,
                summary: "节目开场播放登山短片".into(),
                importance: 2,
                quote: String::new(),
                kind: "节目导语".into(),
                narrative_layer: "framing".into(),
                evidence: vec![],
            },
            StoryBeat {
                id: String::new(),
                start: 166.0,
                end: 178.0,
                summary: "主角开始日常生活".into(),
                importance: 5,
                quote: String::new(),
                kind: "主线".into(),
                narrative_layer: "main".into(),
                evidence: vec![],
            },
        ];

        normalize_beats(&mut beats, 6_000.0, &[]);
        assert_eq!(beats.len(), 1);
        assert_eq!(beats[0].narrative_layer, "main");
    }

    #[test]
    fn fact_check_corrects_valid_findings_but_preserves_invalid_findings() {
        let mut plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 1.0,
            segments: vec![
                EditSegment {
                    src_start: 63.0,
                    src_end: 75.0,
                    narration: "楚门双腿折断仍然坚持登山。".into(),
                    shots: vec![],
                    keep_original_audio: false,
                },
                EditSegment {
                    src_start: 80.0,
                    src_end: 90.0,
                    narration: "字幕无法证明的陌生人名和结局。".into(),
                    shots: vec![],
                    keep_original_audio: false,
                },
            ],
        };
        let entries = vec![
            entry(1, 63.0, 67.0, "You're going to the top of this mountain"),
            entry(2, 82.0, 86.0, "unrelated line"),
        ];
        let evidence = fact_evidence_packet(&entries, &plan);
        apply_fact_check(
            &mut plan,
            FactCheckResponse {
                verdicts: vec![
                    FactCheckVerdict {
                        segment_index: 0,
                        supported: false,
                        evidence_ids: vec!["S0-E1".into()],
                        corrected_narration: "节目开场播放了一段登山短片，与楚门的亲身经历无关。"
                            .into(),
                    },
                    FactCheckVerdict {
                        segment_index: 1,
                        supported: false,
                        evidence_ids: vec!["S0-E999".into()],
                        corrected_narration: String::new(),
                    },
                ],
            },
            &evidence,
        );

        assert_eq!(plan.segments.len(), 2);
        assert!(plan.segments[0].narration.starts_with("节目开场"));
        assert!(!plan.segments[0].narration.contains("双腿折断"));
        assert!(plan.segments[1].narration.contains("字幕无法证明"));
    }

    #[test]
    fn fact_evidence_ids_cannot_cross_neighboring_shot_boundaries() {
        let plan = EditPlan {
            title: "测试".into(),
            style: "剧情解说".into(),
            target_duration_secs: 1.0,
            segments: vec![
                EditSegment {
                    src_start: 3025.5,
                    src_end: 3030.0,
                    narration: "相遇并非偶然。".into(),
                    shots: vec![[3025.5, 3030.0]],
                    keep_original_audio: false,
                },
                EditSegment {
                    src_start: 3033.9,
                    src_end: 3043.2,
                    narration: "对方提到命运。".into(),
                    shots: vec![[3033.9, 3043.2]],
                    keep_original_audio: false,
                },
            ],
        };
        let entries = vec![
            entry(544, 3025.54, 3026.84, "偶然ですか？"),
            entry(545, 3027.36, 3029.94, "ここを通りかかったのは。"),
            entry(546, 3033.90, 3035.26, "運命잖아?"),
        ];
        let packet = fact_evidence_packet(&entries, &plan);
        assert_eq!(packet[0].candidates.len(), 2);
        assert_eq!(packet[1].candidates.len(), 1);
        let crossed = FactCheckVerdict {
            segment_index: 0,
            supported: true,
            evidence_ids: vec!["S1-E546".into()],
            corrected_narration: String::new(),
        };
        assert!(!fact_verdict_has_valid_evidence(&packet[0], &crossed));
    }
}
