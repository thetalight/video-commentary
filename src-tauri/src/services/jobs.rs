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
    inbox_root_for(&repo_root(), dirs::data_local_dir())
}

fn inbox_root_for(repo: &Path, data_dir: Option<PathBuf>) -> PathBuf {
    if repo.join("src-tauri/Cargo.toml").is_file() {
        return repo.join("inbox");
    }
    data_dir
        .or_else(|| dirs::home_dir().map(|home| home.join("Library/Application Support")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.esun.video-commentary/inbox")
}

#[derive(Debug, Clone)]
pub struct JobPaths {
    pub root: PathBuf,
    pub source: PathBuf,
    pub transcript: PathBuf,
    pub meta: PathBuf,
    pub shorts: PathBuf,
}

impl JobPaths {
    /// `output_dir` is kept in the signature for migration compatibility. New
    /// and restored tasks have exactly one canonical directory under inbox.
    pub fn new(_output_dir: impl AsRef<Path>, task_id: &str) -> Self {
        let root = inbox_root().join(task_id);
        Self {
            source: root.join("source.mp4"),
            transcript: root.join("TRANSCRIPT.srt"),
            meta: root.join("META.json"),
            shorts: root.join("shorts"),
            root,
        }
    }

    pub fn ensure(&self) -> io::Result<()> {
        fs::create_dir_all(&self.root)
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
    // On the default case-insensitive macOS filesystem, `transcript.srt`
    // resolves to `TRANSCRIPT.srt`. Only migrate an entry whose actual name
    // is lower-case; otherwise this cleanup deletes the canonical file.
    let has_old_spelling = fs::read_dir(&paths.root)?
        .filter_map(Result::ok)
        .any(|entry| entry.file_name() == "transcript.srt");
    if has_old_spelling {
        if !paths.transcript.exists() {
            move_file(&old_transcript, &paths.transcript)?;
        } else if same_file(&old_transcript, &paths.transcript) {
            fs::rename(&old_transcript, &paths.transcript)?;
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
    // A missing canonical file under a managed/bound task is not permission to
    // resurrect a stale sidecar (possibly from a rejected source or old translation).
    if root.join("source-replacement.json").exists()
        || root.join("rejected-sources").exists()
        || root.join("TRANSCRIPT.install-pending.json").exists()
        || root.join("TRANSCRIPT.source.json").exists()
    {
        return Ok(());
    }
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
                        && !lower.contains(".ocr.")
                        && !lower.starts_with("transcript.")
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
        } else if name.contains(".zh") {
            1
        } else {
            2
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

    entries
        .into_iter()
        .min_by_key(|path| subtitle_language_rank(path))
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
            "画面硬字幕 OCR（已停用）"
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
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let language = stem
        .split_once('.')
        .map(|(_, language)| language.to_ascii_lowercase());
    match language.as_deref() {
        Some("zh-cn" | "zh") => 0,
        Some("zh-hans-zh-cn") => 1,
        Some("zh-hans" | "zh-hant") => 2,
        Some("en" | "en-orig" | "en-us" | "en-gb") => 3,
        Some("ja") => 4,
        Some("ko") => 5,
        // Unknown translation variants are a last resort, not native Chinese.
        _ => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_app_uses_user_data_when_source_checkout_is_absent() {
        let root = std::env::temp_dir().join(format!("vc-inbox-root-{}", std::process::id()));
        let repo = root.join("missing-source");
        let data = root.join("user-data");
        assert_eq!(
            inbox_root_for(&repo, Some(data.clone())),
            data.join("com.esun.video-commentary/inbox")
        );
    }

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
    fn repeated_migration_preserves_canonical_transcript() {
        let tmp = std::env::temp_dir().join(format!("vc-canonical-case-{}", std::process::id()));
        let root = tmp.join("job-case");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("TRANSCRIPT.srt"), b"selected simplified subtitle").unwrap();
        let paths = JobPaths {
            root: root.clone(),
            source: root.join("source.mp4"),
            transcript: root.join("TRANSCRIPT.srt"),
            meta: root.join("META.json"),
            shorts: root.join("shorts"),
        };
        normalize_canonical_names(&paths).unwrap();
        normalize_canonical_names(&paths).unwrap();
        assert_eq!(
            fs::read(&paths.transcript).unwrap(),
            b"selected simplified subtitle"
        );
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn managed_or_replaced_task_never_resurrects_unbound_old_sidecar() {
        let root = std::env::temp_dir().join(format!(
            "vc-no-restore-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let canonical = root.join("TRANSCRIPT.srt");
        std::fs::write(root.join("source.embedded.en.srt"), b"old sidecar").unwrap();
        for marker in [
            "source-replacement.json",
            "TRANSCRIPT.install-pending.json",
            "TRANSCRIPT.source.json",
        ] {
            std::fs::write(root.join(marker), b"marker").unwrap();
            restore_canonical_transcript(&root, &canonical).unwrap();
            assert!(!canonical.exists());
            std::fs::remove_file(root.join(marker)).unwrap();
        }
        std::fs::remove_file(root.join("source.embedded.en.srt")).unwrap();
        std::fs::write(root.join("TRANSCRIPT.zh.srt"), b"retired translated copy").unwrap();
        restore_canonical_transcript(&root, &canonical).unwrap();
        assert!(!canonical.exists());
        std::fs::remove_dir_all(root).unwrap();
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
    fn native_or_known_chinese_beats_unrelated_auto_translation() {
        let simplified = Path::new("source.zh-CN.srt");
        let auto_hans = Path::new("source.zh-Hans.srt");
        let native = Path::new("source.zh-Hans-zh-CN.srt");
        let translated = Path::new("source.zh-Hans-ar.srt");
        let english = Path::new("source.en.srt");
        assert!(subtitle_language_rank(simplified) < subtitle_language_rank(auto_hans));
        assert!(subtitle_language_rank(native) < subtitle_language_rank(translated));
        assert!(subtitle_language_rank(english) < subtitle_language_rank(translated));
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
