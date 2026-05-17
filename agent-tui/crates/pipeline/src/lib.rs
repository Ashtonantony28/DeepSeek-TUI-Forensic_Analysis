//! Hierarchical "Agentless-style" pipeline (extension 3.11).
//!
//! Three deterministic stages running over an `LlmClient` + `Retriever`:
//!
//!   1. **Localise** — query the retriever with the issue title/body and
//!      collect up to `top_k` hits. The hits are deduplicated by path so
//!      one file with N matches becomes one localised candidate.
//!
//!   2. **Repair** — bundle the issue and the (path, snippet) candidates
//!      into a single sub-agent prompt that explicitly asks for a
//!      unified diff. The sub-agent's reply is scanned for a fenced
//!      `diff` block (or a bare `--- a/`/`+++ b/` patch); the first
//!      patch found is returned.
//!
//!   3. **Validate** — re-parse the diff to confirm it is well-formed,
//!      then optionally hand it to the caller's `Validator` (e.g. the
//!      auto-test runner). When no validator is supplied, validation
//!      succeeds on a syntactically clean diff.
//!
//! This module is provider-agnostic: it takes an `Arc<dyn LlmClient>`
//! and an `Arc<dyn Retriever>` so the eval harness, CLI `fix`, and the
//! interactive agent all use the same code path.

use agent_tui_llm::LlmClient;
use agent_tui_retrieval::{Retriever, RetrievalHit};
use agent_tui_subagent::SubAgentManager;
use async_trait::async_trait;
use camino::Utf8PathBuf;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub failing_tests: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patch {
    pub unified_diff: String,
}

#[derive(Debug, Clone)]
pub struct PipelineContext {
    pub workspace_root: Utf8PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalisedCandidate {
    pub path: Utf8PathBuf,
    /// One representative line snippet — useful for the repair prompt.
    pub snippet: String,
    /// Sum of retrieval scores across all hits in this file.
    pub score: f32,
    /// Total number of underlying hits that contributed.
    pub hit_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineTrace {
    pub localised: Vec<LocalisedCandidate>,
    pub repair_attempts: u32,
}

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("retrieval failed: {0}")]
    Retrieval(String),
    #[error("repair failed: {0}")]
    Repair(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("no candidates localised")]
    NoCandidates,
    #[error("no diff in repair response")]
    NoDiff,
    #[error("{0}")]
    Other(String),
}

#[async_trait]
pub trait Pipeline: Send + Sync {
    async fn run(&self, issue: &Issue, ctx: &PipelineContext) -> Result<Patch, PipelineError>;
}

/// Optional post-repair validator. The simplest implementation re-runs
/// the project's test suite; eval drivers can plug in semantic checks.
#[async_trait]
pub trait Validator: Send + Sync {
    async fn validate(&self, patch: &Patch, ctx: &PipelineContext) -> Result<(), String>;
}

/// `Validator` that accepts every patch. Used in tests and as the default
/// when the caller doesn't want to gate on tests.
pub struct AcceptAllValidator;

#[async_trait]
impl Validator for AcceptAllValidator {
    async fn validate(&self, _patch: &Patch, _ctx: &PipelineContext) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct HierarchicalPipeline {
    pub llm: Arc<dyn LlmClient>,
    pub retriever: Arc<dyn Retriever>,
    pub model: String,
    pub top_k_hits: usize,
    pub max_candidates: usize,
    pub repair_attempts: u32,
    pub validator: Option<Arc<dyn Validator>>,
}

impl HierarchicalPipeline {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        retriever: Arc<dyn Retriever>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            llm,
            retriever,
            model: model.into(),
            top_k_hits: 32,
            max_candidates: 6,
            repair_attempts: 1,
            validator: None,
        }
    }

    pub fn with_validator(mut self, v: Arc<dyn Validator>) -> Self {
        self.validator = Some(v);
        self
    }

    pub async fn localise(
        &self,
        issue: &Issue,
    ) -> Result<Vec<LocalisedCandidate>, PipelineError> {
        let query = format!("{}\n{}", issue.title, issue.body);
        let hits: Vec<RetrievalHit> = self
            .retriever
            .search(&query, self.top_k_hits)
            .await
            .map_err(|e| PipelineError::Retrieval(e.to_string()))?;

        let mut by_path: std::collections::HashMap<Utf8PathBuf, LocalisedCandidate> =
            Default::default();
        for h in hits {
            let entry = by_path.entry(h.path.clone()).or_insert(LocalisedCandidate {
                path: h.path.clone(),
                snippet: h.snippet.clone(),
                score: 0.0,
                hit_count: 0,
            });
            entry.score += h.score;
            entry.hit_count += 1;
            if entry.snippet.len() < h.snippet.len() {
                entry.snippet = h.snippet.clone();
            }
        }
        let mut out: Vec<LocalisedCandidate> = by_path.into_values().collect();
        out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(self.max_candidates);
        Ok(out)
    }

    pub async fn repair(
        &self,
        issue: &Issue,
        candidates: &[LocalisedCandidate],
    ) -> Result<Patch, PipelineError> {
        if candidates.is_empty() {
            return Err(PipelineError::NoCandidates);
        }
        let mgr = SubAgentManager::new(self.llm.clone());
        let mut last_err: Option<String> = None;
        for _ in 0..self.repair_attempts.max(1) {
            let id = mgr.open(&self.model, Some(REPAIR_SYSTEM_PROMPT.into())).await;
            let prompt = build_repair_prompt(issue, candidates);
            let res = mgr.eval(&id, prompt).await;
            mgr.close(&id).await;
            let body = match res {
                Ok(b) => b,
                Err(e) => {
                    last_err = Some(e.to_string());
                    continue;
                }
            };
            if let Some(diff) = extract_diff(&body) {
                return Ok(Patch { unified_diff: diff });
            }
            last_err = Some("no diff in repair response".into());
        }
        match last_err {
            Some(_) => Err(PipelineError::NoDiff),
            None => Err(PipelineError::Repair("no repair attempts".into())),
        }
    }

    pub async fn validate(&self, patch: &Patch, ctx: &PipelineContext) -> Result<(), PipelineError> {
        if !looks_like_unified_diff(&patch.unified_diff) {
            return Err(PipelineError::Validation("patch is not a unified diff".into()));
        }
        if let Some(v) = &self.validator {
            v.validate(patch, ctx)
                .await
                .map_err(PipelineError::Validation)?;
        }
        Ok(())
    }
}

