//! Evaluation harness types.
//!
//! Phase 2 only ships the data shapes. The SWE-bench Verified driver
//! and the mutation-strengthening loop arrive in Phase 4.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalConfig {
    pub bench: String,
    pub subset_n: u32,
    pub provider: String,
    pub model: String,
    pub scale: u32,
    pub time_budget_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceResult {
    pub instance_id: String,
    pub pass_at_1: bool,
    pub semantic_pass: Option<bool>,
    pub cost_usd: f32,
    pub turns: u32,
    pub wallclock_s: f32,
    pub patch_diff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSummary {
    pub pass_at_1: f32,
    pub semantic_pass_at_1: Option<f32>,
    pub mean_cost_usd: f32,
    pub mean_turns: f32,
    pub mean_wallclock_s: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRun {
    pub config: EvalConfig,
    pub summary: EvalSummary,
    pub instances: Vec<InstanceResult>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub fn output_path() -> Utf8PathBuf {
    Utf8PathBuf::from(format!(
        "eval-runs/{}.json",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shapes_roundtrip() {
        let run = EvalRun {
            config: EvalConfig {
                bench: "swe-verified".into(),
                subset_n: 5,
                provider: "anthropic".into(),
                model: "claude-opus-4-7".into(),
                scale: 1,
                time_budget_secs: 600,
            },
            summary: EvalSummary {
                pass_at_1: 0.4,
                semantic_pass_at_1: Some(0.3),
                mean_cost_usd: 0.5,
                mean_turns: 8.0,
                mean_wallclock_s: 120.0,
            },
            instances: vec![],
            timestamp: chrono::Utc::now(),
        };
        let s = serde_json::to_string(&run).unwrap();
        let _: EvalRun = serde_json::from_str(&s).unwrap();
    }
}
