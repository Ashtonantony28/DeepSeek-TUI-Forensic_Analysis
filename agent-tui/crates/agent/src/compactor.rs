//! Flash-summary compaction (extension 3.9).
//!
//! Replaces the Phase-2 placeholder strings in the seam and cycle paths.
//! A small "Flash-tier" summarizer is invoked over the head of the
//! message log; its single text response is what `apply_summary` /
//! `cycle_briefing` write into the archived block.
//!
//! Two important design decisions:
//!
//! 1. The summarizer runs against the **router's `Small` tier** so the
//!    main reasoning model isn't billed for it. The caller resolves the
//!    cheap model up-front and passes it in — this crate stays
//!    provider-agnostic. Some providers (notably OpenAI) host their
//!    small/cheap models on a separate endpoint from the large models,
//!    so `FlashCompactor` accepts an *optional* second `LlmClient`
//!    dedicated to the small-tier call. When absent, it falls back to
//!    the main client.
//!
//! 2. Compaction is opt-in via `Engine::compaction_enabled`. Default-on
//!    in `Engine::new` but disabled in tests that don't push summary
//!    text on the mock client. When disabled, the engine falls back to
//!    the previous placeholder strings so seams still apply (they just
//!    carry less information).

use agent_tui_llm::{ChatRequest, LlmClient, StreamEvent};
use agent_tui_protocol::{ContentBlock, Message, Role};
use futures::StreamExt;
use std::sync::Arc;

const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 512;

/// How to render a `Message` slice for a summarization prompt. Kept tiny
/// on purpose: a literal role-prefixed transcript is the most
/// cache-friendly thing we can ask for.
fn render_transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        let prefix = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        for block in &m.content {
            match block {
                ContentBlock::Text { text } => {
                    out.push_str(prefix);
                    out.push_str(": ");
                    out.push_str(text);
                    out.push('\n');
                }
                ContentBlock::Thinking { text } => {
                    out.push_str(prefix);
                    out.push_str(" (thinking): ");
                    out.push_str(text);
                    out.push('\n');
                }
                ContentBlock::ToolUse { name, .. } => {
                    out.push_str(prefix);
                    out.push_str(" (tool_use ");
                    out.push_str(name);
                    out.push_str(")\n");
                }
                ContentBlock::ToolResult { is_error, content, .. } => {
                    out.push_str(prefix);
                    out.push_str(if *is_error { " (tool_error): " } else { " (tool_result): " });
                    out.push_str(content);
                    out.push('\n');
                }
                ContentBlock::Image { .. } => {
                    out.push_str(prefix);
                    out.push_str(" (image)\n");
                }
            }
        }
    }
    out
}

const SEAM_PROMPT: &str =
    "Summarise the conversation history below in <=200 words. Keep file paths, \
     command names, exact identifiers, and outstanding decisions. Drop chatter, \
     pleasantries, and tool noise. Plain text — no markdown.";

const CYCLE_PROMPT: &str =
    "Write a Cycle Briefing for an agent that is about to lose this conversation \
     from memory. <=300 words. Include: the user's overall goal, what has been \
     decided, paths and identifiers touched, what's left to do, and any pinned \
     facts the next cycle must not forget. Plain text — no markdown.";

/// Cheap-model summarizer used for both seam and cycle compactions.
#[derive(Clone)]
pub struct FlashCompactor {
    /// Main reasoning model client. Used as a fallback when no
    /// dedicated small-tier client is configured.
    pub llm: Arc<dyn LlmClient>,
    /// Optional dedicated client for the small/cheap tier. Providers
    /// like OpenAI host gpt-4o-mini and gpt-4o on the same endpoint,
    /// but multi-endpoint setups (e.g. a private gateway that fronts
    /// gpt-4o-mini through a different base URL or credential) require
    /// a distinct `LlmClient`.
    pub small_llm: Option<Arc<dyn LlmClient>>,
    pub model: String,
}

impl FlashCompactor {
    /// Construct a compactor. `small_llm` is the optional dedicated
    /// client for the small-tier summary call; when `None`, the
    /// compactor falls back to `llm`.
    pub fn new(
        llm: Arc<dyn LlmClient>,
        small_llm: Option<Arc<dyn LlmClient>>,
        model: impl Into<String>,
    ) -> Self {
        Self { llm, small_llm, model: model.into() }
    }

