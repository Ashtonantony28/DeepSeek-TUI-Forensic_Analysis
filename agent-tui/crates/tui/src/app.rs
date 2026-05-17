use agent_tui_agent::{Engine, EngineHandle, Session};
use agent_tui_llm::LlmClient;
use agent_tui_protocol::{AppMode, DeltaChannel, Event, Op, PlanItem, Provider};
use agent_tui_tools::ToolRegistry;
use camino::Utf8PathBuf;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyCode, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::sync::Arc;

pub struct App {
    pub session_id: String,
    pub mode: AppMode,
    pub provider: Provider,
    pub model: String,
    pub composer: String,
    pub transcript: Vec<TranscriptEntry>,
    pub status: String,
    pub show_palette: bool,
    pub palette_input: String,
    pub context_used_ratio: f32,
    pub session_cost_usd: f64,
    /// 3.7 — current plan rendered in the side panel.
    pub plan_goal: Option<String>,
    pub plan_items: Vec<PlanItem>,
}

pub enum TranscriptEntry {
    User(String),
    AssistantText(String),
    AssistantThinking(String),
    ToolCall { name: String, input: String },
    ToolResult { name: String, output: String, is_error: bool },
    System(String),
}

impl App {
    pub fn new(model: String, provider: Provider, session_id: String) -> Self {
        Self {
            session_id,
            mode: AppMode::Agent,
            provider,
            model,
            composer: String::new(),
            transcript: Vec::new(),
            status: "ready".into(),
            show_palette: false,
            palette_input: String::new(),
            context_used_ratio: 0.0,
            session_cost_usd: 0.0,
            plan_goal: None,
            plan_items: Vec::new(),
        }
    }

    fn push_assistant_delta(&mut self, channel: DeltaChannel, delta: String) {
        match channel {
            DeltaChannel::Text => {
                if let Some(TranscriptEntry::AssistantText(s)) = self.transcript.last_mut() {
                    s.push_str(&delta);
                    return;
                }
                self.transcript.push(TranscriptEntry::AssistantText(delta));
            }
            DeltaChannel::Thinking => {
                if let Some(TranscriptEntry::AssistantThinking(s)) = self.transcript.last_mut() {
                    s.push_str(&delta);
                    return;
                }
                self.transcript.push(TranscriptEntry::AssistantThinking(delta));
            }
        }
    }
}

/// Knobs the CLI uses to thread `Extensions` config flags through to the
/// engine. Defaults match `Extensions::default()` so passing
/// `EngineKnobs::default()` keeps the historic behaviour.
#[derive(Debug, Clone)]
pub struct EngineKnobs {
    pub checkpoint_enabled: bool,
    pub auto_test_enabled: bool,
    pub memory_enabled: bool,
    pub routing_enabled: bool,
    pub plan_blocks_enabled: bool,
    pub dars_enabled: bool,
    pub dars_verifier_count: usize,
    pub compaction_enabled: bool,
    pub repl_tools_enabled: bool,
}

impl Default for EngineKnobs {
    fn default() -> Self {
        Self {
            checkpoint_enabled: true,
            auto_test_enabled: true,
            memory_enabled: true,
            routing_enabled: false,
            plan_blocks_enabled: true,
            dars_enabled: true,
            dars_verifier_count: 3,
            compaction_enabled: true,
            repl_tools_enabled: false,
        }
    }
}

pub async fn run_tui(
    llm: Arc<dyn LlmClient>,
    workspace_root: Utf8PathBuf,
    model: String,
    yolo: bool,
    knobs: EngineKnobs,
) -> Result<(), Box<dyn std::error::Error>> {
    let session = Session::new(model.clone());
    let session_id = session.id.0.clone();
    let (mut reg, mut ctx) = ToolRegistry::with_builtins(workspace_root.clone());
    ctx.yolo = yolo;
    ToolRegistry::enable_hybrid_retrieval(&mut ctx).await;
    if knobs.repl_tools_enabled {
        reg.enable_repl_tools(&mut ctx);
    }
    let provider = llm.provider();
    let mut engine = Engine::new(session, llm.clone(), Arc::new(reg), Arc::new(ctx));
    engine.checkpoint_enabled = knobs.checkpoint_enabled;
    engine.auto_test_enabled = knobs.auto_test_enabled;
    engine.memory_enabled = knobs.memory_enabled;
    engine.routing_enabled = knobs.routing_enabled;
    engine.plan_blocks_enabled = knobs.plan_blocks_enabled;
    engine.dars_enabled = knobs.dars_enabled;
    engine.dars_verifier_count = knobs.dars_verifier_count;
    engine.compaction_enabled = knobs.compaction_enabled;
    let handle = engine.spawn();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;
    let mut app = App::new(model, provider, session_id);
    let res = event_loop(&mut term, &mut app, &handle).await;

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    term.show_cursor()?;
    let _ = handle.send(Op::Shutdown).await;
    res
}

