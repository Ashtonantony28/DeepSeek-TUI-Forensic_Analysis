use agent_tui_acp::AcpServer;
use agent_tui_cli::{build_client, build_mock_client};
use agent_tui_config::{save_user, user_config_path, CliOverrides, Config};
use agent_tui_llm::{ChatRequest, LlmClient};
use agent_tui_protocol::{AppMode, Op, Provider};
use agent_tui_tui::{run_tui, EngineKnobs};
use anyhow::{anyhow, Context, Result};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use futures::StreamExt;
use std::io::{self, BufRead, Write};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "agent-tui", version, about = "Multi-provider terminal coding agent")]
struct Cli {
    /// Override the provider.
    #[arg(long, global = true)]
    provider: Option<String>,
    /// Override the model name.
    #[arg(long, global = true)]
    model: Option<String>,
    /// Auto-approve all tool calls.
    #[arg(long, global = true)]
    yolo: bool,
    /// One-shot prompt (non-interactive).
    #[arg(short = 'p', long, global = true)]
    prompt: Option<String>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Save the API key for a provider into the user config.
    Login {
        #[arg(long)]
        provider: String,
        /// Read key from stdin if omitted.
        #[arg(long)]
        api_key: Option<String>,
    },
    /// Diagnose the setup.
    Doctor,
    /// List available models per provider.
    Models,
    /// Run a server.
    Serve {
        /// HTTP/SSE server.
        #[arg(long)]
        http: bool,
        /// ACP JSON-RPC server over stdio.
        #[arg(long)]
        acp: bool,
        /// Bind address for `--http`.
        #[arg(long, default_value = "127.0.0.1:7777")]
        addr: String,
    },
    /// Bypass agent loop and run the hierarchical pipeline (Phase 3.11).
    Fix {
        issue: String,
    },
    /// Rebuild the embedding/retrieval index for the workspace.
    Index {
        /// Skip the embeddings rebuild; just refresh the repo graph.
        #[arg(long)]
        graph_only: bool,
    },
    /// MCP server inspection and probing (Phase 3.12).
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
}

