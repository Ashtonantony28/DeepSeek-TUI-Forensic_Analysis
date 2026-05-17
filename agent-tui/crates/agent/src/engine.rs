//! Engine task. Owns a `Session` and drives the turn loop.

use crate::auto_test::{self, MAX_RETRIES, TestRunner};
use crate::compactor::FlashCompactor;
use crate::parser::parse_tool_input;
use crate::session::Session;
use agent_tui_context::{
    CapacityController, CycleManager, SeamManager, estimate_tokens,
};
use agent_tui_llm::{
    ChatRequest, LlmClient, StreamEvent, ToolSchema as LlmToolSchema,
};
use agent_tui_protocol::{
    AppMode, ContentBlock, DeltaChannel, Event, GuardrailAction, Message, Op, PlanItem, Provider,
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
    pub memory_enabled: bool,
    /// Auto-routing (extension 3.6). When true, the engine consults
    /// `Router::pick(prompt)` on each Submit and may swap the session
    /// model. When false, the user's choice of model is kept verbatim.
    pub routing_enabled: bool,
    pub router: crate::routing::Router,
    /// 3.7 — when true, the engine emits `Event::PlanUpdated` whenever
    /// the model calls `update_plan`. Off skips the side-panel update
    /// without disabling the tool itself.
    pub plan_blocks_enabled: bool,
    /// 3.8 — when true, `Op::SpawnSubAgent { prompt }` runs a DARS
    /// branching pass instead of the legacy "deferred" stub. The vote
    /// budget is `verifier_count`.
    pub dars_enabled: bool,
    pub dars_branch_count: usize,
    pub dars_verifier_count: usize,
    /// 3.9 — when true, the engine calls a Flash-tier compactor to produce
    /// real summaries for the seam and cycle archived blocks. When false,
    /// it falls back to a static placeholder string (the Phase 2 behaviour).
    pub compaction_enabled: bool,
    /// 3.9 — explicit Flash model name for the compactor. Resolved from
    /// the router's `Small` tier in `Engine::new` so it always matches the
    /// configured provider.
    pub compactor_model: String,
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
        let provider = llm.provider();
        let compactor_model =
            crate::routing::Router::new(provider).model_for_tier(crate::routing::Tier::Small).to_string();
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
            memory_enabled: true,
            routing_enabled: false,
            router: crate::routing::Router::new(provider),
            plan_blocks_enabled: true,
            dars_enabled: true,
            dars_branch_count: 3,
            dars_verifier_count: 3,
            compaction_enabled: true,
            compactor_model,
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
                        // User-pinned model bypasses the router.
                        self.session.model = m;
                    } else if self.routing_enabled {
                        // 3.6 — auto routing. Picked model is announced via
                        // a Status event so the UI can render the choice.
                        let (tier, picked) = self.router.pick(&content);
                        if picked != self.session.model {
                            let _ = event_tx
                                .send(Event::Status {
                                    turn_id: None,
                                    message: format!(
                                        "router: {tier} -> {picked}",
                                        tier = tier.label(),
                                    ),
                                })
                                .await;
                            self.session.model = picked.to_string();
                        }
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
                Op::SpawnSubAgent { prompt } => {
                    if self.dars_enabled {
                        self.run_dars_pass(prompt, &event_tx).await;
                    } else {
                        let _ = event_tx.send(Event::Status {
                            turn_id: None,
                            message: "sub-agent spawn: dars disabled".into(),
                        }).await;
                    }
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

        // Seam check (non-destructive). 3.9: when compaction is enabled,
        // run the Flash compactor over the head to produce a real summary;
        // otherwise fall back to a static placeholder so seams still
        // archive deterministically.
        if let Some(outcome) = self.seam.evaluate(&self.session.messages) {
            let level = outcome.level as u8;
            let archived = outcome.head_token_estimate;
            let summary = self.maybe_summarize_seam(&outcome).await;
            self.seam.apply_summary(
                &mut self.session.messages,
                &outcome,
                summary,
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

        // 3.5 — pull relevant lessons from cross-session memory and append
        // them to the system prompt for this turn (does not mutate the
        // persistent session.system_prompt, so prefix-cache stays stable
        // across turns with different queries).
        let memory_suffix: Option<String> = if self.memory_enabled {
            let query = latest_user_text(&self.session.messages).unwrap_or_default();
            if query.is_empty() {
                None
            } else {
                self.tool_ctx.memory.suffix_for(&query, 3)
            }
        } else {
            None
        };
        if memory_suffix.is_some() {
            let _ = event_tx.send(Event::Status {
                turn_id: Some(turn_id.clone()),
                message: format!(
                    "memory: injected {} lesson(s)",
                    self.tool_ctx.memory.retrieve(
                        latest_user_text(&self.session.messages).unwrap_or_default().as_str(),
                        3,
                    ).len()
                ),
            }).await;
        }

        // -- Stream from LLM. Loop until end_turn (no tool calls). --
        for _round in 0..8 {
            let mut req = ChatRequest::new(&self.session.model);
            req.system = match (&self.session.system_prompt, &memory_suffix) {
                (Some(s), Some(suffix)) => Some(format!("{s}{suffix}")),
                (Some(s), None) => Some(s.clone()),
                (None, Some(suffix)) => Some(suffix.trim_start().to_string()),
                (None, None) => None,
            };
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
                let result = tool.execute(input.clone(), &self.tool_ctx).await;
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
                    content: output_str.clone(),
                    is_error,
                });

                // 3.7 — surface plan updates from the `update_plan` tool.
                if !is_error && name == "update_plan" && self.plan_blocks_enabled {
                    if let Some((goal, items)) = extract_plan(&input) {
                        let _ = event_tx
                            .send(Event::PlanUpdated {
                                turn_id: turn_id.clone(),
                                goal,
                                items,
                            })
                            .await;
                    }
                }

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

        // Cycle (hard) — try at end of turn if needed. 3.9: real briefing
        // produced by the Flash compactor when enabled.
        let briefing = self.maybe_summarize_cycle().await;
        if let Some(outcome) = self
            .cycle
            .maybe_cycle(&mut self.session.messages, briefing)
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
    /// 3.9 — produce a real seam summary by invoking the Flash compactor.
    /// Falls back to a static placeholder when compaction is disabled or
    /// the LLM call fails; the seam still applies in that case.
    async fn maybe_summarize_seam(
        &self,
        outcome: &agent_tui_context::SeamOutcome,
    ) -> String {
        if !self.compaction_enabled {
            return "[seam summary disabled]".into();
        }
        let head_count = outcome.head_message_count.min(self.session.messages.len());
        let head = &self.session.messages[..head_count];
        let c = FlashCompactor::new(self.llm.clone(), &self.compactor_model);
        match c.summarize_seam(head).await {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => "[empty summary]".into(),
            Err(_) => "[seam summary unavailable]".into(),
        }
    }

    /// 3.9 — Cycle briefings get a higher token budget than seams. Called
    /// unconditionally at end-of-turn; `CycleManager::maybe_cycle` decides
    /// whether the briefing is actually consumed.
    async fn maybe_summarize_cycle(&self) -> String {
        if !self.compaction_enabled {
            return "[cycle briefing disabled]".into();
        }
        // Estimate cheaply whether we're near the cycle threshold; if not,
        // skip the LLM call entirely. The manager will also short-circuit
        // but skipping the call saves money in the common case.
        let tokens = agent_tui_context::estimate_tokens(&self.session.messages);
        if tokens < self.cycle.config.cycle_tokens {
            return String::new();
        }
        let c = FlashCompactor::new(self.llm.clone(), &self.compactor_model);
        match c.summarize_cycle(&self.session.messages).await {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => "[empty briefing]".into(),
            Err(_) => "[cycle briefing unavailable]".into(),
        }
    }

    /// 3.8 — fan a prompt out across `dars_branch_count` parallel
    /// sub-agents, then have `dars_verifier_count` verifier sub-agents
    /// vote on the winner. Streams the winning answer as a normal text
    /// delta and emits `Event::DarsResult` with the vote breakdown.
    async fn run_dars_pass(&self, prompt: String, event_tx: &mpsc::Sender<Event>) {
        use agent_tui_subagent::{run_dars, DarsConfig, SubAgentManager};
        let mgr = SubAgentManager::new(self.llm.clone());
        let cfg = DarsConfig {
            branch_count: self.dars_branch_count.max(1),
            verifier_count: self.dars_verifier_count.max(1),
            model: self.session.model.clone(),
            system_prompt: self.session.system_prompt.clone(),
            max_parallel: self.dars_branch_count.max(1),
        };
        let turn_id = TurnId::new();
        let _ = event_tx.send(Event::TurnStarted { turn_id: turn_id.clone() }).await;
        let _ = event_tx.send(Event::Status {
            turn_id: Some(turn_id.clone()),
            message: format!(
                "dars: {} branches x {} verifiers",
                cfg.branch_count, cfg.verifier_count,
            ),
        }).await;
        match run_dars(&mgr, &cfg, &prompt).await {
            Ok(outcome) => {
                let _ = event_tx.send(Event::Delta {
                    turn_id: turn_id.clone(),
                    channel: DeltaChannel::Text,
                    delta: outcome.winner_answer.clone(),
                }).await;
                let _ = event_tx.send(Event::DarsResult {
                    winner_index: outcome.winner_index,
                    branch_count: outcome.candidates.len(),
                    votes: outcome.votes,
                }).await;
            }
            Err(e) => {
                let _ = event_tx.send(Event::Error {
                    message: format!("dars failed: {e}"),
                }).await;
            }
        }
        let _ = event_tx.send(Event::TurnComplete { turn_id }).await;
    }

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

/// 3.7 — pull `goal` + `steps[]` out of an `update_plan` tool call.
/// Returns `None` if the input shape is unexpected so we never emit a
/// half-formed plan event.
pub(crate) fn extract_plan(input: &serde_json::Value) -> Option<(String, Vec<PlanItem>)> {
    let goal = input.get("goal").and_then(|v| v.as_str())?.to_string();
    let steps = input.get("steps").and_then(|v| v.as_array())?;
    let items: Vec<PlanItem> = steps
        .iter()
        .filter_map(|s| {
            // Steps may be plain strings or {step, done} records.
            if let Some(text) = s.as_str() {
                Some(PlanItem { step: text.to_string(), done: false })
            } else if let Some(obj) = s.as_object() {
                let step = obj.get("step").and_then(|v| v.as_str())?.to_string();
                let done = obj.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
                Some(PlanItem { step, done })
            } else {
                None
            }
        })
        .collect();
    Some((goal, items))
}

/// Walk the message log backwards and return the last user text block.
/// Used by the 3.5 memory injector and the 3.6 router to score complexity.
pub(crate) fn latest_user_text(messages: &[agent_tui_protocol::Message]) -> Option<String> {
    use agent_tui_protocol::{ContentBlock, Role};
    for m in messages.iter().rev() {
        if !matches!(m.role, Role::User) { continue; }
        let mut buf = String::new();
        for c in &m.content {
            if let ContentBlock::Text { text } = c {
                if !buf.is_empty() { buf.push('\n'); }
                buf.push_str(text);
            }
        }
        if !buf.is_empty() {
            return Some(buf);
        }
    }
    None
}
