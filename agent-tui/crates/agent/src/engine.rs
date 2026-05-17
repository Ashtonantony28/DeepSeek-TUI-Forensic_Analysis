//! Engine task. Owns a `Session` and drives the turn loop.

use crate::auto_test::{self, MAX_RETRIES, TestRunner};
use crate::parser::parse_tool_input;
use crate::session::Session;
use agent_tui_context::{
    CapacityController, CycleManager, SeamManager, estimate_tokens,
};
use agent_tui_llm::{
    ChatRequest, LlmClient, StreamEvent, ToolSchema as LlmToolSchema,
};
use agent_tui_protocol::{
    AppMode, ContentBlock, DeltaChannel, Event, GuardrailAction, Message, Op, Provider,
    RiskBand, Role, ToolCallId, TurnId,
};
use agent_tui_tools::{Tool, ToolContext, ToolRegistry};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub struct Engine {
    pub session: Session,
    pub mode: AppMode,
    pub yolo: bool,
    pub llm: Arc<dyn LlmClient>,
    pub tools: Arc<ToolRegistry>,
    pub tool_ctx: Arc<ToolContext>,
    pub seam: SeamManager,
    pub cycle: CycleManager,
    pub capacity: CapacityController,
    pub model_context_window: u32,
    pub checkpoint_enabled: bool,
    pub auto_test_enabled: bool,
}

#[derive(Clone)]
pub struct EngineHandle {
    pub op_tx: mpsc::Sender<Op>,
    pub event_rx: Arc<Mutex<mpsc::Receiver<Event>>>,
}

impl EngineHandle {
    pub async fn send(&self, op: Op) -> Result<(), mpsc::error::SendError<Op>> {
        self.op_tx.send(op).await
    }
    pub async fn next_event(&self) -> Option<Event> {
        self.event_rx.lock().await.recv().await
    }
}

impl Engine {
    pub fn new(
        session: Session,
        llm: Arc<dyn LlmClient>,
        tools: Arc<ToolRegistry>,
        tool_ctx: Arc<ToolContext>,
    ) -> Self {
        Self {
            session,
            mode: AppMode::Agent,
            yolo: false,
            llm,
            tools,
            tool_ctx,
            seam: SeamManager::default(),
            cycle: CycleManager::default(),
            capacity: CapacityController::default(),
            model_context_window: 200_000,
            checkpoint_enabled: true,
            auto_test_enabled: true,
        }
    }

    pub fn spawn(self) -> EngineHandle {
        let (op_tx, op_rx) = mpsc::channel::<Op>(32);
        let (event_tx, event_rx) = mpsc::channel::<Event>(256);
        tokio::spawn(self.run(op_rx, event_tx));
        EngineHandle {
            op_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
        }
    }

    async fn run(mut self, mut op_rx: mpsc::Receiver<Op>, event_tx: mpsc::Sender<Event>) {
        while let Some(op) = op_rx.recv().await {
            match op {
                Op::Submit { content, mode, model, provider: _ } => {
                    self.mode = mode;
                    if let Some(m) = model {
                        self.session.model = m;
                    }
                    self.session.messages.push(Message::user_text(content));
                    if let Err(e) = self.run_turn(&event_tx).await {
                        let _ = event_tx.send(Event::Error { message: e }).await;
                    }
                }
                Op::Cancel => {
                    let _ = event_tx.send(Event::Status {
                        turn_id: None,
                        message: "cancel acknowledged".into(),
                    }).await;
                }
                Op::ChangeMode { mode } => self.mode = mode,
                Op::SetModel { model, .. } => self.session.model = model,
                Op::CompactContext => {
                    // Compaction stub: actual flash call wires in Phase 3.
                    let _ = event_tx.send(Event::Status {
                        turn_id: None,
                        message: "compaction queued".into(),
                    }).await;
                }
                Op::AcceptApproval { id, decision } => {
                    self.tool_ctx
                        .approvals
                        .resolve(&id, decision_to_exec(decision), "pending");
                }
                Op::SpawnSubAgent { .. } => {
                    let _ = event_tx.send(Event::Status {
                        turn_id: None,
                        message: "sub-agent spawn (Phase 3.8 deferred)".into(),
                    }).await;
                }
                Op::Shutdown => break,
            }
        }
    }

