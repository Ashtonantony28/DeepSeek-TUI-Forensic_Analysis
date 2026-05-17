//! LLM provider abstraction and per-provider adapters.
//!
//! The single trait is `LlmClient`. Each provider adapter implements it.
//!
//! For Phase 2 the adapters do real HTTP (Anthropic, OpenAI-compatible),
//! return errors gracefully when credentials are missing, and provide a
//! `MockClient` for tests so that the engine can run without network.

pub mod request;
pub mod stream;

mod adapters;

pub use adapters::{
    anthropic::AnthropicClient, deepseek::DeepSeekClient, groq::GroqClient, mock::MockClient,
    ollama::OllamaClient, openai::OpenAiClient, openai_compat::OpenAiCompatClient, xai::XaiClient,
};

use agent_tui_protocol::{ModelInfo, Provider};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use request::{ChatRequest, ToolSchema};
pub use stream::{ChatStream, StreamEvent, Usage};

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("missing api key for provider `{0}`")]
    MissingApiKey(String),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("provider error ({status}): {body}")]
    Provider { status: u16, body: String },
    #[error("stream ended unexpectedly")]
    StreamEnded,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Other(String),
}

/// All LLM clients implement this.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError>;
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError>;
    fn provider(&self) -> Provider;
}

/// Configuration handed to adapters when constructed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
}
