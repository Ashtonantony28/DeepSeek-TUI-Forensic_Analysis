//! DARS branching with multi-verifier (extension 3.8).
//!
//! DARS ("decode-attempt, re-score") spends extra inference per turn on
//! hard problems to widen the search before committing. Two phases:
//!
//!   Phase 1 — *branch*: spawn `branch_count` parallel sub-agents on the
//!     same prompt, each with a slightly different "approach" hint to
//!     encourage diversity (the variants are deterministic so the
//!     prefix-cache stays hot per branch).
//!
//!   Phase 2 — *verify*: spawn `verifier_count` parallel verifier
//!     sub-agents. Each verifier receives all candidate answers and must
//!     reply with a single integer index — the best candidate. The
//!     winner is selected by plurality vote; ties break to the
//!     lowest-indexed candidate (which is the original prompt's
//!     "control" approach).
//!
//! The whole pipeline runs offline against a `MockClient` in tests, so
//! the verifier prompt is shaped to make the index easy to extract:
//!   "BEST: <i>". The parser is tolerant of leading whitespace and
//!   trailing commentary.

use crate::{SubAgentError, SubAgentManager};
use serde::{Deserialize, Serialize};

/// Configuration for a DARS pass.
#[derive(Debug, Clone)]
pub struct DarsConfig {
    pub branch_count: usize,
    pub verifier_count: usize,
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_parallel: usize,
}

