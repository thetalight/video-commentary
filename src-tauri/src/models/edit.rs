use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPlan {
    pub title: String,
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub target_duration_secs: f64,
    pub segments: Vec<EditSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditSegment {
    /// Explicit source pictures for one spoken paragraph; empty means legacy single range.
    #[serde(default)]
    pub shots: Vec<[f64; 2]>,
    pub src_start: f64,
    pub src_end: f64,
    pub narration: String,
    #[serde(default)]
    pub keep_original_audio: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("edit.json 无效: {0}")]
    Invalid(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl EditPlan {
    pub fn from_json_str(raw: &str) -> Result<Self, EditError> {
        let trimmed = raw.trim();
        let json = extract_json_object(trimmed);
        let plan: EditPlan = serde_json::from_str(&json)?;
        if plan.segments.is_empty() {
            return Err(EditError::Invalid("segments 为空".into()));
        }
        Ok(plan)
    }

    pub fn validate_and_clamp(&mut self, duration: f64) -> Result<(), EditError> {
        if self.segments.is_empty() {
            return Err(EditError::Invalid("至少需要一个片段".into()));
        }
        for (i, seg) in self.segments.iter_mut().enumerate() {
            let mut previous = seg.src_start;
            for &[start, end] in &seg.shots {
                if !start.is_finite()
                    || !end.is_finite()
                    || start < previous
                    || end - start < 0.8
                    || end > seg.src_end
                    || (duration > 0.0 && end > duration)
                {
                    return Err(EditError::Invalid(format!(
                        "第 {} 段多镜头时间无效、重叠或超出本段范围",
                        i + 1
                    )));
                }
                previous = end;
            }
            if seg.keep_original_audio && !seg.shots.is_empty() {
                return Err(EditError::Invalid(
                    "原声接力段应使用一个连续画面区间".into(),
                ));
            }
            if !seg.src_start.is_finite() || !seg.src_end.is_finite() {
                return Err(EditError::Invalid(format!("片段 {i} 时间戳无效")));
            }
            seg.src_start = seg.src_start.max(0.0);
            if duration > 0.0 {
                seg.src_end = seg.src_end.min(duration);
            }
            if seg.src_end - seg.src_start < 0.4 {
                return Err(EditError::Invalid(format!(
                    "片段 {i} 过短（{}s–{}s）",
                    seg.src_start, seg.src_end
                )));
            }
            seg.narration = seg.narration.trim().to_string();
            if seg.narration.is_empty() {
                return Err(EditError::Invalid(format!("片段 {i} 解说词为空")));
            }
        }
        self.target_duration_secs = estimate_spoken_secs(&self.segments);
        if self.title.trim().is_empty() {
            self.title = "解说成片".to_string();
        }
        Ok(())
    }

    pub fn commentary_markdown(&self) -> String {
        let mut out = format!("# {}\n\n", self.title.trim());
        if !self.style.trim().is_empty() {
            out.push_str(&format!("风格：{}\n\n", self.style.trim()));
        }
        for (i, seg) in self.segments.iter().enumerate() {
            let orig = if seg.keep_original_audio {
                "，保留原声"
            } else {
                ""
            };
            out.push_str(&format!(
                "## 片段 {}（{:.1}s – {:.1}s{}）\n\n{}\n\n",
                i + 1,
                seg.src_start,
                seg.src_end,
                orig,
                seg.narration.trim()
            ));
        }
        out
    }
}

fn estimate_spoken_secs(segments: &[EditSegment]) -> f64 {
    let chars: usize = segments.iter().map(|s| s.narration.chars().count()).sum();
    (chars as f64 / 4.0).max(1.0)
}

fn extract_json_object(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                return trimmed[start..=end].to_string();
            }
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plan() {
        let raw = r#"{"title":"t","style":"吐槽解说","target_duration_secs":90,"segments":[{"src_start":1.0,"src_end":8.0,"narration":"你好"}]}"#;
        let mut plan = EditPlan::from_json_str(raw).unwrap();
        plan.validate_and_clamp(60.0).unwrap();
        assert_eq!(plan.segments.len(), 1);
        assert!(plan.target_duration_secs > 0.0);
    }
}
