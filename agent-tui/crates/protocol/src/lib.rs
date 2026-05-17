//! Wire protocol types for agent-tui.
//!
//! These types are pure data: no async, no I/O. They are shared between the
//! engine (`agent-tui-agent`), the LLM adapters (`agent-tui-llm`), the TUI,
//! the ACP/MCP servers, and the eval harness.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use uuid::Uuid;

// =====================================================================
// IDs
// =====================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(format!("sess-{}", Uuid::new_v4()))
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub String);

impl TurnId {
    pub fn new() -> Self {
        Self(format!("turn-{}", Uuid::new_v4()))
    }
}

impl Default for TurnId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

impl ToolCallId {
    pub fn new() -> Self {
        Self(format!("tc-{}", Uuid::new_v4()))
    }
}

impl Default for ToolCallId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApprovalId(pub String);

impl ApprovalId {
    pub fn new() -> Self {
        Self(format!("appr-{}", Uuid::new_v4()))
    }
}

impl Default for ApprovalId {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
// Providers and model info
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Anthropic,
    OpenAi,
    DeepSeek,
    Groq,
    Xai,
    Ollama,
    OpenAiCompat,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::DeepSeek => "deepseek",
            Provider::Groq => "groq",
            Provider::Xai => "xai",
            Provider::Ollama => "ollama",
            Provider::OpenAiCompat => "openai_compat",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub provider: Provider,
    pub name: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub supports_thinking: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
}

// =====================================================================
// Content blocks
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Thinking { text: String },
    ToolUse {
        id: ToolCallId,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: ToolCallId,
        content: String,
        is_error: bool,
    },
    Image { media_type: String, data: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            metadata: HashMap::new(),
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
            metadata: HashMap::new(),
        }
    }

    pub fn system_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::Text { text: text.into() }],
            metadata: HashMap::new(),
        }
    }
}

// =====================================================================
// Turns
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub id: TurnId,
    pub messages: Vec<Message>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

// =====================================================================
// Risk band & guardrails (matches upstream CapacityController)
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskBand {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardrailAction {
    NoIntervention,
    TargetedContextRefresh,
    VerifyWithToolReplay,
    VerifyAndReplan,
}

// =====================================================================
// App modes
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    Plan,
    Agent,
    Yolo,
}

impl AppMode {
    pub fn next(self) -> Self {
        match self {
            AppMode::Plan => AppMode::Agent,
            AppMode::Agent => AppMode::Yolo,
            AppMode::Yolo => AppMode::Plan,
        }
    }
}

// =====================================================================
// Approval
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    ApprovedForSession,
    Denied,
    Abort,
}

// =====================================================================
// Op enum — inputs into the engine
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Submit {
        content: String,
        mode: AppMode,
        model: Option<String>,
        provider: Option<Provider>,
    },
    Cancel,
    AcceptApproval {
        id: ApprovalId,
        decision: Decision,
    },
    SpawnSubAgent {
        prompt: String,
    },
    ChangeMode {
        mode: AppMode,
    },
    SetModel {
        model: String,
        provider: Option<Provider>,
    },
    CompactContext,
    Shutdown,
}

// =====================================================================
// Event enum — outputs from the engine
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaChannel {
    Text,
    Thinking,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanItem {
    pub step: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    TurnStarted { turn_id: TurnId },
    Delta {
        turn_id: TurnId,
        channel: DeltaChannel,
        delta: String,
    },
    ToolCallStarted {
        turn_id: TurnId,
        tool_call_id: ToolCallId,
        name: String,
        input: serde_json::Value,
    },
    ToolCallFinished {
        turn_id: TurnId,
        tool_call_id: ToolCallId,
        name: String,
        output: serde_json::Value,
        is_error: bool,
    },
    ApprovalRequest {
        turn_id: TurnId,
        approval_id: ApprovalId,
        tool_name: String,
        summary: String,
    },
    Status {
        turn_id: Option<TurnId>,
        message: String,
    },
    RiskBandChanged {
        turn_id: TurnId,
        band: RiskBand,
        action: GuardrailAction,
    },
    SeamApplied {
        turn_id: TurnId,
        level: u8,
        tokens_archived: u32,
    },
    CycleAdvanced {
        from: u32,
        to: u32,
    },
    CompactionApplied {
        before_tokens: u32,
        after_tokens: u32,
    },
    TurnComplete { turn_id: TurnId },
    TurnAborted {
        turn_id: TurnId,
        reason: String,
    },
    /// 3.7 — the agent's working plan (from the `update_plan` tool).
    PlanUpdated {
        turn_id: TurnId,
        goal: String,
        items: Vec<PlanItem>,
    },
    /// 3.8 — a DARS branching+verification pass finished. The winner
    /// is also surfaced as a regular `Delta` so the transcript stays
    /// readable.
    DarsResult {
        winner_index: usize,
        branch_count: usize,
        votes: Vec<usize>,
    },
    Error { message: String },
}

// =====================================================================
// Errors
// =====================================================================

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let a = SessionId::new();
        let b = SessionId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn op_roundtrip() {
        let op = Op::Submit {
            content: "hi".into(),
            mode: AppMode::Agent,
            model: Some("claude-opus-4-7".into()),
            provider: Some(Provider::Anthropic),
        };
        let j = serde_json::to_string(&op).unwrap();
        let back: Op = serde_json::from_str(&j).unwrap();
        match back {
            Op::Submit { content, .. } => assert_eq!(content, "hi"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn event_roundtrip() {
        let ev = Event::Delta {
            turn_id: TurnId::new(),
            channel: DeltaChannel::Text,
            delta: "hello".into(),
        };
        let j = serde_json::to_string(&ev).unwrap();
        assert!(j.contains("\"event\":\"delta\""));
        let _back: Event = serde_json::from_str(&j).unwrap();
    }

    #[test]
    fn risk_band_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&RiskBand::Low).unwrap(), "\"low\"");
        assert_eq!(serde_json::to_string(&RiskBand::High).unwrap(), "\"high\"");
    }

    #[test]
    fn app_mode_cycles() {
        assert_eq!(AppMode::Plan.next(), AppMode::Agent);
        assert_eq!(AppMode::Agent.next(), AppMode::Yolo);
        assert_eq!(AppMode::Yolo.next(), AppMode::Plan);
    }
}
