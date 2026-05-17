# ANALYSIS.md — Forensic analysis of upstream DeepSeek-TUI

This document is the Phase 1 deliverable from `agenttuibuildplan.md`. It is
based on a shallow clone of `Hmbown/DeepSeek-TUI` placed at
`/home/user/deepseek-tui-ref/` (sibling of this repo). All `file:LINE`
references are relative to `crates/...` inside that clone.

The companion artifact `crate_map.dot` graphs the 14-crate dependency
topology. The build plan referenced "16 upstream crates"; the actual count
at HEAD is **14**.

---

## 1. Workspace topology

- **agent** (`crates/agent/src/lib.rs:23–174`) — Model registry and
  resolution. Hardcoded list of `ModelInfo` records (DeepSeek V4 Pro/Flash,
  GPT-4.1, OpenRouter, Novita, Fireworks, SGLang, vLLM, Ollama) with
  provider affiliation and capability flags (`supports_tools`,
  `supports_reasoning`). Routes model requests through arity-aware fallback
  chains.

- **app-server** (`crates/app-server/src/lib.rs`, `main.rs`) — HTTP wrapper
  over the core engine. Accepts `ThreadRequest` / `AppRequest` envelopes
  via JSON, dispatches to `Runtime::handle_thread()`, returns
  `EventFrame` streams. Graceful shutdown plus a blocking stdin read-loop.

- **cli** (`crates/cli/src/lib.rs`, `main.rs`) — Command-line binary. Parses
  REPL and file-mode input, manages TUI lifecycle, wires config overrides
  into the runtime, orchestrates the session loop. No engine logic;
  delegates to `Runtime`.

- **config** (`crates/config/src/lib.rs:1–40`) — Deserializes
  `~/.deepseek/config.toml` into `ConfigToml`. Defines `ProviderKind` enum
  (Deepseek, NvidiaNim, Openai, etc. — 10 variants), per-provider
  model/base-URL defaults, secret-source routing (`SecretSource`). Enforces
  Unix 0600 perms on credential files.

- **core** (`crates/core/src/lib.rs:650–1365`) — High-level `Runtime`
  orchestrator wrapping `ThreadManager`, `JobManager`, `ToolRegistry`,
  `McpManager`, `ExecPolicyEngine`, `HookDispatcher`. Thread lifecycle
  (`create`, `resume`, `fork`, `read`, `list`, `archive`), tool invocation
  with approval/sandbox checks, job retry/backoff state machines,
  checkpoint persistence.

- **execpolicy** (`crates/execpolicy/src/lib.rs:14–281`) — Execution
  permission engine. Layered rulesets (`RulesetLayer::BuiltinDefault`,
  `Agent`, `User`); arity-aware matching of `trusted_prefixes` /
  `denied_prefixes`; produces `ExecPolicyDecision` with
  `ExecApprovalRequirement` (`Skip`, `NeedsApproval`, `Forbidden`).

- **hooks** (`crates/hooks/src/lib.rs`) — Async event dispatcher for
  session / tool / approval lifecycle. Enables external observability
  without coupling the engine to specific sinks.

- **mcp** (`crates/mcp/src/lib.rs:80–300`) — MCP server registry and
  tool/resource dispatcher. Per-server `McpManagedClient` trait objects,
  per-server `ToolFilter` (allow/deny). Exposes `list_tools()`,
  `call_tool()`, `list_resources()`, `read_resource()` with filter
  application.

- **protocol** (`crates/protocol/src/lib.rs:1–475`) — Wire protocol. The
  `EventFrame` enum (29 variants — see §2), `ThreadRequest` /
  `ThreadResponse`, `PromptRequest`, `AskForApproval`, `ReviewDecision`
  (`Approved`, `ApprovedExecpolicyAmendment`, `Denied`, `Abort`,
  `NetworkPolicyAmendment`).

- **secrets** (`crates/secrets/src/lib.rs`) — Env/file secret sourcing.
  `SecretSource::EnvVar` or `SecretSource::File`, optional decryption
  placeholder.

- **state** (`crates/state/src/lib.rs:1–40`) — SQLite-backed thread metadata
  and message storage. Persists `ThreadMetadata`, `MessageRecord`,
  `CheckpointRecord`, `JobStateRecord`, dynamic tool definitions.
  Append-only message logs, checkpoint-per-seam for cycle management.