#[async_trait]
impl Pipeline for HierarchicalPipeline {
    async fn run(&self, issue: &Issue, ctx: &PipelineContext) -> Result<Patch, PipelineError> {
        let candidates = self.localise(issue).await?;
        if candidates.is_empty() {
            return Err(PipelineError::NoCandidates);
        }
        let patch = self.repair(issue, &candidates).await?;
        self.validate(&patch, ctx).await?;
        Ok(patch)
    }
}

const REPAIR_SYSTEM_PROMPT: &str =
    "You are a code repair sub-agent. Read the issue and the localised \
     candidate files, then output exactly one unified diff that fixes \
     the issue. The diff MUST start with `diff --git`, `--- a/`, or be \
     fenced as ```diff ... ```. Do not include explanatory prose.";

fn build_repair_prompt(issue: &Issue, candidates: &[LocalisedCandidate]) -> String {
    let mut s = String::new();
    s.push_str("Issue title: ");
    s.push_str(&issue.title);
    s.push_str("\n\nIssue body:\n");
    s.push_str(&issue.body);
    if !issue.failing_tests.is_empty() {
        s.push_str("\n\nFailing tests:\n");
        for t in &issue.failing_tests {
            s.push_str("- ");
            s.push_str(t);
            s.push('\n');
        }
    }
    s.push_str("\n\nLocalised candidate files (top suspects):\n");
    for c in candidates {
        s.push_str(&format!(
            "- {} (score={:.2}, hits={})\n  snippet: {}\n",
            c.path,
            c.score,
            c.hit_count,
            c.snippet.trim(),
        ));
    }
    s.push_str(
        "\nReturn a single unified diff that makes the failing tests \
         pass. No prose.\n",
    );
    s
}

fn extract_diff(body: &str) -> Option<String> {
    // First, try fenced ```diff``` blocks.
    let fence_re = Regex::new(r"(?s)```(?:diff)?\s*\n(.*?)```").ok()?;
    for caps in fence_re.captures_iter(body) {
        if let Some(m) = caps.get(1) {
            let txt = m.as_str().trim_start_matches('\n');
            if looks_like_unified_diff(txt) {
                return Some(txt.to_string());
            }
        }
    }
    // Second, accept a bare diff starting at `diff --git` or `--- a/`.
    for marker in ["diff --git", "--- a/"] {
        if let Some(idx) = body.find(marker) {
            let tail = &body[idx..];
            if looks_like_unified_diff(tail) {
                return Some(tail.to_string());
            }
        }
    }
    None
}

