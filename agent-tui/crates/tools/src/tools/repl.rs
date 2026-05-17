//! REPL tools (extension 3.10).
//!
//! Stateful, named shell sessions exposed as three tools — `repl_open`,
//! `repl_eval`, `repl_close`. Each session owns a long-lived `/bin/sh`
//! subprocess and feeds it commands followed by a sentinel echo so we
//! can capture exactly the output of each `eval` independent of the
//! prompt timing.
//!
//! This is the Rust-only replacement for upstream's Python-kernel RLM
//! surface (ANALYSIS.md §11): the build plan forbids a Python runtime
//! dependency, so REPLs run plain shells instead and the model is
//! responsible for invoking `python`, `node`, etc. as needed.
//!
//! Sessions are only registered when `Extensions::repl_tools = true`.

use super::{Tool, ToolError, ToolResult};
use crate::registry::ToolContext;
use crate::schema::ToolSchema;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

const SENTINEL: &str = "__AGENT_TUI_REPL_DONE__";
const DEFAULT_EVAL_TIMEOUT_SECS: u64 = 30;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

/// A single live shell session.
struct ReplSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ReplSession {
    async fn spawn(workspace_root: &camino::Utf8Path) -> Result<Self, ToolError> {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-i")
            .current_dir(workspace_root.as_std_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Quiet shell — no prompt, no rc file noise, both stderr and
        // stdout go through our pipe.
        cmd.env("PS1", "")
            .env("PS2", "")
            .env("BASH_ENV", "")
            .env("ENV", "");
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(format!("spawn shell: {e}")))?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ToolError::ExecutionFailed("no stdin on shell child".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ToolError::ExecutionFailed("no stdout on shell child".into())
        })?;
        // Merge stderr into stdout so the model sees both streams in
        // submission order. We can't directly redirect with tokio, so
        // we tell the child to merge via `exec 2>&1`.
        let mut s = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        s.stdin.write_all(b"exec 2>&1\n").await
            .map_err(|e| ToolError::ExecutionFailed(format!("init shell: {e}")))?;
        Ok(s)
    }

    async fn eval(&mut self, code: &str, timeout_secs: u64) -> Result<String, ToolError> {
        // Write the user's code, then echo the sentinel so we know when
        // to stop reading.
        self.stdin
            .write_all(code.as_bytes())
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("write code: {e}")))?;
        if !code.ends_with('\n') {
            self.stdin.write_all(b"\n").await.ok();
        }
        let sentinel_cmd = format!("echo {SENTINEL}\n");
        self.stdin
            .write_all(sentinel_cmd.as_bytes())
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("write sentinel: {e}")))?;
        self.stdin.flush().await.ok();

        let mut buf = String::new();
        let read_fut = async {
            let mut line = String::new();
            loop {
                line.clear();
                let n = self
                    .stdout
                    .read_line(&mut line)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("read shell: {e}")))?;
                if n == 0 {
                    return Err(ToolError::ExecutionFailed(
                        "shell exited unexpectedly".into(),
                    ));
                }
                if line.trim_end() == SENTINEL {
                    return Ok::<(), ToolError>(());
                }
                if buf.len() + line.len() > MAX_OUTPUT_BYTES {
                    buf.push_str("\n[truncated]");
                    // keep draining to the sentinel without storing more
                    continue;
                }
                buf.push_str(&line);
            }
        };
        match timeout(Duration::from_secs(timeout_secs), read_fut).await {
            Ok(Ok(())) => Ok(buf),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ToolError::ExecutionFailed(format!(
                "eval timed out after {timeout_secs}s"
            ))),
        }
    }

    async fn close(mut self) {
        let _ = self.stdin.write_all(b"exit\n").await;
        let _ = self.stdin.flush().await;
        // best-effort; kill_on_drop covers the leak case
        let _ = timeout(Duration::from_secs(2), self.child.wait()).await;
    }
}

/// Session registry shared across `repl_*` tools. Wrapped in `Arc` so
/// `ToolContext` can hand out clones to each tool invocation.
#[derive(Default, Clone)]
pub struct ReplRegistry {
    inner: Arc<Mutex<HashMap<String, ReplSession>>>,
}

impl ReplRegistry {
    pub fn new() -> Self { Self::default() }

    pub async fn open(
        &self,
        name: String,
        workspace_root: &camino::Utf8Path,
    ) -> Result<(), ToolError> {
        let mut g = self.inner.lock().await;
        if g.contains_key(&name) {
            return Err(ToolError::InvalidInput(format!(
                "repl session `{name}` already open"
            )));
        }
        let s = ReplSession::spawn(workspace_root).await?;
        g.insert(name, s);
        Ok(())
    }