    async fn run_turn(&mut self, event_tx: &mpsc::Sender<Event>) -> Result<(), String> {
        let turn_id = TurnId::new();
        let _ = event_tx.send(Event::TurnStarted { turn_id: turn_id.clone() }).await;

        // -- Pre-turn: context capacity & seam check --
        let total_tokens = estimate_tokens(&self.session.messages);
        let used_ratio = total_tokens as f32 / self.model_context_window.max(1) as f32;
        let band = self.capacity.observe(agent_tui_context::CapacityObservation {
            context_used_ratio: used_ratio,
            tool_calls_recent: 0,
            consecutive_tool_errors: 0,
        });
        let action = self.capacity.decide(0, band);
        if !matches!(action, GuardrailAction::NoIntervention)
            || !matches!(band, RiskBand::Low)
        {
            let _ = event_tx.send(Event::RiskBandChanged {
                turn_id: turn_id.clone(),
                band,
                action,
            }).await;
        }

        // Seam check (non-destructive); summary is a stub for Phase 2.
        if let Some(outcome) = self.seam.evaluate(&self.session.messages) {
            let level = outcome.level as u8;
            let archived = outcome.head_token_estimate;
            self.seam.apply_summary(
                &mut self.session.messages,
                &outcome,
                "[Phase-2 seam summary placeholder]".into(),
            );
            let _ = event_tx.send(Event::SeamApplied {
                turn_id: turn_id.clone(),
                level,
                tokens_archived: archived,
            }).await;
        }

        // Choose active tool set (Plan-mode narrows to read-only + planning).
        let active_tools: Vec<Arc<dyn Tool>> = match self.mode {
            AppMode::Plan => self.tools.for_plan_mode(),
            AppMode::Agent | AppMode::Yolo => self.tools.list(),
        };
        let tool_schemas: Vec<LlmToolSchema> = active_tools
            .iter()
            .map(|t| {
                let s = t.schema();
                LlmToolSchema {
                    name: s.name,
                    description: s.description,
                    input_schema: s.input_schema,
                }
            })
            .collect();

        // -- Stream from LLM. Loop until end_turn (no tool calls). --
        for _round in 0..8 {
            let mut req = ChatRequest::new(&self.session.model);
            req.system = self.session.system_prompt.clone();
            req.messages = self.session.messages.clone();
            req.tools = tool_schemas.clone();
            req.max_output_tokens = Some(2048);

            let mut stream = self
                .llm
                .stream(req)
                .await
                .map_err(|e| format!("llm error: {e}"))?;

            let mut assistant_text = String::new();
            let mut tool_calls: HashMap<ToolCallId, (String, String)> = HashMap::new();
            let mut tool_call_order: Vec<ToolCallId> = Vec::new();
            let mut stop_reason = String::from("end_turn");

            while let Some(ev_res) = stream.next().await {
                let ev = match ev_res {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = event_tx.send(Event::Error {
                            message: format!("stream error: {e}"),
                        }).await;
                        return Err(format!("stream: {e}"));
                    }
                };
                match ev {
                    StreamEvent::TextDelta(t) => {
                        let _ = event_tx.send(Event::Delta {
                            turn_id: turn_id.clone(),
                            channel: DeltaChannel::Text,
                            delta: t.clone(),
                        }).await;
                        assistant_text.push_str(&t);
                    }
                    StreamEvent::ThinkingDelta(t) => {
                        let _ = event_tx.send(Event::Delta {
                            turn_id: turn_id.clone(),
                            channel: DeltaChannel::Thinking,
                            delta: t,
                        }).await;
                    }
                    StreamEvent::ToolCallStart { id, name } => {
                        tool_calls.insert(id.clone(), (name, String::new()));
                        tool_call_order.push(id);
                    }
                    StreamEvent::ToolCallDelta { id, json_fragment } => {
                        if let Some(e) = tool_calls.get_mut(&id) {
                            e.1.push_str(&json_fragment);
                        }
                    }
                    StreamEvent::ToolCallEnd { .. } => {}
                    StreamEvent::MessageEnd { stop_reason: sr, .. } => {
                        stop_reason = sr;
                    }
                }
            }

            // Persist the assistant text/tool-use blocks for context.
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if !assistant_text.is_empty() {
                assistant_blocks.push(ContentBlock::Text { text: assistant_text });
            }
            for id in &tool_call_order {
                if let Some((name, buf)) = tool_calls.get(id) {
                    let parsed = parse_tool_input(buf).unwrap_or(serde_json::json!({}));
                    assistant_blocks.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: parsed,
                    });
                }
            }
            if !assistant_blocks.is_empty() {
                self.session.messages.push(Message {
                    role: Role::Assistant,
                    content: assistant_blocks,
                    metadata: Default::default(),
                });
            }

            // No tool calls or finished — done.
            if tool_call_order.is_empty() || stop_reason == "end_turn" || stop_reason == "stop" {
                break;
            }

            // Execute tool calls. Read-only -> in parallel; destructive -> serial.
            let mut tool_results: Vec<ContentBlock> = Vec::new();
            let mut any_destructive_success = false;
            for id in &tool_call_order {
                let (name, buf) = match tool_calls.get(id) {
                    Some(p) => p.clone(),
                    None => continue,
                };
                let Some(tool) = self.tools.get(&name) else {
                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!("tool `{name}` not available"),
                        is_error: true,
                    });
                    continue;
                };
                let input = parse_tool_input(&buf).unwrap_or(serde_json::json!({}));
                let _ = event_tx.send(Event::ToolCallStarted {
                    turn_id: turn_id.clone(),
                    tool_call_id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }).await;
                let is_destructive = !tool.is_read_only();
                let result = tool.execute(input, &self.tool_ctx).await;
                let (output_str, is_error) = match result {
                    Ok(r) => (r.content, r.is_error),
                    Err(e) => (format!("error: {e}"), true),
                };
                let _ = event_tx.send(Event::ToolCallFinished {
                    turn_id: turn_id.clone(),
                    tool_call_id: id.clone(),
                    name: name.clone(),
                    output: serde_json::json!(output_str),
                    is_error,
                }).await;
                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: output_str,
                    is_error,
                });

                // 3.1 — post-tool checkpoint for successful destructive calls.
                if is_destructive && !is_error && self.checkpoint_enabled {
                    any_destructive_success = true;
                    let _ = self
                        .tool_ctx
                        .checkpoints
                        .create(turn_seq(&turn_id), &name);
                }
            }
            self.session.messages.push(Message {
                role: Role::User,
                content: tool_results,
                metadata: Default::default(),
            });

            // 3.3 — auto-test loop after destructive tool batches.
            if any_destructive_success && self.auto_test_enabled {
                if let Some(runner) = auto_test::detect_runner(&self.tool_ctx.workspace_root) {
                    self.run_auto_test_round(runner, &turn_id, event_tx).await;
                }
            }
            // Loop back for another LLM call.
        }

        // Cycle (hard) — try at end of turn if needed.
        if let Some(outcome) = self
            .cycle
            .maybe_cycle(&mut self.session.messages, "[Phase-2 cycle briefing placeholder]".into())
        {
            let _ = event_tx.send(Event::CycleAdvanced {
                from: outcome.from,
                to: outcome.to,
            }).await;
        }

        let _ = event_tx.send(Event::TurnComplete { turn_id }).await;
        Ok(())
    }
}

