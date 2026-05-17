//! Shared CLI helpers used by `main.rs`.

use agent_tui_config::{Config, ProviderConfig};
use agent_tui_llm::{
    AnthropicClient, ClientConfig, DeepSeekClient, GroqClient, LlmClient, MockClient, OllamaClient,
    OpenAiClient, OpenAiCompatClient, XaiClient,
};
use agent_tui_protocol::Provider;
use std::sync::Arc;

fn provider_to_cc(p: ProviderConfig) -> ClientConfig {
    ClientConfig {
        api_key: p.api_key,
        base_url: p.base_url,
        default_model: p.default_model,
    }
}

pub fn build_client(provider: Provider, cfg: &Config) -> Arc<dyn LlmClient> {
    let pc = provider_to_cc(
        cfg.providers
            .get(provider.as_str())
            .cloned()
            .unwrap_or_default(),
    );
    match provider {
        Provider::Anthropic => Arc::new(AnthropicClient::new(pc)),
        Provider::OpenAi => Arc::new(OpenAiClient::new(pc)),
        Provider::DeepSeek => Arc::new(DeepSeekClient::new(pc)),
        Provider::Groq => Arc::new(GroqClient::new(pc)),
        Provider::Xai => Arc::new(XaiClient::new(pc)),
        Provider::Ollama => Arc::new(OllamaClient::new(pc)),
        Provider::OpenAiCompat => Arc::new(OpenAiCompatClient::from_config(pc)),
    }
}

pub fn build_mock_client() -> Arc<dyn LlmClient> {
    Arc::new(MockClient::new())
}
