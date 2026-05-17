# Changelog

All notable changes to `agent-tui` are documented here.
This project follows [Semantic Versioning](https://semver.org/).

---

## [0.1.0] — 2026-05-17

Initial release. Phases 1–5 complete.

### What was replicated from DeepSeek-TUI

- **Streaming engine** — `Op` / `Event` ABI, `EngineHandle`, turn loop
  with multi-stage tool input recovery (raw JSON → code fence strip →
  double-decode → balanced-brace extraction)
- **Three-tier context management** — `SeamManager` (192 K / 384 K /
  576 K thresholds, non-destructive, prefix-cache-preserving),
  `CycleManager` (768 K hard reset), destructive `Compaction`
- **`CapacityController`** — `RiskBand` (Low / Medium / High),
  `GuardrailAction` up to `VerifyAndReplan`
- **Tool registry** — `Tool` async trait, approval gate via
  `execpolicy`, read/write/exec/git built-in tools
- **Tree-sitter syntax check** — every edit/write/patch rejected if
  the language is recognised and parsing fails
- **Plan mode** — tool catalog restricted to read-only + planning tools
- **MCP client** — stdio transport, `tools/list` / `tools/call`
- **LSP post-edit diagnostics** — language server queried after every
  file edit; diagnostics injected as the next turn's context
- **Sub-agent sessions** — `SubAgentManager::open` / `eval` / `close`
- **Execpolicy** — layered rulesets, approval gate, OS-native sandbox
  (seccomp / sandbox-exec / Job Objects), egress policy, secret redaction
- **ratatui TUI** — five panels: Composer, Transcript, Plan side panel,
  Status bar, Command palette
- **ACP server** — `serve --acp` (JSON-RPC 2.0 over stdio); Zed /
  JetBrains compatible
- **HTTP/SSE server** — `serve --http`

### Multi-provider abstraction (new)

Providers added beyond the upstream DeepSeek-only surface:
Anthropic, OpenAI, Groq, xAI, Ollama, generic OpenAI-compat.
Five-layer config cascade with per-provider API key support.

### Twelve extensions (new)

| # | Extension | Config flag |
|---|---|---|
| 3.1 | Checkpoint + rollback | `checkpoint_enabled` |
| 3.2 | Linter-on-edit (SWE-agent ACI) | `lint_on_edit` |
| 3.3 | Auto-test validation loop | `auto_test` |
| 3.4 | Hybrid retrieval (AST + PageRank + embeddings) | `retrieval_mode` |
| 3.5 | Hierarchical memory + Reflexion | `memory_enabled` |
| 3.6 | Auto model routing | `routing` |
| 3.7 | Plan + reflect dual blocks | `plan_blocks` |
| 3.8 | DARS-style branching + multi-verifier | `dars_branching` |
| 3.9 | Flash-tier seam/cycle compaction | `compaction_enabled` |
| 3.10 | Stateful REPL tools | `repl_tools` |
| 3.11 | Agentless hierarchical pipeline | `pipeline_enabled` |
| 3.12 | MCP tool integration | (`mcp_config_path`) |

### Evaluation harness (new)

- SWE-bench Verified subset evaluation with Docker and git-clone fallback
- Mutation-strengthened `semantic_pass@1` (SWE-ABS insight)
- RTV test-time scaling (`--scale N`)
- Run comparison (`--compare a.json b.json`)

### Known limitations in v0.1.0

- Flash compactor reuses the same `LlmClient` for all providers. On
  providers where small and reasoning models are on different endpoints,
  a second client is needed.
- REPL tools (`repl_tools = true`) are not yet routed through the
  execpolicy sandbox.
- The hierarchical pipeline runs the repair sub-agent once; the
  multi-attempt loop is wired but not exercised from the CLI.
- No per-workspace MCP overlay (only user-level `mcp_config_path`).
- SWE-bench eval requires Docker for full isolation; git-clone fallback
  is used when Docker is unavailable.

### Research citations

Extensions are grounded in published research:

- SWE-agent (Yang et al., NeurIPS 2024) — linter-on-edit ACI
- Agentless (Xia et al., 2024) — hierarchical pipeline
- AutoCodeRover (Zhang et al., 2024) — SBFL
- Reflexion (Shinn et al., NeurIPS 2023) — memory
- MAR (2025) — Reflexion failure-mode mitigation
- cAST (2025) — AST-aware chunking for code RAG
- DARS (Aggarwal et al., ACL 2025) — dynamic branching
- Scaling Test-Time Compute (Meta, 2026) — RTV
- Multi-Agent Verification (Lifshitz et al., 2025) — verifier voting
- EnIGMA (ICML 2025) — stateful REPL tools
- SWE-ABS (2026) — mutation-strengthened evaluation

Full citations in [docs/EXTENSIONS.md](docs/EXTENSIONS.md) and
[docs/EVAL.md](docs/EVAL.md).

### Semver commitment

Starting from v0.1.0:

- **Patch releases** (0.1.x) — bug fixes, no API changes
- **Minor releases** (0.x.0) — new features, backward-compatible config
  changes
- **Major releases** (x.0.0) — breaking changes to the `Op` / `Event`
  ABI or config schema

The `[extensions]` config block is additive: new keys are always opt-in
with safe defaults, so minor releases do not break existing configs.
