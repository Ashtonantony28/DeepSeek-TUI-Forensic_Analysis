//! Exec / sandbox / approval policy.
//!
//! In Phase 2 the sandbox is intentionally lightweight: we centralise
//! _decisions_ (approval, network egress, secret redaction) here and
//! provide a `run_sandboxed` helper that shells out to `seccomp` /
//! `sandbox-exec` / Job Objects on the host. The OS hardening hooks
//! are stubbed (`SandboxBackend::None`) by default; Phase 5 packaging
//! turns them on per platform.

pub mod approval;
pub mod egress;
pub mod redact;
pub mod sandbox;

pub use approval::{ApprovalGate, ApprovalRequest, Decision};
pub use egress::{EgressPolicy, EgressVerdict};
pub use redact::redact_secrets;
pub use sandbox::{SandboxBackend, SandboxedCommand, SandboxedOutput};

use agent_tui_protocol::AppMode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("denied: {0}")]
    Denied(String),
    #[error("approval required")]
    ApprovalRequired,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequirement {
    /// Skip — auto-approved.
    Skip,
    /// Needs interactive approval.
    NeedsApproval,
    /// Forbidden regardless of approval.
    Forbidden,
}

#[derive(Debug, Clone)]
pub struct PolicyContext {
    pub mode: AppMode,
    pub yolo: bool,
}

impl PolicyContext {
    pub fn pre_decide(&self, requires_approval: bool) -> ApprovalRequirement {
        if self.yolo || self.mode == AppMode::Yolo {
            return ApprovalRequirement::Skip;
        }
        if requires_approval {
            ApprovalRequirement::NeedsApproval
        } else {
            ApprovalRequirement::Skip
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yolo_bypasses_approval() {
        let c = PolicyContext { mode: AppMode::Agent, yolo: true };
        assert_eq!(c.pre_decide(true), ApprovalRequirement::Skip);
    }

    #[test]
    fn agent_mode_requests_approval_for_destructive() {
        let c = PolicyContext { mode: AppMode::Agent, yolo: false };
        assert_eq!(c.pre_decide(true), ApprovalRequirement::NeedsApproval);
        assert_eq!(c.pre_decide(false), ApprovalRequirement::Skip);
    }
}
