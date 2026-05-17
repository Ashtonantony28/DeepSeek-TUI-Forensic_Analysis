use super::openai_compat::OpenAiCompatClient;
use crate::{ChatRequest, ChatStream, ClientConfig, LlmClient, LlmError};
use agent_tui_protocol::{ModelInfo, Provider};
use async_trait::async_trait;

pub struct XaiClient { inner: OpenAiCompatClient }

impl XaiClient {
    pub fn new(cfg: ClientConfig) -> Self {
        Self { inner: OpenAiCompatClient::new(cfg, Provider::Xai, "https://api.x.ai") }
    }
}

#[async_trait]
impl LlmClient for XaiClient {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        self.inner.stream(req).await
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> { Ok(vec![]) }
    fn provider(&self) -> Provider { Provider::Xai }
}