impl Engine {
    async fn run_auto_test_round(
        &mut self,
        runner: TestRunner,
        turn_id: &TurnId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        // Bounded retries are tracked via session metadata so the budget
        // resets on a new turn but persists across the per-round loop.
        let attempts_key = "auto_test_attempts";
        let mut attempts: u32 = self
            .session
            .messages
            .last()
            .and_then(|m| m.metadata.get(attempts_key))
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(0);

        let outcome = auto_test::run_tests(&self.tool_ctx.workspace_root, runner).await;
        let _ = event_tx
            .send(Event::Status {
                turn_id: Some(turn_id.clone()),
                message: format!(
                    "auto-test ({}): {}{}",
                    outcome.runner.label(),
                    if outcome.passed { "pass" } else { "fail" },
                    if outcome.timed_out { " (timeout)" } else { "" },
                ),
            })
            .await;

        if outcome.passed {
            return;
        }

        attempts += 1;
        if attempts > MAX_RETRIES {
            let _ = event_tx
                .send(Event::Status {
                    turn_id: Some(turn_id.clone()),
                    message: format!(
                        "auto-test still failing after {MAX_RETRIES} attempts; surfacing to user"
                    ),
                })
                .await;
            return;
        }

        let body = auto_test::failure_message(&outcome, attempts);
        let mut msg = Message::user_text(body);
        msg.metadata.insert(
            attempts_key.into(),
            serde_json::Value::from(attempts as u64),
        );
        self.session.messages.push(msg);
    }
}

fn turn_seq(turn_id: &TurnId) -> u32 {
    // The TurnId is a UUID string; use a stable hash modulo u32 as the
    // checkpoint's "turn" label. It is only used for display.
    let s = &turn_id.0;
    let mut h: u32 = 2166136261;
    for b in s.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

fn decision_to_exec(d: agent_tui_protocol::Decision) -> agent_tui_execpolicy::Decision {
    use agent_tui_protocol::Decision as P;
    use agent_tui_execpolicy::Decision as E;
    match d {
        P::Approved => E::Approved,
        P::ApprovedForSession => E::ApprovedForSession,
        P::Denied => E::Denied,
        P::Abort => E::Abort,
    }
}

#[allow(dead_code)]
fn provider_label(p: Provider) -> &'static str { p.as_str() }
