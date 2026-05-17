use crate::tools::Tool;
use agent_tui_execpolicy::{ApprovalGate, EgressPolicy};
use agent_tui_protocol::AppMode;
use camino::Utf8PathBuf;
use std::collections::HashMap;
use std::sync::Arc;

/// Context handed to every tool invocation.
pub struct ToolContext {
    pub workspace_root: Utf8PathBuf,
    pub mode: AppMode,
    pub yolo: bool,
    pub egress: Arc<EgressPolicy>,
    pub approvals: Arc<ApprovalGate>,
}

impl ToolContext {
    pub fn new(workspace_root: Utf8PathBuf) -> Self {
        Self {
            workspace_root,
            mode: AppMode::Agent,
            yolo: false,
            egress: Arc::new(EgressPolicy::default()),
            approvals: Arc::new(ApprovalGate::new()),
        }
    }
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.values().cloned().collect()
    }

    /// Filter the registry for Plan mode: only read-only + planning tools.
    pub fn for_plan_mode(&self) -> Vec<Arc<dyn Tool>> {
        self.tools
            .values()
            .filter(|t| t.is_read_only() || t.is_plan_tool())
            .cloned()
            .collect()
    }

    pub fn with_builtins(workspace_root: Utf8PathBuf) -> (Self, ToolContext) {
        use crate::tools::builtins::*;
        let mut reg = Self::new();
        reg.register(Arc::new(ReadFileTool));
        reg.register(Arc::new(WriteFileTool));
        reg.register(Arc::new(EditFileTool));
        reg.register(Arc::new(ListDirTool));
        reg.register(Arc::new(SearchFilesTool));
        reg.register(Arc::new(GlobTool));
        reg.register(Arc::new(ShellTool));
        reg.register(Arc::new(FetchUrlTool));
        reg.register(Arc::new(GitStatusTool));
        reg.register(Arc::new(GitDiffTool));
        reg.register(Arc::new(GitCommitTool));
        reg.register(Arc::new(GitLogTool));
        reg.register(Arc::new(UpdatePlanTool));
        let ctx = ToolContext::new(workspace_root);
        (reg, ctx)
    }
}