- **tools** (`crates/tools/src/lib.rs:1–50`) — `Tool` trait and registry.
  `ToolCapability` (`ReadOnly`, `WritesFiles`, `ExecutesCode`, `Network`,
  `Sandboxable`, `RequiresApproval`); `ApprovalRequirement` (`Auto`,
  `Suggest`, `Required`); `ToolError` enum (`InvalidInput`, `MissingField`,
  `PathEscape`, `ExecutionFailed`, `Timeout`, `NotAvailable`,
  `PermissionDenied`). Per-tool dispatch with timeout and sandbox
  enforcement.

- **tui** (`crates/tui/src/main.rs`) — ratatui-based terminal UI for
  interactive chat. Panes: Chat, Diff, Tasks, Agents, Status, Jobs.
  Streams `Event` updates, manages approval gates and tool results in
  real time. Hosts the engine modules (`core/engine/...`).

- **tui-core** (`crates/tui-core/src/lib.rs:1–62`) — Lightweight
  state-reducer for the UI (pane routing, status line, pending
  task/job counters). Maps `UiEvent` → `Vec<UiEffect>` (`Render`,
  `PersistCheckpoint`, `EmitStatusLine`). **Note:** the name is
  misleading — this is *not* the engine core; the engine lives in
  `crates/tui/src/core/`.

---

## 2. Engine ABI

### `EventFrame` (`crates/protocol/src/lib.rs:391–474`)

29 variants (selected fields shown):

- `ResponseStart { response_id }` — text block start.
- `ResponseDelta { response_id, delta, channel: ResponseChannel }` —
  streaming text or reasoning.
- `ResponseEnd { response_id }` — text block end.
- `ToolCallStart { response_id, tool_name, arguments: Value }`.
- `ToolCallResult { response_id, tool_name, output: Value }`.
- `McpStartupUpdate { update: McpStartupUpdateEvent }`.
- `McpStartupComplete { summary: McpStartupCompleteEvent }`.
- `McpToolCallBegin { server_name, tool_name }`.
- `McpToolCallEnd { server_name, tool_name, ok: bool }`.
- `ExecApprovalRequest { request: ExecApprovalRequestEvent }`.
- `ApplyPatchApprovalRequest { request: ExecApprovalRequestEvent }`.
- `ElicitationRequest { server_name, request_id, prompt }`.
- `ExecCommandBegin { command, cwd }`.
- `ExecCommandOutputDelta { command, delta }`.
- `ExecCommandEnd { command, exit_code }`.
- `PatchApplyBegin { path }`.
- `PatchApplyEnd { path, ok: bool }`.
- `TurnStarted { turn_id }`.
- `TurnComplete { turn_id }`.
- `TurnAborted { turn_id, reason }`.
- `Error { response_id, message }`.

Example `ResponseDelta` payload:

```json
{
  "event": "response_delta",
  "response_id": "resp-abc123",
  "delta": "The solution involves...",
  "channel": "text"
}
```

### `Op` (`crates/tui/src/core/ops.rs:14–85`)

- `SendMessage { content, mode, model, goal_objective, reasoning_effort,
  reasoning_effort_auto, auto_model, allow_shell, trust_mode,
  auto_approve, approval_mode, translation_enabled }` — user prompt.
- `CancelRequest` — abort current turn.
- `ApproveToolCall { id }` / `DenyToolCall { id }`.
- `SpawnSubAgent { prompt }` / `ListSubAgents`.
- `ChangeMode { mode }` / `SetModel { model }`.
- `SetCompaction { config }` / `CompactContext`.
- `SyncSession { session_id, messages, system_prompt, model, workspace }`.
- `EditLastTurn { new_message }`.
- `Shutdown`.

Example `SendMessage` payload:

```json
{
  "content": "What's the sum?",
  "mode": "chat",
  "model": "deepseek-v4-pro",
  "reasoning_effort": "high",
  "approval_mode": "unless_trusted",
  "allow_shell": true
}
```

> **Departure from plan.** The plan anticipated `Op::Submit(String)`,
> `Op::Cancel`, `Op::AcceptApproval`. The real shapes are richer —
> `SendMessage` carries mode/model/reasoning/approval fields together.
> Phase 2's `agent-tui-protocol` should follow the upstream shape unless
> there is a clear simplification.

