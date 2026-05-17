//! Built-in tools: read/write/edit, list/search, shell, fetch, git.

use super::{Tool, ToolError, ToolResult};
use crate::registry::ToolContext;
use crate::schema::ToolSchema;
use crate::syntax;
use agent_tui_execpolicy::{EgressVerdict, SandboxedCommand};
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{json, Value};

// ---------- helpers ----------

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingField(key.to_string()))
}

fn resolve_path(ctx: &ToolContext, rel: &str) -> Result<Utf8PathBuf, ToolError> {
    let p = if rel.starts_with('/') {
        Utf8PathBuf::from(rel)
    } else {
        ctx.workspace_root.join(rel)
    };
    let canon = match std::fs::canonicalize(p.as_std_path()) {
        Ok(p) => Utf8PathBuf::from_path_buf(p).map_err(|_| ToolError::PathEscape(rel.into()))?,
        Err(_) => p, // for not-yet-existing files (write_file)
    };
    let root = match std::fs::canonicalize(ctx.workspace_root.as_std_path()) {
        Ok(p) => Utf8PathBuf::from_path_buf(p).unwrap_or(ctx.workspace_root.clone()),
        Err(_) => ctx.workspace_root.clone(),
    };
    if !canon.starts_with(&root) && !canon.starts_with(&ctx.workspace_root) {
        return Err(ToolError::PathEscape(rel.into()));
    }
    Ok(canon)
}

// ---------- read_file ----------

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Read the contents of a file relative to the workspace root.",
            json!({
                "type":"object",
                "properties": {
                    "path": {"type":"string","description":"Path relative to workspace root"}
                },
                "required":["path"]
            }),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path = require_str(&args, "path")?;
        let abs = resolve_path(ctx, path)?;
        let body = std::fs::read_to_string(abs.as_std_path())?;
        Ok(ToolResult::ok(body))
    }
}

// ---------- write_file ----------

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str { "write_file" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Create or overwrite a file. Runs a tree-sitter syntax check after writing.",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "content":{"type":"string"}
                },
                "required":["path","content"]
            }),
        )
    }
    fn requires_approval(&self) -> bool { true }
    fn is_read_only(&self) -> bool { false }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path = require_str(&args, "path")?;
        let content = require_str(&args, "content")?;
        let abs = resolve_path(ctx, path)?;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent.as_std_path())?;
        }
        std::fs::write(abs.as_std_path(), content)?;
        if let Err(e) = syntax::check_file(&abs, content) {
            // Revert? Keep the file on disk but report the failure to the
            // model so it can re-edit. Matches SWE-agent ACI behaviour.
            return Err(ToolError::Other(format!(
                "wrote file but syntax check failed: {} (line {}, col {})",
                e.message, e.row, e.column
            )));
        }
        Ok(ToolResult::ok(format!("wrote {} bytes to {}", content.len(), abs)))
    }
}

// ---------- edit_file (apply_patch with old/new pair) ----------

pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str { "edit_file" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Replace the first occurrence of `old_text` with `new_text` in `path`. \
             Runs a tree-sitter syntax check after edit.",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "old_text":{"type":"string"},
                    "new_text":{"type":"string"}
                },
                "required":["path","old_text","new_text"]
            }),
        )
    }
    fn requires_approval(&self) -> bool { true }
    fn is_read_only(&self) -> bool { false }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path = require_str(&args, "path")?;
        let old_text = require_str(&args, "old_text")?;
        let new_text = require_str(&args, "new_text")?;
        let abs = resolve_path(ctx, path)?;
        let current = std::fs::read_to_string(abs.as_std_path())?;
        let idx = current
            .find(old_text)
            .ok_or_else(|| ToolError::InvalidInput("old_text not found".into()))?;
        let mut next = String::with_capacity(current.len() - old_text.len() + new_text.len());
        next.push_str(&current[..idx]);
        next.push_str(new_text);
        next.push_str(&current[idx + old_text.len()..]);
        std::fs::write(abs.as_std_path(), &next)?;
        if let Err(e) = syntax::check_file(&abs, &next) {
            // Rollback so the workspace stays valid.
            let _ = std::fs::write(abs.as_std_path(), &current);
            return Err(ToolError::Other(format!(
                "edit rejected by syntax check: {} (line {}, col {})",
                e.message, e.row, e.column
            )));
        }
        Ok(ToolResult::ok(format!("edited {}", abs)))
    }
}

