# Tasks

<!-- Scenario A: Phase 1, 2, 3 marked [x] based on the planning record. The first cycle -->
<!-- of the orchestrator dispatches the AUDITOR worker to verify those [x] entries against -->
<!-- the actual code in the repo and flip any that don't match. Do not trust [x] entries -->
<!-- on completed phases until the auditor confirms them. -->

## Phase 0 — Onboarding (the orchestrator's first cycle)

- [ ] TASK-000: Auditor inventory of branch `claude/agent-tui-phase-3-final-vE9RD` —
  deps: none — files: STATUS.md (append-only). Verifies workspace builds, lists actual
  crates present, confirms which Phase 3 extensions are wired and tested, flags any
  divergence from the [x] entries below. **First action of the workflow.**

## Phase 1 — Forensic analysis (complete; auditor will verify)

- [x] TASK-101: Clone DeepSeek-TUI reference repo and produce ANALYSIS.md —
  files: ANALYSIS.md, crate_map.dot
- [x] TASK-102: Identify gaps and locked-in stack decisions — files: ANALYSIS.md
- [x] TASK-103: Produce crate dependency graph — files: crate_map.dot

## Phase 2 — Replication (complete; auditor will verify)

- [x] TASK-201: agent-tui-protocol crate (types, Op/Event enums) —
  files: crates/agent-tui-protocol/
- [x] TASK-202: agent-tui-config crate (TOML, 5-layer cascade) —
  files: crates/agent-tui-config/
- [x] TASK-203: agent-tui-llm crate + 7 provider adapters (Anthropic, OpenAI, DeepSeek,
  Groq, xAI, Ollama, OpenAI-compat) — files: crates/agent-tui-llm/
- [x] TASK-204: agent-tui-execpolicy (seccomp / sandbox-exec / Job Objects + approval gate +
  egress) — files: crates/agent-tui-execpolicy/
- [x] TASK-205: agent-tui-tools (read/write/edit/list/search/shell/fetch/git with
  linter-on-edit) — files: crates/agent-tui-tools/
- [x] TASK-206: agent-tui-context (SeamManager 192K/384K/576K, CycleManager 768K,
  Compaction, CapacityController) — files: crates/agent-tui-context/
- [x] TASK-207: agent-tui-retrieval (RipgrepRetriever; trait for later expansion) —
  files: crates/agent-tui-retrieval/
- [x] TASK-208: agent-tui-agent (Engine, turn loop, Op/Event mpsc, multi-stage parser) —
  files: crates/agent-tui-agent/
- [x] TASK-209: agent-tui-subagent (open/eval/close + parallel fan-out) —
  files: crates/agent-tui-subagent/
- [x] TASK-210: agent-tui-pipeline stub (real impl in 3.11) —
  files: crates/agent-tui-pipeline/
- [x] TASK-211: agent-tui-mcp (rmcp client, stdio + HTTP) — files: crates/agent-tui-mcp/
- [x] TASK-212: agent-tui-acp (ACP server, first-class) — files: crates/agent-tui-acp/
- [x] TASK-213: agent-tui-tui (5-panel ratatui UI) — files: crates/agent-tui-tui/
- [x] TASK-214: agent-tui-cli (dispatcher, login, doctor, models, serve, fix stub) —
  files: crates/agent-tui-cli/

## Phase 3 — Extensions (complete per conversation log; auditor will verify each)

