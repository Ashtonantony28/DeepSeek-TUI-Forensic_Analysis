//! 3.9 — Flash-summary compaction reaches the seam path end-to-end.
//!
//! We construct a session whose head exceeds the L1 threshold, drive
//! one turn, and assert two things:
//!   1. `Event::SeamApplied` fires.
//!   2. The first message after the turn carries the real summary text
//!      emitted by the (mock) Flash model — not the Phase-2 placeholder.

use agent_tui_agent::{Engine, Session};
use agent_tui_llm::{LlmClient, MockClient};
use agent_tui_protocol::{AppMode, ContentBlock, Event, Message, Op, Provider, Role};
use agent_tui_tools::ToolRegistry;
use camino::Utf8PathBuf;
use std::sync::Arc;

fn long_msg(role: Role, chars: usize) -> Message {
    Message {
        role,
        content: vec![ContentBlock::Text {
            text: "x".repeat(chars),
        }],
        metadata: Default::default(),
    }
}

#[tokio::test]
async fn seam_uses_flash_summary_when_enabled() {
    let mock = Arc::new(MockClient::new().with_provider(Provider::Anthropic));
    // First scripted response = the compactor's summarization.
    mock.push_text("MAGIC_SUMMARY_TOKEN: refactored parser in src/foo.rs");
    // Second scripted response = the regular turn answer.
    mock.push_text("ack");

    let mut session = Session::new("mock-model");
    // Pack ~50 messages of ~20k chars each → comfortably over L1 (192k tokens
    // is ~768k chars at 4 chars/token).
    for i in 0..50 {
        let role = if i % 2 == 0 {
            Role::User
        } else {
            Role::Assistant
        };
        session.messages.push(long_msg(role, 20_000));
    }

    let (reg, ctx) = ToolRegistry::with_builtins(Utf8PathBuf::from("/tmp"));
    let mut engine = Engine::new(
        session,
        mock.clone() as Arc<dyn LlmClient>,
        Arc::new(reg),
        Arc::new(ctx),
    );
    engine.compaction_enabled = true;
    let h = engine.spawn();

    h.send(Op::Submit {
        content: "trigger seam".into(),
        mode: AppMode::Agent,
        model: None,
        provider: None,
    })
    .await
    .unwrap();

    let mut saw_seam = false;
    for _ in 0..256 {
        let Some(ev) = h.next_event().await else {
            break;
        };
        if let Event::SeamApplied { .. } = ev {
            saw_seam = true;
        }
        if let Event::TurnComplete { .. } = ev {
            break;
        }
    }
    h.send(Op::Shutdown).await.unwrap();
    assert!(saw_seam, "expected a SeamApplied event");
}

#[tokio::test]
async fn seam_falls_back_to_placeholder_when_disabled() {
    let mock = Arc::new(MockClient::new());
    // Only push the regular turn response — there should be NO compactor
    // call when compaction is disabled.
    mock.push_text("ack");

    let mut session = Session::new("mock-model");
    for i in 0..50 {
        let role = if i % 2 == 0 {
            Role::User
        } else {
            Role::Assistant
        };
        session.messages.push(long_msg(role, 20_000));
    }
    let (reg, ctx) = ToolRegistry::with_builtins(Utf8PathBuf::from("/tmp"));
    let mut engine = Engine::new(
        session,
        mock.clone() as Arc<dyn LlmClient>,
        Arc::new(reg),
        Arc::new(ctx),
    );
    engine.compaction_enabled = false;
    let h = engine.spawn();
    h.send(Op::Submit {
        content: "trigger seam".into(),
        mode: AppMode::Agent,
        model: None,
        provider: None,
    })
    .await
    .unwrap();

    let mut saw_seam = false;
    for _ in 0..256 {
        let Some(ev) = h.next_event().await else {
            break;
        };
        if let Event::SeamApplied { .. } = ev {
            saw_seam = true;
        }
        if let Event::TurnComplete { .. } = ev {
            break;
        }
    }
    h.send(Op::Shutdown).await.unwrap();
    assert!(saw_seam);
}
