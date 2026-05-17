//! ACP (Agent Client Protocol) server over stdio.
//!
//! Implements the subset Phase 2 requires:
//!   - `initialize`            -> advertise capabilities
//!   - `session/new`           -> create a session
//!   - `session/load`          -> resume an existing session
//!   - `session/prompt`        -> stream a prompt, return text + tool events
//!   - `fs/read_text_file`     -> read a text file
//!   - `fs/write_text_file`    -> write a text file
//!
//! Transport: line-delimited JSON-RPC 2.0 over stdin/stdout (matches the
//! ACP reference behaviour; Zed and JetBrains both speak this).

use agent_tui_agent::{Engine, Session};
use agent_tui_llm::LlmClient;
use agent_tui_protocol::{AppMode, Event, Op, SessionId};
use agent_tui_tools::ToolRegistry;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdin, Stdout};
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum AcpError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rpc: {0}")]
    Rpc(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcReq {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResp<T: Serialize> {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)] // wired in Phase 3 for tool-call streaming
struct JsonRpcNotify {
    jsonrpc: &'static str,
    method: String,
    params: Value,
}

pub struct AcpServer {
    llm: Arc<dyn LlmClient>,
    workspace_root: Utf8PathBuf,
    sessions: Arc<Mutex<HashMap<SessionId, agent_tui_agent::EngineHandle>>>,
    default_model: String,
}

impl AcpServer {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        workspace_root: Utf8PathBuf,
        default_model: String,
    ) -> Self {
        Self {
            llm,
            workspace_root,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            default_model,
        }
    }

    /// Run the server, reading from `stdin` and writing JSON-RPC frames
    /// to `stdout`. One frame per line.
    pub async fn run_stdio(self) -> Result<(), AcpError> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        self.run(stdin, stdout).await
    }

    pub async fn run(self, stdin: Stdin, mut stdout: Stdout) -> Result<(), AcpError> {
        let mut reader = BufReader::new(stdin).lines();
        let me = Arc::new(self);
        while let Some(line) = reader.next_line().await? {
            if line.trim().is_empty() { continue; }
            let req: JsonRpcReq = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let resp = JsonRpcResp::<Value> {
                        jsonrpc: "2.0",
                        id: Value::Null,
                        result: None,
                        error: Some(JsonRpcError { code: -32700, message: e.to_string() }),
                    };
                    write_line(&mut stdout, &resp).await?;
                    continue;
                }
            };
            let id = req.id.clone().unwrap_or(Value::Null);
            let server = me.clone();
            let res = server.dispatch(&req).await;
            match res {
                Ok(value) => {
                    let resp = JsonRpcResp {
                        jsonrpc: "2.0",
                        id,
                        result: Some(value),
                        error: None,
                    };
                    write_line(&mut stdout, &resp).await?;
                }
                Err(e) => {
                    let resp = JsonRpcResp::<Value> {
                        jsonrpc: "2.0",
                        id,
                        result: None,
                        error: Some(JsonRpcError { code: -32603, message: e.to_string() }),
                    };
                    write_line(&mut stdout, &resp).await?;
                }
            }
        }
        Ok(())
    }

    async fn dispatch(&self, req: &JsonRpcReq) -> Result<Value, AcpError> {
        match req.method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": { "image": true },
                },
                "serverInfo": {
                    "name": "agent-tui",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            })),
            "session/new" => {
                let id = self.new_session().await;
                Ok(json!({ "sessionId": id.0 }))
            }
            "session/load" => {
                // Phase 2: idempotent — return the existing id if known,
                // else create one with the supplied id.
                let sid = req
                    .params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| AcpError::Rpc("missing sessionId".into()))?;
                self.ensure_session(SessionId(sid.clone())).await;
                Ok(json!({ "sessionId": sid }))
            }
            "session/prompt" => {
                let sid = req
                    .params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| AcpError::Rpc("missing sessionId".into()))?;
                let prompt = req
                    .params
                    .get("prompt")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| AcpError::Rpc("missing prompt".into()))?;
                let h = self.ensure_session(SessionId(sid.clone())).await;
                h.send(Op::Submit {
                    content: prompt,
                    mode: AppMode::Yolo,
                    model: None,
                    provider: None,
                }).await.map_err(|e| AcpError::Rpc(e.to_string()))?;
                // Drain events into a single response payload.
                let mut text = String::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                for _ in 0..256 {
                    let Some(ev) = h.next_event().await else { break };
                    match ev {
                        Event::Delta { delta, .. } => text.push_str(&delta),
                        Event::ToolCallStarted { tool_call_id, name, input, .. } => {
                            tool_calls.push(json!({
                                "id": tool_call_id.0,
                                "name": name,
                                "input": input,
                            }));
                        }
                        Event::TurnComplete { .. } => break,
                        _ => {}
                    }
                }
                Ok(json!({
                    "sessionId": sid,
                    "text": text,
                    "toolCalls": tool_calls,
                }))
            }
            "fs/read_text_file" => {
                let path = req.params.get("path").and_then(Value::as_str)
                    .ok_or_else(|| AcpError::Rpc("missing path".into()))?;
                let abs = self.resolve(path)?;
                let body = std::fs::read_to_string(abs.as_std_path())?;
                Ok(json!({ "content": body }))
            }
            "fs/write_text_file" => {
                let path = req.params.get("path").and_then(Value::as_str)
                    .ok_or_else(|| AcpError::Rpc("missing path".into()))?;
                let content = req.params.get("content").and_then(Value::as_str)
                    .ok_or_else(|| AcpError::Rpc("missing content".into()))?;
                let abs = self.resolve(path)?;
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent.as_std_path())?;
                }
                std::fs::write(abs.as_std_path(), content)?;
                Ok(json!({ "ok": true }))
            }
            other => Err(AcpError::Rpc(format!("unknown method: {other}"))),
        }
    }

    fn resolve(&self, rel: &str) -> Result<Utf8PathBuf, AcpError> {
        let p = if rel.starts_with('/') {
            Utf8PathBuf::from(rel)
        } else {
            self.workspace_root.join(rel)
        };
        Ok(p)
    }

    async fn new_session(&self) -> SessionId {
        let id = SessionId::new();
        self.ensure_session(id.clone()).await;
        id
    }

    async fn ensure_session(&self, id: SessionId) -> agent_tui_agent::EngineHandle {
        let mut g = self.sessions.lock().await;
        if let Some(h) = g.get(&id) {
            return h.clone();
        }
        let session = Session::new(self.default_model.clone());
        let (reg, ctx) = ToolRegistry::with_builtins(self.workspace_root.clone());
        let engine = Engine::new(session, self.llm.clone(), Arc::new(reg), Arc::new(ctx));
        let h = engine.spawn();
        g.insert(id, h.clone());
        h
    }
}

