use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn inbox_root() -> PathBuf {
    repo_root().join("inbox")
}

#[derive(Debug, Clone)]
pub struct JobPaths {
    pub root: PathBuf,
    pub source: PathBuf,
    pub transcript: PathBuf,
    pub brief: PathBuf,
    pub meta: PathBuf,
    pub edit: PathBuf,
    pub commentary: PathBuf,
    pub compact: PathBuf,
    pub normalized_transcript: PathBuf,
    pub tts_dir: PathBuf,
    pub clips_dir: PathBuf,
    pub director_dir: PathBuf,
    pub review_dir: PathBuf,
    pub renders_dir: PathBuf,
    pub render_manifest: PathBuf,
    pub output: PathBuf,
    pub output_srt: PathBuf,
}

impl JobPaths {
    /// `output_dir` is kept in the signature for migration compatibility. New
    /// and restored tasks have exactly one canonical directory under inbox.
    pub fn new(_output_dir: impl AsRef<Path>, task_id: &str) -> Self {
        let root = inbox_root().join(task_id);
        let brief = root.clone();
        Self {
            source: root.join("source.mp4"),
            transcript: root.join("TRANSCRIPT.srt"),
            meta: brief.join("META.json"),
            edit: brief.join("edit.json"),
            commentary: brief.join("commentary.md"),
            compact: brief.join("TRANSCRIPT.compact.txt"),
            normalized_transcript: brief.join("TRANSCRIPT.normalized.json"),
            output: root.join("output.mp4"),
            output_srt: root.join("output.srt"),
            tts_dir: root.join("tts"),
            clips_dir: root.join("clips"),
            director_dir: root.join("director"),
            review_dir: root.join("review"),
            renders_dir: root.join("renders"),
            render_manifest: root.join("render-manifest.json"),
            brief,
            root,
        }
    }

    pub fn ensure(&self) -> io::Result<()> {
        fs::create_dir_all(&self.brief)?;
        fs::create_dir_all(&self.tts_dir)?;
        fs::create_dir_all(&self.clips_dir)?;
        fs::create_dir_all(&self.director_dir)?;
        fs::create_dir_all(&self.review_dir)?;
        fs::create_dir_all(&self.renders_dir)?;
        Ok(())
    }
}

/// Move resources from the pre-v2 `<output>/jobs/<task>` layout into the
/// canonical `inbox/<task>` directory. Existing inbox files win; conflicting
/// legacy files are retained under `legacy/` so migration never loses data.
pub fn migrate_legacy_job(output_dir: impl AsRef<Path>, task_id: &str) -> io::Result<JobPaths> {
    let paths = JobPaths::new(&output_dir, task_id);
    let legacy = output_dir.as_ref().join("jobs").join(task_id);
    if !legacy.exists() || legacy == paths.root {
        paths.ensure()?;
        normalize_canonical_names(&paths)?;
        return Ok(paths);
    }
    fs::create_dir_all(&paths.root)?;
    merge_directory(&legacy, &paths.root, &paths.root.join("legacy"))?;
    if legacy.exists() && fs::read_dir(&legacy)?.next().is_none() {
        fs::remove_dir(&legacy)?;
    }
    paths.ensure()?;
    normalize_canonical_names(&paths)?;
    Ok(paths)
}

fn normalize_canonical_names(paths: &JobPaths) -> io::Result<()> {
    let old_transcript = paths.root.join("transcript.srt");
    if old_transcript.exists() {
        if !paths.transcript.exists() {
            move_file(&old_transcript, &paths.transcript)?;
        } else if same_file(&old_transcript, &paths.transcript) {
            fs::remove_file(old_transcript)?;
        } else {
            let conflicts = paths.root.join("legacy");
            fs::create_dir_all(&conflicts)?;
            move_file(
                &old_transcript,
                &unique_path(conflicts.join("transcript.srt")),
            )?;
        }
    }
    if !paths.transcript.exists() {
        restore_canonical_transcript(&paths.root, &paths.transcript)?;
    }
    Ok(())
}

fn restore_canonical_transcript(root: &Path, transcript: &Path) -> io::Result<()> {
    let mut candidates = fs::read_dir(root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_subtitle_file(path))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    let lower = name.to_ascii_lowercase();
                    name != "output.srt"
                        && name != "TRANSCRIPT.srt"
                        && !lower.contains("sensevoice")
                        && !lower.contains("whisper")
                        && !lower.contains(".asr.")
                })
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if name.contains(".embedded.") {
            0
        } else if name.contains(".ocr.") {
            1
        } else if name.contains(".zh") {
            2
        } else {
            3
        }
    });
    if let Some(source) = candidates.first() {
        fs::copy(source, transcript)?;
    }
    Ok(())
}

