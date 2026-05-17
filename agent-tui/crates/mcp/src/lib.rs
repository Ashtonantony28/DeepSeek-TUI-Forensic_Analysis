//! MCP client (stdio transport).
//!
//! Phase 2: minimal MCP-style JSON-RPC client over a child process's
//! stdin/stdout. Implements `initialize`, `tools/list`, `tools/call`.
//! Phase 5 packaging may migrate to the official `rmcp` crate.

use async_trait::async_trait;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum McpError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rpc error: {0}")]
    Rpc(String),
    #[error("server `{0}` not registered")]
    NotRegistered(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub server_name: String,
    pub tool_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

impl McpToolDescriptor {
    pub fn qualified_name(&self) -> String {
        format!("{}:{}", self.server_name, self.tool_name)
    }
}

#[async_trait]
pub trait McpManagedClient: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError>;
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, McpError>;
}

pub struct StdioMcpClient {
    inner: Arc<Mutex<StdioInner>>,
    server_name: String,
}

struct StdioInner {
    _child: Child,
    stdin: ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    next_id: AtomicU64,
}

impl StdioMcpClient {
    pub async fn spawn(cfg: &McpServerConfig) -> Result<Self, McpError> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .envs(&cfg.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let reader = BufReader::new(stdout);
        let inner = Arc::new(Mutex::new(StdioInner {
            _child: child,
            stdin,
            reader,
            next_id: AtomicU64::new(1),
        }));
        let me = Self {
            inner,
            server_name: cfg.name.clone(),
        };
        me.send_request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "agent-tui", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
        .await?;
        Ok(me)
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let mut g = self.inner.lock().await;
        let id = g.next_id.fetch_add(1, Ordering::SeqCst);
        let req = json!({
            "jsonrpc":"2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let body = serde_json::to_string(&req)?;
        g.stdin.write_all(body.as_bytes()).await?;
        g.stdin.write_all(b"\n").await?;
        g.stdin.flush().await?;

        let mut line = String::new();
        g.reader.read_line(&mut line).await?;
        let resp: Value = serde_json::from_str(line.trim())?;
        if let Some(err) = resp.get("error") {
            return Err(McpError::Rpc(err.to_string()));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[async_trait]
impl McpManagedClient for StdioMcpClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        let r = self.send_request("tools/list", json!({})).await?;
        let arr = r
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for t in arr {
            out.push(McpToolDescriptor {
                server_name: self.server_name.clone(),
                tool_name: t
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                description: t
                    .get("description")
                    .and_then(Value::as_str)
                    .map(String::from),
                input_schema: t.get("inputSchema").cloned().unwrap_or(json!({})),
            });
        }
        Ok(out)
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, McpError> {
        self.send_request("tools/call", json!({ "name": name, "arguments": args }))
            .await
    }
}

#[derive(Default)]
pub struct McpManager {
    clients: HashMap<String, Arc<dyn McpManagedClient>>,
    configs: HashMap<String, McpServerConfig>,
}

impl McpManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, cfg: McpServerConfig, client: Arc<dyn McpManagedClient>) {
        self.clients.insert(cfg.name.clone(), client);
        self.configs.insert(cfg.name.clone(), cfg);
    }

    /// 3.12 — look up the registered client by server name. Used by the
    /// tools registry when building adapters so each adapter shares the
    /// transport instead of re-spawning a child process.
    pub fn client_for(&self, server: &str) -> Option<Arc<dyn McpManagedClient>> {
        self.clients.get(server).cloned()
    }

    pub fn server_names(&self) -> Vec<String> {
        self.configs.keys().cloned().collect()
    }

    /// 3.12 — convenience: spawn every enabled server in `cfgs` as a
    /// `StdioMcpClient` and register the result. Servers that fail to
    /// spawn are returned in the error list; ones marked
    /// `enabled = false` are silently skipped.
    pub async fn spawn_and_register_all(
        &mut self,
        cfgs: Vec<McpServerConfig>,
    ) -> Vec<(String, McpError)> {
        let mut failed: Vec<(String, McpError)> = Vec::new();
        for cfg in cfgs {
            if !cfg.enabled {
                continue;
            }
            match StdioMcpClient::spawn(&cfg).await {
                Ok(c) => self.register(cfg, Arc::new(c)),
                Err(e) => failed.push((cfg.name.clone(), e)),
            }
        }
        failed
    }

    pub async fn list_all_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        let mut out = Vec::new();
        for c in self.clients.values() {
            let mut t = c.list_tools().await?;
            out.append(&mut t);
        }
        Ok(out)
    }

    pub async fn call(&self, server: &str, tool: &str, args: Value) -> Result<Value, McpError> {
        let c = self
            .clients
            .get(server)
            .ok_or_else(|| McpError::NotRegistered(server.into()))?;
        c.call_tool(tool, args).await
    }

    /// Load `~/.agent-tui/mcp.toml` or a given path.
    pub fn load_config(path: &Utf8PathBuf) -> Result<Vec<McpServerConfig>, McpError> {
        let raw = std::fs::read_to_string(path.as_std_path())?;
        let table: HashMap<String, McpServerConfig> =
            toml::from_str(&raw).map_err(|e| McpError::Rpc(format!("toml: {e}")))?;
        Ok(table.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeMcp;

    #[async_trait]
    impl McpManagedClient for FakeMcp {
        async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
            Ok(vec![McpToolDescriptor {
                server_name: "fake".into(),
                tool_name: "ping".into(),
                description: Some("ping".into()),
                input_schema: json!({"type":"object","properties":{}}),
            }])
        }
        async fn call_tool(&self, name: &str, _args: Value) -> Result<Value, McpError> {
            if name == "ping" {
                Ok(json!("pong"))
            } else {
                Err(McpError::Rpc("nope".into()))
            }
        }
    }

    #[tokio::test]
    async fn fake_lists_and_calls() {
        let mut m = McpManager::new();
        m.register(
            McpServerConfig {
                name: "fake".into(),
                command: "x".into(),
                args: vec![],
                env: HashMap::new(),
                enabled: true,
            },
            Arc::new(FakeMcp),
        );
        let tools = m.list_all_tools().await.unwrap();
        assert_eq!(tools.len(), 1);
        let r = m.call("fake", "ping", json!({})).await.unwrap();
        assert_eq!(r, json!("pong"));
    }

    #[tokio::test]
    async fn missing_server_errors() {
        let m = McpManager::new();
        let r = m.call("nope", "x", json!({})).await;
        assert!(matches!(r, Err(McpError::NotRegistered(_))));
    }
}