async fn event_loop(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    handle: &EngineHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let tick = std::time::Duration::from_millis(80);
    let event_rx = handle.event_rx.clone();
    loop {
        term.draw(|f| crate::render::draw(f, app))?;

        // Drain a small batch of engine events first.
        let mut guard = event_rx.lock().await;
        while let Ok(ev) = guard.try_recv() {
            match ev {
                Event::TurnStarted { .. } => app.status = "thinking...".into(),
                Event::Delta { channel, delta, .. } => app.push_assistant_delta(channel, delta),
                Event::ToolCallStarted { name, input, .. } => {
                    app.transcript.push(TranscriptEntry::ToolCall {
                        name,
                        input: input.to_string(),
                    });
                }
                Event::ToolCallFinished { name, output, is_error, .. } => {
                    app.transcript.push(TranscriptEntry::ToolResult {
                        name,
                        output: output.to_string(),
                        is_error,
                    });
                }
                Event::TurnComplete { .. } => app.status = "ready".into(),
                Event::TurnAborted { reason, .. } => app.status = format!("aborted: {reason}"),
                Event::Status { message, .. } => app.status = message,
                Event::SeamApplied { level, tokens_archived, .. } => {
                    app.transcript.push(TranscriptEntry::System(format!(
                        "seam L{level} archived ~{tokens_archived} tokens"
                    )));
                }
                Event::CycleAdvanced { from, to } => {
                    app.transcript.push(TranscriptEntry::System(format!(
                        "cycle advanced {from} -> {to}"
                    )));
                }
                Event::CompactionApplied { before_tokens, after_tokens } => {
                    app.transcript.push(TranscriptEntry::System(format!(
                        "compaction {before_tokens} -> {after_tokens} tokens"
                    )));
                }
                Event::RiskBandChanged { .. } => {}
                Event::ApprovalRequest { .. } => {
                    app.status = "approval required".into();
                }
                Event::PlanUpdated { goal, items, .. } => {
                    app.plan_goal = Some(goal);
                    app.plan_items = items;
                }
                Event::DarsResult { winner_index, branch_count, votes } => {
                    app.transcript.push(TranscriptEntry::System(format!(
                        "dars: winner=[{winner_index}] of {branch_count} branches, votes={votes:?}"
                    )));
                }
                Event::Error { message } => app.status = format!("error: {message}"),
            }
        }
        drop(guard);

        if event::poll(tick)? {
            match event::read()? {
                CtEvent::Key(k) => {
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    match k.code {
                        KeyCode::Char('c') if ctrl => {
                            let _ = handle.send(Op::Cancel).await;
                        }
                        KeyCode::Char('d') if ctrl => break,
                        KeyCode::Char('k') if ctrl => {
                            app.show_palette = !app.show_palette;
                            app.palette_input.clear();
                        }
                        KeyCode::Tab => app.mode = app.mode.next(),
                        KeyCode::Esc => {
                            app.show_palette = false;
                        }
                        KeyCode::Enter => {
                            if app.show_palette {
                                app.show_palette = false;
                            } else if !app.composer.trim().is_empty() {
                                let content = std::mem::take(&mut app.composer);
                                app.transcript.push(TranscriptEntry::User(content.clone()));
                                let _ = handle.send(Op::Submit {
                                    content,
                                    mode: app.mode,
                                    model: None,
                                    provider: None,
                                }).await;
                            }
                        }
                        KeyCode::Backspace => {
                            if app.show_palette {
                                app.palette_input.pop();
                            } else {
                                app.composer.pop();
                            }
                        }
                        KeyCode::Char(c) => {
                            if app.show_palette {
                                app.palette_input.push(c);
                            } else {
                                app.composer.push(c);
                            }
                        }
                        _ => {}
                    }
                }
                CtEvent::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}