pub fn migrate_all_legacy_jobs(output_dir: impl AsRef<Path>) -> io::Result<()> {
    let jobs_root = output_dir.as_ref().join("jobs");
    let Ok(entries) = fs::read_dir(&jobs_root) else {
        return Ok(());
    };
    for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
        let Some(task_id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if task_id.is_empty()
            || !task_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            continue;
        }
        migrate_legacy_job(&output_dir, &task_id)?;
    }
    if jobs_root.exists() && fs::read_dir(&jobs_root)?.next().is_none() {
        fs::remove_dir(jobs_root)?;
    }
    Ok(())
}

fn merge_directory(source: &Path, destination: &Path, conflicts: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            merge_directory(&from, &to, &conflicts.join(entry.file_name()))?;
            if fs::read_dir(&from)?.next().is_none() {
                fs::remove_dir(&from)?;
            }
            continue;
        }
        if to.exists() {
            if same_file(&from, &to) {
                fs::remove_file(&from)?;
            } else {
                fs::create_dir_all(conflicts)?;
                move_file(&from, &unique_path(conflicts.join(entry.file_name())))?;
            }
        } else {
            move_file(&from, &to)?;
        }
    }
    Ok(())
}

fn move_file(source: &Path, destination: &Path) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, destination)?;
            fs::remove_file(source)
        }
    }
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().map(|value| value.to_string_lossy());
    for index in 1..10_000 {
        let name = match &extension {
            Some(extension) => format!("{stem}-{index}.{extension}"),
            None => format!("{stem}-{index}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

/// Crash-safe write for task metadata and director artifacts.
pub fn atomic_write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let temporary =
        path.with_file_name(format!(".{file_name}.{}.{}.tmp", std::process::id(), nonce));
    let mut file = fs::File::create(&temporary)?;
    file.write_all(bytes.as_ref())?;
    file.sync_all()?;
    fs::rename(&temporary, path).or_else(|error| {
        let _ = fs::remove_file(&temporary);
        Err(error)
    })
}

pub fn stable_hash(parts: &[&[u8]]) -> String {
    // FNV-1a is deterministic and sufficient for local cache/revision keys.
    let mut hash = 0xcbf29ce484222325_u64;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(meta_a), Ok(meta_b)) = (fs::metadata(a), fs::metadata(b)) else {
        return false;
    };
    meta_a.len() == meta_b.len()
        && meta_a
            .modified()
            .ok()
            .zip(meta_b.modified().ok())
            .is_some_and(|(ta, tb)| ta == tb)
}

pub fn is_subtitle_file(path: &Path) -> bool {
    path.extension()
        .map(|ext| {
            let ext = ext.to_string_lossy().to_lowercase();
            ext == "srt" || ext == "vtt"
        })
        .unwrap_or(false)
}

fn is_derived_subtitle(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    name.contains(".embedded.")
        || name.contains(".ocr.")
        || name.contains("sensevoice")
        || name.contains("whisper")
        || name.contains(".asr.")
        || name == "transcript.srt"
        || name == "output.srt"
}

pub fn find_subtitle_in_dir(dir: &Path, stem: &str) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_subtitle_file(p))
        .filter(|p| !is_derived_subtitle(p))
        .filter(|p| {
            p.file_stem()
                .map(|s| s.to_string_lossy().starts_with(stem))
                .unwrap_or(false)
        })
        .collect();

    for lang in ["zh", "en", "ja", "ko"] {
        if let Some(path) = entries.iter().find(|p| {
            p.to_string_lossy()
                .to_lowercase()
                .contains(&format!(".{lang}"))
        }) {
            return Some(path.clone());
        }
    }

    entries.into_iter().next()
}

/// Best-effort migration helper for tasks created before transcript provenance
/// was persisted explicitly. It compares the canonical transcript with source
/// sidecars instead of guessing solely from whatever files happen to exist.
pub fn infer_subtitle_source(root: &Path) -> Option<String> {
    let canonical = fs::read(root.join("TRANSCRIPT.srt")).ok()?;
    let mut candidates = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_subtitle_file(path))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name != "TRANSCRIPT.srt" && name != "output.srt")
        })
        .filter(|path| fs::read(path).is_ok_and(|raw| raw == canonical))
        .collect::<Vec<_>>();
    candidates.sort();
    let name = candidates
        .first()?
        .file_name()?
        .to_string_lossy()
        .to_ascii_lowercase();
    Some(
        if name.contains(".embedded.") {
            "视频内封字幕"
        } else if name.contains(".ocr.") {
            "画面硬字幕 OCR"
        } else if name.contains("sensevoice") || name.contains("whisper") || name.contains(".asr.")
        {
            "旧版语音字幕（历史任务）"
        } else {
            "平台独立字幕"
        }
        .to_string(),
    )
}

