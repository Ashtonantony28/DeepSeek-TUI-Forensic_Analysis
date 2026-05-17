use agent_tui_protocol::ToolCallId;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

use crate::LlmError;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolCallStart {
        id: ToolCallId,
        name: String,
    },
    ToolCallDelta {
        id: ToolCallId,
        json_fragment: String,
    },
    ToolCallEnd {
        id: ToolCallId,
    },
    MessageEnd {
        stop_reason: String,
        usage: Usage,
    },
}

pub type ChatStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, LlmError>> + Send>>;
