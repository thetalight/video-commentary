use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::services::{jobs, task_store};

pub const PROMPT_VERSION: &str = "director-v14-dialogue-attribution-no-asr";
const BEAT_COMPATIBLE_PROMPT_VERSIONS: &[&str] = &[];

fn input_revision(
    transcript: &[u8],
    provider: &str,
    model: &str,
    style: &str,
    prompt_version: &str,
) -> String {
    jobs::stable_hash(&[
        transcript,
        provider.as_bytes(),
        model.as_bytes(),
        style.as_bytes(),
        prompt_version.as_bytes(),
    ])
}

fn seed_compatible_beats(
    director_root: &Path,
    destination: &Path,
    transcript: &[u8],
    provider: &str,
    model: &str,
    style: &str,
) -> Result<(), String> {
    if destination.exists() {
        return Ok(());
    }
    for prompt_version in BEAT_COMPATIBLE_PROMPT_VERSIONS {
        let prior_revision = input_revision(transcript, provider, model, style, prompt_version);
        let source = director_root
            .join("runs")
            .join(prior_revision)
            .join("beats.json");
        let Ok(raw) = std::fs::read(source) else {
            continue;
        };
        let valid = serde_json::from_slice::<serde_json::Value>(&raw)
            .ok()
            .and_then(|value| value.as_array().map(|items| !items.is_empty()))
            .unwrap_or(false);
        if valid {
            jobs::atomic_write(destination, raw).map_err(|error| error.to_string())?;
            break;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointMeta {
    schema_version: u32,
    prompt_version: String,
    input_revision: String,
    provider: String,
    model: String,
    style: String,
}

#[derive(Debug, Clone)]
pub struct DirectorCheckpoint {
    task_id: String,
    root: PathBuf,
    input_revision: String,
}

impl DirectorCheckpoint {
    pub fn open(
        task_id: &str,
        director_root: &Path,
        transcript: &[u8],
        provider: &str,
        model: &str,
        style: &str,
    ) -> Result<Self, String> {
        let input_revision = input_revision(transcript, provider, model, style, PROMPT_VERSION);
        let root = director_root.join("runs").join(&input_revision);
        std::fs::create_dir_all(root.join("chapters")).map_err(|error| error.to_string())?;
        seed_compatible_beats(
            director_root,
            &root.join("beats.json"),
            transcript,
            provider,
            model,
            style,
        )?;
        let meta = CheckpointMeta {
            schema_version: 1,
            prompt_version: PROMPT_VERSION.to_string(),
            input_revision: input_revision.clone(),
            provider: provider.to_string(),
            model: model.to_string(),
            style: style.to_string(),
        };
        let raw = serde_json::to_vec_pretty(&meta).map_err(|error| error.to_string())?;
        jobs::atomic_write(root.join("checkpoint.json"), raw).map_err(|error| error.to_string())?;
        Ok(Self {
            task_id: task_id.to_string(),
            root,
            input_revision,
        })
    }

    pub fn read<T: DeserializeOwned>(&self, relative: impl AsRef<Path>) -> Option<T> {
        let raw = std::fs::read(self.root.join(relative)).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    pub fn write<T: Serialize>(
        &self,
        kind: &str,
        relative: impl AsRef<Path>,
        value: &T,
    ) -> Result<(), String> {
        let relative = relative.as_ref();
        let path = self.root.join(relative);
        let raw = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
        let revision = jobs::stable_hash(&[&raw]);
        jobs::atomic_write(&path, &raw).map_err(|error| error.to_string())?;
        let _ = task_store::artifact(
            &self.task_id,
            &format!("{kind}:{}", relative.to_string_lossy()),
            &path.to_string_lossy(),
            &revision,
            &self.input_revision,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "video-commentary-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn incompatible_prompt_revision_does_not_reuse_old_beats() {
        let root = scratch_dir("checkpoint-isolation");
        let transcript = b"same transcript";
        let provider = "ollama";
        let model = "qwen3.8:27b-mlx";
        let style = "cinematic";
        let old_revision = input_revision(transcript, provider, model, style, "director-v12");
        let old_root = root.join("runs").join(old_revision);
        std::fs::create_dir_all(&old_root).expect("create old run");
        std::fs::write(old_root.join("beats.json"), br#"[{"id":"B0001"}]"#)
            .expect("write old beats");
        let destination = root.join("runs/current/beats.json");
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("create current run");

        seed_compatible_beats(&root, &destination, transcript, provider, model, style)
            .expect("seed beats");

        assert!(!destination.exists());
        std::fs::remove_dir_all(root).expect("clean scratch directory");
    }
}