- [x] TASK-301: Checkpoint and rollback (git stash + Ctrl+Z overlay) — extension 3.1
- [x] TASK-302: Linter-on-edit ACI strengthening + integration test — extension 3.2
- [x] TASK-303: Auto-test validation loop (cargo/pytest/npm/go, max 3 retries) — 3.3
- [x] TASK-304: Semantic + structural retrieval (tree-sitter, PageRank, sqlite-vec) — 3.4
- [x] TASK-305: Hierarchical memory + Reflexion (working/episodic/semantic) — 3.5
- [x] TASK-306: Auto model routing (pre-turn routing call + heuristic fallback) — 3.6
- [x] TASK-307: Plan + reflect dual blocks (side panel) — 3.7
- [x] TASK-308: DARS-style branching at uncertainty points — 3.8
- [x] TASK-309: FlashCompactor with flash-tier compaction (5 tests added) — 3.9 variant
- [x] TASK-310: REPL tools (repl_open/eval/close for /bin/sh) — 3.12 variant
- [x] TASK-311: Hierarchical pipeline (localise → repair → validate) — 3.11
- [x] TASK-312: MCP integration polish (McpToolAdapter, list/probe subcommand) — extension
- [x] TASK-313: FlashCompactor multi-endpoint fix (optional second LlmClient) — bonus session

## Known gaps from Phase 3 (auditor must check and flag)

- [ ] TASK-320: Verify REPL sessions go through execpolicy `SandboxedCommand` layer —
  deps: TASK-000 — files: crates/agent-tui-tools/src/repl.rs (or wherever REPL lives).
  Phase 3 status report noted REPLs run plain `/bin/sh` without sandboxing. If still true,
  this is a constraint violation (see PLAN.md regulation 5).
- [ ] TASK-321: Verify pipeline's `repair_attempts > 1` is actually exercised by at least
  one test — deps: TASK-000 — files: crates/agent-tui-pipeline/. Phase 3 noted the field
  is wired but no caller uses >1. Add a test or document as known limitation.

## Phase 4 — Evaluation harness (the next implementation work)

- [ ] TASK-401: Scaffold agent-tui-eval crate with module skeleton —
  deps: TASK-000 — files: crates/agent-tui-eval/Cargo.toml,
  crates/agent-tui-eval/src/lib.rs, workspace Cargo.toml
- [ ] TASK-402: Implement SWE-bench Verified instance fetcher (HuggingFace via hf-hub or
  shell-out, `--subset N` and `--subset-ids` flags) — deps: TASK-401 —
  files: crates/agent-tui-eval/src/dataset.rs
- [ ] TASK-403: Implement instance isolation (Docker if available; clean git-clone
  fallback; startup detection logged) — deps: TASK-401 —
  files: crates/agent-tui-eval/src/isolation.rs
- [ ] TASK-404: Implement runner invoking `agent-tui fix --yolo --headless` per instance
  with time budget; capture patch, turns, tokens, cost, wallclock —
  deps: TASK-402, TASK-403 — files: crates/agent-tui-eval/src/runner.rs
- [ ] TASK-405: Implement mutation-strengthening module with tree-sitter AST rewrites for
  4 operators (return flips, comparison swaps, conditional negations, off-by-one). Header
  comment cites arxiv 2603.00520 — deps: TASK-401 —
  files: crates/agent-tui-eval/src/mutation.rs
- [ ] TASK-406: Implement results aggregation and JSON output to
  `eval-runs/<timestamp>.json` per spec in PLAN.md — deps: TASK-404, TASK-405 —
  files: crates/agent-tui-eval/src/output.rs
- [ ] TASK-407: Implement `--compare run-a.json run-b.json` producing markdown diff table
  (pass rates, semantic-pass rates, cost delta, turn delta, per-instance flips) —
  deps: TASK-406 — files: crates/agent-tui-eval/src/compare.rs
- [ ] TASK-408: Implement `--dry-run` mode with 2+ synthetic instances exercising the full
  pipeline including mutation strengthening — deps: TASK-406 —
  files: crates/agent-tui-eval/src/dry_run.rs
- [ ] TASK-409: Wire `agent-tui eval` subcommand into agent-tui-cli — deps: TASK-406,
  TASK-407, TASK-408 — files: crates/agent-tui-cli/src/commands/eval.rs,
  crates/agent-tui-cli/src/main.rs
- [ ] TASK-410: Reject `--scale N > 1` in interactive mode with clear error message —
  deps: TASK-409 — files: crates/agent-tui-cli/src/commands/eval.rs (or main.rs)
