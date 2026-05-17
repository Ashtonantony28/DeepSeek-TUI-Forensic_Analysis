//! Evaluation harness for agent-tui.
//!
//! Phase 4 — SWE-bench Verified driver, Docker/git isolation, mutation-
//! strengthened test evaluation, RTV test-time scaling, and comparison mode.
//!
//! The data shapes from Phase 2 (`EvalConfig`, `InstanceResult`, `EvalSummary`,
//! `EvalRun`) are kept here; all pipeline logic lives in the sub-modules.

pub mod compare;
pub mod dataset;
pub mod isolation;
pub mod mutation;
pub mod rtv;
pub mod runner;

pub use compare::compare_runs;
pub use dataset::{synthetic_instances, SweInstance};
pub use isolation::{detect_isolation, IsolationMode};
pub use mutation::{generate_mutants, Mutant, MutationOperator};
pub use rtv::{rtv_vote, RolloutSummary};
pub use runner::{run_eval, RunConfig};

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
    /// None when mutation could not run (unsupported language or parse failure).
    pub semantic_pass: Option<bool>,
    pub cost_usd: f32,
    pub turns: u32,
    pub wallclock_s: f32,
    pub patch_diff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSummary {
    pub pass_at_1: f32,
    pub semantic_pass_at_1: f32,
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

impl EvalRun {
    pub fn compute_summary(config: EvalConfig, instances: Vec<InstanceResult>) -> Self {
        let n = instances.len() as f32;
        let pass_count = instances.iter().filter(|i| i.pass_at_1).count() as f32;
        let sem_eligible: Vec<_> = instances.iter().filter(|i| i.semantic_pass.is_some()).collect();
        let sem_pass = sem_eligible.iter().filter(|i| i.semantic_pass == Some(true)).count() as f32;
        let sem_total = sem_eligible.len() as f32;

        let summary = EvalSummary {
            pass_at_1: if n > 0.0 { pass_count / n } else { 0.0 },
            semantic_pass_at_1: if sem_total > 0.0 { sem_pass / sem_total } else { 0.0 },
            mean_cost_usd: if n > 0.0 {
                instances.iter().map(|i| i.cost_usd).sum::<f32>() / n
            } else { 0.0 },
            mean_turns: if n > 0.0 {
                instances.iter().map(|i| i.turns as f32).sum::<f32>() / n
            } else { 0.0 },
            mean_wallclock_s: if n > 0.0 {
                instances.iter().map(|i| i.wallclock_s).sum::<f32>() / n
            } else { 0.0 },
        };
        Self { config, summary, instances, timestamp: chrono::Utc::now() }
    }
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
                semantic_pass_at_1: 0.3,
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

    #[test]
    fn compute_summary_pass_rate() {
        let config = EvalConfig {
            bench: "test".into(),
            subset_n: 2,
            provider: "mock".into(),
            model: "mock".into(),
            scale: 1,
            time_budget_secs: 60,
        };
        let instances = vec![
            InstanceResult {
                instance_id: "a".into(),
                pass_at_1: true,
                semantic_pass: Some(true),
                cost_usd: 1.0,
                turns: 10,
                wallclock_s: 100.0,
                patch_diff: None,
            },
            InstanceResult {
                instance_id: "b".into(),
                pass_at_1: false,
                semantic_pass: Some(false),
                cost_usd: 0.5,
                turns: 5,
                wallclock_s: 50.0,
                patch_diff: None,
            },
        ];
        let run = EvalRun::compute_summary(config, instances);
        assert!((run.summary.pass_at_1 - 0.5).abs() < 0.01);
        assert!((run.summary.semantic_pass_at_1 - 0.5).abs() < 0.01);
        assert!((run.summary.mean_cost_usd - 0.75).abs() < 0.01);
    }
}