---

## 3. Turn lifecycle

`handle_deepseek_turn` lives at
`crates/tui/src/core/engine/turn_loop.rs:15–500`.

1. **Setup** (`turn_loop.rs:15–37`) — initialise loop guard, tool catalog,
   context-recovery counter (max 3).
2. **Steer drain** (`turn_loop.rs:54–71`) — non-blocking pull of pending
   user steers for later injection.
3. **Max-steps guard** (`turn_loop.rs:76–82`) — break on `at_max_steps()`.
4. **Auto-compaction check** (`turn_loop.rs:84–167`) — if enabled and
   thresholds crossed, `compact_messages_safe()` with pin/path hints;
   emits `CompactionStarted`/`Completed`/`Failed`.
5. **Capacity pre-request checkpoint** (`turn_loop.rs:169–174`) —
   `run_capacity_pre_request_checkpoint(client)`; `true` = intervention
   applied, continue.
6. **Input token budget** (`turn_loop.rs:176–207`) — estimate
   `estimated_input_tokens()` against model budget; if over and recovery
   budget remains, `recover_context_overflow()` and loop; else fail.
7. **LSP flush** (`turn_loop.rs:212`) — `flush_pending_lsp_diagnostics()`
   injects compile-error synthetic messages.
8. **Seam checkpoint** (`turn_loop.rs:217`) — `layered_context_checkpoint()`
   (Flash-summary seam manager). Opt-in since v0.7.5.
9. **Build request** (`turn_loop.rs:219–306`) — `MessageRequest` with
   `messages_with_turn_metadata()`, active tools (sanitised when
   `strict_tool_mode`), resolved reasoning effort, prefix-cache stability
   check, `tool_choice` (auto/required).
10. **Prefix-cache stability** (`turn_loop.rs:247–283`) —
    `pm.check_and_update()` detects system-prompt / tool-set drift; emits
    `PrefixCacheChange` with stability %.
11. **Stream request** (`turn_loop.rs:312–340`) —
    `client.create_message_stream()`. Auth/context-length error →
    context-recovery loop (up to `MAX_CONTEXT_RECOVERY_ATTEMPTS`). Other
    errors → fail with decorated message.
12. **Stream event loop** (`turn_loop.rs:392–525`) — per event:
    - `MessageStart` → record usage.
    - `ContentBlockStart{Text}` → init text buffer, check fake-tool
      wrappers.
    - `ContentBlockStart{Thinking}` → init thinking buffer.
    - `ContentBlockStart{ToolUse}` → create `ToolUseState`, record in
      `current_tool_indices`.
    - `ContentBlockDelta{TextDelta}` → append, emit `MessageDelta`.
    - `ContentBlockDelta{ThinkingDelta}` → append, emit `ThinkingDelta`.
    - `ContentBlockDelta{InputJsonDelta}` → append to tool input buffer;
      per-delta parse mirror into `tool_state.input`.
    - On stream error, `should_transparently_retry_stream()` (no content
      yet + retry budget) → drop and re-issue; else fail.
    - Wall-clock guard: `STREAM_MAX_DURATION_SECS`.
    - Content-size guard: `STREAM_MAX_CONTENT_BYTES`.
13. **Finalise tool inputs** — see §6 (`dispatch.rs:148–181`).
14. **Tool execution plan** (`dispatch.rs:38–57`) — classify parallel vs
    serial; set `approval_required`, `supports_parallel`, `read_only`,
    `blocked_error`, `guard_result`.
15. **Tool execution** — parallel batches via tokio tasks, serial
    one-by-one; `engine.invoke_tool()` → `exec_policy.check()` →
    `tool_registry.dispatch()`. Emits `ToolCallStart`/`ToolCallResult`;
    records `ToolExecOutcome`.
16. **Post-tool LSP hook** — `run_post_edit_lsp_hook()` extracts paths
    from edit tools, queries the LSP manager, queues blocks in
    `pending_lsp_blocks` for next turn (see §9).
17. **Loop-guard** — identical `tool_call_id+input` twice in a row →
    inject guard error and skip.
