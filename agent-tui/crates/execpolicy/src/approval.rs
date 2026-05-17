//! Per-tool approval gate built on a tokio oneshot channel per request.
//!
//! Engine flow:
//!   1. Tool calls `gate.request(req)` -> awaits a `Decision`.
//!   2. UI sees the `ApprovalRequest` event, prompts the user.
//!   3. UI sends `gate.resolve(approval_id, Decision)` to wake the awaiter.

use agent_tui_protocol::ApprovalId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approved,
    ApprovedForSession,
    Denied,
    Abort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub tool_name: String,
    pub summary: String,
}

#[derive(Default)]
pub struct ApprovalGate {
    pending: Mutex<HashMap<ApprovalId, oneshot::Sender<Decision>>>,
    session_approved_tools: Mutex<std::collections::HashSet<String>>,
}

impl ApprovalGate {
    pub fn new() -> Self { Self::default() }

    /// Register a request; returns the receiver to await on.
    pub fn register(&self, req: &ApprovalRequest) -> oneshot::Receiver<Decision> {
        if self
            .session_approved_tools
            .lock()
            .unwrap()
            .contains(&req.tool_name)
        {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Decision::Approved);
            return rx;
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(req.id.clone(), tx);
        rx
    }

    pub fn resolve(&self, id: &ApprovalId, decision: Decision, tool_name: &str) {
        if matches!(decision, Decision::ApprovedForSession) {
            self.session_approved_tools
                .lock()
                .unwrap()
                .insert(tool_name.to_string());
        }
        if let Some(tx) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.send(decision);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approve_resolves_pending() {
        let gate = ApprovalGate::new();
        let req = ApprovalRequest {
            id: ApprovalId::new(),
            tool_name: "shell".into(),
            summary: "ls".into(),
        };
        let rx = gate.register(&req);
        gate.resolve(&req.id, Decision::Approved, &req.tool_name);
        let d = rx.await.unwrap();
        assert_eq!(d, Decision::Approved);
    }

    #[tokio::test]
    async fn session_approval_short_circuits() {
        let gate = ApprovalGate::new();
        let req1 = ApprovalRequest {
            id: ApprovalId::new(),
            tool_name: "shell".into(),
            summary: "ls".into(),
        };
        let rx1 = gate.register(&req1);
        gate.resolve(&req1.id, Decision::ApprovedForSession, &req1.tool_name);
        let _ = rx1.await.unwrap();

        let req2 = ApprovalRequest {
            id: ApprovalId::new(),
            tool_name: "shell".into(),
            summary: "pwd".into(),
        };
        let rx2 = gate.register(&req2);
        let d2 = rx2.await.unwrap();
        assert_eq!(d2, Decision::Approved);
    }
}
