//! Optional local scene-change candidate detection. No model, image upload or story inference.
use super::{ffmpeg, jobs, stage_manifest};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Command};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneIndex {
    pub version: u32,
    pub source_revision: String,
    pub method: String,
    pub duration: f64,
    pub cut_candidates: Vec<f64>,
    pub warnings: Vec<String>,
}
fn parse(log: &str, duration: f64) -> Vec<f64> {
    let pattern = regex::Regex::new(r"pts_time:\s*([0-9]+(?:\.[0-9]+)?)").unwrap();
    let mut cuts: Vec<f64> = pattern
        .captures_iter(log)
        .filter_map(|c| c[1].parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0 && *v < duration)
        .collect();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 0.001);
    cuts
}
pub fn detect(source: &Path, output: &Path) -> Result<SceneIndex, String> {
    let revision = stage_manifest::file_revision(source)?;
    if let Ok(bytes) = std::fs::read(output) {
        if let Ok(index) = serde_json::from_slice::<SceneIndex>(&bytes) {
            if index.version == 1 && index.source_revision == revision {
                return Ok(index);
            }
        }
    }
    let duration = ffmpeg::get_duration(source).map_err(|e| e.to_string())?;
    let binary = ffmpeg::resolve_ffmpeg_bin().ok_or("未找到FFmpeg")?;
    let result = Command::new(binary)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(source)
        .args([
            "-an",
            "-vf",
            "select='gt(scene,0.35)',showinfo",
            "-f",
            "null",
            "-",
        ])
        .output()
        .map_err(|_| "无法启动本机切点检测".to_string())?;
    if !result.status.success() {
        return Err("本机切点检测失败，未写成功索引".into());
    }
    let index=SceneIndex{version:1,source_revision:revision,duration,method:"ffmpeg-scene-0.35-pts-candidates-v1".into(),cut_candidates:parse(&String::from_utf8_lossy(&result.stderr),duration),warnings:vec!["切点是基于像素变化的候选，闪光/运动可能误报，叠化/暗场可能漏报；不等于剧情或VFR完整帧索引，不自动挪动必保/对白边界。".into()]};
    jobs::atomic_write(
        output,
        serde_json::to_vec_pretty(&index).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(index)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cut_times_use_pts_not_nominal_fps_and_are_unique() {
        assert_eq!(
            parse(
                "pts_time:1.001 pts_time:1.001 pts_time:3.125 pts_time:11.0",
                10.0
            ),
            vec![1.001, 3.125]
        );
    }
}