#[derive(Subcommand, Debug)]
enum McpCmd {
    /// List MCP servers from the merged config and the tools they expose.
    List,
    /// Spawn one MCP server by command + args and print its tools.
    Probe {
        /// Executable to run.
        #[arg(long)]
        command: String,
        /// Args to pass.
        #[arg(long, num_args = 0..)]
        args: Vec<String>,
        /// Optional server name for the qualified `server:tool` form.
        #[arg(long, default_value = "probe")]
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let cli = Cli::parse();
    let workspace = current_workspace_root()?;
    let overrides = CliOverrides {
        provider: cli.provider.as_deref().and_then(parse_provider),
        model: cli.model.clone(),
        yolo: if cli.yolo { Some(true) } else { None },
    };
    let cfg = agent_tui_config::load(&workspace, &overrides)
        .context("loading config")?;

    match cli.cmd {
        Some(Cmd::Login { provider, api_key }) => cmd_login(&provider, api_key, cfg).await,
        Some(Cmd::Doctor) => cmd_doctor(&cfg).await,
        Some(Cmd::Models) => cmd_models(&cfg).await,
        Some(Cmd::Serve { http, acp, addr }) => {
            cmd_serve(http, acp, addr, &cfg, workspace).await
        }
        Some(Cmd::Fix { issue }) => cmd_fix(issue, &cfg, workspace).await,
        Some(Cmd::Index { graph_only }) => cmd_index(workspace, graph_only).await,
        Some(Cmd::Mcp { cmd }) => cmd_mcp(cmd, &cfg).await,
        None => {
            if let Some(prompt) = cli.prompt {
                cmd_oneshot(prompt, &cfg, workspace).await
            } else {
                cmd_interactive(&cfg, workspace).await
            }
        }
    }
}

fn parse_provider(s: &str) -> Option<Provider> {
    match s.to_lowercase().as_str() {
        "anthropic" => Some(Provider::Anthropic),
        "openai" => Some(Provider::OpenAi),
        "deepseek" => Some(Provider::DeepSeek),
        "groq" => Some(Provider::Groq),
        "xai" => Some(Provider::Xai),
        "ollama" => Some(Provider::Ollama),
        "openai_compat" | "openai-compat" => Some(Provider::OpenAiCompat),
        _ => None,
    }
}

fn current_workspace_root() -> Result<Utf8PathBuf> {
    let pwd = std::env::current_dir()?;
    Utf8PathBuf::from_path_buf(pwd).map_err(|p| anyhow!("non-utf8 cwd: {}", p.display()))
}

fn resolve_provider(cfg: &Config) -> Provider {
    cfg.provider.unwrap_or(Provider::Anthropic)
}

fn resolve_model(cfg: &Config, provider: Provider) -> String {
    if let Some(m) = &cfg.model {
        return m.clone();
    }
    if let Some(pc) = cfg.providers.get(provider.as_str()) {
        if let Some(m) = &pc.default_model {
            return m.clone();
        }
    }
    match provider {
        Provider::Anthropic => "claude-opus-4-7".into(),
        Provider::OpenAi => "gpt-4o".into(),
        Provider::DeepSeek => "deepseek-chat".into(),
        Provider::Groq => "llama-3.1-70b-versatile".into(),
        Provider::Xai => "grok-2".into(),
        Provider::Ollama => "llama3.1".into(),
        Provider::OpenAiCompat => "default".into(),
    }
}

async fn cmd_login(
    provider: &str,
    api_key: Option<String>,
    mut cfg: Config,
) -> Result<()> {
    let p = parse_provider(provider).ok_or_else(|| anyhow!("unknown provider: {provider}"))?;
    let key = match api_key {
        Some(k) => k,
        None => {
            print!("API key for {provider}: ");
            io::stdout().flush().ok();
            let mut s = String::new();
            io::stdin().lock().read_line(&mut s)?;
            s.trim().to_string()
        }
    };
    let entry = cfg
        .providers
        .entry(p.as_str().to_string())
        .or_default();
    entry.api_key = Some(key);
    let path = save_user(&cfg).context("writing user config")?;
    println!("saved key for {provider} to {path}");
    Ok(())
}

async fn cmd_doctor(cfg: &Config) -> Result<()> {
    println!("agent-tui doctor:");
    println!("  rust  : ok ({})", env!("CARGO_PKG_RUST_VERSION"));
    let path = user_config_path()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "<none>".into());
    println!("  config: {path}");
    let providers: Vec<&str> = cfg
        .providers
        .iter()
        .filter(|(_, v)| v.api_key.is_some())
        .map(|(k, _)| k.as_str())
        .collect();
    if providers.is_empty() {
        println!("  keys  : (none configured — run `agent-tui login --provider ...`)");
    } else {
        println!("  keys  : {}", providers.join(", "));
    }
    println!("  tools : git={}", check_bin("git"));
    println!("  default provider: {}", resolve_provider(cfg).as_str());
    println!("  default model   : {}", resolve_model(cfg, resolve_provider(cfg)));
    Ok(())
}

fn check_bin(name: &str) -> &'static str {
    let r = std::process::Command::new(name).arg("--version").output();
    if r.is_ok() { "ok" } else { "MISSING" }
}

async fn cmd_models(cfg: &Config) -> Result<()> {
    let provider = resolve_provider(cfg);
    let client = build_client(provider, cfg);
    let models = client
        .list_models()
        .await
        .context("listing models")?;
    println!("{}:", provider.as_str());
    for m in models {
        println!(
            "  {} (ctx={}k, max_out={}, thinking={}, tools={})",
            m.name,
            m.context_window / 1000,
            m.max_output_tokens,
            m.supports_thinking,
            m.supports_tools,
        );
    }
    Ok(())
}

fn load_mcp_servers(cfg: &Config) -> Vec<agent_tui_mcp::McpServerConfig> {
    let Some(path) = &cfg.mcp_config_path else { return Vec::new() };
    match agent_tui_mcp::McpManager::load_config(path) {
        Ok(servers) => servers,
        Err(e) => {
            eprintln!("warning: failed to load mcp config {}: {}", path, e);
            Vec::new()
        }
    }
}

