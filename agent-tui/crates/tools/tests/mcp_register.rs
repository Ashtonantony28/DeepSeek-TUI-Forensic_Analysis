//! 3.12 — `ToolRegistry::register_mcp_tools` adds qualified adapters.

use agent_tui_mcp::{
    McpError, McpManagedClient, McpManager, McpServerConfig, McpToolDescriptor,
};
use agent_tui_tools::{ToolContext, ToolRegistry};
use async_trait::async_trait;
use camino::Utf8PathBuf;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

struct FakeClient {
    server: String,
}

#[async_trait]
impl McpManagedClient for FakeClient {
    async fn list_tools(&self) -> Result<Vec<McpToolDescriptor>, McpError> {
        Ok(vec![
            McpToolDescriptor {
                server_name: self.server.clone(),
                tool_name: "alpha".into(),
                description: Some("first".into()),
                input_schema: json!({"type":"object","properties":{}}),
            },
            McpToolDescriptor {
                server_name: self.server.clone(),
                tool_name: "beta".into(),
                description: None,
                input_schema: json!({"type":"object","properties":{}}),
            },
        ])
    }
    async fn call_tool(&self, name: &str, _args: Value) -> Result<Value, McpError> {
        Ok(json!({"content":[{"type":"text","text":format!("called {name}")}]}))
    }
}

#[tokio::test]
async fn register_mcp_tools_adds_qualified_adapters() {
    let mut mgr = McpManager::new();
    let cfg = McpServerConfig {
        name: "svc".into(),
        command: "x".into(),
        args: vec![],
        env: HashMap::new(),
        enabled: true,
    };
    mgr.register(cfg, Arc::new(FakeClient { server: "svc".into() }));

    let (mut reg, _ctx) = ToolRegistry::with_builtins(Utf8PathBuf::from("/tmp"));
    let n = reg.register_mcp_tools(&mgr).await.unwrap();
    assert_eq!(n, 2);

    let alpha = reg.get("svc:alpha").expect("alpha registered");
    let beta = reg.get("svc:beta").expect("beta registered");
    assert_eq!(alpha.name(), "svc:alpha");
    assert_eq!(beta.name(), "svc:beta");

    // Dispatch through the adapter.
    let ctx = ToolContext::new(Utf8PathBuf::from("/tmp"));
    let out = alpha.execute(json!({}), &ctx).await.unwrap();
    assert!(out.content.contains("called alpha"));
}
