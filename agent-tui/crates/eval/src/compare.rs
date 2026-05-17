//! Comparison mode: produce a markdown diff table from two `EvalRun` files.

use crate::{EvalRun, InstanceResult};
use std::collections::HashMap;

/// Compare two eval runs and return a markdown-formatted diff table.
///
/// Output sections:
///   1. Aggregate metrics table (pass rate, cost, turns, wallclock).
///   2. Per-instance flip list (A passed / B failed and vice versa).
pub fn compare_runs(a: &EvalRun, b: &EvalRun) -> String {
    let mut out = String::new();

    out.push_str("## Comparison: eval run A vs eval run B\n\n");
    out.push_str(&format!(
        "| Config | Run A | Run B |\n|--------|-------|-------|\n\
         | provider | {} | {} |\n\
         | model | {} | {} |\n\
         | scale | {} | {} |\n\
         | time_budget_s | {} | {} |\n\n",
        a.config.provider,
        b.config.provider,
        a.config.model,
        b.config.model,
        a.config.scale,
        b.config.scale,
        a.config.time_budget_secs,
        b.config.time_budget_secs,
    ));

    out.push_str("## Aggregate metrics\n\n");
    out.push_str("| Metric | Run A | Run B | Delta |\n");
    out.push_str("|--------|-------|-------|-------|\n");

    let pa = a.summary.pass_at_1;
    let pb = b.summary.pass_at_1;
    out.push_str(&format!(
        "| pass@1 | {:.3} | {:.3} | {:+.3} |\n",
        pa, pb, pb - pa
    ));

    let sa = a.summary.semantic_pass_at_1;
    let sb = b.summary.semantic_pass_at_1;
    out.push_str(&format!(
        "| semantic_pass@1 | {:.3} | {:.3} | {:+.3} |\n",
        sa, sb, sb - sa
    ));

    let ca = a.summary.mean_cost_usd;
    let cb = b.summary.mean_cost_usd;
    out.push_str(&format!(
        "| mean_cost_usd | ${:.4} | ${:.4} | {:+.4} |\n",
        ca, cb, cb - ca
    ));

    let ta = a.summary.mean_turns;
    let tb = b.summary.mean_turns;
    out.push_str(&format!(
        "| mean_turns | {:.1} | {:.1} | {:+.1} |\n",
        ta, tb, tb - ta
    ));

    let wa = a.summary.mean_wallclock_s;
    let wb = b.summary.mean_wallclock_s;
    out.push_str(&format!(
        "| mean_wallclock_s | {:.1} | {:.1} | {:+.1} |\n",
        wa, wb, wb - wa
    ));

    out.push('\n');

    // ── Per-instance flips ─────────────────────────────────────────────────
    let a_map: HashMap<&str, &InstanceResult> =
        a.instances.iter().map(|i| (i.instance_id.as_str(), i)).collect();
    let b_map: HashMap<&str, &InstanceResult> =
        b.instances.iter().map(|i| (i.instance_id.as_str(), i)).collect();

    let mut a_pass_b_fail: Vec<&str> = Vec::new();
    let mut b_pass_a_fail: Vec<&str> = Vec::new();
    let mut both_pass: Vec<&str> = Vec::new();
    let mut both_fail: Vec<&str> = Vec::new();

    // Collect all instance ids from both runs.
    let mut all_ids: Vec<&str> = a_map.keys().copied().chain(b_map.keys().copied()).collect();
    all_ids.sort_unstable();
    all_ids.dedup();

    for id in all_ids {
        let a_pass = a_map.get(id).map(|r| r.pass_at_1).unwrap_or(false);
        let b_pass = b_map.get(id).map(|r| r.pass_at_1).unwrap_or(false);
        match (a_pass, b_pass) {
            (true, false)  => a_pass_b_fail.push(id),
            (false, true)  => b_pass_a_fail.push(id),
            (true, true)   => both_pass.push(id),
            (false, false) => both_fail.push(id),
        }
    }

    out.push_str("## Per-instance flips\n\n");

    out.push_str(&format!("**Both pass** ({})\n", both_pass.len()));
    if !both_pass.is_empty() {
        for id in &both_pass { out.push_str(&format!("- {id}\n")); }
    }
    out.push('\n');

    out.push_str(&format!("**Both fail** ({})\n", both_fail.len()));
    if !both_fail.is_empty() {
        for id in &both_fail { out.push_str(&format!("- {id}\n")); }
    }
    out.push('\n');

    out.push_str(&format!("**A passed, B failed** ({})\n", a_pass_b_fail.len()));
    if !a_pass_b_fail.is_empty() {
        for id in &a_pass_b_fail { out.push_str(&format!("- {id}: A=pass, B=fail\n")); }
    }
    out.push('\n');

    out.push_str(&format!("**B passed, A failed** ({})\n", b_pass_a_fail.len()));
    if !b_pass_a_fail.is_empty() {
        for id in &b_pass_a_fail { out.push_str(&format!("- {id}: A=fail, B=pass\n")); }
    }
    out.push('\n');

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EvalConfig, EvalRun, EvalSummary, InstanceResult};

    fn make_run(
        provider: &str,
        model: &str,
        scale: u32,
        instances: Vec<InstanceResult>,
    ) -> EvalRun {
        let config = EvalConfig {
            bench: "test".into(),
            subset_n: instances.len() as u32,
            provider: provider.into(),
            model: model.into(),
            scale,
            time_budget_secs: 60,
        };
        EvalRun::compute_summary(config, instances)
    }

    fn inst(id: &str, pass: bool) -> InstanceResult {
        InstanceResult {
            instance_id: id.into(),
            pass_at_1: pass,
            semantic_pass: Some(pass),
            cost_usd: 1.0,
            turns: 5,
            wallclock_s: 30.0,
            patch_diff: None,
        }
    }

    #[test]
    fn compare_produces_markdown_headers() {
        let a = make_run("anthropic", "claude-opus", 1, vec![inst("x", true), inst("y", false)]);
        let b = make_run("openai", "gpt-4o", 4, vec![inst("x", false), inst("y", true)]);
        let md = compare_runs(&a, &b);
        assert!(md.contains("## Comparison"), "should have comparison header");
        assert!(md.contains("pass@1"), "should include pass@1 metric");
        assert!(md.contains("semantic_pass@1"), "should include semantic metric");
        assert!(md.contains("Per-instance flips"), "should have flip section");
    }

    #[test]
    fn compare_detects_flips() {
        let a = make_run("m", "m", 1, vec![inst("alpha", true), inst("beta", false)]);
        let b = make_run("m", "m", 1, vec![inst("alpha", false), inst("beta", true)]);
        let md = compare_runs(&a, &b);
        assert!(md.contains("A passed, B failed"), "should flag A→fail flip");
        assert!(md.contains("B passed, A failed"), "should flag B→pass flip");
        assert!(md.contains("alpha: A=pass, B=fail"));
        assert!(md.contains("beta: A=fail, B=pass"));
    }

    #[test]
    fn compare_pass_rate_delta_is_correct() {
        // A: 2/2 pass (1.0), B: 1/2 pass (0.5) → delta = -0.5
        let a = make_run("m", "m", 1, vec![inst("x", true), inst("y", true)]);
        let b = make_run("m", "m", 1, vec![inst("x", true), inst("y", false)]);
        let md = compare_runs(&a, &b);
        assert!(md.contains("-0.500"), "pass@1 delta should be -0.500");
    }
}