18. **Capacity post-tool checkpoint** —
    `run_capacity_post_tool_checkpoint()` after each tool.
19. **Tool-batch completion** — `finish_reason == "tool_calls"` → loop to
    step 2; `"end_turn"` → break.
20. **Session persistence** — append assistant message to
    `self.session.messages`, persist via `save_checkpoint()`.
21. **Return** (`turn_loop.rs:22`) — `(TurnOutcomeStatus::{Completed,
    Interrupted, Failed}, turn_error)`.

---

## 4. Context management

### `SeamManager` (`crates/tui/src/seam_manager.rs:116–258`)

- **Thresholds** (`seam_manager.rs:48–52`): L1 = 192 K, L2 = 384 K,
  L3 = 576 K, cycle boundary = 768 K (active input estimate, tokens).
- **Behaviour** (`seam_manager.rs:179–258`, `produce_soft_seam()`):
  summarise a message range via the Flash model, emit an
  `<archived_context level="N">` XML block, append as a synthetic
  assistant message. **Append-only** — never replaces. Preserves the
  verbatim tail window (default 16 turns).
- **Prefix-cache impact:** non-destructive. The stable prefix stays hot
  (file notes the "90% discount at 128-token granularity"); cache breaks
  only at new seam-block boundaries.

### `CycleManager` (`crates/tui/src/cycle_manager.rs`)

- **Threshold:** hard cycle at 768 K active tokens.
- **Behaviour:** archive all but the recent tail into a `CycleBriefing`,
  reset the in-memory buffer to seed messages from the prior cycle, emit
  `CycleAdvanced { from, to, briefing }`.
- **Prefix-cache impact:** **hard boundary.** Everything after the swap
  must recompute; pre-cycle messages persist on disk but the live cache
  is invalidated.

### `Compaction` (`crates/tui/src/compaction.rs`; call site
`turn_loop.rs:111–166`)

- **Trigger:** `should_compact()` over message count / token estimate /
  compaction config (`turn_loop.rs:90–97`).
- **Behaviour:** `compact_messages_safe()` calls
  `client.create_message` with a summary prompt and **replaces** the old
  message range. Retries up to 3.
- **Prefix-cache impact:** **destructive.** The first replaced message
  invalidates everything after.

---

## 5. CapacityController

`crates/tui/src/core/capacity.rs:1–250`.

- **`RiskBand`** (`capacity.rs:142–158`): `Low` (≤ `low_risk_max`, default
  0.50), `Medium` (≤ `medium_risk_max`, default 0.62), `High` (above).
- **`GuardrailAction`** (`capacity.rs:122–138`): `NoIntervention`,
  `TargetedContextRefresh`, `VerifyWithToolReplay`, `VerifyAndReplan`.
- **Inputs** (`capacity.rs:162–169`, `CapacityObservationInput`):
  `turn_index`, `model`, `action_count_this_turn`,
  `tool_calls_recent_window`, `unique_reference_ids_recent_window`,
  `context_used_ratio`.
- **Computation** (`capacity.rs:235–250`, `observe_pre_turn()`): estimate
  slack `h_hat` from model prior + action pressure; estimate `c_hat`
  from context ratio; track rolling slack window; compute violation
  ratio and volatility; assign `RiskBand` and `p_fail`.
- **`VerifyAndReplan` trigger** (`capacity.rs:250–300`, `decide()`):
  fires when `RiskBand::High` AND the cooldown (5 turns, line 49) has
  expired.
- **Default-off** since v0.8.11 (`capacity.rs:41`, `enabled: false`) —
  silent message rewrites break the prefix cache, so users must opt in.

> **Departure from plan.** Plan lists a `RiskBand::Severe` variant; the
> upstream has only `Low`/`Medium`/`High`. Phase 2 should follow upstream.

---

## 6. Tool input parser

`crates/tui/src/core/engine/dispatch.rs:134–223`.

Recovery ladder in `final_tool_input()` (line 148) and
`parse_tool_input()` (line 157):

1. **Streamed `input_buffer`** — concatenated `InputJsonDelta` events. Try
   `crate::tools::arg_repair::repair()` (deterministic ladder for trailing
   commas, unclosed braces, embedded control chars).
2. **Fallback 1: code fences** (lines 169–172) — `strip_code_fences()`
   (line 183) strips triple backticks, retry parse.
