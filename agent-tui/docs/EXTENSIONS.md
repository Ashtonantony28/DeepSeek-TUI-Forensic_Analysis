# Extensions

`agent-tui` ships twelve research-backed extensions on top of the core
DeepSeek-TUI replication. Every extension is togglable in
`[extensions]` in the config. This document describes each extension,
its research basis, its config flag, expected behaviour, an end-to-end
example, and known limitations.

---

## 3.1 — Checkpoint and rollback

**Config flag:** `checkpoint_enabled = true` (default: on)

**Research basis:** Side-git snapshotting from the DeepSeek-TUI reference.

**What it does:**
After every tool call that modifies files, a named checkpoint is written to
`.agent-tui/checkpoints/<timestamp>-<turn>`. The TUI shows a live list on
`Ctrl+Z`. Each checkpoint includes a diff preview (`similar` crate) against
the workspace at that point.

**Tools exposed:**
- `list_checkpoints` — list available checkpoints with timestamp and summary
- `checkpoint_diff <id>` — show the diff for a checkpoint
- `rollback <id>` — restore the workspace to that checkpoint

**Example:**

```
User: Add a new validate() function to utils.py
Agent: [write_file utils.py ...]
       Checkpoint cp-001 saved.

User: Actually revert that
Agent: [rollback cp-001]
       Workspace restored to cp-001.
```

**Limitations:** Checkpoints store diffs relative to the workspace at
snapshot time. Rolling back across a `git commit` may leave the working tree
in a modified state.

---

## 3.2 — Linter-on-edit (ACI)

**Config flag:** `lint_on_edit = true` (default: on, always enforced at tool layer)

