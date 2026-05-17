//! MCP tool adapter (extension 3.12).
//!
//! Each remote MCP tool is wrapped in an `McpToolAdapter` that
//! implements the local `Tool` trait so the agent loop can dispatch
//! through the same registry as built-in tools. The tool's `name()` is
//! the qualified `"server:tool"` form so collisions across MCP servers
//! are impossible.
//!
//! The adapter holds an `Arc<dyn McpManagedClient>` so the same
//! transport (e.g. a stdio child process) is reused across calls.

use super::{Tool, ToolError, ToolResult};
use crate::registry::ToolContext;
use crate::schema::ToolSchema;
use agent_tui_mcp::{McpManagedClient, McpToolDescriptor};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub struct McpToolAdapter {
    pub descriptor: McpToolDescriptor,
    pub client: Arc<dyn McpManagedClient>,
    /// Cached qualified name — `"server:tool"`. Stored once so `name()`
    /// can return a `&str` borrowed from the adapter itself.
    qualified: String,
}

impl McpToolAdapter {
    pub fn new(descriptor: McpToolDescriptor, client: Arc<dyn McpManagedClient>) -> Self {
        let qualified = descriptor.qualified_name();
        Self { descriptor, client, qualified }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str { &self.qualified }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            &self.qualified,
            self.descriptor
                .description
                .clone()
                .unwrap_or_else(|| format!(
                    "MCP tool `{}` on server `{}`",
                    self.descriptor.tool_name, self.descriptor.server_name,
                )),
            self.descriptor.input_schema.clone(),
        )
    }

    /// MCP tools are remote and side-effectful by default. We mark them
    /// as requiring approval and treat them as non-read-only so they go
    /// through the standard approval gate.
    fn requires_approval(&self) -> bool { true }
    fn is_read_only(&self) -> bool { false }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let res = self
            .client
            .call_tool(&self.descriptor.tool_name, args)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        // MCP returns a structured JSON envelope. Most servers respond
        // with `{ "content": [{ "type": "text", "text": "..." }] }`; we
        // flatten that for the model. If the shape is unknown, return
        // the raw JSON so the model can still reason about it.
        let body = if let Some(arr) = res.get("content").and_then(Value::as_array) {
            let mut s = String::new();
            for block in arr {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    if !s.is_empty() { s.push('\n'); }
                    s.push_str(t);
                }
            }
            if s.is_empty() { res.to_string() } else { s }
        } else {
            res.to_string()
        };
        let is_error = res
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(ToolResult { content: body, is_error })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_mcp::{McpError, McpToolDescriptor};
    use camino::Utf8PathBuf;
    use serde_json::json;

    struct FakeMcp;

    #[async_trait]
    impl McpManagedClient for FakeMcp {
        async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
            Ok(vec![])
        }
        async fn call_tool(&self, name: &str, _args: Value) -> Result<Value, McpError> {
            match name {
                "echo" => Ok(json!({"content":[{"type":"text","text":"hello"}]})),
                "raw" => Ok(json!({"weird":"shape"})),
                "fail" => Ok(json!({"content":[{"type":"text","text":"oops"}],"isError":true})),
                _ => Err(McpError::Rpc("unknown".into())),
            }
        }
    }

    fn adapter(name: &str) -> McpToolAdapter {
        McpToolAdapter::new(
            McpToolDescriptor {
                server_name: "fake".into(),
                tool_name: name.into(),
                description: Some(format!("test tool {name}")),
                input_schema: json!({"type":"object","properties":{}}),
            },
            Arc::new(FakeMcp),
        )
    }

    fn ctx() -> ToolContext {
        ToolContext::new(Utf8PathBuf::from("/tmp"))
    }

    #[tokio::test]
    async fn adapter_returns_qualified_name() {
        let a = adapter("echo");
        assert_eq!(a.name(), "fake:echo");
    }

    #[tokio::test]
    async fn adapter_flattens_text_blocks() {
        let a = adapter("echo");
        let r = a.execute(json!({}), &ctx()).await.unwrap();
        assert_eq!(r.content, "hello");
        assert!(!r.is_error);
    }

    #[tokio::test]
    async fn adapter_preserves_unknown_shapes() {
        let a = adapter("raw");
        let r = a.execute(json!({}), &ctx()).await.unwrap();
        assert!(r.content.contains("weird"));
    }

    #[tokio::test]
    async fn adapter_surfaces_is_error_flag() {
        let a = adapter("fail");
        let r = a.execute(json!({}), &ctx()).await.unwrap();
        assert!(r.is_error);
        assert!(r.content.contains("oops"));
    }
}
