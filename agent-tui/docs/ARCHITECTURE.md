# Architecture

`agent-tui` is a Cargo workspace of 15 crates. Each crate has a single
well-defined responsibility. The binary (`agent-tui-cli`) is the only crate
with a `main` function; everything else is a library.

---

## Crate map

```
protocol → config → llm → execpolicy → tools → context → retrieval
         → agent → subagent → pipeline → mcp → acp → tui → cli
                                                            ↘
                                                           eval
```

### `agent-tui-protocol`

Pure data types with no async or I/O. Shared between every other crate.
Defines `Message`, `ContentBlock`, `Role`, `Turn`, `SessionId`, `TurnId`,
`ToolCallId`, `ApprovalId`, `Provider`, `ModelInfo`, `RiskBand`,
`GuardrailAction`, `AppMode`, `Decision`, the `Op` input enum, and the
`Event` output enum. Heavy `serde` use; all types are JSON round-trippable.

### `agent-tui-config`

Five-layer config cascade (see `docs/CONFIGURATION.md`). Loads and merges
TOML files, applies CLI overrides, applies env vars. Defines the
`Extensions` struct that controls Phase 3 feature flags. Enforces the
`FORBIDDEN_PROJECT_KEYS` rule so project overlays cannot carry API secrets.
`save_user()` writes the user config with mode `0600` on Unix.

### `agent-tui-llm`

The `LlmClient` async trait plus per-provider adapters:
`AnthropicClient`, `OpenAiClient`, `DeepSeekClient`, `GroqClient`,
`XaiClient`, `OllamaClient`, `OpenAiCompatClient`. Each adapter handles
streaming chunks, tool-call events, thinking blocks, and prefix-cache
hit reporting. `MockClient` is a scripted fake for tests. `ChatRequest`,
`ChatStream`, `StreamEvent`, and `Usage` are the wire types.

### `agent-tui-execpolicy`

Execution permission engine. `ExecPolicyEngine::check()` returns
`ExecApprovalRequirement` (`Skip`, `NeedsApproval`, `Forbidden`).
`SandboxedCommand` wraps `tokio::process::Command` and applies OS-native
sandboxing: `seccomp` on Linux, `sandbox-exec` on macOS, Job Objects on
Windows. `EgressPolicy` blocks private IP ranges by default and validates
outbound hostnames. `SecretRedactor` scans tool output for API-key-shaped
strings and replaces them with `[REDACTED]`.

### `agent-tui-tools`

The `Tool` async trait and `ToolRegistry`. Built-in tools: `read_file`,
`write_file`, `edit_file`, `apply_patch`, `list_dir`, `search_files`,
`glob`, `shell`, `exec`, `fetch_url`, `git_status`, `git_diff`,
`git_commit`, `git_log`, `git_checkout`, `semantic_search`,
`update_plan`, `remember`, `recall`, `list_checkpoints`,
`checkpoint_diff`, `rollback`. Phase 3.10 adds `repl_open`, `repl_eval`,
`repl_close` (gated by `extensions.repl_tools`). Phase 3.12 adds
`McpToolAdapter` so MCP server tools appear in the registry under the
`server:tool` name scheme. Every edit/write/patch tool runs
`syntax::check_syntax()` before returning success.

### `agent-tui-context`

Three-tier context management:

- `SeamManager` — non-destructive; appends `<archived_context>` summary
  blocks at 192 K / 384 K / 576 K token thresholds. Preserves prefix cache.
- `CycleManager` — hard-reset at 768 K; archives the session and restarts
  with a Briefing prompt. Invalidates prefix cache.
- `Compaction` — destructive last-resort; summarises old messages into the
  system prompt. Off by default.
- `CapacityController` — computes `RiskBand` from `context_used_ratio` and
  tool volatility. `RiskBand::High` with an expired cooldown triggers
  `VerifyAndReplan`.
- `TokenEstimator` — cheap character-based token estimate used before the
  real API call is made.

### `agent-tui-retrieval`

`Retriever` async trait with three implementations fused by
Reciprocal Rank Fusion:

- `RipgrepRetriever` — wraps `rg`, returns file:line snippets.
- `RepoDependencyRetriever` — builds an import-edge graph and ranks files
  with PageRank.
- `SemanticRetriever` — AST chunker (tree-sitter) + Ollama embeddings
  stored in `sqlite-vec`. Falls back gracefully when Ollama is unreachable.

`HybridRetriever` composes all three. `build_repo_graph()` and
`build_or_update_embeddings()` are the indexing entry points called by
`agent-tui index`.

### `agent-tui-agent`

