use super::openai_compat::OpenAiCompatClient;
use crate::{ChatRequest, ChatStream, ClientConfig, LlmClient, LlmError};
use agent_tui_protocol::{ModelInfo, Provider};
use async_trait::async_trait;

pub struct OllamaClient {
    inner: OpenAiCompatClient,
}

impl OllamaClient {
    pub fn new(cfg: ClientConfig) -> Self {
        Self {
            inner: OpenAiCompatClient::new(cfg, Provider::Ollama, "http://localhost:11434"),
        }
    }
}

#[async_trait]
impl LlmClient for OllamaClient {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.inner.stream(req).await
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![])
    }
    fn provider(&self) -> Provider {
        Provider::Ollama
    }
}
