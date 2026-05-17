//! In-memory test client. Replays scripted events.

use crate::{ChatRequest, ChatStream, ClientConfig, LlmClient, LlmError, StreamEvent, Usage};
use agent_tui_protocol::{ModelInfo, Provider, ToolCallId};
use async_trait::async_trait;
use futures::stream;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MockClient {
    scripts: Arc<Mutex<Vec<Vec<StreamEvent>>>>,
    pub provider: Provider,
}

impl Default for MockClient {
    fn default() -> Self { Self::new() }
}

impl MockClient {
    pub fn new() -> Self {
        Self {
            scripts: Arc::new(Mutex::new(Vec::new())),
            provider: Provider::Anthropic,
        }
    }

    pub fn with_provider(mut self, p: Provider) -> Self {
        self.provider = p;
        self
    }

    pub fn push_script(&self, events: Vec<StreamEvent>) {
        self.scripts.lock().unwrap().push(events);
    }

    pub fn push_text(&self, text: impl Into<String>) {
        self.push_script(vec![
            StreamEvent::TextDelta(text.into()),
            StreamEvent::MessageEnd {
                stop_reason: "end_turn".into(),
                usage: Usage::default(),
            },
        ]);
    }

    pub fn push_tool_call(&self, name: impl Into<String>, args: serde_json::Value) {
        let id = ToolCallId::new();
        let name = name.into();
        self.push_script(vec![
            StreamEvent::ToolCallStart { id: id.clone(), name: name.clone() },
            StreamEvent::ToolCallDelta {
                id: id.clone(),
                json_fragment: serde_json::to_string(&args).unwrap(),
            },
            StreamEvent::ToolCallEnd { id },
            StreamEvent::MessageEnd {
                stop_reason: "tool_calls".into(),
                usage: Usage::default(),
            },
        ]);
    }
}

impl MockClient {
    pub fn from_config(_cfg: ClientConfig) -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmClient for MockClient {
    async fn stream(&self, _req: ChatRequest) -> Result<ChatStream, LlmError> {
        let next = {
            let mut g = self.scripts.lock().unwrap();
            if g.is_empty() {
                vec![StreamEvent::MessageEnd {
                    stop_reason: "end_turn".into(),
                    usage: Usage::default(),
                }]
            } else {
                g.remove(0)
            }
        };
        let s = stream::iter(next.into_iter().map(Ok));
        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![ModelInfo {
            provider: self.provider,
            name: "mock-model".into(),
            context_window: 200_000,
            max_output_tokens: 8192,
            supports_thinking: true,
            supports_tools: true,
            supports_vision: false,
        }])
    }

    fn provider(&self) -> Provider {
        self.provider
    }
}