The engine. Runs as a `tokio::task` driven by an `mpsc` channel of `Op`
values; emits `Event` values out. `Engine::spawn()` returns an
`EngineHandle`. The turn loop implements the full state machine documented
in the [Turn lifecycle](#turn-lifecycle) section below. `FlashCompactor`
(Phase 3.9) produces seam and cycle summaries via the small-tier model.
`Router` (Phase 3.6) selects provider, model, and reasoning effort per turn.

### `agent-tui-subagent`

`SubAgentManager` opens, evaluates, and closes ephemeral sub-agent sessions.
Each session reuses the parent's `LlmClient`. `parallel_fan_out()` runs up
to N concurrent calls for DARS branching and RTV rollouts.

### `agent-tui-pipeline`

`HierarchicalPipeline` implements the Agentless-style
localise → repair → validate flow invoked by `agent-tui fix`. Steps:
(1) retrieve candidate files with `HybridRetriever`, (2) rank by summed
score, (3) call a sub-agent with a repair prompt including candidate
snippets, (4) extract a unified diff, (5) validate. Gated by
`extensions.pipeline_enabled`.

### `agent-tui-mcp`

Minimal JSON-RPC-over-stdio MCP client. `StdioMcpClient` spawns a child
process and speaks the MCP `initialize` / `tools/list` / `tools/call`
protocol. `McpManager` registers multiple servers and exposes
`list_all_tools()` and `call_qualified_tool()`. Used by `agent-tui mcp
list`, `agent-tui mcp probe`, and the engine's startup path.

### `agent-tui-acp`

ACP (Agent Client Protocol) server over stdio. `AcpServer::run_stdio()`
reads line-delimited JSON-RPC 2.0 from stdin and writes responses to
stdout. Implements: `initialize`, `session/new`, `session/load`,
`session/prompt` (streaming), `fs/read_text_file`, `fs/write_text_file`.
Invoked by `agent-tui serve --acp`.

### `agent-tui-tui`

ratatui + crossterm terminal UI. Five panels: Composer (bottom), Transcript
(centre), Plan side panel (right, toggle `p`), Status bar (top), Command
palette (`Ctrl+K`). Keys: `F1` help, `Esc` backs out, `Tab`/`Shift+Tab`
cycles Plan → Agent → Yolo modes, `Ctrl+C` cancels turn. `run_tui()` is
the entry point; `EngineKnobs` carries the extension flags from config.

### `agent-tui-cli`

The dispatcher binary. Parses `clap` subcommands and delegates to handler
functions. `build_client()` and `build_mock_client()` are helpers used by
all entry points. See `docs/CONFIGURATION.md` for the full subcommand
reference.

### `agent-tui-eval`

SWE-bench evaluation harness (Phase 4). `RunConfig` drives `run_eval()`.
`MutationEngine` applies basic tree-sitter mutations (return-value flips,
comparison-operator swaps, conditional negations, off-by-one offsets).
`compare::compare_runs()` produces a markdown diff table.
`RtvEngine` implements Recursive Tournament Voting for `--scale N > 1`.
The output JSON format is documented in `docs/EVAL.md`.

---

## Engine ABI

### `Op` — inputs into the engine

Sent via `EngineHandle::send()`. All variants serialize with `"op"` as the
discriminant tag.

| Variant | Key fields | Purpose |
|---|---|---|
| `Submit` | `content`, `mode`, `model`, `provider` | Send a user message and start a turn |
| `Cancel` | — | Abort the current turn |
| `AcceptApproval` | `id`, `decision` | Resolve a pending approval gate |
| `SpawnSubAgent` | `prompt` | Launch a DARS sub-agent pass |
| `ChangeMode` | `mode` | Switch between Plan / Agent / Yolo |
| `SetModel` | `model`, `provider` | Change the active model mid-session |
| `CompactContext` | — | Force an immediate compaction |
| `Shutdown` | — | Cleanly terminate the engine task |

Example `Submit` payload:

```json
{
  "op": "submit",
  "content": "Fix the bug in clamp.py",
  "mode": "agent",
  "model": "claude-opus-4-7",
  "provider": "anthropic"
}
```

### `Event` — outputs from the engine

Emitted via `EngineHandle::next_event()`. All variants serialize with
`"event"` as the discriminant tag.

| Variant | Key fields | Purpose |
|---|---|---|
| `TurnStarted` | `turn_id` | A new turn has begun |
| `Delta` | `turn_id`, `channel` (`text`/`thinking`), `delta` | Incremental text |
| `ToolCallStarted` | `turn_id`, `tool_call_id`, `name`, `input` | Tool about to run |
| `ToolCallFinished` | `turn_id`, `tool_call_id`, `name`, `output`, `is_error` | Tool result |
| `ApprovalRequest` | `turn_id`, `approval_id`, `tool_name`, `summary` | Waiting for user |
| `Status` | `turn_id?`, `message` | Informational status line update |
| `RiskBandChanged` | `turn_id`, `band`, `action` | Capacity controller fired |
| `SeamApplied` | `turn_id`, `level`, `tokens_archived` | Seam manager archived context |
| `CycleAdvanced` | `from`, `to` | Session cycled |
| `CompactionApplied` | `before_tokens`, `after_tokens` | Destructive compaction ran |
| `PlanUpdated` | `turn_id`, `goal`, `items` | Agent updated its plan (3.7) |
| `DarsResult` | `winner_index`, `branch_count`, `votes` | DARS pass complete (3.8) |
| `TurnComplete` | `turn_id` | Turn finished normally |
| `TurnAborted` | `turn_id`, `reason` | Turn aborted |
| `Error` | `message` | Non-recoverable error |

Example `Delta` payload:

```json
{
  "event": "delta",
  "turn_id": "turn-4a3b2c1d-...",
  "channel": "text",
  "delta": "The fix involves changing line 7 to..."
}
```

---

## Turn lifecycle

The turn loop lives in `crates/agent/src/engine.rs`. Steps in order:

1. **Pre-turn snapshot** — if `checkpoint_enabled`, write a side-git
   snapshot of the workspace before any mutations.
2. **Context pre-check** — `CapacityController::observe_pre_turn()`.
   If `RiskBand::High` and cooldown expired, inject a synthetic
   `VerifyAndReplan` message.
3. **Seam check** — `SeamManager::maybe_apply()`. At 192 K / 384 K / 576 K
   tokens: summarise the head slice via `FlashCompactor` (if
   `compaction_enabled`), append an `<archived_context>` block, emit
   `Event::SeamApplied`. Non-destructive; prefix cache intact.
4. **Cycle check** — `CycleManager::maybe_apply()`. At 768 K tokens:
   archive the full session, restart with a Briefing prompt, emit
   `Event::CycleAdvanced`. Prefix cache hard-reset.
5. **Tool catalog setup** — in `AppMode::Plan`, strip write/exec tools;
   keep read + planning tools only.
6. **Auto-routing** — if `routing_enabled`, call `Router::pick(last_message)`
   to select provider + model + reasoning effort. Emit `Event::Status`.
7. **Build LLM request** — `ChatRequest` with system prompt, session
   messages (with seam blocks appended), tool schemas.
8. **Stream request** — `LlmClient::stream()`. Render `Delta` events
   as each chunk arrives. Accumulate tool-call input buffers.
9. **Finalise tool inputs** — multi-stage recovery: raw JSON → strip code
   fences → double-decode → balanced-brace extraction.
10. **Tool execution** — read-only tools run in parallel; destructive tools
    run serially. Each destructive tool goes through
    `ExecPolicyEngine::check()`. If `NeedsApproval`, emit
    `Event::ApprovalRequest` and block until `Op::AcceptApproval` arrives.
11. **Post-tool syntax check** — `syntax::check_syntax()` on every edited
    file. On failure, return `ToolError::SyntaxError` to the model.
12. **Post-tool auto-test** — if `auto_test_enabled`, detect and run the
    project test runner. On failure, inject output as the next turn message.
    Max 3 retries before surfacing to user.
13. **Checkpoint** — if files were modified, record a named checkpoint in
    `.agent-tui/checkpoints/`.
14. **Loop** — if `finish_reason == "tool_calls"`, return to step 2.
15. **Post-turn snapshot** — final side-git snapshot.
16. **Emit** `Event::TurnComplete`.

---

## Extension toggles

All toggles live under `[extensions]` in the config. See
`docs/CONFIGURATION.md` for defaults and allowed values.

| Config key | Phase | Default | What it controls |
|---|---|---|---|
| `checkpoint_enabled` | 3.1 | `true` | Per-turn workspace snapshots and rollback |
| `lint_on_edit` | 3.2 | `true` | Tree-sitter syntax check on every edit (always on at tool layer) |
| `auto_test` | 3.3 | `true` | Auto-run test suite after file mutations |
| `retrieval_mode` | 3.4 | `"hybrid"` | `"ripgrep"`, `"graph"`, `"semantic"`, or `"hybrid"` |
| `memory_enabled` | 3.5 | `true` | Cross-session lesson memory (`~/.agent-tui/lessons.json`) |
| `routing` | 3.6 | `"auto"` | `"auto"` (pre-turn model selection) or `"off"` |
| `plan_blocks` | 3.7 | `true` | `<plan>` + `<reflect>` structured blocks and side panel |
| `dars_branching` | 3.8 | `true` | DARS parallel sub-agent branching at decision points |
| `verifier_count` | 3.8 | `3` | Number of verifier sub-agents in DARS |
| `compaction_enabled` | 3.9 | `true` | Flash-tier LLM summarization for seam/cycle blocks |
| `repl_tools` | 3.10 | `false` | Stateful `/bin/sh` REPL tools (`repl_open/eval/close`) |
| `pipeline_enabled` | 3.11 | `false` | Hierarchical pipeline for `agent-tui fix` |
