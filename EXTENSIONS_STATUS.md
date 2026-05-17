# EXTENSIONS_STATUS.md

Final status of Phase 3 extensions for `agent-tui`. Phase 3.1–3.8 were
completed in prior sessions; this session implemented 3.9 through 3.12.

> **Note on the build plan.** `agent-tui-build-plan.md` was not present
> in this branch when the session started, only `ANALYSIS.md` and
> `crate_map.dot`. The names and feature scopes for 3.9–3.12 below are
> inferred from the existing scaffolding (config flags, stubbed
> commands, placeholder strings, and reserved crate slots) plus the
> hint that "3.11 is when `agent-tui-pipeline` gets its real
> implementation." Anyone with the original plan should diff this
> assignment against it.

---

## 3.1 — Checkpoint store *(prior session)*

Per-turn checkpoints under `.agent-tui/checkpoints/`. Snapshots are
created after every successful destructive tool call when
`Extensions::checkpoint_enabled` is true. Surfaces `list_checkpoints`,
`checkpoint_diff`, `rollback` tools.

## 3.2 — Lint-on-edit *(prior session)*

Tree-sitter syntax check on every edit/write/apply_patch result.
Failure surfaces as a `ToolError` so the model retries.

## 3.3 — Auto-test loop *(prior session)*

After a destructive tool batch the engine detects the project's test
runner (cargo / pytest / npm / go) and runs it. Failures get injected
as the next turn's user message, up to `MAX_RETRIES`.

## 3.4 — Hybrid retrieval *(prior session)*

AST chunker + repo PageRank + Ollama-backed embeddings fused with RRF.
`HybridRetriever` falls back gracefully when Ollama is unreachable.
Exposed to the engine as the `semantic_search` tool and as the
`Retriever` used by Phase 3.11.

## 3.5 — Cross-session memory *(prior session)*

`MemoryStore` reads `~/.agent-tui/lessons.json`. Top-K relevant
lessons are appended to the system prompt on each turn without
mutating the persistent `session.system_prompt` — keeps the prefix
cache hot. Tools `remember` and `recall` are user-facing.

## 3.6 — Auto routing *(prior session)*

`Router::pick(prompt)` classifies prompts as Small / Mid / Large and
maps to provider-specific model names. The engine swaps the session
model and emits an `Event::Status` (`router: tier -> model`) when
`routing == "auto"`.

## 3.7 — Plan blocks *(prior session)*

The `update_plan` tool call now triggers an `Event::PlanUpdated` for
the TUI's right-hand plan pane. `extract_plan` accepts both string and
`{step, done}` step shapes.

## 3.8 — DARS branching with multi-verifier *(prior session)*

`Op::SpawnSubAgent` runs a DARS pass: `branch_count` parallel
sub-agents on the same prompt with diverse approach hints, then
`verifier_count` verifier sub-agents vote on the winner. Ties break
to the lowest-indexed (control) candidate. Surfaces `Event::DarsResult`
with the vote breakdown.

## 3.9 — Flash-tier compaction *(this session)*

**Goal.** Replace the Phase-2 placeholder strings
(`"[Phase-2 seam summary placeholder]"`,
`"[Phase-2 cycle briefing placeholder]"`) with real summaries
produced by a cheap-model LLM call.

**Implementation.**
- `agent-tui-agent::FlashCompactor` (new file `compactor.rs`).
- `Engine::maybe_summarize_seam(outcome)` runs the compactor over the
  head slice; `maybe_summarize_cycle()` runs over the whole message
  log only when tokens already exceed `cycle.config.cycle_tokens`.
- New `Extensions::compaction_enabled` (default on). When off, the
  engine emits a static `[seam summary disabled]` string so seams
  still archive deterministically.
- Compactor model is resolved from `Router::Small` for the active
  provider — never the user-pinned reasoning model.

**Tests.** 3 unit tests in `compactor.rs` + 2 end-to-end tests in
`tests/flash_compaction.rs`.

## 3.10 — REPL tools *(this session)*

**Goal.** Replace the upstream RLM Python-kernel surface with a
Rust-only equivalent (ANALYSIS.md §11). Build plan forbids the Python
runtime dependency.

**Implementation.**
- New `crates/tools/src/tools/repl.rs`. `ReplRegistry` keyed by name
  owns long-lived `/bin/sh` subprocesses (interactive, with merged
  stderr→stdout). Each `eval` writes the code followed by a sentinel
  `echo` so output is bounded per call.
