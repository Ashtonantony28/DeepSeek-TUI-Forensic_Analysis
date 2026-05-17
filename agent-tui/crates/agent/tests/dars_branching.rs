//! 3.8 — `Op::SpawnSubAgent` triggers a DARS branching pass and the
//! engine emits a `DarsResult` event with majority-vote breakdown.

use agent_tui_agent::{Engine, Session};
use agent_tui_llm::{LlmClient, MockClient};
use agent_tui_protocol::{Event, Op};
use agent_tui_tools::ToolRegistry;
use camino::Utf8PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn spawn_subagent_runs_dars_and_emits_result() {
    let td = tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    let mock = Arc::new(MockClient::new());
    // 3 branches
    mock.push_text("draft alpha");
    mock.push_text("draft beta");
    mock.push_text("draft gamma");
    // 3 verifier votes — 2 for index 2, 1 for index 0
    mock.push_text("BEST: 2");
    mock.push_text("BEST: 0");
    mock.push_text("BEST: 2");

    let (reg, ctx) = ToolRegistry::with_builtins(root);
    let mut engine = Engine::new(
        Session::new("mock-model"),
        mock.clone() as Arc<dyn LlmClient>,
        Arc::new(reg),
        Arc::new(ctx),
    );
    engine.dars_enabled = true;
    engine.dars_branch_count = 3;
    engine.dars_verifier_count = 3;
    let h = engine.spawn();
    h.send(Op::SpawnSubAgent {
        prompt: "what is the canonical fix for X?".into(),
    })
    .await
    .unwrap();

    let mut winner_index: Option<usize> = None;
    let mut votes: Vec<usize> = Vec::new();
    let mut winner_text = String::new();
    while let Some(ev) = h.next_event().await {
        match ev {
            Event::Delta { delta, .. } => winner_text.push_str(&delta),
            Event::DarsResult {
                winner_index: w,
                votes: v,
                ..
            } => {
                winner_index = Some(w);
                votes = v;
            }
            Event::TurnComplete { .. } => break,
            _ => {}
        }
    }
    assert_eq!(
        winner_index,
        Some(2),
        "expected the majority winner (index 2)"
    );
    assert_eq!(votes.len(), 3);
    assert_eq!(winner_text, "draft gamma");
}

#[tokio::test]
async fn dars_disabled_yields_status_message() {
    let td = tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    let mock = Arc::new(MockClient::new());
    let (reg, ctx) = ToolRegistry::with_builtins(root);
    let mut engine = Engine::new(
        Session::new("mock-model"),
        mock.clone() as Arc<dyn LlmClient>,
        Arc::new(reg),
        Arc::new(ctx),
    );
    engine.dars_enabled = false;
    let h = engine.spawn();
    h.send(Op::SpawnSubAgent {
        prompt: "anything".into(),
    })
    .await
    .unwrap();
    let mut saw_dars = false;
    let mut saw_disabled_status = false;
    // Drain a bounded number of events; without DARS we won't get
    // TurnComplete for this op, so cap the loop.
    for _ in 0..6 {
        let Some(ev) = h.next_event().await else {
            break;
        };
        if matches!(ev, Event::DarsResult { .. }) {
            saw_dars = true;
        }
        if let Event::Status { message, .. } = ev {
            if message.contains("dars disabled") {
                saw_disabled_status = true;
                break;
            }
        }
    }
    assert!(!saw_dars);
    assert!(saw_disabled_status);
}
