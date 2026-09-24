use std::path::Path;

use serde::Serialize;

use crate::services::jobs::{self, JobPaths};
use crate::services::subtitle::{self, SubtitleEntry};

const SCHEMA_MD: &str = r#"# edit.json schema

每段可增加 shots: [[开始秒,结束秒], ...]，表示一段连续配音所对应的多个证据画面。
不使用多镜头时填 []。镜头按时间递增且不重叠，均位于本段 src_start/src_end 内。
原声接力段 shots 必须为 []，在引导解说结束后播放原声，需为对白留足时长。

Write a single JSON object (no markdown fences) to `edit.json`:

```json
{
  "title": "成片标题",
  "style": "剧情解说",
  "target_duration_secs": 75,
  "segments": [
    {
      "src_start": 12.4,
      "src_end": 28.1,
      "narration": "这一段要讲的解说词",
      "keep_original_audio": false
    }
  ]
}
```

Rules:
- `style` 必须与 META.json 的 style 一致，narration 遵守同目录 STYLE.md。
- `src_start` / `src_end` are seconds on the ORIGINAL video, taken from transcript timestamps.
- Each segment must be at least 0.8 seconds.
- `narration` is spoken Chinese commentary. 普通段落要不看原片也能听懂；保留原声的段落解说要短，把空间留给原片台词/音乐。
- 成片时长由字幕里的关键节拍决定：写完解说后，把 `target_duration_secs` 写成估算口述时长（约每秒 4 个汉字）。不要凑预设秒数，也不要铺满整部原片。
- 纯解说口播时长必须达到过滤后有效字幕覆盖时长的 30%；按每秒约 4 个汉字换算最低字数。无字幕空镜、音乐、片头片尾、广告和被过滤区间不进入计算基数，保留原声时长不计入口播。
- 片段数量不设上限，以完整覆盖人物目标、阻力、选择、转折和后果为准；不要为了控制段数删掉必要因果。
- 章节不设最低字数，不为凑字数重写整章；字幕事实核验仍须通过，时长目标只在整稿阶段评估。
- 普通选片控制在 4–20 秒，精确落在能证明解说的动作或反应；不要给几十秒的大范围。
- 经典名场面、神句、关键对白：只有字幕中存在完整原句且选片为 4–12 秒时，该段 `keep_original_audio` 为 true。其余段落 false。
- 不得选择片头曲、片尾曲、演职员表、中间广告、赞助口播、下载引导或下集预告。
- 前 45 字从具体人物的一次反常选择或关系困境自然进入；可以留下问题，但不夸大危险、不连续反问，不得使用「开场」「镜头来到」「故事开始」等元话语。
- 解说围绕中心问题、人物选择和后果推进；剧情用于举证，理解必须由多处字幕共同支撑。不要逐句复述字幕、机械制造钩子、堆砌空泛情绪或使用 AI 套话。
"#;

#[derive(Serialize)]
struct MetaFile {
    title: String,
    url: String,
    duration_secs: f64,
    style: String,
    task_id: String,
}

const STYLES_CATALOG: &str = include_str!("../../../styles/STYLES.md");

fn style_file(style: &str) -> String {
    format!(
        "# 本任务风格：{style}\n\n\
         严格按下面目录里「{style}」一节写口吻、开头、句长和选片。找不到同名风格则按「剧情解说」。\n\n\
         ---\n\n{STYLES_CATALOG}"
    )
}

pub fn write_brief(
    paths: &JobPaths,
    task_id: &str,
    title: &str,
    url: &str,
    duration_secs: f64,
    style: &str,
    entries: &[SubtitleEntry],
) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(&paths.brief)?;

    let meta = MetaFile {
        title: title.to_string(),
        url: url.to_string(),
        duration_secs,
        style: style.to_string(),
        task_id: task_id.to_string(),
    };
    jobs::atomic_write(&paths.meta, serde_json::to_string_pretty(&meta).unwrap())?;
    jobs::atomic_write(paths.brief.join("SCHEMA.md"), SCHEMA_MD)?;
    jobs::atomic_write(paths.brief.join("STYLE.md"), style_file(style))?;
    let legacy_agent_file = paths.brief.join("AGENTS.md");
    if legacy_agent_file.exists() {
        std::fs::remove_file(legacy_agent_file)?;
    }

    let compact = subtitle::compact_transcript(entries, 60_000);
    jobs::atomic_write(&paths.compact, compact)?;
    jobs::atomic_write(
        &paths.normalized_transcript,
        serde_json::to_vec_pretty(entries).unwrap(),
    )?;

    refresh_inbox_index()?;
    Ok(())
}

pub fn refresh_inbox_index() -> Result<(), std::io::Error> {
    std::fs::create_dir_all(jobs::inbox_root())?;
    let current = jobs::inbox_root().join("CURRENT.md");
    if current.exists() {
        std::fs::remove_file(current)?;
    }
    Ok(())
}

pub fn mark_current_ready(_task_id: &str) -> Result<(), std::io::Error> {
    refresh_inbox_index()
}

pub fn copy_transcript_into_job(src: &Path, dest: &Path) -> Result<(), std::io::Error> {
    if src == dest {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = std::fs::read(src)?;
    jobs::atomic_write(dest, bytes)?;
    Ok(())
}
