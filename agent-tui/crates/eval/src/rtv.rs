//! Recursive Tournament Voting (RTV) for `--scale N` runs.
//!
//! Based on "Scaling Test-Time Compute for Agentic Coding" (Meta, April 2026),
//! arxiv 2604.16529. The key insight: voting must be over structured trajectory
//! summaries, not raw bash transcripts. Raw transcripts are too noisy for
//! reliable winner selection.
//!
//! Algorithm:
//!  1. Run N independent pipeline rollouts, each producing a `RolloutSummary`.
//!  2. Pair summaries into groups of GROUP_SIZE (3–4).
//!  3. Use a judge model to pick the best summary per group.
//!  4. Repeat until one winner remains.

use agent_tui_llm::{ChatRequest, LlmClient, StreamEvent};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;

const GROUP_SIZE: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutSummary {
    /// Unified diff produced by this rollout (may be empty on failure).
    pub patch: String,
    /// True if the patch passed tests in this rollout.
    pub pass: bool,
    /// Number of tool calls / turns in this rollout.
    pub turns: u32,
    /// Brief rationale from the pipeline (extracted from LLM response).
    pub rationale: String,
}

impl RolloutSummary {
    pub fn failed() -> Self {
        Self { patch: String::new(), pass: false, turns: 0, rationale: "pipeline failed".into() }
    }
}

/// Select the winning rollout via Recursive Tournament Voting.
///
/// `judge` is the LLM used for pairwise comparison. In `--dry-run` mode,
/// the judge is a `MockClient` that returns scripted winners.
pub async fn rtv_vote(
    summaries: Vec<RolloutSummary>,
    judge: Arc<dyn LlmClient>,
    model: &str,
) -> RolloutSummary {
    if summaries.is_empty() {
        return RolloutSummary::failed();
    }
    if summaries.len() == 1 {
        return summaries.into_iter().next().unwrap();
    }

    // Fast path: if any summary passes, run tournament only on passing ones.
    let passing: Vec<RolloutSummary> = summaries.iter().filter(|s| s.pass).cloned().collect();
    let mut pool = if passing.is_empty() { summaries } else { passing };

    // Iterative tournament: repeatedly reduce pool by judging groups.
    while pool.len() > 1 {
        let mut next_round: Vec<RolloutSummary> = Vec::new();
        let chunks: Vec<Vec<RolloutSummary>> = pool
            .chunks(GROUP_SIZE)
            .map(|c| c.to_vec())
            .collect();
        for group in chunks {
            let winner = judge_group(&group, judge.clone(), model).await;
            next_round.push(winner);
        }
        pool = next_round;
    }

    pool.into_iter().next().unwrap_or_else(RolloutSummary::failed)
}

/// Ask the judge LLM to pick the best summary from a small group.
async fn judge_group(
    group: &[RolloutSummary],
    judge: Arc<dyn LlmClient>,
    model: &str,
) -> RolloutSummary {
    let prompt = build_judge_prompt(group);
    let mut req = ChatRequest::new(model.to_string());
    req.messages = vec![agent_tui_protocol::Message::user_text(prompt)];
    req.max_output_tokens = Some(64);

    let stream_result = judge.stream(req).await;
    let mut stream = match stream_result {
        Ok(s) => s,
        Err(_) => return group[0].clone(),
    };

    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        if let Ok(StreamEvent::TextDelta(t)) = ev {
            text.push_str(&t);
        }
    }

    let idx = parse_winner_index(&text, group.len());
    debug!("rtv judge picked index {} from group of {}", idx, group.len());
    group[idx].clone()
}

fn build_judge_prompt(group: &[RolloutSummary]) -> String {
    let mut s = String::from(
        "You are judging candidate patches for a software fix. \
         Pick the BEST candidate by responding with ONLY its number (1, 2, 3…).\n\
         Criteria: correct fix > fewer turns > longer rationale.\n\n",
    );
    for (i, summary) in group.iter().enumerate() {
        s.push_str(&format!(
            "Candidate {}:\n  pass={}\n  turns={}\n  rationale: {}\n  patch (first 200 chars): {}\n\n",
            i + 1,
            summary.pass,
            summary.turns,
            summary.rationale,
            &summary.patch[..summary.patch.len().min(200)],
        ));
    }
    s.push_str("Respond with only the number of the best candidate:");
    s
}

fn parse_winner_index(response: &str, max: usize) -> usize {
    for ch in response.chars() {
        if ch.is_ascii_digit() {
            let n = (ch as usize) - ('0' as usize);
            if n >= 1 && n <= max {
                return n - 1;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_llm::MockClient;

    fn summary(pass: bool, turns: u32, patch: &str) -> RolloutSummary {
        RolloutSummary { patch: patch.into(), pass, turns, rationale: "test".into() }
    }

    #[tokio::test]
    async fn single_summary_returned_as_is() {
        let mock = Arc::new(MockClient::new());
        let s = summary(true, 5, "diff...");
        let result = rtv_vote(vec![s.clone()], mock, "mock-model").await;
        assert_eq!(result.patch, s.patch);
    }

    #[tokio::test]
    async fn passing_summary_preferred_over_failing() {
        let mock = Arc::new(MockClient::new());
        mock.push_text("1");
        let failing = summary(false, 3, "bad-patch");
        let passing = summary(true, 5, "good-patch");
        let result = rtv_vote(vec![failing, passing], mock, "mock-model").await;
        assert!(result.pass, "should pick the passing summary");
    }

    #[tokio::test]
    async fn tournament_picks_judge_winner() {
        let mock = Arc::new(MockClient::new());
        mock.push_text("2");
        let a = summary(true, 10, "patch-a");
        let b = summary(true, 3, "patch-b");
        let result = rtv_vote(vec![a, b], mock, "mock-model").await;
        assert_eq!(result.patch, "patch-b");
    }

    #[tokio::test]
    async fn empty_pool_returns_failed() {
        let mock = Arc::new(MockClient::new());
        let result = rtv_vote(vec![], mock, "mock-model").await;
        assert!(!result.pass);
    }

    #[test]
    fn parse_winner_index_extracts_digit() {
        assert_eq!(parse_winner_index("The winner is 2.", 3), 1);
        assert_eq!(parse_winner_index("1", 3), 0);
        assert_eq!(parse_winner_index("no digit here", 3), 0);
        assert_eq!(parse_winner_index("9 out of range", 3), 0);
    }
}