3. **Fallback 2: double-decode** (lines 174–177) — parse as JSON-encoded
   string, then parse the inner string (handles models emitting
   `"{\"key\": ...}"`).
4. **Fallback 3: segment extraction** (lines 179–180) —
   `extract_json_segment()` (line 203) calls
   `extract_balanced_segment(text, '{', '}')` (line 207): find first
   `{`, track depth, emit at depth 0; else try `[...]` for arrays.
5. **Final fallback** (line 154) — fall back to `state.input` (the
   per-delta best-effort mirror captured during streaming).

---

## 7. Tool deferral and Plan-mode narrowing

- **Tool loading:** the turn loader passes a `Vec<Tool>` from
  `tool_registry.list()` (turn_loop.rs:32–34). Tools absent from the
  active set are never advertised; if the model tries to call one, the
  registry returns "not available".
- **Plan mode** (`AppMode::Plan`): `ensure_advanced_tooling()`
  (turn_loop.rs:33) filters the catalog — strips shell + file-write
  tools, keeps read + planning (`list_files`, `search_files`,
  `update_plan`).
- **Per-tool spec** (in `crates/tui/src/tools/spec.rs` /
  `crates/tools/src/spec.rs`): each `Tool` carries `name`, `description`,
  `input_schema`, `capabilities`, `approval_requirement`, and an
  optional `allowed_callers` (restricting which caller types — e.g.
  `"plan"`, `"direct"` — may invoke).
- **Caller policy** is enforced at `dispatch.rs:82–100`.

---

## 8. MCP integration

`crates/mcp/src/lib.rs:80–300`.

- **Transport:** abstract `McpManagedClient` trait
  (`crates/mcp/src/lib.rs:80`). Concrete implementations live outside
  this crate (likely the `tui` or `core` crates). Standard MCP stdio
  and HTTP transports are assumed.
- **Registration** (lines 153–161, `register_server()`): caller supplies
  `McpServerConfig { name, command, args, env, enabled }`, a
  `ToolFilter` (allow/deny), and a `Box<dyn McpManagedClient>`. Stored
  in `clients` and `configs` hash maps.
- **Tool listing** (lines 226–247, `list_tools()`): iterate enabled
  servers, call `client.list_tools()`, apply `ToolFilter`
  (`allowed_by_filter()`, lines 234–236), return
  `Vec<McpToolDescriptor>` with `qualified_name` = `"server:tool"`.
- **Tool invocation** (lines 249–265, `call_tool()` /
  `call_qualified_tool()`): server lookup, `client.call_tool(name, args)`.
  No automatic retry/fallback.
- **Deferral:** skipped when the server is not ready (line 229: missing
  `client` entry → `continue`); deferred calls return
  `"server not available"` to the model.
- **Startup lifecycle** (lines 163–208, `start_all()`): per config, emit
  `Cancelled` (disabled) / `Starting` (enabled) / `Failed` (no client);
  return `McpStartupCompleteEvent { ready, failed, cancelled }`.

---

## 9. LSP integration

`crates/tui/src/core/engine/lsp_hooks.rs:1–122`.

- **`run_post_edit_lsp_hook()`** (lines 80–103) is called by the turn loop
  after each successful tool execution. Takes `tool_name` and
  `tool_input: Value`. Extracts edited paths via
  `edited_paths_for_tool()` (lines 16–49):
  - `edit_file` / `write_file` → `path` field.
  - `apply_patch` → `path` or `files[].path`, falling back to
    `parse_patch_paths()` (lines 56–70).
- Resolves each path to absolute, calls
  `self.lsp_manager.diagnostics_for(&absolute, seq)` (line 99), queues
  results in `self.pending_lsp_blocks`.
- **`flush_pending_lsp_diagnostics()`** (lines 110–121) is called just
  before the next API request (`turn_loop.rs:212`): drains
  `pending_lsp_blocks`, renders via `crate::lsp::render_blocks()`, and
  injects them as a synthetic user message via `add_session_message()`
  (line 119). The model sees compile errors on the next reasoning step.

---

## 10. Execpolicy

`crates/execpolicy/src/lib.rs:14–281`.