    /// The client that will actually be used for the small-tier
    /// summary call.
    pub fn summary_client(&self) -> &Arc<dyn LlmClient> {
        self.small_llm.as_ref().unwrap_or(&self.llm)
    }

    pub async fn summarize_seam(&self, head: &[Message]) -> Result<String, String> {
        self.summarize_with(SEAM_PROMPT, head, DEFAULT_MAX_OUTPUT_TOKENS).await
    }

    pub async fn summarize_cycle(&self, head: &[Message]) -> Result<String, String> {
        self.summarize_with(CYCLE_PROMPT, head, DEFAULT_MAX_OUTPUT_TOKENS).await
    }

    async fn summarize_with(
        &self,
        system_prompt: &str,
        head: &[Message],
        max_tokens: u32,
    ) -> Result<String, String> {
        if head.is_empty() {
            return Ok(String::new());
        }
        let mut req = ChatRequest::new(&self.model);
        req.system = Some(system_prompt.to_string());
        req.messages = vec![Message::user_text(render_transcript(head))];
        req.max_output_tokens = Some(max_tokens);

        let mut stream = self
            .summary_client()
            .stream(req)
            .await
            .map_err(|e| format!("flash compactor llm: {e}"))?;
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            match ev {
                Ok(StreamEvent::TextDelta(t)) => text.push_str(&t),
                Ok(_) => {}
                Err(e) => return Err(format!("flash compactor stream: {e}")),
            }
        }
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_llm::MockClient;
    use agent_tui_protocol::Provider;

    fn user(s: &str) -> Message { Message::user_text(s) }
    fn asst(s: &str) -> Message { Message::assistant_text(s) }

    #[tokio::test]
    async fn seam_summary_round_trip() {
        let mock = Arc::new(MockClient::new());
        mock.push_text("Refactored parser; updated tests; left lint warning in src/foo.rs");
        let c = FlashCompactor::new(mock as Arc<dyn LlmClient>, None, "mock-flash");
        let head = vec![user("hi"), asst("hello"), user("please refactor src/foo.rs")];
        let out = c.summarize_seam(&head).await.unwrap();
        assert!(out.contains("foo.rs"));
    }

    #[tokio::test]
    async fn cycle_summary_round_trip() {
        let mock = Arc::new(MockClient::new());
        mock.push_text("Goal: build agent-tui. Decided: 14 crates. TODO: ship eval harness.");
        let c = FlashCompactor::new(mock as Arc<dyn LlmClient>, None, "mock-flash");
        let head = vec![user("scope?"), asst("we're building agent-tui in 14 crates")];
        let out = c.summarize_cycle(&head).await.unwrap();
        assert!(out.contains("agent-tui"));
        assert!(out.contains("crates"));
    }

    #[tokio::test]
    async fn empty_head_returns_empty() {
        let mock = Arc::new(MockClient::new());
        let c = FlashCompactor::new(mock as Arc<dyn LlmClient>, None, "mock-flash");
        let out = c.summarize_seam(&[]).await.unwrap();
        assert!(out.is_empty());
    }

    /// Multi-endpoint wiring: when a dedicated small-tier client is
    /// supplied, the compactor stores it and routes the summary call
    /// through it instead of the main client. We don't invoke the
    /// client — we verify the constructor wired it correctly.
    #[test]
    fn small_client_wired_in_constructor() {
        // Two clients with distinct providers so we can tell them apart
        // without invoking either one.
        let main = Arc::new(MockClient::new().with_provider(Provider::Anthropic))
            as Arc<dyn LlmClient>;
        let small = Arc::new(MockClient::new().with_provider(Provider::OpenAi))
            as Arc<dyn LlmClient>;

        let c = FlashCompactor::new(main.clone(), Some(small.clone()), "mock-flash");

        assert!(c.small_llm.is_some(), "small_llm should be stored");
        assert_eq!(c.llm.provider(), Provider::Anthropic);
        assert_eq!(
            c.summary_client().provider(),
            Provider::OpenAi,
            "summary call should route through the small-tier client",
        );
    }

    /// When no small-tier client is supplied, `summary_client()` must
    /// fall back to the main client.
    #[test]
    fn falls_back_to_main_client_when_small_is_none() {
        let main = Arc::new(MockClient::new().with_provider(Provider::Anthropic))
            as Arc<dyn LlmClient>;
        let c = FlashCompactor::new(main, None, "mock-flash");
        assert!(c.small_llm.is_none());
        assert_eq!(c.summary_client().provider(), Provider::Anthropic);
    }
}