async fn cmd_mcp(cmd: McpCmd, cfg: &Config) -> Result<()> {
    use agent_tui_mcp::{McpManager, McpServerConfig, StdioMcpClient};
    use std::collections::HashMap;
    match cmd {
        McpCmd::List => {
            let servers = load_mcp_servers(cfg);
            if servers.is_empty() {
                println!("(no MCP servers configured — set `mcp_config_path` in ~/.agent-tui/config.toml)");
                return Ok(());
            }
            let mut mgr = McpManager::new();
            let failed = mgr.spawn_and_register_all(servers).await;
            for (name, err) in &failed {
                eprintln!("{name}: failed to spawn ({err})");
            }
            let tools = mgr
                .list_all_tools()
                .await
                .map_err(|e| anyhow!("list_tools: {e}"))?;
            if tools.is_empty() {
                println!("(servers up but advertised no tools)");
            }
            for t in tools {
                println!(
                    "{server}:{tool} — {desc}",
                    server = t.server_name,
                    tool = t.tool_name,
                    desc = t.description.as_deref().unwrap_or(""),
                );
            }
            Ok(())
        }
        McpCmd::Probe { command, args, name } => {
            let cfg = McpServerConfig {
                name: name.clone(),
                command,
                args,
                env: HashMap::new(),
                enabled: true,
            };
            let client = StdioMcpClient::spawn(&cfg)
                .await
                .map_err(|e| anyhow!("spawn: {e}"))?;
            let tools = agent_tui_mcp::McpManagedClient::list_tools(&client)
                .await
                .map_err(|e| anyhow!("list_tools: {e}"))?;
            if tools.is_empty() {
                println!("(server `{name}` advertised no tools)");
            }
            for t in tools {
                println!(
                    "{name}:{tool} — {desc}",
                    tool = t.tool_name,
                    desc = t.description.as_deref().unwrap_or(""),
                );
            }
            Ok(())
        }
    }
}

async fn cmd_fix(issue: String, cfg: &Config, workspace: Utf8PathBuf) -> Result<()> {
    use agent_tui_pipeline::{HierarchicalPipeline, Issue, Pipeline, PipelineContext};
    let provider = resolve_provider(cfg);
    let model = resolve_model(cfg, provider);
    let client: Arc<dyn LlmClient> = if std::env::var("AGENT_TUI_MOCK").is_ok() {
        build_mock_client()
    } else {
        build_client(provider, cfg)
    };

    // Build the same hybrid retriever the agent loop uses.
    let retriever = agent_tui_retrieval::HybridRetriever::new(workspace.clone())
        .prepare()
        .await;
    let retriever: Arc<dyn agent_tui_retrieval::Retriever> = Arc::new(retriever);

    // Treat the argument as either a path (read file as the issue body) or
    // a raw inline issue title.
    let (title, body) = match std::fs::read_to_string(&issue) {
        Ok(text) => {
            let first_line = text.lines().next().unwrap_or("").to_string();
            (first_line, text)
        }
        Err(_) => (issue.clone(), issue.clone()),
    };

    let pipeline = HierarchicalPipeline::new(client, retriever, model);
    let pctx = PipelineContext { workspace_root: workspace };
    let patch = pipeline
        .run(&Issue { title, body, failing_tests: vec![] }, &pctx)
        .await
        .map_err(|e| anyhow!("pipeline: {e}"))?;
    println!("{}", patch.unified_diff);
    Ok(())
}

async fn cmd_index(workspace: Utf8PathBuf, graph_only: bool) -> Result<()> {
    let g = agent_tui_retrieval::build_repo_graph(&workspace);
    eprintln!("graph: {} files, {} edges",
        g.len(),
        g.edges.iter().map(|e| e.len()).sum::<usize>(),
    );
    if graph_only {
        return Ok(());
    }
    let client = agent_tui_retrieval::EmbeddingClient::new();
    if !client.is_reachable().await {
        eprintln!(
            "ollama not reachable at {}; skipping embeddings (graph-only index)",
            client.base_url,
        );
        return Ok(());
    }
    eprintln!("indexing embeddings with model `{}`...", client.model);
    let idx = agent_tui_retrieval::build_or_update_embeddings(&workspace, &client)
        .await
        .map_err(|e| anyhow!("embedding build failed: {e}"))?;
    eprintln!("indexed {} chunks", idx.chunks.len());
    Ok(())
}