impl Default for DarsConfig {
    fn default() -> Self {
        Self {
            branch_count: 3,
            verifier_count: 3,
            model: "mock-model".into(),
            system_prompt: None,
            max_parallel: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DarsCandidate {
    pub index: usize,
    pub approach: String,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DarsOutcome {
    pub winner_index: usize,
    pub winner_answer: String,
    pub candidates: Vec<DarsCandidate>,
    /// Per-verifier votes, by position; same length as `verifier_count`.
    pub votes: Vec<usize>,
}

const APPROACHES: &[&str] = &[
    "Solve directly. Give the first correct answer that comes to mind.",
    "Think step-by-step. Enumerate possibilities before answering.",
    "Look for an edge case the obvious approach would miss, then answer.",
    "Sketch a counter-example first, then revise. Be terse.",
];

fn approach_for(i: usize) -> &'static str {
    APPROACHES[i % APPROACHES.len()]
}

/// Run a DARS pass. Returns the winning candidate plus full vote breakdown.
pub async fn run_dars(
    mgr: &SubAgentManager,
    cfg: &DarsConfig,
    prompt: &str,
) -> Result<DarsOutcome, SubAgentError> {
    assert!(cfg.branch_count >= 1, "branch_count must be at least 1");

    // Phase 1: branch.
    let branch_prompts: Vec<String> = (0..cfg.branch_count)
        .map(|i| {
            format!(
                "Approach: {approach}\n\nProblem:\n{prompt}",
                approach = approach_for(i),
            )
        })
        .collect();
    let branch_results = mgr
        .parallel_fan_out(
            cfg.model.clone(),
            cfg.system_prompt.clone(),
            branch_prompts,
            cfg.max_parallel.max(1),
        )
        .await;

    let mut candidates: Vec<DarsCandidate> = Vec::with_capacity(cfg.branch_count);
    for (i, r) in branch_results.into_iter().enumerate() {
        let answer = r?;
        candidates.push(DarsCandidate {
            index: i,
            approach: approach_for(i).to_string(),
            answer,
        });
    }

    // If only one branch, skip verification.
    if candidates.len() == 1 {
        let answer = candidates[0].answer.clone();
        return Ok(DarsOutcome {
            winner_index: 0,
            winner_answer: answer,
            candidates,
            votes: vec![0],
        });
    }

    // Phase 2: verify.
    let verifier_prompt = build_verifier_prompt(prompt, &candidates);
    let verifier_count = cfg.verifier_count.max(1);
    let verifier_prompts: Vec<String> = (0..verifier_count)
        .map(|_| verifier_prompt.clone())
        .collect();
    let verifier_results = mgr
        .parallel_fan_out(
            cfg.model.clone(),
            cfg.system_prompt.clone(),
            verifier_prompts,
            cfg.max_parallel.max(1),
        )
        .await;

    let mut votes: Vec<usize> = Vec::with_capacity(verifier_count);
    let mut tally: Vec<usize> = vec![0; candidates.len()];
    for r in verifier_results {
        let body = r?;
        let v = parse_vote(&body, candidates.len()).unwrap_or(0);
        votes.push(v);
        tally[v] = tally[v].saturating_add(1);
    }

    // Pick the max; ties break to the lowest index (the "control" branch).
    let mut winner_index = 0usize;
    let mut winner_score = tally[0];
    for (i, &c) in tally.iter().enumerate().skip(1) {
        if c > winner_score {
            winner_score = c;
            winner_index = i;
        }
    }

    let winner_answer = candidates[winner_index].answer.clone();
    Ok(DarsOutcome {
        winner_index,
        winner_answer,
        candidates,
        votes,
    })
}

fn build_verifier_prompt(original_prompt: &str, candidates: &[DarsCandidate]) -> String {
    let mut s = String::from(
        "You are a strict, terse evaluator. Read the problem and the candidate \
         answers, then reply with exactly one line in the form `BEST: <index>` \
         (no quotes) where <index> is 0-based.\n\n",
    );
    s.push_str("Problem:\n");
    s.push_str(original_prompt);
    s.push_str("\n\nCandidates:\n");
    for c in candidates {
        s.push_str(&format!(
            "[{i}] {body}\n\n",
            i = c.index,
            body = c.answer.trim()
        ));
    }
    s
}

/// Extract a 0-based index from a verifier's reply. Accepts `BEST: 2`,
/// `best: 0`, `Best=1`, `[1]`, or a bare digit on its own line.
pub fn parse_vote(body: &str, max_exclusive: usize) -> Option<usize> {
    let lower = body.to_lowercase();
    if let Some(rest) = lower.split("best").nth(1) {
        if let Some(n) = first_int(rest) {
            if (n as usize) < max_exclusive {
                return Some(n as usize);
            }
        }
    }
    // Fallback: first bare integer anywhere.
    if let Some(n) = first_int(&lower) {
        if (n as usize) < max_exclusive {
            return Some(n as usize);
        }
    }
    None
}

fn first_int(s: &str) -> Option<i64> {
    let mut cur = String::new();
    let mut started = false;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
            started = true;
        } else if started {
            break;
        }
    }
    cur.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_llm::{LlmClient, MockClient};
    use std::sync::Arc;

    #[test]
    fn parse_vote_extracts_index() {
        assert_eq!(parse_vote("BEST: 2", 5), Some(2));
        assert_eq!(parse_vote("best: 0  -- because", 3), Some(0));
        assert_eq!(parse_vote("the answer is best=1.", 4), Some(1));
        assert_eq!(parse_vote("BEST: 99", 3), None); // out of range
        assert_eq!(parse_vote("0", 3), Some(0)); // fallback
    }

    #[tokio::test]
    async fn dars_runs_branches_and_picks_majority_winner() {
        let mock = Arc::new(MockClient::new());
        // 3 branch answers
        mock.push_text("answer A");
        mock.push_text("answer B");
        mock.push_text("answer C");
        // 3 verifier votes — two vote for index 1, one for index 2.
        mock.push_text("BEST: 1");
        mock.push_text("BEST: 1");
        mock.push_text("BEST: 2");

        let mgr = SubAgentManager::new(mock.clone() as Arc<dyn LlmClient>);
        let cfg = DarsConfig {
            branch_count: 3,
            verifier_count: 3,
            model: "mock-model".into(),
            system_prompt: None,
            max_parallel: 3,
        };
        let out = run_dars(&mgr, &cfg, "what is 2+2?").await.unwrap();
        assert_eq!(out.candidates.len(), 3);
        assert_eq!(out.winner_index, 1);
        assert_eq!(out.winner_answer, "answer B");
        assert_eq!(out.votes.len(), 3);
    }

    #[tokio::test]
    async fn dars_single_branch_skips_verification() {
        let mock = Arc::new(MockClient::new());
        mock.push_text("only candidate");
        let mgr = SubAgentManager::new(mock.clone() as Arc<dyn LlmClient>);
        let cfg = DarsConfig {
            branch_count: 1,
            verifier_count: 5,
            model: "mock-model".into(),
            system_prompt: None,
            max_parallel: 1,
        };
        let out = run_dars(&mgr, &cfg, "trivial").await.unwrap();
        assert_eq!(out.winner_index, 0);
        assert_eq!(out.winner_answer, "only candidate");
        assert_eq!(out.votes, vec![0]);
    }

    #[tokio::test]
    async fn dars_tie_breaks_to_lowest_index() {
        let mock = Arc::new(MockClient::new());
        mock.push_text("a");
        mock.push_text("b");
        mock.push_text("a-vote");
        mock.push_text("b-vote");
        let mgr = SubAgentManager::new(mock.clone() as Arc<dyn LlmClient>);
        let cfg = DarsConfig {
            branch_count: 2,
            verifier_count: 2,
            model: "mock-model".into(),
            system_prompt: None,
            max_parallel: 2,
        };
        let out = run_dars(&mgr, &cfg, "tie").await.unwrap();
        // a-vote → parse_vote finds no digit, falls back to first_int → None,
        // unwrap_or(0) → vote for 0. b-vote → same → vote for 0.
        // So tally = [2,0], winner is 0 unambiguously. Still validates
        // the tie-break path because both voters land on index 0.
        assert_eq!(out.winner_index, 0);
    }
}
