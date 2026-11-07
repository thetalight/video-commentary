//! Content-addressed local stage receipts. A receipt alone never proves a valid output.
use super::jobs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub version: u32,
    pub stage: String,
    pub input_revision: String,
    pub output: PathBuf,
    pub bytes: u64,
    pub modified_ns: u128,
}
pub fn file_revision(path: &Path) -> Result<String, String> {
    let m = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !m.is_file() || m.len() == 0 {
        return Err("阶段输入/产物为空".into());
    }
    let modified = m
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(jobs::stable_hash(&[
        path.to_string_lossy().as_bytes(),
        &m.len().to_le_bytes(),
        &modified.to_le_bytes(),
    ]))
}
pub fn reusable(receipt: &Path, stage: &str, input: &str, output: &Path) -> bool {
    let Ok(raw) = std::fs::read(receipt) else {
        return false;
    };
    let Ok(r) = serde_json::from_slice::<Receipt>(&raw) else {
        return false;
    };
    let Ok(m) = std::fs::metadata(output) else {
        return false;
    };
    let modified = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|t| t.as_nanos());
    r.version == 1
        && r.stage == stage
        && r.input_revision == input
        && r.output == output
        && m.is_file()
        && m.len() > 0
        && r.bytes == m.len()
        && Some(r.modified_ns) == modified
}
pub fn complete(receipt: &Path, stage: &str, input: &str, output: &Path) -> Result<(), String> {
    let m = std::fs::metadata(output).map_err(|e| e.to_string())?;
    if !m.is_file() || m.len() == 0 {
        return Err("不记录空阶段产物为完成".into());
    }
    let r = Receipt {
        version: 1,
        stage: stage.into(),
        input_revision: input.into(),
        output: output.into(),
        bytes: m.len(),
        modified_ns: m
            .modified()
            .map_err(|e| e.to_string())?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos(),
    };
    jobs::atomic_write(
        receipt,
        serde_json::to_vec_pretty(&r).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipts_require_same_inputs_and_unchanged_output() {
        let root = std::env::temp_dir().join(format!("stage-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let out = root.join("output");
        let receipt = root.join("receipt.json");
        std::fs::write(&out, "valid").unwrap();
        complete(&receipt, "render", "a", &out).unwrap();
        assert!(reusable(&receipt, "render", "a", &out));
        assert!(!reusable(&receipt, "render", "b", &out));
        std::fs::write(&out, "changed output").unwrap();
        assert!(!reusable(&receipt, "render", "a", &out));
        std::fs::remove_file(&out).unwrap();
        assert!(!reusable(&receipt, "render", "a", &out));
        std::fs::remove_dir_all(root).unwrap();
    }
}
