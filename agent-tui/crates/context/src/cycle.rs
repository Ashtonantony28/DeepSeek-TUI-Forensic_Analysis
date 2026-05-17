//! Hard cycle reset — archives the session, restarts with a Briefing.
//!
//! Unlike `SeamManager`, this is destructive: pre-cycle messages persist
//! on disk (engine responsibility) but the live in-memory buffer is
//! truncated to a small briefing seed.

use crate::tokens::estimate_tokens;
use agent_tui_protocol::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleConfig {
    pub cycle_tokens: u32,
    pub keep_recent_messages: usize,
}

impl Default for CycleConfig {
    fn default() -> Self {
        Self {
            cycle_tokens: 768_000,
            keep_recent_messages: 8,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CycleManager {
    pub config: CycleConfig,
    pub current_cycle: u32,
}

#[derive(Debug, Clone)]
pub struct CycleOutcome {
    pub from: u32,
    pub to: u32,
    pub briefing_message: Message,
}

impl CycleManager {
    pub fn new(config: CycleConfig) -> Self {
        Self {
            config,
            current_cycle: 0,
        }
    }

    /// If tokens exceed `cycle_tokens`, build the briefing and replace
    /// `messages` with a small seed: the briefing + the most recent
    /// `keep_recent_messages` messages.
    ///
    /// Caller-supplied `briefing_summary` should be produced by an LLM
    /// call summarising what's about to be archived.
    pub fn maybe_cycle(
        &mut self,
        messages: &mut Vec<Message>,
        briefing_summary: String,
    ) -> Option<CycleOutcome> {
        let tokens = estimate_tokens(messages);
        if tokens < self.config.cycle_tokens {
            return None;
        }
        let from = self.current_cycle;
        let to = self.current_cycle + 1;
        let keep = self.config.keep_recent_messages.min(messages.len());
        let tail: Vec<Message> = messages.drain(messages.len() - keep..).collect();

        let xml = format!(
            "<cycle_briefing from=\"{from}\" to=\"{to}\">\n{briefing_summary}\n</cycle_briefing>"
        );
        let briefing = Message {
            role: Role::System,
            content: vec![ContentBlock::Text { text: xml }],
            metadata: Default::default(),
        };
        messages.clear();
        messages.push(briefing.clone());
        messages.extend(tail);
        self.current_cycle = to;
        Some(CycleOutcome {
            from,
            to,
            briefing_message: briefing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doesnt_cycle_below_threshold() {
        let mut m = CycleManager::default();
        let mut msgs = vec![Message::user_text("hi")];
        assert!(m.maybe_cycle(&mut msgs, "s".into()).is_none());
    }

    #[test]
    fn cycles_above_threshold_and_keeps_tail() {
        let mut m = CycleManager::default();
        let mut msgs: Vec<Message> = (0..160)
            .map(|_| Message::user_text("x".repeat(20_000)))
            .collect();
        let _last = msgs.last().unwrap().clone();
        let out = m.maybe_cycle(&mut msgs, "summary".into()).unwrap();
        assert_eq!(out.from, 0);
        assert_eq!(out.to, 1);
        // briefing + keep_recent_messages
        assert!(msgs.len() <= 1 + m.config.keep_recent_messages);
        match &msgs[0].content[0] {
            ContentBlock::Text { text } => assert!(text.contains("cycle_briefing")),
            _ => panic!(),
        }
        assert_eq!(m.current_cycle, 1);
    }
}