async fn write_line<T: Serialize>(stdout: &mut Stdout, value: &T) -> Result<(), AcpError> {
    let s = serde_json::to_string(value)?;
    stdout.write_all(s.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

/// Helper: synchronously handle a single JSON-RPC string and produce a
/// response string. Used by tests so we don't need real stdio plumbing.
pub async fn handle_one(server: &AcpServer, body: &str) -> Result<String, AcpError> {
    let req: JsonRpcReq = serde_json::from_str(body)?;
    let id = req.id.clone().unwrap_or(Value::Null);
    match server.dispatch(&req).await {
        Ok(value) => Ok(serde_json::to_string(&JsonRpcResp {
            jsonrpc: "2.0",
            id,
            result: Some(value),
            error: None,
        })?),
        Err(e) => Ok(serde_json::to_string(&JsonRpcResp::<Value> {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code: -32603, message: e.to_string() }),
        })?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_llm::MockClient;

    #[tokio::test]
    async fn initialize_advertises_caps() {
        let llm = Arc::new(MockClient::new());
        let s = AcpServer::new(llm as Arc<dyn LlmClient>, Utf8PathBuf::from("."), "mock-model".into());
        let out = handle_one(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        ).await.unwrap();
        assert!(out.contains("loadSession"));
        assert!(out.contains("agent-tui"));
    }

    #[tokio::test]
    async fn new_session_returns_id() {
        let llm = Arc::new(MockClient::new());
        let s = AcpServer::new(llm as Arc<dyn LlmClient>, Utf8PathBuf::from("."), "mock-model".into());
        let out = handle_one(
            &s,
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}"#,
        ).await.unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["result"]["sessionId"].as_str().unwrap().starts_with("sess-"));
    }

    #[tokio::test]
    async fn prompt_returns_streamed_text() {
        let llm = Arc::new(MockClient::new());
        llm.push_text("hi from acp");
        let s = AcpServer::new(llm.clone() as Arc<dyn LlmClient>, Utf8PathBuf::from("."), "mock-model".into());
        let new = handle_one(
            &s,
            r#"{"jsonrpc":"2.0","id":3,"method":"session/new","params":{}}"#,
        ).await.unwrap();
        let nv: Value = serde_json::from_str(&new).unwrap();
        let sid = nv["result"]["sessionId"].as_str().unwrap().to_string();
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"session/prompt","params":{{"sessionId":"{sid}","prompt":"hello"}}}}"#
        );
        let resp = handle_one(&s, &req).await.unwrap();
        assert!(resp.contains("hi from acp"));
    }

    #[tokio::test]
    async fn fs_roundtrip() {
        let td = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(td.path().to_path_buf()).unwrap();
        let llm = Arc::new(MockClient::new());
        let s = AcpServer::new(llm as Arc<dyn LlmClient>, root, "mock-model".into());
        let _ = handle_one(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"fs/write_text_file","params":{"path":"a.txt","content":"hi"}}"#,
        ).await.unwrap();
        let r = handle_one(
            &s,
            r#"{"jsonrpc":"2.0","id":2,"method":"fs/read_text_file","params":{"path":"a.txt"}}"#,
        ).await.unwrap();
        assert!(r.contains("\"content\":\"hi\""));
    }
}
