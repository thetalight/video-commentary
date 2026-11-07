//! Canonical transcript provenance. No content sent outside this process.
use super::{jobs, script, subtitle};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn source_revision(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let modified = meta
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(jobs::stable_hash(&[
        path.to_string_lossy().as_bytes(),
        meta.len().to_string().as_bytes(),
        modified.to_string().as_bytes(),
    ]))
}
fn sidecar(root: &Path) -> PathBuf {
    root.join("TRANSCRIPT.source.json")
}
pub fn pending(root: &Path) -> PathBuf {
    root.join("TRANSCRIPT.install-pending.json")
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Binding {
    pub binding_version: u32,
    pub source: String,
    pub source_path: PathBuf,
    pub canonical_path: PathBuf,
    pub source_revision: String,
    pub transcript_revision: String,
    pub canonical_bytes_revision: String,
}
pub fn create(
    source: &Path,
    canonical: &Path,
    candidate: &Path,
    label: &str,
    entries: &[subtitle::SubtitleEntry],
) -> Result<Binding, String> {
    Ok(Binding {
        binding_version: 1,
        source: label.into(),
        source_path: candidate.into(),
        canonical_path: canonical.into(),
        source_revision: source_revision(source)?,
        transcript_revision: script::transcript_revision(entries),
        canonical_bytes_revision: jobs::stable_hash(&[subtitle::to_srt(entries).as_bytes()]),
    })
}
pub fn write(root: &Path, binding: &Binding) -> Result<(), String> {
    jobs::atomic_write(
        sidecar(root),
        serde_json::to_vec_pretty(binding).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
/// Existing episode/shorts records can prove a historical source+text pair. Do
/// not attach a new binding automatically or infer identity just from language.
fn legacy_episode_matches(root: &Path, source_rev: &str, transcript_rev: &str) -> bool {
    [
        root.join("episode/episode.json"),
        root.join("shorts/shorts.json"),
    ]
    .iter()
    .any(|path| {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|j| {
                j.get("source_revision").and_then(|v| v.as_str()) == Some(source_rev)
                    && j.get("transcript_revision").and_then(|v| v.as_str()) == Some(transcript_rev)
            })
    })
}
pub fn verify(root: &Path, source: &Path, canonical: &Path) -> Result<(), String> {
    if pending(root).exists() {
        return Err(
            "字幕安装上次未完成，禁止使用中间状态。请通过应用重新获取字幕；原文件保留。".into(),
        );
    }
    let raw = std::fs::read(sidecar(root)).map_err(|_| {
        "字幕缺少可核验的来源记录，请通过应用重新获取字幕；不会自动绑定旧字幕到新原片。".to_string()
    })?;
    let info: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|_| "字幕来源记录损坏，请重新获取字幕".to_string())?;
    let entries = subtitle::parse_file(canonical)
        .map_err(|_| "无法读取规范字幕，请重新获取字幕".to_string())?;
    let source_rev = source_revision(source)?;
    let transcript_rev = script::transcript_revision(&entries);
    if info.get("binding_version").is_none() {
        if legacy_episode_matches(root, &source_rev, &transcript_rev) {
            return Ok(());
        }
        return Err("历史字幕没有绑定原片版本，且没有匹配的历史稿件证明来源；请通过应用重新获取字幕，旧文件不删除。".into());
    }
    let binding: Binding = serde_json::from_value(info)
        .map_err(|_| "字幕版本绑定记录格式无效，请重新获取字幕".to_string())?;
    if binding.binding_version != 1 || binding.source_revision != source_rev {
        return Err("当前字幕属于其他原片版本（或原片已替换），不能用于写稿/评审/出片。请重新获取当前原片字幕；旧字幕保留。".into());
    }
    let canonical_bytes = std::fs::read(canonical).map_err(|e| e.to_string())?;
    if binding.transcript_revision != transcript_rev
        || binding.canonical_bytes_revision != jobs::stable_hash(&[&canonical_bytes])
    {
        return Err("规范字幕与已安装来源版本不一致，请重新获取字幕；不能沿用旧评审。".into());
    }
    Ok(())
}
/// Narrow legacy exception for a retired translation failure which installed the
/// original first. Never use it after an observed source replacement or pending
/// install. No file is written here. New bindings are always strictly verified.
pub fn recover_legacy_translation(root: &Path, source: &Path, canonical: &Path) -> bool {
    if pending(root).exists()
        || root.join("source-replacement.json").exists()
        || root.join("rejected-sources").exists()
    {
        return false;
    }
    let Ok(raw) = std::fs::read(sidecar(root)) else {
        return false;
    };
    let Ok(info) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return false;
    };
    if info.get("binding_version").is_some() {
        return verify(root, source, canonical).is_ok();
    }
    let modified = |p: &Path| std::fs::metadata(p).ok()?.modified().ok();
    // Old path writes the original transcript and sidecar only after download.
    match (
        modified(source),
        modified(canonical),
        modified(&sidecar(root)),
    ) {
        (Some(s), Some(t), Some(p)) => s <= t && s <= p,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, PathBuf, PathBuf, Vec<subtitle::SubtitleEntry>) {
        let root = std::env::temp_dir().join(format!(
            "binding-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.mp4");
        let canonical = root.join("TRANSCRIPT.srt");
        std::fs::write(&source, b"synthetic source").unwrap();
        let entries = vec![subtitle::SubtitleEntry {
            index: 1,
            start: "00:00:01,000".into(),
            end: "00:00:03,000".into(),
            text: "Synthetic dialogue".into(),
        }];
        std::fs::write(&canonical, subtitle::to_srt(&entries)).unwrap();
        (root, source, canonical, entries)
    }
    #[test]
    fn binding_rejects_replaced_source_modified_subtitle_and_incomplete_install() {
        let (root, source, canonical, entries) = fixture();
        let b = create(&source, &canonical, &canonical, "本机语音转写", &entries).unwrap();
        write(&root, &b).unwrap();
        verify(&root, &source, &canonical).unwrap();
        std::fs::write(pending(&root), b"pending").unwrap();
        assert!(verify(&root, &source, &canonical).is_err());
        std::fs::remove_file(pending(&root)).unwrap();
        std::fs::write(&canonical, "changed").unwrap();
        assert!(verify(&root, &source, &canonical).is_err());
        std::fs::write(&canonical, subtitle::to_srt(&entries)).unwrap();
        std::fs::write(&source, b"replacement with different identity").unwrap();
        assert!(verify(&root, &source, &canonical)
            .unwrap_err()
            .contains("其他原片"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unbound_history_requires_matching_episode_and_does_not_rewrite_sidecar() {
        let (root, source, canonical, entries) = fixture();
        let raw = br#"{"source":"platform"}"#;
        std::fs::write(sidecar(&root), raw).unwrap();
        assert!(verify(&root, &source, &canonical).is_err());
        std::fs::create_dir_all(root.join("episode")).unwrap();
        std::fs::write(root.join("episode/episode.json"),serde_json::to_vec(&serde_json::json!({"source_revision":source_revision(&source).unwrap(),"transcript_revision":script::transcript_revision(&entries)})).unwrap()).unwrap();
        verify(&root, &source, &canonical).unwrap();
        assert_eq!(std::fs::read(sidecar(&root)).unwrap(), raw);
        std::fs::write(&source, b"different source identity").unwrap();
        assert!(verify(&root, &source, &canonical).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn legacy_translation_is_not_reused_after_quarantine() {
        let (root, source, canonical, _) = fixture();
        std::fs::write(sidecar(&root), br#"{"source":"asr"}"#).unwrap();
        assert!(recover_legacy_translation(&root, &source, &canonical));
        std::fs::create_dir_all(root.join("rejected-sources")).unwrap();
        assert!(!recover_legacy_translation(&root, &source, &canonical));
        std::fs::remove_dir_all(root).unwrap();
    }
}