    pub async fn eval(
        &self,
        name: &str,
        code: &str,
        timeout_secs: u64,
    ) -> Result<String, ToolError> {
        let mut g = self.inner.lock().await;
        let s = g.get_mut(name).ok_or_else(|| {
            ToolError::InvalidInput(format!("repl session `{name}` not open"))
        })?;
        s.eval(code, timeout_secs).await
    }

    pub async fn close(&self, name: &str) -> Result<(), ToolError> {
        let s = {
            let mut g = self.inner.lock().await;
            g.remove(name)
        };
        if let Some(s) = s {
            s.close().await;
            Ok(())
        } else {
            Err(ToolError::InvalidInput(format!(
                "repl session `{name}` not open"
            )))
        }
    }

    pub async fn list(&self) -> Vec<String> {
        let g = self.inner.lock().await;
        g.keys().cloned().collect()
    }
}

// ---------- repl_open ----------

pub struct ReplOpenTool;

#[async_trait]
impl Tool for ReplOpenTool {
    fn name(&self) -> &str { "repl_open" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Open a long-lived shell session under the given name. State (cwd, \
             env vars, shell functions) persists between repl_eval calls.",
            json!({"type":"object","properties":{
                "name":{"type":"string","description":"Identifier for the session"}
            },"required":["name"]}),
        )
    }
    fn requires_approval(&self) -> bool { true }
    fn is_read_only(&self) -> bool { false }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::MissingField("name".into()))?
            .to_string();
        let reg = ctx
            .repl
            .as_ref()
            .ok_or_else(|| ToolError::NotAvailable)?;
        reg.open(name.clone(), &ctx.workspace_root).await?;
        Ok(ToolResult::ok(format!("opened repl `{name}`")))
    }
}

// ---------- repl_eval ----------

pub struct ReplEvalTool;

#[async_trait]
impl Tool for ReplEvalTool {
    fn name(&self) -> &str { "repl_eval" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Run a chunk of shell code in an open REPL session and return its \
             merged stdout+stderr. State persists across calls.",
            json!({"type":"object","properties":{
                "name":{"type":"string"},
                "code":{"type":"string"},
                "timeout_secs":{"type":"integer","default":30}
            },"required":["name","code"]}),
        )
    }
    fn requires_approval(&self) -> bool { true }
    fn is_read_only(&self) -> bool { false }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::MissingField("name".into()))?;
        let code = args
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::MissingField("code".into()))?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_EVAL_TIMEOUT_SECS);
        let reg = ctx
            .repl
            .as_ref()
            .ok_or_else(|| ToolError::NotAvailable)?;
        let out = reg.eval(name, code, timeout_secs).await?;
        Ok(ToolResult::ok(out))
    }
}

// ---------- repl_close ----------

pub struct ReplCloseTool;

#[async_trait]
impl Tool for ReplCloseTool {
    fn name(&self) -> &str { "repl_close" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Close a REPL session and release its shell process.",
            json!({"type":"object","properties":{
                "name":{"type":"string"}
            },"required":["name"]}),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { false }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::MissingField("name".into()))?;
        let reg = ctx
            .repl
            .as_ref()
            .ok_or_else(|| ToolError::NotAvailable)?;
        reg.close(name).await?;
        Ok(ToolResult::ok(format!("closed repl `{name}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    #[tokio::test]
    async fn eval_captures_stdout() {
        let r = ReplRegistry::new();
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
        r.open("s".into(), &root).await.unwrap();
        let out = r.eval("s", "echo hello", 5).await.unwrap();
        assert!(out.contains("hello"));
        r.close("s").await.unwrap();
    }

    #[tokio::test]
    async fn state_persists_between_evals() {
        let r = ReplRegistry::new();
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
        r.open("s".into(), &root).await.unwrap();
        let _ = r.eval("s", "X=42", 5).await.unwrap();
        let out = r.eval("s", "echo $X", 5).await.unwrap();
        assert!(out.contains("42"), "got: {out:?}");
        r.close("s").await.unwrap();
    }

    #[tokio::test]
    async fn double_open_errors() {
        let r = ReplRegistry::new();
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
        r.open("s".into(), &root).await.unwrap();
        let err = r.open("s".into(), &root).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
        r.close("s").await.unwrap();
    }

    #[tokio::test]
    async fn close_unknown_errors() {
        let r = ReplRegistry::new();
        let err = r.close("nope").await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn timeout_returns_error() {
        let r = ReplRegistry::new();
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
        r.open("s".into(), &root).await.unwrap();
        let err = r.eval("s", "sleep 5", 1).await.unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed(_)));
        // The shell is now in an unknown state; closing it is best effort.
        let _ = r.close("s").await;
    }
}
