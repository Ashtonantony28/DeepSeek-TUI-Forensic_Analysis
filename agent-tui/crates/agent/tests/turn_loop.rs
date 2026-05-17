use agent_tui_agent::{Engine, Session};
use agent_tui_llm::{LlmClient, MockClient, Usage};
use agent_tui_protocol::{AppMode, Event, Op, Provider};
use agent_tui_tools::ToolRegistry;
use camino::Utf8PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn text_only_turn_emits_delta_and_complete() {
    let mock = Arc::new(MockClient::new().with_provider(Provider::Anthropic));
    mock.push_text("hello!");
    let session = Session::new("mock-model");
    let (reg, ctx) = ToolRegistry::with_builtins(Utf8PathBuf::from("/tmp"));
    let engine = Engine::new(session, mock.clone() as Arc<dyn LlmClient>, Arc::new(reg), Arc::new(ctx));
    let h = engine.spawn();
    h.send(Op::Submit {
        content: "say hi".into(),
        mode: AppMode::Agent,
        model: None,
        provider: None,
    })
    .await
    .unwrap();

    let mut got_text = false;
    let mut got_complete = false;
    for _ in 0..32 {
        let Some(ev) = h.next_event().await else { break };
        match ev {
            Event::Delta { delta, .. } => {
                if delta.contains("hello") { got_text = true; }
            }
            Event::TurnComplete { .. } => {
                got_complete = true;
                break;
            }
            _ => {}
        }
    }
    assert!(got_text);
    assert!(got_complete);
    h.send(Op::Shutdown).await.unwrap();
}

#[tokio::test]
async fn tool_call_turn_executes_tool() {
    let mock = Arc::new(MockClient::new());
    let td = tempfile::tempdir().unwrap();
    let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
    std::fs::write(root.join("README.md"), "hello world").unwrap();

    mock.push_tool_call("read_file", serde_json::json!({"path":"README.md"}));
    mock.push_text("Done.");

    let session = Session::new("mock-model");
    let (reg, mut ctx) = ToolRegistry::with_builtins(root.clone());
    ctx.workspace_root = root;
    let engine = Engine::new(session, mock.clone() as Arc<dyn LlmClient>, Arc::new(reg), Arc::new(ctx));
    let h = engine.spawn();
    h.send(Op::Submit {
        content: "summarize README".into(),
        mode: AppMode::Yolo,
        model: None,
        provider: None,
    })
    .await
    .unwrap();

    let mut saw_tool = false;
    let mut saw_complete = false;
    for _ in 0..64 {
        let Some(ev) = h.next_event().await else { break };
        match ev {
            Event::ToolCallStarted { name, .. } => {
                assert_eq!(name, "read_file");
                saw_tool = true;
            }
            Event::TurnComplete { .. } => {
                saw_complete = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_tool);
    assert!(saw_complete);
    h.send(Op::Shutdown).await.unwrap();
    let _ = Usage::default();
}
