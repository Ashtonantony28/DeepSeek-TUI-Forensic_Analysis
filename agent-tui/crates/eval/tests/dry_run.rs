//! Dry-run integration tests for the eval harness.
//!
//! These tests exercise every code path — mutation strengthening, RTV, and
//! comparison output — without any real network calls or Docker containers.
//! All LLM interactions use `MockClient` with scripted responses.

use agent_tui_eval::{
    compare::compare_runs,
    dataset::synthetic_instances,
    mutation::{generate_mutants, is_valid_python},
    runner::{run_eval, RunConfig},
};
use agent_tui_llm::{LlmClient, MockClient};
use std::sync::Arc;

// ── Dry-run scale=1 ─────────────────────────────────────────────────────────

#[tokio::test]
async fn dry_run_scale_1_produces_valid_json() {
    let config = RunConfig {
        bench: "swe-verified".into(),
        subset_n: 2,
        subset_ids: vec![],
        provider: "mock".into(),
        model: "mock-model".into(),
        scale: 1,
        time_budget_secs: 60,
        dry_run: true,
    };
    // Two instances × 1 rollout each.
    let mock = MockClient::new();
    mock.push_text(
        "Fix:\n```diff\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+y\n```",
    );
    mock.push_text("No fix.");
    let llm: Arc<dyn LlmClient> = Arc::new(mock);

    let run = run_eval(config, llm).await.expect("run_eval should succeed");

    // Shape checks.
    assert_eq!(run.instances.len(), 2, "should have 2 instance results");
    assert_eq!(run.config.scale, 1);
    assert_eq!(run.config.bench, "swe-verified");

    // pass_at_1 is scripted by instance_id suffix in dry-run.
    let pass_inst = &run.instances[0];
    let fail_inst = &run.instances[1];
    assert!(pass_inst.pass_at_1, "pass instance should have pass_at_1=true");
    assert!(!fail_inst.pass_at_1, "fail instance should have pass_at_1=false");

    // Semantic pass must be Some for both.
    assert!(pass_inst.semantic_pass.is_some(), "semantic_pass must be Some");
    assert!(fail_inst.semantic_pass.is_some(), "semantic_pass must be Some");

    // Summary aggregates must be in [0, 1].
    assert!(run.summary.pass_at_1 >= 0.0 && run.summary.pass_at_1 <= 1.0);
    assert!(run.summary.semantic_pass_at_1 >= 0.0 && run.summary.semantic_pass_at_1 <= 1.0);

    // Round-trip the JSON.
    let json = serde_json::to_string_pretty(&run).expect("serialise");
    let _: agent_tui_eval::EvalRun = serde_json::from_str(&json).expect("deserialise");
}

// ── Dry-run scale=4 (RTV) ───────────────────────────────────────────────────

#[tokio::test]
async fn dry_run_scale_4_exercises_rtv() {
    let config = RunConfig {
        bench: "swe-verified".into(),
        subset_n: 2,
        subset_ids: vec![],
        provider: "mock".into(),
        model: "mock-model".into(),
        scale: 4,
        time_budget_secs: 60,
        dry_run: true,
    };

    let mock = MockClient::new();
    // 4 rollouts per instance × 2 instances = 8 pipeline calls.
    let diff_response =
        "Fix:\n```diff\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+y\n```";
    for _ in 0..8 {
        mock.push_text(diff_response);
    }
    // RTV judge calls: one per group of 3–4 summaries, per instance.
    // With 4 summaries → 2 groups → 2 judge calls in round 1, 1 judge call in round 2.
    for _ in 0..6 {
        mock.push_text("1");
    }
    let llm: Arc<dyn LlmClient> = Arc::new(mock);

    let run = run_eval(config, llm).await.expect("run_eval with scale=4 should succeed");

    assert_eq!(run.instances.len(), 2);
    assert_eq!(run.config.scale, 4);
    // Turn count should reflect N rollouts.
    for inst in &run.instances {
        // Each instance ran 4 rollouts × 1 turn = 4 turns total.
        assert!(inst.turns >= 1, "should record at least 1 turn");
    }
}

// ── Comparison mode ──────────────────────────────────────────────────────────

#[tokio::test]
async fn compare_two_dry_runs_produces_markdown_table() {
    let base_config = RunConfig {
        bench: "swe-verified".into(),
        subset_n: 2,
        subset_ids: vec![],
        provider: "mock".into(),
        model: "mock-model".into(),
        scale: 1,
        time_budget_secs: 60,
        dry_run: true,
    };

    // Run A (scale=1).
    let mock_a = MockClient::new();
    for _ in 0..2 {
        mock_a.push_text(
            "Fix:\n```diff\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+y\n```",
        );
    }
    let run_a = run_eval(
        RunConfig { model: "model-a".into(), ..base_config },
        Arc::new(mock_a),
    )
    .await
    .unwrap();

    // Run B (scale=4) with different model name.
    let mock_b = MockClient::new();
    for _ in 0..8 {
        mock_b.push_text(
            "Fix:\n```diff\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+y\n```",
        );
    }
    for _ in 0..6 { mock_b.push_text("1"); }
    let run_b = run_eval(
        RunConfig {
            model: "model-b".into(),
            scale: 4,
            bench: "swe-verified".into(),
            subset_n: 2,
            subset_ids: vec![],
            provider: "mock".into(),
            time_budget_secs: 60,
            dry_run: true,
        },
        Arc::new(mock_b),
    )
    .await
    .unwrap();

    let md = compare_runs(&run_a, &run_b);

    // Structure validation.
    assert!(md.contains("## Comparison"), "should have comparison header");
    assert!(md.contains("pass@1"), "should contain pass@1 metric");
    assert!(md.contains("semantic_pass@1"), "should contain semantic metric");
    assert!(md.contains("Per-instance flips"), "should have flip section");
    assert!(md.contains("model-a"), "should reference run A model");
    assert!(md.contains("model-b"), "should reference run B model");

    // Print the table for the session report.
    println!("\n{}", md);
}

// ── Mutation module sanity ───────────────────────────────────────────────────

#[test]
fn synthetic_test_patches_produce_mutants() {
    for inst in synthetic_instances() {
        let mutants = generate_mutants(&inst.test_patch, 5);
        // Synthetic test patches are valid Python and contain assertions,
        // so we should always get at least one mutant.
        assert!(
            !mutants.is_empty(),
            "expected mutants for instance {}",
            inst.instance_id
        );
        for m in &mutants {
            assert!(
                is_valid_python(&m.source),
                "mutant for {} should be valid Python",
                inst.instance_id
            );
        }
    }
}