pub fn relocate_downloaded_subtitles(
    video_path: &Path,
    dest_dir: &Path,
) -> io::Result<Option<PathBuf>> {
    fs::create_dir_all(dest_dir)?;
    let Some(parent) = video_path.parent() else {
        return Ok(None);
    };
    let stem = video_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut candidates = Vec::new();

    for entry in fs::read_dir(parent)? {
        let path = entry?.path();
        if !is_subtitle_file(&path) || is_derived_subtitle(&path) {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !name.starts_with(&stem) {
            continue;
        }
        let dest = dest_dir.join(path.file_name().unwrap_or_default());
        if path == dest {
            candidates.push(dest);
            continue;
        }
        if dest.exists() {
            let _ = fs::remove_file(&dest);
        }
        match fs::rename(&path, &dest) {
            Ok(()) => {
                candidates.push(dest);
            }
            Err(_) => {
                fs::copy(&path, &dest)?;
                let _ = fs::remove_file(&path);
                candidates.push(dest);
            }
        }
    }

    candidates.sort_by_key(|path| subtitle_language_rank(path));
    Ok(candidates.into_iter().next())
}

fn subtitle_language_rank(path: &Path) -> u8 {
    let name = path.to_string_lossy().to_lowercase();
    if name.contains(".zh") {
        0
    } else if name.contains(".en") {
        1
    } else if name.contains(".ja") {
        2
    } else if name.contains(".ko") {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_legacy_resources_without_overwriting_inbox() {
        let tmp = std::env::temp_dir().join(format!("vc-merge-{}", std::process::id()));
        let legacy = tmp.join("jobs/job-test");
        let inbox = tmp.join("inbox/job-test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(legacy.join("tts")).unwrap();
        fs::create_dir_all(&inbox).unwrap();
        fs::write(legacy.join("source.mp4"), b"legacy-source").unwrap();
        fs::write(legacy.join("tts/a.mp3"), b"audio").unwrap();
        fs::write(inbox.join("source.mp4"), b"inbox-source").unwrap();

        merge_directory(&legacy, &inbox, &inbox.join("legacy")).unwrap();

        assert_eq!(fs::read(inbox.join("source.mp4")).unwrap(), b"inbox-source");
        assert_eq!(fs::read(inbox.join("tts/a.mp3")).unwrap(), b"audio");
        assert_eq!(
            fs::read(inbox.join("legacy/source.mp4")).unwrap(),
            b"legacy-source"
        );
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn atomic_write_replaces_complete_file() {
        let tmp = std::env::temp_dir().join(format!("vc-atomic-{}", std::process::id()));
        let path = tmp.join("state.json");
        let _ = fs::remove_dir_all(&tmp);
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new-complete").unwrap();
        assert_eq!(fs::read(path).unwrap(), b"new-complete");
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn keeps_same_directory_subtitles_and_prefers_chinese() {
        let tmp = std::env::temp_dir().join(format!(
            "vc-subs-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let video = tmp.join("source.mp4");
        let english = tmp.join("source.en.srt");
        let chinese = tmp.join("source.zh-Hans.srt");
        fs::write(&video, b"video").unwrap();
        fs::write(&english, b"english").unwrap();
        fs::write(&chinese, b"chinese").unwrap();

        let selected = relocate_downloaded_subtitles(&video, &tmp)
            .unwrap()
            .unwrap();
        assert_eq!(selected, chinese);
        assert!(english.exists());
        assert!(selected.exists());
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn does_not_restore_removed_speech_recognition_subtitles() {
        let tmp = std::env::temp_dir().join(format!(
            "vc-restore-transcript-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let transcript = tmp.join("TRANSCRIPT.srt");
        let source = tmp.join("source.sensevoice.srt");
        fs::write(&source, "1\n00:00:01,000 --> 00:00:02,000\n对白\n").unwrap();

        restore_canonical_transcript(&tmp, &transcript).unwrap();

        assert!(source.exists());
        assert!(!transcript.exists());
        let _ = fs::remove_dir_all(tmp);
    }
}
