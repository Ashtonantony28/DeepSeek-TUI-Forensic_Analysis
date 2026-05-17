//! 3.7 — when the model calls `update_plan`, the engine emits a
//! structured `PlanUpdated` event that the TUI side panel can render.

use agent_tui_agent::{Engine, Session};
use agent_tui_llm::{LlmClient, MockClient};
use agent_tui_protocol::{AppMode, Event, Op};
use agent_tui_tools::ToolRegistry;
use camino::Utf8PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn update_plan_tool_emits_plan_event() {
    let td = tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    let mock = Arc::new(MockClient::new());
    mock.push_tool_call(
        "update_plan",
        serde_json::json!({
            "goal": "wire-up the new parser",
            "steps": [
                "read existing tokenizer",
                "draft new parser interface",
                "migrate callers",
            ]
        }),
    );
    mock.push_text("plan registered.");
    let (reg, mut ctx) = ToolRegistry::with_builtins(root);
    ctx.yolo = true;
    let mut engine = Engine::new(
        Session::new("mock-model"),
        mock.clone() as Arc<dyn LlmClient>,
        Arc::new(reg),
        Arc::new(ctx),
    );
    engine.plan_blocks_enabled = true;
    engine.memory_enabled = false;
    let h = engine.spawn();
    h.send(Op::Submit {
        content: "make a plan".into(),
        mode: AppMode::Yolo,
        model: None,
        provider: None,
    })
    .await
    .unwrap();

    let mut goal: Option<String> = None;
    let mut item_count = 0;
    while let Some(ev) = h.next_event().await {
        if let Event::PlanUpdated { goal: g, items, .. } = &ev {
            goal = Some(g.clone());
            item_count = items.len();
        }
        if matches!(ev, Event::TurnComplete { .. }) {
            break;
        }
    }
    assert_eq!(goal.as_deref(), Some("wire-up the new parser"));
    assert_eq!(item_count, 3);
}

#[tokio::test]
async fn plan_blocks_disabled_suppresses_event() {
    let td = tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    let mock = Arc::new(MockClient::new());
    mock.push_tool_call(
        "update_plan",
        serde_json::json!({"goal":"x","steps":["a","b"]}),
    );
    mock.push_text("ok");
    let (reg, mut ctx) = ToolRegistry::with_builtins(root);
    ctx.yolo = true;
    let mut engine = Engine::new(
        Session::new("mock-model"),
        mock.clone() as Arc<dyn LlmClient>,
        Arc::new(reg),
        Arc::new(ctx),
    );
    engine.plan_blocks_enabled = false;
    engine.memory_enabled = false;
    let h = engine.spawn();
    h.send(Op::Submit {
        content: "go".into(),
        mode: AppMode::Yolo,
        model: None,
        provider: None,
    })
    .await
    .unwrap();
    let mut saw_plan = false;
    while let Some(ev) = h.next_event().await {
        if matches!(ev, Event::PlanUpdated { .. }) {
            saw_plan = true;
        }
        if matches!(ev, Event::TurnComplete { .. }) {
            break;
        }
    }
    assert!(
        !saw_plan,
        "PlanUpdated fired despite plan_blocks_enabled=false"
    );
}
