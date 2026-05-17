use agent_tui_acp::AcpServer;
use agent_tui_cli::{build_client, build_mock_client};
use agent_tui_config::{save_user, user_config_path, CliOverrides, Config};
use agent_tui_llm::{ChatRequest, LlmClient};
use agent_tui_protocol::{AppMode, Op, Provider};
use agent_tui_tui::run_tui;
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
        Some(Cmd::Fix { issue: _ }) => {
            eprintln!("`fix` is stubbed in Phase 2; lands in Phase 3.11");
            Ok(())
        }
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
    let (reg, mut ctx) = ToolRegistry::with_builtins(workspace);
    ctx.yolo = cfg.yolo || std::env::var("AGENT_TUI_YOLO").is_ok();
    let engine = Engine::new(session, client, Arc::new(reg), Arc::new(ctx));
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
    run_tui(client, workspace, model, cfg.yolo)
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
    loop {
        let (mut sock, _peer) = listener.accept().await?;
        let client = client.clone();
        let model = model.clone();
        let workspace = workspace.clone();
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
            let (reg, ctx) = ToolRegistry::with_builtins(workspace);
            let engine = Engine::new(session, client, Arc::new(reg), Arc::new(ctx));
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
