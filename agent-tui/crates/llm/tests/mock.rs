use agent_tui_llm::{ChatRequest, LlmClient, MockClient, StreamEvent};
use futures::StreamExt;

#[tokio::test]
async fn mock_replays_scripted_text() {
    let c = MockClient::new();
    c.push_text("hello world");
    let mut s = c.stream(ChatRequest::new("mock-model")).await.unwrap();
    let mut text = String::new();
    let mut got_end = false;
    while let Some(ev) = s.next().await {
        match ev.unwrap() {
            StreamEvent::TextDelta(t) => text.push_str(&t),
            StreamEvent::MessageEnd { .. } => got_end = true,
            _ => {}
        }
    }
    assert_eq!(text, "hello world");
    assert!(got_end);
}

#[tokio::test]
async fn mock_emits_tool_call() {
    let c = MockClient::new();
    c.push_tool_call("read_file", serde_json::json!({"path":"README.md"}));
    let mut s = c.stream(ChatRequest::new("mock-model")).await.unwrap();
    let mut saw_start = false;
    let mut json_acc = String::new();
    while let Some(ev) = s.next().await {
        match ev.unwrap() {
            StreamEvent::ToolCallStart { name, .. } => {
                assert_eq!(name, "read_file");
                saw_start = true;
            }
            StreamEvent::ToolCallDelta { json_fragment, .. } => json_acc.push_str(&json_fragment),
            _ => {}
        }
    }
    assert!(saw_start);
    let v: serde_json::Value = serde_json::from_str(&json_acc).unwrap();
    assert_eq!(v["path"], "README.md");
}
