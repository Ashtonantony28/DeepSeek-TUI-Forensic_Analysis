//! Append-only seam manager — preserves prefix cache.
//!
//! Three escalating thresholds (defaults from upstream ANALYSIS.md §4):
//!   L1 =  192K, L2 = 384K, L3 = 576K.
//!
//! When `evaluate()` returns `Some(level)`, the engine should:
//!   1. Hand the older slice of messages (head) to a cheap LLM call.
//!   2. Replace those messages with a single `<archived_context level="N">`
//!      block via `apply_summary()` — but keep a verbatim tail window
//!      (default 16 turns / 32 messages).
//!
//! Note: the manager itself is pure logic; the LLM call is the caller's
//! responsibility so this crate stays sync-test-friendly.

use crate::tokens::estimate_tokens;
use agent_tui_protocol::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeamLevel {
    L1 = 1,
    L2 = 2,
    L3 = 3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeamConfig {
    pub l1_tokens: u32,
    pub l2_tokens: u32,
    pub l3_tokens: u32,
    pub tail_messages: usize,
}

impl Default for SeamConfig {
    fn default() -> Self {
        Self {
            l1_tokens: 192_000,
            l2_tokens: 384_000,
            l3_tokens: 576_000,
            tail_messages: 32,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeamOutcome {
    pub level: SeamLevel,
    pub head_token_estimate: u32,
    pub head_message_count: usize,
}

#[derive(Debug, Default, Clone)]
pub struct SeamManager {
    pub config: SeamConfig,
    pub last_applied: Option<SeamLevel>,
}

impl SeamManager {
    pub fn new(config: SeamConfig) -> Self {
        Self {
            config,
            last_applied: None,
        }
    }

    /// Return the next-higher level that hasn't been applied yet, if the
    /// current token count crosses its threshold.
    pub fn evaluate(&self, messages: &[Message]) -> Option<SeamOutcome> {
        let tokens = estimate_tokens(messages);
        let target = match (tokens, self.last_applied) {
            (t, None) if t >= self.config.l1_tokens && t < self.config.l2_tokens => SeamLevel::L1,
            (t, None) if t >= self.config.l2_tokens && t < self.config.l3_tokens => SeamLevel::L2,
            (t, None) if t >= self.config.l3_tokens => SeamLevel::L3,
            (t, Some(SeamLevel::L1)) if t >= self.config.l2_tokens && t < self.config.l3_tokens => {
                SeamLevel::L2
            }
            (t, Some(SeamLevel::L1)) if t >= self.config.l3_tokens => SeamLevel::L3,
            (t, Some(SeamLevel::L2)) if t >= self.config.l3_tokens => SeamLevel::L3,
            _ => return None,
        };
        // Tail window preserved verbatim
        let tail = self.config.tail_messages.min(messages.len());
        let head_count = messages.len().saturating_sub(tail);
        if head_count == 0 {
            return None;
        }
        let head_tokens = estimate_tokens(&messages[..head_count]);
        Some(SeamOutcome {
            level: target,
            head_token_estimate: head_tokens,
            head_message_count: head_count,
        })
    }

    /// Replace the head of `messages` with a single archived_context block.
    /// The caller supplies the summary text (produced by a flash model call).
    /// This is append-only at the message-list level: nothing in the
    /// preserved tail is altered, which is what keeps the prefix cache hot.
    pub fn apply_summary(
        &mut self,
        messages: &mut Vec<Message>,
        outcome: &SeamOutcome,
        summary_text: String,
    ) {
        let head_count = outcome.head_message_count.min(messages.len());
        if head_count == 0 {
            return;
        }
        let xml = format!(
            "<archived_context level=\"{}\">\n{}\n</archived_context>",
            outcome.level as u8, summary_text
        );
        let archived = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: xml }],
            metadata: Default::default(),
        };
        messages.drain(..head_count);
        messages.insert(0, archived);
        self.last_applied = Some(outcome.level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_user_msg(chars: usize) -> Message {
        Message::user_text("x".repeat(chars))
    }

    #[test]
    fn no_seam_under_l1() {
        let m = SeamManager::default();
        let msgs = vec![long_user_msg(1_000)];
        assert!(m.evaluate(&msgs).is_none());
    }

    #[test]
    fn l1_fires_at_threshold() {
        let m = SeamManager::default();
        // 200k chars ≈ 50k tokens — won't trip L1.
        let msgs = vec![long_user_msg(192_000 * 4 + 100)];
        let out = m.evaluate(&msgs);
        // single message is the entire tail, head_count = 0 → no outcome
        assert!(out.is_none());
    }

    #[test]
    fn l1_fires_with_head_when_tail_present() {
        let m = SeamManager::default();
        let mut msgs: Vec<Message> = Vec::new();
        // Pack 200k tokens of head
        for _ in 0..40 {
            msgs.push(long_user_msg(20_000));
        }
        let out = m.evaluate(&msgs).unwrap();
        assert_eq!(out.level, SeamLevel::L1);
        assert!(out.head_message_count > 0);
        assert!(out.head_message_count <= msgs.len() - m.config.tail_messages);
    }

    #[test]
    fn apply_summary_preserves_tail() {
        let mut m = SeamManager::default();
        let mut msgs: Vec<Message> = (0..40).map(|_| long_user_msg(20_000)).collect();
        let last = msgs.last().unwrap().clone();
        let outcome = m.evaluate(&msgs).unwrap();
        m.apply_summary(&mut msgs, &outcome, "summary".into());
        // first is the archived block
        match &msgs[0].content[0] {
            ContentBlock::Text { text } => assert!(text.contains("archived_context")),
            _ => panic!(),
        }
        // tail preserved
        let last_after = msgs.last().unwrap();
        assert_eq!(last_after.content.len(), last.content.len());
        assert_eq!(m.last_applied, Some(SeamLevel::L1));
    }

    #[test]
    fn levels_escalate() {
        let mut m = SeamManager {
            last_applied: Some(SeamLevel::L1),
            ..Default::default()
        };
        let mut msgs: Vec<Message> = (0..80).map(|_| long_user_msg(20_000)).collect();
        let out = m.evaluate(&msgs).unwrap();
        assert_eq!(out.level, SeamLevel::L2);
        m.apply_summary(&mut msgs, &out, "s".into());
        assert_eq!(m.last_applied, Some(SeamLevel::L2));
    }
}
