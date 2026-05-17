//! Destructive compaction (fallback when seam slots exhausted).
//!
//! Summarises a run of old messages into a single system-prompt addendum.
//! Breaks the prefix cache; only used when SeamManager cannot help.

use crate::tokens::estimate_tokens;
use agent_tui_protocol::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    pub trigger_tokens: u32,
    pub keep_recent_messages: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            trigger_tokens: 700_000, // sit below the 768K cycle line
            keep_recent_messages: 16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub before_tokens: u32,
    pub after_tokens: u32,
}

#[derive(Default)]
pub struct Compactor {
    pub config: CompactionConfig,
}

impl Compactor {
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    pub fn should_compact(&self, messages: &[Message]) -> bool {
        estimate_tokens(messages) >= self.config.trigger_tokens
    }

    /// Apply the compaction: replace [0, len - keep_recent_messages] with a
    /// single system message containing `summary_text`. Returns
    /// before/after token counts so the engine can report.
    pub fn apply(&self, messages: &mut Vec<Message>, summary_text: String) -> CompactionResult {
        let before = estimate_tokens(messages);
        let keep = self.config.keep_recent_messages.min(messages.len());
        let head_count = messages.len().saturating_sub(keep);
        if head_count == 0 {
            return CompactionResult {
                before_tokens: before,
                after_tokens: before,
            };
        }
        let tail: Vec<Message> = messages.drain(messages.len() - keep..).collect();
        messages.clear();
        let summary = Message {
            role: Role::System,
            content: vec![ContentBlock::Text {
                text: format!("<compacted_history>\n{summary_text}\n</compacted_history>"),
            }],
            metadata: Default::default(),
        };
        messages.push(summary);
        messages.extend(tail);
        let after = estimate_tokens(messages);
        CompactionResult {
            before_tokens: before,
            after_tokens: after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_drops_head() {
        let c = Compactor::default();
        let mut msgs: Vec<Message> = (0..200)
            .map(|_| Message::user_text("x".repeat(20_000)))
            .collect();
        assert!(c.should_compact(&msgs));
        let r = c.apply(&mut msgs, "summary".into());
        assert!(r.after_tokens < r.before_tokens);
        // head replaced by 1 system message + retained tail
        assert_eq!(msgs.len(), 1 + c.config.keep_recent_messages);
    }
}
