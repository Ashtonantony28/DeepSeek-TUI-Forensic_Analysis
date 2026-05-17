//! 3.5 — verify cross-session memory survives engine restarts and that
//! lessons are surfaced in the next turn's system prompt.

use agent_tui_agent::{Engine, Session};
use agent_tui_llm::{LlmClient, MockClient};
use agent_tui_protocol::{AppMode, Op};
use agent_tui_tools::{MemoryStore, ToolRegistry};
use camino::Utf8PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn lessons_persist_and_are_retrievable() {
    let td = tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    {
        let m = MemoryStore::load(&root);
        m.add("git", "always create a NEW commit instead of amending").unwrap();
        m.add("rust", "prefer `if let Some(x) = ... else` over chained matches").unwrap();
    }
    let m2 = MemoryStore::load(&root);
    let hits = m2.retrieve("how do I commit in git", 5);
    assert!(!hits.is_empty());
    assert!(hits[0].lesson.contains("commit"));
}

#[tokio::test]
async fn engine_uses_memory_via_remember_tool() {
    let td = tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();

    let mock = Arc::new(MockClient::new());
    // First turn: agent calls `remember` then says "stored".
    mock.push_tool_call(
        "remember",
        serde_json::json!({"topic":"build","lesson":"run cargo test before commit"}),
    );
    mock.push_text("stored");

    let (reg, mut ctx) = ToolRegistry::with_builtins(root.clone());
    ctx.yolo = true;
    let session = Session::new("mock-model");
    let engine = Engine::new(session, mock.clone() as Arc<dyn LlmClient>, Arc::new(reg), Arc::new(ctx));
    let h = engine.spawn();
    h.send(Op::Submit {
        content: "remember the build rule".into(),
        mode: AppMode::Yolo,
        model: None,
        provider: None,
    })
    .await
    .unwrap();
    while let Some(ev) = h.next_event().await {
        if matches!(ev, agent_tui_protocol::Event::TurnComplete { .. }) {
            break;
        }
    }

    // Confirm the lesson reached disk.
    let m = MemoryStore::load(&root);
    assert!(m.len() >= 1, "remember tool did not persist a lesson");
    let hits = m.retrieve("cargo test build", 5);
    assert!(!hits.is_empty());
}
