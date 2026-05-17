//! 3.6 — auto-routing wires the classifier into the engine and the
//! session's `model` field is updated on Submit.

use agent_tui_agent::{Engine, Router, Session, Tier};
use agent_tui_llm::{LlmClient, MockClient};
use agent_tui_protocol::{AppMode, Op, Provider};
use agent_tui_tools::ToolRegistry;
use camino::Utf8PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

#[test]
fn router_classifies_basic_buckets() {
    let r = Router::new(Provider::Anthropic);
    assert_eq!(r.pick("list files").0, Tier::Small);
    // refactor (+2) + path (+1) = Large under the >=3 threshold.
    assert_eq!(
        r.pick("refactor src/core/engine.rs to use a state machine")
            .0,
        Tier::Large,
    );
    // Long plain text with no hard signals lands in Mid.
    let mid = "explain ".repeat(150);
    assert_eq!(r.pick(&mid).0, Tier::Mid);
}

#[tokio::test]
async fn engine_swaps_model_when_routing_enabled() {
    let td = tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    let mock = Arc::new(MockClient::new());
    mock.push_text("ok");

    let (reg, ctx) = ToolRegistry::with_builtins(root);
    let mut engine = Engine::new(
        Session::new("claude-opus-4-7"),
        mock.clone() as Arc<dyn LlmClient>,
        Arc::new(reg),
        Arc::new(ctx),
    );
    engine.routing_enabled = true;
    engine.memory_enabled = false;

    let h = engine.spawn();
    h.send(Op::Submit {
        content: "list the files in src".into(),
        mode: AppMode::Yolo,
        model: None,
        provider: None,
    })
    .await
    .unwrap();

    // Drain to completion so the engine has had a chance to react.
    let mut saw_status = false;
    let mut routed_to: Option<String> = None;
    while let Some(ev) = h.next_event().await {
        if let agent_tui_protocol::Event::Status { message, .. } = &ev {
            if message.starts_with("router:") {
                saw_status = true;
                routed_to = Some(message.clone());
            }
        }
        if matches!(ev, agent_tui_protocol::Event::TurnComplete { .. }) {
            break;
        }
    }
    assert!(saw_status, "expected a router: status event");
    let msg = routed_to.unwrap();
    assert!(msg.contains("small"), "expected small tier, got: {msg}");
    assert!(
        msg.contains("haiku"),
        "expected an Anthropic haiku model, got: {msg}"
    );
}

#[tokio::test]
async fn engine_keeps_model_when_routing_disabled() {
    let td = tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    let mock = Arc::new(MockClient::new());
    mock.push_text("ok");
    let (reg, ctx) = ToolRegistry::with_builtins(root);
    let mut engine = Engine::new(
        Session::new("claude-opus-4-7"),
        mock.clone() as Arc<dyn LlmClient>,
        Arc::new(reg),
        Arc::new(ctx),
    );
    engine.routing_enabled = false;
    engine.memory_enabled = false;

    let h = engine.spawn();
    h.send(Op::Submit {
        content: "list files".into(),
        mode: AppMode::Yolo,
        model: None,
        provider: None,
    })
    .await
    .unwrap();
    let mut saw_router = false;
    while let Some(ev) = h.next_event().await {
        if let agent_tui_protocol::Event::Status { message, .. } = &ev {
            if message.starts_with("router:") {
                saw_router = true;
            }
        }
        if matches!(ev, agent_tui_protocol::Event::TurnComplete { .. }) {
            break;
        }
    }
    assert!(!saw_router, "router fired despite being disabled");
}
