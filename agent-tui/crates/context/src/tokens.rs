//! Cheap token estimator. ~4 characters per token is good enough for the
//! seam/cycle decisions; the LLM client reports real counts after each turn.

use agent_tui_protocol::{ContentBlock, Message};

pub fn estimate_tokens(messages: &[Message]) -> u32 {
    let mut total: u32 = 0;
    for m in messages {
        for c in &m.content {
            match c {
                ContentBlock::Text { text } | ContentBlock::Thinking { text } => {
                    total = total.saturating_add(((text.len() as u32) + 3) / 4);
                }
                ContentBlock::ToolUse { input, .. } => {
                    let s = serde_json::to_string(input).unwrap_or_default();
                    total = total.saturating_add(((s.len() as u32) + 3) / 4);
                }
                ContentBlock::ToolResult { content, .. } => {
                    total = total.saturating_add(((content.len() as u32) + 3) / 4);
                }
                ContentBlock::Image { data, .. } => {
                    total = total.saturating_add(((data.len() as u32) + 3) / 4);
                }
            }
        }
    }
    total
}