fn looks_like_unified_diff(text: &str) -> bool {
    let has_header = text.contains("diff --git ") || text.contains("--- a/");
    let has_hunk = text.contains("@@") || text.contains("+++ b/");
    has_header && has_hunk
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_llm::MockClient;
    use agent_tui_retrieval::{RetrievalError, Retriever};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct FakeRetriever(Vec<RetrievalHit>);

    #[async_trait]
    impl Retriever for FakeRetriever {
        async fn search(
            &self,
            _q: &str,
            _k: usize,
        ) -> Result<Vec<RetrievalHit>, RetrievalError> {
            Ok(self.0.clone())
        }
    }

    fn hit(path: &str, line: u32, snippet: &str, score: f32) -> RetrievalHit {
        RetrievalHit {
            path: Utf8PathBuf::from(path),
            line,
            snippet: snippet.into(),
            score,
        }
    }

    #[test]
    fn extract_diff_finds_fenced_block() {
        let body = "Sure, here's the fix:\n```diff\ndiff --git a/foo b/foo\n--- a/foo\n+++ b/foo\n@@ -1 +1 @@\n-hi\n+hello\n```";
        let d = extract_diff(body).unwrap();
        assert!(d.starts_with("diff --git"));
        assert!(d.contains("@@"));
    }

    #[test]
    fn extract_diff_finds_bare_diff() {
        let body = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n";
        let d = extract_diff(body).unwrap();
        assert!(d.starts_with("diff --git"));
    }

    #[test]
    fn extract_diff_rejects_non_diff_text() {
        assert!(extract_diff("nothing to see here").is_none());
        // A code fence with non-diff content
        assert!(extract_diff("```\nprint('hi')\n```").is_none());
    }

    #[tokio::test]
    async fn localise_deduplicates_by_path_and_ranks() {
        let mock = Arc::new(MockClient::new());
        let r = Arc::new(FakeRetriever(vec![
            hit("a.rs", 1, "fn foo()", 1.0),
            hit("a.rs", 4, "fn bar()", 1.5),
            hit("b.rs", 2, "fn baz()", 0.5),
        ])) as Arc<dyn Retriever>;
        let p = HierarchicalPipeline::new(mock as Arc<dyn LlmClient>, r, "mock-model");
        let issue = Issue { title: "x".into(), body: "y".into(), failing_tests: vec![] };
        let cands = p.localise(&issue).await.unwrap();
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].path.to_string(), "a.rs");
        assert!(cands[0].score >= cands[1].score);
        assert_eq!(cands[0].hit_count, 2);
    }

    #[tokio::test]
    async fn full_pipeline_runs_against_mock() {
        let mock = Arc::new(MockClient::new());
        mock.push_text(
            "Here's the fix:\n```diff\ndiff --git a/a.rs b/a.rs\n\
            --- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n```",
        );
        let r = Arc::new(FakeRetriever(vec![hit("a.rs", 1, "fn foo()", 1.0)]))
            as Arc<dyn Retriever>;
        let p = HierarchicalPipeline::new(mock as Arc<dyn LlmClient>, r, "mock-model");
        let issue = Issue {
            title: "foo broken".into(),
            body: "calling foo() returns wrong value".into(),
            failing_tests: vec!["a::foo_works".into()],
        };
        let ctx = PipelineContext { workspace_root: Utf8PathBuf::from(".") };
        let patch = p.run(&issue, &ctx).await.unwrap();
        assert!(patch.unified_diff.contains("diff --git"));
        assert!(patch.unified_diff.contains("@@"));
    }

    #[tokio::test]
    async fn repair_errors_when_model_yields_no_diff() {
        let mock = Arc::new(MockClient::new());
        mock.push_text("Sorry, I can't help with that.");
        let r = Arc::new(FakeRetriever(vec![hit("a.rs", 1, "fn foo()", 1.0)]))
            as Arc<dyn Retriever>;
        let p = HierarchicalPipeline::new(mock as Arc<dyn LlmClient>, r, "mock-model");
        let issue = Issue { title: "x".into(), body: "y".into(), failing_tests: vec![] };
        let ctx = PipelineContext { workspace_root: Utf8PathBuf::from(".") };
        let err = p.run(&issue, &ctx).await.unwrap_err();
        assert!(matches!(err, PipelineError::NoDiff));
    }

    #[tokio::test]
    async fn validator_can_reject_patch() {
        struct Rejector;
        #[async_trait]
        impl Validator for Rejector {
            async fn validate(&self, _: &Patch, _: &PipelineContext) -> Result<(), String> {
                Err("tests still fail".into())
            }
        }
        let mock = Arc::new(MockClient::new());
        mock.push_text("```diff\ndiff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n```");
        let r = Arc::new(FakeRetriever(vec![hit("a", 1, "x", 1.0)])) as Arc<dyn Retriever>;
        let p = HierarchicalPipeline::new(mock as Arc<dyn LlmClient>, r, "mock-model")
            .with_validator(Arc::new(Rejector));
        let issue = Issue { title: "x".into(), body: "y".into(), failing_tests: vec![] };
        let ctx = PipelineContext { workspace_root: Utf8PathBuf::from(".") };
        let err = p.run(&issue, &ctx).await.unwrap_err();
        assert!(matches!(err, PipelineError::Validation(_)));
    }

    #[tokio::test]
    async fn empty_retrieval_returns_no_candidates() {
        let mock = Arc::new(MockClient::new());
        let r = Arc::new(FakeRetriever(vec![])) as Arc<dyn Retriever>;
        let p = HierarchicalPipeline::new(mock as Arc<dyn LlmClient>, r, "mock-model");
        let issue = Issue { title: "x".into(), body: "y".into(), failing_tests: vec![] };
        let ctx = PipelineContext { workspace_root: Utf8PathBuf::from(".") };
        let err = p.run(&issue, &ctx).await.unwrap_err();
        assert!(matches!(err, PipelineError::NoCandidates));
    }
}