async fn cmd_oneshot(prompt: String, cfg: &Config, workspace: Utf8PathBuf) -> Result<()> {
    let provider = resolve_provider(cfg);
    let model = resolve_model(cfg, provider);
    let client: Arc<dyn LlmClient> = if std::env::var("AGENT_TUI_MOCK").is_ok() {
        // Prefill the mock so smoke tests see realistic tool + text flow.
        let mock = agent_tui_llm::MockClient::new();
        mock.push_tool_call(
            "read_file",
            serde_json::json!({"path": "README.md"}),
        );
        mock.push_text("(mock summary) README contents read successfully.");
        Arc::new(mock) as Arc<dyn LlmClient>
    } else {
        build_client(provider, cfg)
    };
    use agent_tui_agent::{Engine, Session};
    use agent_tui_tools::ToolRegistry;
    let session = Session::new(model);
    let (mut reg, mut ctx) = ToolRegistry::with_builtins(workspace);
    ctx.yolo = cfg.yolo || std::env::var("AGENT_TUI_YOLO").is_ok();
    ToolRegistry::enable_hybrid_retrieval(&mut ctx).await;
    if cfg.extensions.repl_tools {
        reg.enable_repl_tools(&mut ctx);
    }
    // 3.12 — register MCP tools from the merged config (if any).
    let mcp_servers = load_mcp_servers(cfg);
    if !mcp_servers.is_empty() {
        let mut mgr = agent_tui_mcp::McpManager::new();
        let failed = mgr.spawn_and_register_all(mcp_servers).await;
        for (name, err) in &failed {
            eprintln!("warning: mcp server `{name}` failed to spawn: {err}");
        }
        if let Err(e) = reg.register_mcp_tools(&mgr).await {
            eprintln!("warning: mcp tool registration failed: {e}");
        }
    }
    let mut engine = Engine::new(session, client, Arc::new(reg), Arc::new(ctx));
    apply_extensions(&mut engine, cfg);
    let h = engine.spawn();
    h.send(Op::Submit {
        content: prompt,
        mode: if ctx_yolo() { AppMode::Yolo } else { AppMode::Agent },
        model: None,
        provider: None,
    })
    .await
    .map_err(|e| anyhow!("send: {e}"))?;
    loop {
        let Some(ev) = h.next_event().await else { break };
        use agent_tui_protocol::Event;
        match ev {
            Event::Delta { delta, channel, .. } => match channel {
                agent_tui_protocol::DeltaChannel::Text => {
                    print!("{delta}");
                    let _ = io::stdout().flush();
                }
                _ => {}
            },
            Event::ToolCallStarted { name, .. } => {
                eprintln!("\n[tool: {name}]");
            }
            Event::TurnComplete { .. } => break,
            Event::Error { message } => {
                eprintln!("\n[error: {message}]");
                break;
            }
            _ => {}
        }
    }
    println!();
    let _ = h.send(Op::Shutdown).await;
    Ok(())
}

fn ctx_yolo() -> bool {
    std::env::var("AGENT_TUI_YOLO").is_ok()
}

async fn cmd_interactive(cfg: &Config, workspace: Utf8PathBuf) -> Result<()> {
    let provider = resolve_provider(cfg);
    let model = resolve_model(cfg, provider);
    let client = if std::env::var("AGENT_TUI_MOCK").is_ok() {
        build_mock_client()
    } else {
        build_client(provider, cfg)
    };
    let knobs = EngineKnobs {
        checkpoint_enabled: cfg.extensions.checkpoint_enabled,
        auto_test_enabled: cfg.extensions.auto_test,
        memory_enabled: cfg.extensions.memory_enabled,
        routing_enabled: cfg.extensions.routing == "auto",
        plan_blocks_enabled: cfg.extensions.plan_blocks,
        dars_enabled: cfg.extensions.dars_branching,
        dars_verifier_count: cfg.extensions.verifier_count as usize,
        compaction_enabled: cfg.extensions.compaction_enabled,
        repl_tools_enabled: cfg.extensions.repl_tools,
    };
    run_tui(client, workspace, model, cfg.yolo, knobs)
        .await
        .map_err(|e| anyhow!("tui: {e}"))
}

async fn cmd_serve(
    http: bool,
    acp: bool,
    addr: String,
    cfg: &Config,
    workspace: Utf8PathBuf,
) -> Result<()> {
    if acp {
        let provider = resolve_provider(cfg);
        let model = resolve_model(cfg, provider);
        let client = if std::env::var("AGENT_TUI_MOCK").is_ok() {
            build_mock_client()
        } else {
            build_client(provider, cfg)
        };
        let server = AcpServer::new(client, workspace, model);
        return server.run_stdio().await.map_err(|e| anyhow!("acp: {e}"));
    }
    if http {
        return serve_http(addr, cfg, workspace).await;
    }
    Err(anyhow!("specify --http or --acp"))
}