- Three tools registered when `Extensions::repl_tools = true`:
  `repl_open`, `repl_eval`, `repl_close`. Default off.
- Per-call timeout (`timeout_secs`, default 30); output capped at
  `MAX_OUTPUT_BYTES = 32 KiB`.
- `ToolContext::repl: Option<Arc<ReplRegistry>>` so disabling the
  feature makes the tools return `ToolError::NotAvailable` instead of
  panicking.

**Tests.** 5 unit tests covering open / eval / state persistence /
double-open / close-unknown / timeout.

## 3.11 — Hierarchical pipeline *(this session)*

**Goal.** Replace the Phase-2 `AgentLoopPipeline` stub
(`PipelineError::NotImplemented`) with a real Agentless-style
localise → repair → validate pipeline; wire the CLI `fix` command.

**Implementation.**
- New `HierarchicalPipeline` in `crates/pipeline/src/lib.rs`.
  - **Localise:** queries `Retriever::search(title + body)`, groups
    hits by path, ranks by summed score, takes top `max_candidates`
    (default 6).
  - **Repair:** spawns a `SubAgentManager` session with a templated
    prompt that includes per-candidate snippets and explicitly asks
    for a unified diff. `extract_diff()` accepts fenced ```diff blocks
    and bare `diff --git` / `--- a/` patches.
  - **Validate:** `looks_like_unified_diff()` syntactic check first;
    if a `Validator` impl is plugged in, hand the patch to it.
- CLI: `agent-tui fix <issue-or-path>` constructs the pipeline against
  the workspace's `HybridRetriever` and prints the resulting diff.
- New deps on `pipeline`: `llm`, `retrieval`, `subagent`,
  `serde_json`, `regex`, `tracing`.

**Tests.** 8 unit tests covering diff extraction, dedup-and-rank
localisation, full pipeline against MockClient, the no-diff failure
path, validator-driven rejection, and the empty-retrieval case.

## 3.12 — MCP tool integration *(this session)*

**Goal.** Expose remote MCP tools to the engine through the same
`Tool` trait + registry that built-ins use.

**Implementation.**
- New `crates/tools/src/tools/mcp_adapter.rs`. `McpToolAdapter` wraps
  an `Arc<dyn McpManagedClient>` + `McpToolDescriptor`, returns the
  qualified `server:tool` name, flattens the standard MCP
  `{content:[{type:"text", text:...}]}` envelope, and surfaces the
  `isError` flag.
- `ToolRegistry::register_mcp_tools(manager)` bulk-registers
  everything `McpManager::list_all_tools()` reports.
- `McpManager::client_for(server)`, `server_names()`, and
  `spawn_and_register_all(cfgs)` round out the manager API so callers
  can spawn every stdio child in one shot and report which ones
  failed.
- CLI: new `agent-tui mcp` subcommand with `list` (loads from
  `cfg.mcp_config_path`) and `probe --command X --args ...` actions.
- Interactive + oneshot entry points now auto-register MCP tools at
  startup when `mcp_config_path` is set; failures are surfaced as
  `warning: mcp server ...` and do NOT block the rest of the engine.

**Tests.** 4 unit tests on the adapter + 1 integration test
(`tests/mcp_register.rs`) on `register_mcp_tools`.

---

## Aggregate test count

```
$ cargo test --workspace
passed: 135   failed: 0
```

Up from 119 at start-of-session (3.8 head).

## Open follow-ups

- The Flash compactor reuses the **same** `LlmClient` that the main
  reasoning model uses, just with a `Small`-tier model name. For
  providers where small and reasoning models live on different
  endpoints this needs a second `Arc<dyn LlmClient>` plumbed through.
- The pipeline currently runs the repair sub-agent **once**
  (`repair_attempts: 1` by default); the multi-attempt loop is wired
  but only documented, not exercised from any caller.
- The CLI `mcp list` reads `cfg.mcp_config_path` directly. There is no
  per-workspace MCP overlay yet — that probably wants a separate
  `[mcp]` section in `.agent-tui/config.toml`.
- REPL sessions run as plain `/bin/sh` children; they inherit the
  process environment. The execpolicy / sandbox layer is **not**
  applied to them yet. A future hardening pass should reroute them
  through `SandboxedCommand`.
- `agent-tui-eval` is still just the data shapes; the mutation-
  strengthening loop is correctly deferred to Phase 4 per the source
  comment in `crates/eval/src/lib.rs`.