// ---------- list_dir ----------

pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str { "list_dir" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "List entries in a directory (non-recursive).",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let path = require_str(&args, "path")?;
        let abs = resolve_path(ctx, path)?;
        let mut names: Vec<String> = std::fs::read_dir(abs.as_std_path())?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        Ok(ToolResult::ok(names.join("\n")))
    }
}

// ---------- search_files (ripgrep-like via ignore crate) ----------

pub struct SearchFilesTool;

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str { "search_files" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Plain-text search across the workspace (respects .gitignore).",
            json!({"type":"object","properties":{
                "pattern":{"type":"string"},
                "max_results":{"type":"integer","default":50}
            },"required":["pattern"]}),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let pattern = require_str(&args, "pattern")?.to_string();
        let max = args
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(50) as usize;
        let root = ctx.workspace_root.clone();
        let hits = tokio::task::spawn_blocking(move || {
            let mut out: Vec<String> = Vec::new();
            let walker = ignore::WalkBuilder::new(root.as_std_path()).build();
            'outer: for dent in walker.filter_map(|e| e.ok()) {
                if !dent.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                if let Ok(body) = std::fs::read_to_string(dent.path()) {
                    for (i, line) in body.lines().enumerate() {
                        if line.contains(&pattern) {
                            out.push(format!("{}:{}: {}", dent.path().display(), i + 1, line));
                            if out.len() >= max {
                                break 'outer;
                            }
                        }
                    }
                }
            }
            out
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(ToolResult::ok(hits.join("\n")))
    }
}

// ---------- glob ----------

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str { "glob" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "List files matching a glob pattern (e.g. `**/*.rs`).",
            json!({"type":"object","properties":{"pattern":{"type":"string"}},"required":["pattern"]}),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let pat = require_str(&args, "pattern")?.to_string();
        let root = ctx.workspace_root.clone();
        let names = tokio::task::spawn_blocking(move || {
            let mut out: Vec<String> = Vec::new();
            for dent in walkdir::WalkDir::new(root.as_std_path())
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if dent.file_type().is_file() {
                    let p = dent.path().to_string_lossy().into_owned();
                    if glob_match(&pat, &p) {
                        out.push(p);
                    }
                }
            }
            out
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(ToolResult::ok(names.join("\n")))
    }
}

fn glob_match(pat: &str, s: &str) -> bool {
    // Convert pattern to regex.
    let mut re = String::with_capacity(pat.len() * 2);
    re.push('^');
    let mut chars = pat.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    re.push_str(".*");
                } else {
                    re.push_str("[^/]*");
                }
            }
            '?' => re.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '|' | '{' | '}' | '[' | ']' | '\\' | '^' | '$' => {
                re.push('\\');
                re.push(c);
            }
            _ => re.push(c),
        }
    }
    re.push('$');
    regex::Regex::new(&re).map(|r| r.is_match(s)).unwrap_or(false)
}

// ---------- shell ----------

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str { "shell" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Run a shell command (sandboxed). Requires approval unless --yolo.",
            json!({"type":"object","properties":{
                "command":{"type":"string"},
                "timeout_secs":{"type":"integer","default":60}
            },"required":["command"]}),
        )
    }
    fn requires_approval(&self) -> bool { true }
    fn is_read_only(&self) -> bool { false }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let command = require_str(&args, "command")?.to_string();
        let timeout = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(60);
        let mut cmd = SandboxedCommand::new("/bin/sh");
        cmd.args = vec!["-c".into(), command.clone()];
        cmd.cwd = Some(ctx.workspace_root.clone());
        cmd.timeout_secs = timeout;
        let out = cmd
            .run()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let mut body = String::new();
        if !out.stdout.is_empty() {
            body.push_str("stdout:\n");
            body.push_str(&out.stdout);
        }
        if !out.stderr.is_empty() {
            if !body.is_empty() { body.push('\n'); }
            body.push_str("stderr:\n");
            body.push_str(&out.stderr);
        }
        body.push_str(&format!(
            "\nexit={}{}",
            out.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            if out.timed_out { " (timed out)" } else { "" }
        ));
        Ok(ToolResult { content: body, is_error: out.exit_code != Some(0) })
    }
}

