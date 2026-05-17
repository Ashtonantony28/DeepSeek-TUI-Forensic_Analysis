//! Sandboxed command execution.
//!
//! Backends per OS:
//!   - Linux: `firejail` if present, otherwise `seccomp` filter (Phase 5
//!     hardening). Phase 2 falls through to the host's shell with no
//!     additional sandboxing if neither is present.
//!   - macOS: `sandbox-exec` with a generated profile.
//!   - Windows: Job Object with kill-on-close.
//!
//! In Phase 2 the default backend is `None` (no extra sandboxing) — the
//! API surface is the goal so Phase 5 can swap implementations without
//! engine changes.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SandboxBackend {
    None,
    #[cfg(target_os = "linux")]
    Seccomp,
    #[cfg(target_os = "macos")]
    SandboxExec,
    #[cfg(target_os = "windows")]
    JobObject,
}

#[derive(Debug, Clone)]
pub struct SandboxedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<Utf8PathBuf>,
    pub env: Vec<(String, String)>,
    pub backend: SandboxBackend,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct SandboxedOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

impl SandboxedCommand {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            backend: SandboxBackend::None,
            timeout_secs: 60,
        }
    }

    pub async fn run(self) -> Result<SandboxedOutput, std::io::Error> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd.as_str());
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let dur = std::time::Duration::from_secs(self.timeout_secs);
        let fut = cmd.output();
        match tokio::time::timeout(dur, fut).await {
            Ok(Ok(o)) => Ok(SandboxedOutput {
                stdout: crate::redact::redact_secrets(&String::from_utf8_lossy(&o.stdout)),
                stderr: crate::redact::redact_secrets(&String::from_utf8_lossy(&o.stderr)),
                exit_code: o.status.code(),
                timed_out: false,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(SandboxedOutput {
                stdout: String::new(),
                stderr: format!("timed out after {}s", self.timeout_secs),
                exit_code: None,
                timed_out: true,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_runs() {
        let mut c = SandboxedCommand::new("/bin/echo");
        c.args = vec!["hello".into()];
        let out = c.run().await.unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(out.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn timeout_works() {
        let mut c = SandboxedCommand::new("/bin/sh");
        c.args = vec!["-c".into(), "sleep 5".into()];
        c.timeout_secs = 1;
        let out = c.run().await.unwrap();
        assert!(out.timed_out);
    }
}
