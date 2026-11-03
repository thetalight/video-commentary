use std::path::Path;

use serde::Serialize;

use crate::services::jobs::{self, JobPaths};
use crate::services::subtitle::{self, SubtitleEntry};

#[derive(Serialize)]
struct MetaFile {
    title: String,
    url: String,
    duration_secs: f64,
    style: String,
    task_id: String,
}

pub fn write_meta(
    paths: &JobPaths,
    task_id: &str,
    title: &str,
    url: &str,
    duration_secs: f64,
    style: &str,
) -> Result<(), std::io::Error> {
    let meta = MetaFile {
        title: title.to_string(),
        url: url.to_string(),
        duration_secs,
        style: style.to_string(),
        task_id: task_id.to_string(),
    };
    jobs::atomic_write(&paths.meta, serde_json::to_string_pretty(&meta).unwrap())?;
    // Files written by the retired long-form director; nothing reads them now.
    for stale in [
        "SCHEMA.md",
        "STYLE.md",
        "AGENTS.md",
        "TRANSCRIPT.compact.txt",
        "TRANSCRIPT.normalized.json",
    ] {
        let path = paths.root.join(stale);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Install the selected source as the canonical, normalized transcript.
pub fn install_transcript(src: &Path, dest: &Path) -> Result<Vec<SubtitleEntry>, String> {
    let entries = subtitle::parse_file(src).map_err(|error| error.to_string())?;
    if entries.is_empty() {
        return Err("字幕整理后没有可用对白".into());
    }
    let normalized = subtitle::to_srt(&entries);
    jobs::atomic_write(dest, normalized.as_bytes()).map_err(|error| error.to_string())?;
    if std::fs::read(dest).map_err(|error| error.to_string())? != normalized.as_bytes() {
        return Err("保存的字幕与选中来源不一致".into());
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_the_selected_source_even_when_canonical_contains_another_language() {
        let root =
            std::env::temp_dir().join(format!("vc-transcript-install-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let selected = root.join("source.zh-CN.srt");
        let canonical = root.join("TRANSCRIPT.srt");
        std::fs::write(&selected, "1\n00:00:01,000 --> 00:00:02,000\n简体字幕\n").unwrap();
        std::fs::write(&canonical, "1\n00:00:01,000 --> 00:00:02,000\n繁體字幕\n").unwrap();

        let entries = install_transcript(&selected, &canonical).unwrap();
        assert_eq!(entries[0].text, "简体字幕");
        assert!(std::fs::read_to_string(&canonical)
            .unwrap()
            .contains("简体字幕"));

        let _ = std::fs::remove_dir_all(root);
    }
}