- **Approval-gate pattern.** `ExecPolicyEngine::check()` (line 205) is
  synchronous and returns `ExecApprovalRequirement::{Skip, NeedsApproval,
  Forbidden}` (enum at line 74). When `NeedsApproval`, the engine emits
  `EventFrame::ExecApprovalRequest` (`protocol.rs:430–431`) and the TUI
  blocks tool execution until a `ReviewDecision`
  (`protocol.rs:302–313`: `Approved`, `ApprovedExecpolicyAmendment`,
  `Denied`, `Abort`, `NetworkPolicyAmendment`) arrives.
  `remember_session_approval()` (line 197) caches approvals in
  `approved_for_session: HashSet` for the rest of the session.
  *Note:* the plan describes a `tokio::sync::oneshot` channel
  specifically; upstream uses event-based blocking via the protocol
  channel, not a per-call oneshot.
- **Layered rulesets** (lines 14–52). `RulesetLayer::BuiltinDefault` (0)
  < `Agent` (1) < `User` (2). `resolve_prefixes()` (line 179) collects
  prefixes across layers. Deny rules first (line 209, simple prefix
  match); trusted rules second (line 226, arity-aware via
  `arity_dict.allow_rule_matches()`).
- **Sandbox layers per OS.** Not implemented in this crate — execpolicy
  is *policy only*. OS-specific sandboxing (seccomp on Linux,
  sandbox-exec on macOS, Job Objects on Windows) is enforced at tool
  invocation time, in the `tools` / `tui` layers. The plan's expectation
  that everything is centralised in `execpolicy` is **not** how upstream
  is structured.
- **Egress.** When a command is unmatched, `check()` (line 261) suggests
  a `NetworkPolicyAmendment { host: ctx.cwd, action: Allow }`. This is
  *advisory*; actual network filtering happens in MCP / sandbox layers,
  not here.

---

## 11. Sub-agent and RLM session lifecycle

**Sub-agent.** `agent_open` / `agent_eval` / `agent_close` are not
free functions in the listed locations; the sub-agent system lives in
`crates/tui/src/tools/subagent/mod.rs` (~150 lines) as a tool-spawning
mechanism driven by `Op::SpawnSubAgent { prompt }` and `Op::ListSubAgents`
(ops.rs).

**RLM (Recursive Language Model).** Three-tool surface in
`crates/tui/src/tools/rlm.rs:32–250`.

- **`rlm_open`** (`rlm.rs:32–141`, `RlmOpenTool`): input is `name`
  (optional) plus exactly one of `file_path` / `content` / `url`. Steps:
  1. Validate single source (lines 84–92).
  2. Load via `load_source()` (line 94).
  3. Derive or accept caller `name` (lines 101–107).
  4. Uniqueness check in `context.runtime.rlm_sessions` (lines 109–115).
  5. Write context to temp file via `write_context_file()`
     (`rlm/session.rs:111–121`).
  6. Spawn Python kernel via `PythonRuntime::spawn_with_context()`
     (line 121).
  7. Build `RlmSession` (line 125), insert into `rlm_sessions`
     (lines 128–129).
  8. Return `{ name, id, length, type, preview_500, sha256 }`.
- **`rlm_eval`** (`rlm.rs:143–250`, `RlmEvalTool`): input is `name`
  + `code`. Steps:
  1. Lookup session (line 191), acquire lock (line 192).
  2. Verify kernel open (lines 195–198).
  3. Optionally build `RlmBridge` for nested RLM calls
     (lines 202–208); depth cap = `HARD_SUB_RLM_DEPTH_CAP = 3`
     (line 206).
  4. `kernel.run(code, Some(&bridge))` (lines 209–211).
  5. Update `rpc_count`, `total_duration`, `last_used_at`
     (lines 223–225).
  6. If `FINAL` called, capture as `var_handle` (lines 227–242).
  7. Return bounded projection (head/tail stdout + metadata)
     (lines 244–250).
- **`rlm_close`** — closes the Python kernel, releases temp file, removes
  from `rlm_sessions` (not fully read; symmetric with `rlm_open`).

**`RlmSession`** (`rlm/session.rs:22–60`): `name`, `id` (`rlm:uuid`),
`kernel: Option<PythonRuntime>` (None after close), `context_meta`,
`config` (output feedback, sub-query timeout, sub-RLM max depth,
share-session flag), `created_at`, `last_used_at`, `rpc_count`,
`total_duration`, `peak_var_count`, `final_count`.

