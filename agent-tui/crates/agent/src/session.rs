use agent_tui_protocol::{Message, SessionId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub model: String,
}

impl Session {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            id: SessionId::new(),
            system_prompt: Some(default_system_prompt()),
            messages: Vec::new(),
            model: model.into(),
        }
    }
}

pub fn default_system_prompt() -> String {
    "You are agent-tui, a terminal coding agent. \
     Operate carefully: prefer read-only investigation, propose plans \
     before destructive edits, and respond concisely. \
     Use available tools to read files, search, and (with approval) \
     write code. Always run a tree-sitter syntax check on edits \
     (this is enforced by the tool layer)."
        .to_string()
}