**Research basis:** SWE-agent Agent-Computer Interface insight —
"verify edits before they propagate" (Yang et al., NeurIPS 2024,
[arxiv 2405.15793](https://arxiv.org/abs/2405.15793)).

**What it does:**
Every `write_file`, `edit_file`, and `apply_patch` tool call runs a
tree-sitter syntax check on the resulting file content before returning
success. Supported languages: Rust, Python, JavaScript, TypeScript, Go, C,
C++, Java. Unknown extensions pass unconditionally.

If parsing fails, the tool returns `ToolError::SyntaxError { position, message }`
and the model sees: "Syntax error at line N: <message>. Fix before
continuing." The edit is not persisted.

**Example:**

```
Agent: [write_file src/main.rs]  ← file missing a closing brace
Tool:  SyntaxError at line 42: unexpected EOF
Agent: [write_file src/main.rs]  ← corrected version
Tool:  ok
```

**Limitations:** Tree-sitter grammars are pinned (see `AGENTS.md`). Adding
a new language grammar requires updating all grammar versions together.

---

## 3.3 — Auto-test validation loop

**Config flag:** `auto_test = true` (default: on)

**What it does:**
After a destructive tool batch, the engine detects the project's test runner
and runs a scoped test pass:

| Indicator | Runner |
|---|---|
| `Cargo.toml` | `cargo test` |
| `pyproject.toml` or `pytest.ini` | `pytest` |
| `package.json` with a `test` script | `npm test` |
| `go.mod` | `go test ./...` |

On failure, the test output is injected as the next turn's user message:
"Tests failed after your last edit. Fix them without reverting correct
changes." Up to **3 auto-retry rounds** are attempted before surfacing
to the user.

**Example:**

```
Agent: [write_file src/lib.rs]
Auto-test: cargo test ... FAILED (2 tests)
Agent: [sees failure output, fixes the issue]
Auto-test: cargo test ... ok (all tests pass)
```

**Limitations:** The test runner is detected by file presence, not by
actually parsing the project. Projects with non-standard layouts may need
a project overlay to disable auto_test and run tests manually.

---

## 3.4 — Semantic + structural retrieval

**Config flag:** `retrieval_mode = "hybrid"` (default)

**Research basis:**
- cAST — AST-aware chunking for code RAG
  ([arxiv 2506.15655](https://arxiv.org/abs/2506.15655))
- Aider's repo-map — PageRank over the import graph (production system)

**What it does:**
Three retrievers run in parallel and their results are fused with
Reciprocal Rank Fusion (RRF):

1. **`RipgrepRetriever`** — grep search over source files; always
   available.
2. **`RepoDependencyRetriever`** — builds a directed import-edge graph
   and ranks files with PageRank. Surfaces the most structurally
   connected files first.
3. **`SemanticRetriever`** — chunks each source file by function/class
   with tree-sitter language-specific queries. Embeds each chunk with
   `nomic-embed-text` via Ollama (fallback: OpenAI embeddings). Stores
   in `sqlite-vec`. Invalidates stale chunks on file mtime change.

**Tool exposed:** `semantic_search(query, top_k)` — returns file:line
snippets ranked by hybrid score.

**Command:** `agent-tui index [--graph-only]` — full rebuild. Runs
incrementally after every file edit during a session.

**Example:**

```
User: Find all places that call the validate() function
Agent: [semantic_search "validate() calls" top_k=10]
Tool:  src/api.rs:47, src/batch.rs:112, tests/integration.rs:23
```

**Limitations:** Semantic search requires Ollama to be running for
embeddings. When Ollama is unreachable, only ripgrep and graph retrieval
are used; a one-line warning is emitted at startup.

---

## 3.5 — Hierarchical memory and Reflexion

**Config flag:** `memory_enabled = true` (default: on)

**Research basis:**
- Reflexion (Shinn et al., NeurIPS 2023,
  [arxiv 2303.11366](https://arxiv.org/abs/2303.11366))
- MAR failure-mode mitigation ([arxiv 2512.20845](https://arxiv.org/abs/2512.20845))

**What it does:**
Three memory tiers:

1. **Working memory** — the current session context (always present).
2. **Episodic memory** — rolling per-session summary compacted by the
   flash model when context approaches the next seam threshold.
3. **Semantic memory** — persistent lessons file at
   `~/.agent-tui/lessons.json`. The top-K relevant lessons (cosine
   similarity to the current prompt embedding) are appended to the
   system prompt on each turn without mutating the session messages
   (prefix cache stays hot).

**Tools exposed:**
- `remember <lesson>` — add a lesson to semantic memory
- `recall <query>` — retrieve similar lessons

**Reflexion buffer:** After any failed tool batch or test failure, the
agent emits a `<reflect>` block analysing what went wrong. Reflections
are injected as low-priority context on the next turn. Reflections that
diverge from the original task (cosine similarity below threshold) are
dropped to prevent the MAR hallucination failure mode.

**Example:**

```
Agent: [tool fails]
Agent: <reflect>The path was relative; I should always use absolute paths
       when calling write_file.</reflect>
[next turn]: reflection injected into context
```

**Limitations:** Cosine-similarity gating for the MAR mitigation requires
Ollama for embedding. When Ollama is unavailable, all reflections are kept
(no filtering).

---

## 3.6 — Auto model routing

**Config flag:** `routing = "auto"` (default: on)

**What it does:**
Before each turn, a single cheap routing call (the Small-tier model for the
active provider) classifies the prompt and selects provider, model, and
reasoning effort. The selection is shown in the TUI status bar.

Routing tiers and their heuristics:

| Tier | Conditions | Default mapping |
|---|---|---|
| Small | Short Q&A, no file edits | Haiku / DeepSeek Flash |
| Mid | Single-file edits, debugging | Sonnet / DeepSeek Chat |
| Large | Architecture, security, multi-file | Opus / DeepSeek V4 Pro |

The routing call fails gracefully: if the provider is unreachable or the
response cannot be parsed, the local heuristics apply instead.

**Override commands:**
- `/model <name>` — pin the model for the rest of the session
- `/provider <name>` — pin the provider

**Example:**

```
[status bar]: router: Mid → claude-sonnet-4-6
```

**Limitations:** Routing adds one cheap API call per turn. Disable with
`routing = "off"` if latency matters more than cost.

---

## 3.7 — Plan and reflect dual blocks

**Config flag:** `plan_blocks = true` (default: on)

**What it does:**
When in Plan mode (or after a failed tool batch), the agent emits two
structured blocks:

- `<plan goal="..." subtasks="...">` — goal, sub-tasks, estimated complexity
- `<reflect>` — what just succeeded or failed and what to do differently

The `update_plan` tool triggers `Event::PlanUpdated` which the TUI renders
in the right-hand side panel (toggle with `p`). Plans are re-injected into
the next turn at high priority. Reflect blocks feed the Reflexion buffer
from 3.5 at low priority.

**Example:**

```
Agent: <plan goal="Fix the off-by-one in clamp.py">
         <step done="false">Read clamp.py</step>
         <step done="false">Edit line 7</step>
         <step done="false">Run tests</step>
       </plan>
```

**Limitations:** The plan side panel is not yet persistent across sessions.
Plan state is lost when the TUI exits.

---

## 3.8 — DARS-style branching

**Config flags:** `dars_branching = true`, `verifier_count = 3` (defaults)

**Research basis:** DARS — Dynamic Action Re-Sampling (Aggarwal et al.,
ACL 2025, [arxiv 2503.14269](https://arxiv.org/abs/2503.14269)).

**What it does:**
At "high uncertainty" decision points, the engine spawns 2–4 parallel
sub-agents (branches), each with the same context up to the branch point but
instructed to pursue a different approach. `verifier_count` verifier
sub-agents then score each candidate on a rubric and vote. The winner is
adopted and surfaced as a regular `Delta` event; `Event::DarsResult` carries
the vote breakdown.

Uncertainty signals: multiple retrieval hits for the same symbol, ambiguous
test failures, a tool call whose first attempt errored.

Hard limits: max 3 active branches per turn; max 1 branching event per turn.

**Example:**

```
[status]: DARS: 3 branches spawned
[status]: DARS: winner=branch-1 (votes: [2,1,0])
```

**Limitations:** DARS multiplies the token cost of the turn by
`branch_count + verifier_count`. On long contexts this can be expensive.
Disable with `dars_branching = false` if cost is a concern.

---

## 3.9 — Flash-tier compaction

**Config flag:** `compaction_enabled = true` (default: on)

**What it does:**
Replaces the Phase-2 placeholder strings in seam and cycle archived blocks
with real summaries produced by the Small-tier model. The compactor uses
the same provider as the main session but routes to the router's Small model
so the reasoning model is never used for summarisation.

When `compaction_enabled = false`, archived blocks contain the literal string
`[seam summary disabled]` — useful for fully-offline testing with a mock
client.

**Limitations:** The compactor reuses the same `LlmClient` as the main
reasoning model. On providers where small and reasoning models live on
different endpoints, two clients would be needed. This is a known open
follow-up (see `EXTENSIONS_STATUS.md`).

---

## 3.10 — Stateful REPL tools

**Config flag:** `repl_tools = false` (default: off)

**Research basis:** EnIGMA (ICML 2025) — stateful subprocess tools for
coding agents.

**What it does:**
Three tools backed by persistent long-running subprocesses:

- `repl_open(name, workdir)` — spawn a `/bin/sh` REPL and return a handle
- `repl_eval(name, code)` — execute code in the REPL; output is bounded to
  32 KiB per call; per-call timeout is 30 s
- `repl_close(name)` — terminate the REPL

State persists across tool calls within a session. The model can open a
shell, run a build, inspect output, and run further commands without
losing shell state.

**Example:**

```
Agent: [repl_open "build" "/workspace"]
Agent: [repl_eval "build" "cargo build 2>&1"]
Tool:  Compiling agent-tui v0.1.0 ...
       Finished release [optimized]
Agent: [repl_eval "build" "ls target/release/agent-tui"]
Tool:  target/release/agent-tui
```

**Limitations:** REPL sessions run as plain `/bin/sh` children and inherit
the process environment. The `execpolicy` sandbox layer is not yet applied.
A future hardening pass should route REPL sessions through `SandboxedCommand`.

---

## 3.11 — Hierarchical pipeline (Agentless)

**Config flag:** `pipeline_enabled = false` (default: off)

**Research basis:**
- Agentless (Xia et al., 2024,
  [arxiv 2407.01489](https://arxiv.org/abs/2407.01489)) — hierarchical
  localise → repair → validate pipeline.
- AutoCodeRover (Zhang et al., 2024,
  [arxiv 2404.05427](https://arxiv.org/abs/2404.05427)) — SBFL integration.

**What it does:**
`agent-tui fix <issue>` bypasses the free-form agent loop and runs the
`HierarchicalPipeline`:

1. **Localise** — query `HybridRetriever` with the issue title + body.
   Group hits by file path; rank by summed score. Take top 6 candidates.
2. **Repair** — spawn a `SubAgentManager` session with a prompt containing
   the per-candidate code snippets and an explicit instruction to produce
   a unified diff.
3. **Validate** — syntactic `looks_like_unified_diff()` check; optional
   `Validator` plug-in (e.g. run the test suite against the patch).
4. **Output** — print the unified diff to stdout.

**Example:**

```bash
agent-tui fix "clamp() returns wrong value for boundary inputs"
# or pass a file:
agent-tui fix issue-42.md
```

**Limitations:** The repair sub-agent runs once by default
(`repair_attempts = 1`). The multi-attempt loop is wired but not exercised
from the CLI yet. SBFL (spectrum-based fault localisation) is not yet
integrated; the pipeline uses retrieval ranking only.

---

## 3.12 — MCP tool integration

**Config flag:** set `mcp_config_path` in the user config (see `docs/MCP.md`)

**What it does:**
Remote MCP server tools appear in the engine's tool registry under the
`server:tool` qualified name scheme. `McpToolAdapter` wraps
`Arc<dyn McpManagedClient>` and translates the standard MCP
`{content:[{type:"text",text:...}]}` response envelope into the `Tool`
trait's `ToolResult`. The `isError` flag is surfaced as `is_error: true`
on the `ToolCallFinished` event.

`ToolRegistry::register_mcp_tools(manager)` bulk-registers everything a
`McpManager` reports. Registration failures are surfaced as warnings and do
not block the rest of the engine.

**Commands:**
- `agent-tui mcp list` — load the MCP config, spawn all servers, print
  their tools
- `agent-tui mcp probe --command <cmd> [--args ...]` — spawn one server
  ad-hoc and print its tools

**Example:**

```
$ agent-tui mcp list
filesystem:read_file — Read the contents of a file
filesystem:write_file — Write content to a file
brave-search:web_search — Search the web using Brave Search
```

**Limitations:** There is no per-workspace MCP overlay yet. All MCP
configuration lives in the user-level file set by `mcp_config_path`.