> **Major departure from plan.** The plan promises a *pure-Rust binary
> with no Python dependency*. Upstream's RLM tool surface spawns a
> **Python kernel** (`PythonRuntime`). For Phase 2 we either drop the
> RLM surface (matching the no-Python rule) or stub it behind a
> feature flag. **Recommendation: stub it as an MCP-server-backed
> capability instead, keeping the binary pure Rust.**

---

## 12. Identified gaps

1. **DeepSeek-only provider surface.** Engine is hardcoded to DeepSeek
   streaming + OpenAI-compatible tool-calling format. Model registry in
   `agent` crate lists multiple providers but the engine assumes one
   wire format.
2. **No semantic search / RAG.** No vector store, no embedding pipeline.
   Tools can read files; nothing ranks by relevance.
3. **No test-time scaling.** No multi-rollout, no tournament voting,
   no adaptive reasoning effort.
4. **No multi-agent verification.** Sub-agents run in parallel but no
   cross-verification, voting, or rubric scoring.
5. **Partial ACP server.** `app-server` exposes basic HTTP thread/prompt
   endpoints but no SSE streaming; not an ACP server in the Zed/JetBrains
   sense.
6. **No mutation-strengthened evaluation.** No harness for
   semantic-equivalence checking of patches (SWE-ABS insight unaddressed).
7. **No DARS-style branching.** Each turn is deterministic; no per-turn
   branch-and-pick.
8. **No persistent Reflexion memory.** Session history is append-only;
   no cross-session lesson buffer.
9. **No Agentless-style pipeline.** No localise→repair→validate
   hierarchical path; only the free-form agent loop.
10. **No SBFL integration.** No spectrum-based blame for failing tests.
11. **Compaction breaks the prefix cache.** Default-disabled
    `CapacityController` exists to mitigate, but compaction itself is
    still destructive when it runs.
12. **No streaming of tool results back mid-turn.** Tool outputs are
    finalised before the next API call.
13. **No think-then-act separation.** Thinking and tool calls interleave
    inside the same end-turn block.
14. **No dynamic message re-ranking.** Messages always appear in
    conversation order.
15. **No dry-run tool execution.** Approval is binary; no sandboxed
    preview ("what would this patch do?").
16. **RLM tool spawns a Python kernel.** Conflicts with the no-Python
    rule the new project adopts.
17. **`tui-core` is misnamed.** It is a UI reducer, not the engine core;
    the engine lives in `crates/tui/src/core/`. Phase 2 should pick names
    that don't repeat this confusion.

---

## Surprises and risks for replication

The codebase is tightly coupled to DeepSeek's streaming API and OpenAI-style
tool-calling — replication for other providers requires a real `LlmClient`
abstraction, not a thin shim. The prefix-cache awareness is the upstream's
signature feature but is fragile: the seam manager is default-on while the
capacity controller is default-off, precisely because silent message
rewrites destroy cache coherence and user trust. The turn loop is monolithic
(~500 lines of state machine in `turn_loop.rs` plus tightly-coupled
dispatch/capacity/LSP hooks) — refactoring it for modularity in
`agent-tui-agent` is the single biggest replication risk. The RLM
Python-kernel surface conflicts with the plan's "no Python runtime
dependency" rule and must be redesigned (recommend: stub as an MCP
capability). Finally, the upstream `Op` and `EventFrame` enums are much
richer than the plan's sketches; Phase 2 should match upstream shapes
unless there is a clear simplification.

---

## Ready for Phase 2

Worked:
- Full forensic pass of all 14 crates.
- Engine ABI, turn lifecycle, context-management, capacity, parser,
  Plan-mode narrowing, MCP, LSP, execpolicy, RLM lifecycle all
  documented with real `file:LINE` refs.
- Companion `crate_map.dot` graphs the dependency topology.

Glitchy:
- Some line ranges are end-of-range guesses (e.g. `compaction.rs`,
  `cycle_manager.rs` not fully read top-to-bottom). Anyone extending
  Phase 2 around those files should re-verify.
- `rlm_close` body not read; behaviour inferred from `rlm_open`.

Ready for Phase 2.
