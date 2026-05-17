//! Agentless-style pipeline trait.
//!
//! Phase 2 ships only the trait + a placeholder implementation that
//! delegates to the free-form agent loop. The real localise → repair →
//! validate pipeline lands in Phase 3.11 alongside SBFL.

use async_trait::async_trait;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub title: String,
    pub body: String,
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

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("not implemented in Phase 2")]
    NotImplemented,
    #[error("{0}")]
    Other(String),
}

#[async_trait]
pub trait Pipeline: Send + Sync {
    async fn run(&self, issue: &Issue, ctx: &PipelineContext) -> Result<Patch, PipelineError>;
}

/// Placeholder used until Phase 3.11.
pub struct AgentLoopPipeline;

#[async_trait]
impl Pipeline for AgentLoopPipeline {
    async fn run(&self, _issue: &Issue, _ctx: &PipelineContext) -> Result<Patch, PipelineError> {
        Err(PipelineError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn placeholder_returns_not_implemented() {
        let issue = Issue { title: "x".into(), body: "y".into(), failing_tests: vec![] };
        let ctx = PipelineContext { workspace_root: Utf8PathBuf::from(".") };
        let r = AgentLoopPipeline.run(&issue, &ctx).await;
        assert!(matches!(r, Err(PipelineError::NotImplemented)));
    }
}