async fn serve_http(addr: String, cfg: &Config, workspace: Utf8PathBuf) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;
    let provider = resolve_provider(cfg);
    let model = resolve_model(cfg, provider);
    let client = if std::env::var("AGENT_TUI_MOCK").is_ok() {
        build_mock_client()
    } else {
        build_client(provider, cfg)
    };
    let listener = TcpListener::bind(&addr).await.context("bind")?;
    eprintln!("agent-tui http+sse listening on {addr}");
    let ext = cfg.extensions.clone();
    loop {
        let (mut sock, _peer) = listener.accept().await?;
        let client = client.clone();
        let model = model.clone();
        let workspace = workspace.clone();
        let ext = ext.clone();
        tokio::spawn(async move {
            let (rd, mut wr) = sock.split();
            let mut br = BufReader::new(rd);
            let mut request_line = String::new();
            if br.read_line(&mut request_line).await.is_err() {
                return;
            }
            // Burn through headers until empty line.
            loop {
                let mut hdr = String::new();
                if br.read_line(&mut hdr).await.unwrap_or(0) == 0 { break; }
                if hdr == "\r\n" || hdr == "\n" { break; }
            }
            // Phase 2: very small POST /v1/sessions surface. Body: { "prompt": "..." }
            // For Phase 2 the body parsing is best-effort; we just echo
            // a single SSE-formatted response.
            let _ = wr
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n",
                )
                .await;
            use agent_tui_agent::{Engine, Session};
            use agent_tui_tools::ToolRegistry;
            let session = Session::new(model);
            let (mut reg, mut ctx) = ToolRegistry::with_builtins(workspace);
            if ext.repl_tools {
                reg.enable_repl_tools(&mut ctx);
            }
            let mut engine = Engine::new(session, client, Arc::new(reg), Arc::new(ctx));
            engine.checkpoint_enabled = ext.checkpoint_enabled;
            engine.auto_test_enabled = ext.auto_test;
            engine.memory_enabled = ext.memory_enabled;
            engine.routing_enabled = ext.routing == "auto";
            engine.plan_blocks_enabled = ext.plan_blocks;
            engine.dars_enabled = ext.dars_branching;
            engine.dars_verifier_count = ext.verifier_count as usize;
            engine.compaction_enabled = ext.compaction_enabled;
            let h = engine.spawn();
            let _ = h
                .send(Op::Submit {
                    content: "hello (HTTP smoke test)".into(),
                    mode: AppMode::Agent,
                    model: None,
                    provider: None,
                })
                .await;
            loop {
                let Some(ev) = h.next_event().await else { break };
                let payload = serde_json::to_string(&ev).unwrap_or_default();
                let frame = format!("data: {payload}\n\n");
                if wr.write_all(frame.as_bytes()).await.is_err() {
                    break;
                }
                if matches!(ev, agent_tui_protocol::Event::TurnComplete { .. }) {
                    break;
                }
            }
            let _ = h.send(Op::Shutdown).await;
        });
    }
}

/// Cheap typing of helper to keep the chat-stream type from leaking into
/// `cmd_oneshot`'s prelude.
#[allow(dead_code)]
async fn _unused_drain(stream: &mut agent_tui_llm::ChatStream) {
    while let Some(_) = stream.next().await {}
}

#[allow(dead_code)]
fn _force_request() -> ChatRequest {
    ChatRequest::new("x")
}

#[allow(dead_code)]
fn _force_llm(_: Arc<dyn LlmClient>) {}

/// Map the [extensions] config block onto the engine flags. Phase 3.5–3.8
/// extensions are toggled here so every entrypoint (oneshot, interactive,
/// http, acp) gets the same behaviour.
fn apply_extensions(engine: &mut agent_tui_agent::Engine, cfg: &Config) {
    let ext = &cfg.extensions;
    engine.checkpoint_enabled = ext.checkpoint_enabled;
    engine.auto_test_enabled = ext.auto_test;
    engine.memory_enabled = ext.memory_enabled;
    engine.routing_enabled = ext.routing == "auto";
    engine.plan_blocks_enabled = ext.plan_blocks;
    engine.dars_enabled = ext.dars_branching;
    engine.dars_verifier_count = ext.verifier_count as usize;
    engine.compaction_enabled = ext.compaction_enabled;
}
