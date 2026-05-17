use super::openai_compat::OpenAiCompatClient;
use crate::{ChatRequest, ChatStream, ClientConfig, LlmClient, LlmError};
use agent_tui_protocol::{ModelInfo, Provider};
use async_trait::async_trait;

pub struct DeepSeekClient {
    inner: OpenAiCompatClient,
}

impl DeepSeekClient {
    pub fn new(cfg: ClientConfig) -> Self {
        Self {
            inner: OpenAiCompatClient::new(
                cfg,
                Provider::DeepSeek,
                "https://api.deepseek.com/beta",
            ),
        }
    }
}

#[async_trait]
impl LlmClient for DeepSeekClient {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.inner.stream(req).await
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![
            ModelInfo {
                provider: Provider::DeepSeek,
                name: "deepseek-chat".into(),
                context_window: 64_000,
                max_output_tokens: 8_192,
                supports_thinking: false,
                supports_tools: true,
                supports_vision: false,
            },
            ModelInfo {
                provider: Provider::DeepSeek,
                name: "deepseek-reasoner".into(),
                context_window: 64_000,
                max_output_tokens: 8_192,
                supports_thinking: true,
                supports_tools: true,
                supports_vision: false,
            },
        ])
    }
    fn provider(&self) -> Provider {
        Provider::DeepSeek
    }
}
