use super::openai_compat::OpenAiCompatClient;
use crate::{ChatRequest, ChatStream, ClientConfig, LlmClient, LlmError};
use agent_tui_protocol::{ModelInfo, Provider};
use async_trait::async_trait;

pub struct OpenAiClient {
    inner: OpenAiCompatClient,
}

impl OpenAiClient {
    pub fn new(cfg: ClientConfig) -> Self {
        Self {
            inner: OpenAiCompatClient::new(cfg, Provider::OpenAi, "https://api.openai.com"),
        }
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.inner.stream(req).await
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![
            ModelInfo {
                provider: Provider::OpenAi,
                name: "gpt-4o".into(),
                context_window: 128_000,
                max_output_tokens: 16_384,
                supports_thinking: false,
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                provider: Provider::OpenAi,
                name: "o4-mini".into(),
                context_window: 200_000,
                max_output_tokens: 16_384,
                supports_thinking: true,
                supports_tools: true,
                supports_vision: false,
            },
        ])
    }
    fn provider(&self) -> Provider { Provider::OpenAi }
}