// ---------- fetch_url ----------

pub struct FetchUrlTool;

#[async_trait]
impl Tool for FetchUrlTool {
    fn name(&self) -> &str { "fetch_url" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "HTTP GET a URL; subject to the egress policy.",
            json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let url = require_str(&args, "url")?.to_string();
        match ctx.egress.check_url(&url) {
            EgressVerdict::Allow => {}
            EgressVerdict::Block(reason) => {
                return Err(ToolError::PermissionDenied(reason));
            }
        }
        let client = reqwest::Client::builder()
            .user_agent("agent-tui/0.1")
            .build()
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(ToolResult::ok(format!("HTTP {status}\n\n{body}")))
    }
}

// ---------- git_status / git_diff / git_commit / git_log ----------

async fn run_git(cwd: &Utf8Path, args: Vec<String>) -> Result<ToolResult, ToolError> {
    let mut cmd = SandboxedCommand::new("git");
    cmd.args = args;
    cmd.cwd = Some(cwd.to_owned());
    cmd.timeout_secs = 30;
    let out = cmd
        .run()
        .await
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
    let mut body = out.stdout;
    if !out.stderr.is_empty() {
        body.push_str(&out.stderr);
    }
    Ok(ToolResult {
        content: body,
        is_error: out.exit_code != Some(0),
    })
}

pub struct GitStatusTool;
#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str { "git_status" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(self.name(), "Run `git status --short`.", json!({"type":"object","properties":{}}))
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }
    async fn execute(&self, _: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        run_git(&ctx.workspace_root, vec!["status".into(), "--short".into()]).await
    }
}

pub struct GitDiffTool;
#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str { "git_diff" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Run `git diff [path]`.",
            json!({"type":"object","properties":{"path":{"type":"string"}}}),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut a = vec!["diff".to_string()];
        if let Some(p) = args.get("path").and_then(Value::as_str) {
            a.push(p.to_string());
        }
        run_git(&ctx.workspace_root, a).await
    }
}

pub struct GitCommitTool;
#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str { "git_commit" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Stage all changes and commit with the given message.",
            json!({"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}),
        )
    }
    fn requires_approval(&self) -> bool { true }
    fn is_read_only(&self) -> bool { false }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let msg = require_str(&args, "message")?.to_string();
        let add = run_git(&ctx.workspace_root, vec!["add".into(), "-A".into()]).await?;
        if add.is_error { return Ok(add); }
        run_git(&ctx.workspace_root, vec!["commit".into(), "-m".into(), msg]).await
    }
}

pub struct GitLogTool;
#[async_trait]
impl Tool for GitLogTool {
    fn name(&self) -> &str { "git_log" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Run `git log --oneline -n N`. Default N=20.",
            json!({"type":"object","properties":{"n":{"type":"integer","default":20}}}),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }
    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let n = args.get("n").and_then(Value::as_u64).unwrap_or(20);
        run_git(
            &ctx.workspace_root,
            vec!["log".into(), "--oneline".into(), "-n".into(), n.to_string()],
        )
        .await
    }
}

// ---------- update_plan (Plan-mode planning tool) ----------

pub struct UpdatePlanTool;
#[async_trait]
impl Tool for UpdatePlanTool {
    fn name(&self) -> &str { "update_plan" }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            self.name(),
            "Record or update the agent's plan (Plan mode).",
            json!({"type":"object","properties":{
                "goal":{"type":"string"},
                "steps":{"type":"array","items":{"type":"string"}}
            },"required":["goal","steps"]}),
        )
    }
    fn requires_approval(&self) -> bool { false }
    fn is_read_only(&self) -> bool { true }
    fn is_plan_tool(&self) -> bool { true }
    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolResult, ToolError> {
        let goal = require_str(&args, "goal")?;
        let steps: Vec<String> = args
            .get("steps")
            .and_then(|s| s.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut body = format!("Plan goal: {goal}\nSteps:\n");
        for (i, s) in steps.iter().enumerate() {
            body.push_str(&format!("  {}. {}\n", i + 1, s));
        }
        Ok(ToolResult::ok(body))
    }
}