- [ ] TASK-411: Unit tests for eval crate (`cargo test -p agent-tui-eval`) covering
  mutation operators, JSON shape, compare output, dry-run pipeline — deps: TASK-408 —
  files: crates/agent-tui-eval/tests/
- [ ] TASK-412: Run 5-instance `--scale 1 --dry-run`; record output —
  deps: TASK-411 — files: eval-runs/ (output), STATUS.md (summary)
- [ ] TASK-413: Run 5-instance `--scale 4 --dry-run`; record output — deps: TASK-412 —
  files: eval-runs/ (output), STATUS.md (summary)
- [ ] TASK-414: Run `agent-tui eval --compare` on the two dry-run files; record markdown
  table — deps: TASK-413 — files: STATUS.md
- [ ] TASK-415: Phase 4 status block to STATUS.md — deps: TASK-414 — files: STATUS.md

## Phase 5 — Docs, CI, release packaging

- [ ] TASK-501: Write AGENTS.md (stable-Rust constraints, pitfalls, PR shape) —
  deps: TASK-415 — files: AGENTS.md
- [ ] TASK-502: Write docs/ARCHITECTURE.md (crate map, engine ABI, turn lifecycle,
  extension toggles) — deps: TASK-415 — files: docs/ARCHITECTURE.md
- [ ] TASK-503: Write docs/CONFIGURATION.md (full TOML schema, env vars, cascade,
  `[extensions]` keys) — deps: TASK-415 — files: docs/CONFIGURATION.md
- [ ] TASK-504: Write docs/EXTENSIONS.md (12 extensions with arxiv citations) —
  deps: TASK-415 — files: docs/EXTENSIONS.md
- [ ] TASK-505: Write docs/EVAL.md (harness usage, mutation rationale, limitations) —
  deps: TASK-415 — files: docs/EVAL.md
- [ ] TASK-506: Write docs/MCP.md and docs/ACP.md — deps: TASK-415 —
  files: docs/MCP.md, docs/ACP.md
- [ ] TASK-507: Write docs/PROVIDERS.md (7 providers with auth, base URLs, quirks) —
  deps: TASK-415 — files: docs/PROVIDERS.md
- [ ] TASK-508: Write README.md and CHANGELOG.md — deps: TASK-501..TASK-507 —
  files: README.md, CHANGELOG.md
- [ ] TASK-509: GitHub Actions CI workflow (matrix build + fmt + clippy + test) —
  deps: TASK-415 — files: .github/workflows/ci.yml
- [ ] TASK-510: GitHub Actions release workflow (5 prebuilt targets, SHA-256, attach to
  release) — deps: TASK-415 — files: .github/workflows/release.yml
- [ ] TASK-511: npm wrapper package (postinstall downloads correct prebuilt binary) —
  deps: TASK-510 — files: npm/agent-tui/package.json, npm/agent-tui/install.js
- [ ] TASK-512: Final smoke test sequence (cargo build/fmt/clippy/test; agent-tui
  login/doctor/models; interactive; fix on tiny repo; eval --subset 5 --dry-run) —
  deps: TASK-501..TASK-511 — files: STATUS.md
- [ ] TASK-513: Tag v0.1.0 and push — **deps: TASK-512 AND every Definition of Done
  item ticked**. Pause for human confirmation before tagging — risky, irreversible action.
  files: git tag, STATUS.md final entry

## Notes on parallelism

- TASK-401 must finish first (scaffold the crate).
- TASK-402, TASK-403, TASK-405 are independent after TASK-401 → can run in parallel
  (3 workers, disjoint files: `dataset.rs`, `isolation.rs`, `mutation.rs`).
- TASK-404 needs both TASK-402 and TASK-403 → sequential.
- TASK-406, TASK-407, TASK-408 touch different files (`output.rs`, `compare.rs`,
  `dry_run.rs`) → parallel after their deps complete.
- TASK-501..TASK-507 (docs) are all independent → can batch into one or two implementer
  workers since they touch different files but are small and similar in nature.
- TASK-509 and TASK-510 (CI yamls) → parallel.
