//! Lightweight sub-agent sessions.
//!
//! Each session reuses the parent's `LlmClient`. The Python-kernel RLM
//! variant from upstream is intentionally NOT ported (build plan
//! forbids the Python runtime dependency; see ANALYSIS.md §11).
//!
//! `parallel_fan_out` runs up to N concurrent calls and returns all
//! results in order — this is the primitive used by Phase 3.8 (DARS,
//! see the `dars` module) and the later multi-verifier work.

pub mod dars;
pub use dars::{run_dars, DarsCandidate, DarsConfig, DarsOutcome};

use agent_tui_llm::{ChatRequest, LlmClient, StreamEvent};
use agent_tui_protocol::Message;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum SubAgentError {
    #[error("session not found: {0}")]
    NotFound(String),
    #[error("llm: {0}")]
    Llm(#[from] agent_tui_llm::LlmError),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubAgentId(pub String);

impl SubAgentId {
    pub fn new() -> Self {
        Self(format!("sub-{}", Uuid::new_v4()))
    }
}

impl Default for SubAgentId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct SubAgentState {
    pub system_prompt: Option<String>,
    pub model: String,
    pub messages: Vec<Message>,
}

#[derive(Clone)]
pub struct SubAgentManager {
    llm: Arc<dyn LlmClient>,
    sessions: Arc<Mutex<HashMap<SubAgentId, SubAgentState>>>,
}

impl SubAgentManager {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        Self {
            llm,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn open(&self, model: impl Into<String>, system: Option<String>) -> SubAgentId {
        let id = SubAgentId::new();
        self.sessions.lock().await.insert(
            id.clone(),
            SubAgentState {
                system_prompt: system,
                model: model.into(),
                messages: Vec::new(),
            },
        );
        id
    }

    pub async fn eval(&self, id: &SubAgentId, prompt: String) -> Result<String, SubAgentError> {
        let state = {
            let g = self.sessions.lock().await;
            g.get(id)
                .cloned()
                .ok_or_else(|| SubAgentError::NotFound(id.0.clone()))?
        };
        let mut messages = state.messages.clone();
        messages.push(Message::user_text(prompt.clone()));
        let mut req = ChatRequest::new(state.model.clone());
        req.system = state.system_prompt.clone();
        req.messages = messages.clone();
        req.max_output_tokens = Some(1024);
        let mut stream = self.llm.stream(req).await?;
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            if let Ok(StreamEvent::TextDelta(t)) = ev {
                text.push_str(&t);
            }
        }
        let mut g = self.sessions.lock().await;
        if let Some(s) = g.get_mut(id) {
            s.messages.push(Message::user_text(prompt));
            s.messages.push(Message::assistant_text(text.clone()));
        }
        Ok(text)
    }

    pub async fn close(&self, id: &SubAgentId) {
        self.sessions.lock().await.remove(id);
    }

    /// Fan-out a list of prompts across fresh sub-agents (each gets its
    /// own session), bounded by `max_parallel`.
    pub async fn parallel_fan_out(
        &self,
        model: impl Into<String>,
        system: Option<String>,
        prompts: Vec<String>,
        max_parallel: usize,
    ) -> Vec<Result<String, SubAgentError>> {
        let model = model.into();
        let mut results: Vec<Option<Result<String, SubAgentError>>> =
            (0..prompts.len()).map(|_| None).collect();
        let mut tasks = FuturesUnordered::new();
        let mut iter = prompts.into_iter().enumerate();
        let mut active = 0usize;

        loop {
            while active < max_parallel {
                if let Some((idx, prompt)) = iter.next() {
                    let mgr = self.clone();
                    let model = model.clone();
                    let system = system.clone();
                    tasks.push(async move {
                        let id = mgr.open(model, system).await;
                        let res = mgr.eval(&id, prompt).await;
                        mgr.close(&id).await;
                        (idx, res)
                    });
                    active += 1;
                } else {
                    break;
                }
            }
            if active == 0 {
                break;
            }
            if let Some((idx, res)) = tasks.next().await {
                results[idx] = Some(res);
                active -= 1;
            }
        }

        results.into_iter().map(|r| r.expect("scheduled")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_llm::MockClient;

    #[tokio::test]
    async fn open_eval_close_roundtrip() {
        let mock = Arc::new(MockClient::new());
        mock.push_text("response-A");
        let mgr = SubAgentManager::new(mock.clone() as Arc<dyn LlmClient>);
        let id = mgr.open("mock-model", Some("be brief".into())).await;
        let out = mgr.eval(&id, "hi".into()).await.unwrap();
        assert!(out.contains("response-A"));
        mgr.close(&id).await;
    }

    #[tokio::test]
    async fn fan_out_runs_all() {
        let mock = Arc::new(MockClient::new());
        for _ in 0..3 {
            mock.push_text("ok");
        }
        let mgr = SubAgentManager::new(mock.clone() as Arc<dyn LlmClient>);
        let r = mgr
            .parallel_fan_out(
                "mock-model",
                None,
                vec!["a".into(), "b".into(), "c".into()],
                2,
            )
            .await;
        assert_eq!(r.len(), 3);
        for x in r {
            x.unwrap();
        }
    }
}
