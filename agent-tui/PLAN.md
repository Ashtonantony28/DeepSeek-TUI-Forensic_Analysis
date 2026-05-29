# Project Plan — agent-tui

## Goal

Finish `agent-tui`, a multi-provider terminal coding agent written in Rust, modeled on
DeepSeek-TUI (https://github.com/Hmbown/DeepSeek-TUI) and extended with twelve research-backed
capabilities from 2024–2026 coding-agent literature. Phases 1–3 are complete on branch
`claude/agent-tui-phase-3-final-vE9RD`, including a follow-up FlashCompactor multi-endpoint
fix. Phases 4 (evaluation harness) and 5 (docs, CI, release packaging) remain. Ship v0.1.0
when all smoke tests pass.

## Repository & branch

- **Repo:** `https://github.com/Ashtonantony28/DeepSeek-TUI-Forensic_Analysis` —
  confirm with `git remote -v` on first cycle.
- **Working branch:** `claude/agent-tui-phase-3-final-vE9RD` — verify on first cycle with
  `git rev-parse --abbrev-ref HEAD`.
- **All work for Phases 4 and 5 lands on this branch** until tagging v0.1.0.

## Architecture & key decisions (locked in — do not re-derive)

| Concern | Choice |
|---|---|
| Language | Stable Rust ≥ 1.88, no nightly features |
| Async runtime | `tokio` with `full` features |
| TUI | `ratatui` 0.30 + `crossterm` |
| HTTP | `reqwest` with `rustls-tls`, `json`, `stream` |
| Parsing | `tree-sitter` + per-language grammars |
| Vector store | `sqlite-vec` via `rusqlite` |
| Embeddings | Ollama (`nomic-embed-text`) default; OpenAI embeddings fallback |
| Diff/patch | `similar` + `diffy` |
| Paths | `camino::Utf8PathBuf` (never `std::path::PathBuf` in cross-platform code) |
| Logging | `tracing` + `tracing-subscriber` |
| Errors | `thiserror` in libs, `anyhow` at binary boundary |
| CLI | `clap` derive |
| LSP | `lsp-types` + `tower-lsp` (client side) |
| MCP client | `rmcp` |
| ACP server | `agent-client-protocol` |
| Sandbox | `seccomp` (Linux), `sandbox-exec` (macOS), Job Objects (Windows) |

The project is a Cargo workspace under `agent-tui/` (or the repo root if Phase 2 placed it
there — verify on first cycle). Crates:

```
agent-tui-protocol, agent-tui-config, agent-tui-llm, agent-tui-execpolicy,
agent-tui-tools, agent-tui-context, agent-tui-retrieval, agent-tui-agent,
agent-tui-subagent, agent-tui-pipeline, agent-tui-mcp, agent-tui-acp,
agent-tui-tui, agent-tui-cli, agent-tui-eval  ← to be built in Phase 4
```

## What remains: Phases 4 and 5

### Phase 4 — Evaluation harness

Build the `agent-tui-eval` crate and wire `agent-tui eval` subcommand:

1. Pull configurable subset of SWE-bench Verified instances (default 50, `--subset N` or
   `--subset-ids id1,id2,...`) from HuggingFace via `hf-hub` or shell-out.
2. Isolate each instance: Docker container per instance using official SWE-bench instance
   images if Docker is available; fall back to clean `git clone` + host-side execution if not.
   Detect at startup; log which path is being used.
3. Run `agent-tui fix --yolo --headless --time-budget <T>` per instance. Capture: final
   patch, total turns, total tokens, total cost USD, wall-clock seconds.
4. Apply patch; run SWE-bench's official test command to get raw `pass@1`.
5. Mutation-strengthen: for each instance generate 5 mutants of the existing test file using
   tree-sitter AST rewrites (return-value flips, comparison-operator swaps, conditional
   negations, off-by-one offsets). Record `semantic_pass@1` as the fraction passing both the
   original AND a majority of mutant tests.
6. Output `eval-runs/<timestamp>.json` with `config`, `summary` (pass_at_1,
   semantic_pass_at_1, mean_cost_usd, mean_turns, mean_wallclock_s), `instances`.
7. Implement `agent-tui eval --compare run-a.json run-b.json` → markdown diff table.
8. Implement `--dry-run` mode: synthetic instances, no network calls, exercises every code
   path including mutation strengthening and comparison output. **Not a stub** — full pipeline.
9. Smoke tests: 5-instance `--scale 1 --dry-run`; 5-instance `--scale 4 --dry-run`;
   `--compare` between them. Report all three.
10. Cite arxiv 2603.00520 (SWE-ABS) in a comment at the top of the mutation-strengthening
    module explaining the rationale.

### Phase 5 — Docs, CI, release packaging

1. `AGENTS.md` — stable-Rust constraints, common pitfalls, PR shape expectations
2. `docs/ARCHITECTURE.md` — crate map, engine ABI with example Op/Event JSON, turn lifecycle,
   extension toggles
3. `docs/CONFIGURATION.md` — full TOML schema, env vars, five-layer cascade, all
   `[extensions]` keys
4. `docs/EXTENSIONS.md` — twelve extensions with arxiv citations, config flag, expected
   behaviour, end-to-end example, known limitations
5. `docs/EVAL.md` — harness usage, mutation-strengthening explanation, interpreting results,
   Docker/SWE-bench limitations
6. `docs/MCP.md` — adding MCP servers, `agent-tui mcp list/probe`
7. `docs/ACP.md` — using agent-tui as ACP agent in Zed, JetBrains, Neovim, Emacs
8. `docs/PROVIDERS.md` — Anthropic, OpenAI, DeepSeek, Groq, xAI, Ollama, OpenAI-compat
9. `README.md` — quickstart (cargo, npm, prebuilt), feature matrix, providers, ACP, eval
10. `CHANGELOG.md` — v0.1.0 notes; all 12 extensions; semver commitment
11. `.github/workflows/ci.yml` — matrix `{ os: [ubuntu, macos, windows], rust: [stable, 1.88] }`;
    `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
    `cargo build --release --workspace`, `cargo test --workspace`; cache registry + target
12. `.github/workflows/release.yml` — on tag, build prebuilts for `x86_64-unknown-linux-gnu`,
    `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`,
    `x86_64-pc-windows-msvc`; SHA-256 checksums; attach to GitHub release
13. `npm/agent-tui/package.json` — wrapper downloading the matching prebuilt on
    `npm install -g agent-tui`; mirror upstream's `npm/deepseek-tui` postinstall structure
14. Run final smoke test sequence (cargo build, fmt, clippy, test; `agent-tui login`,
    `doctor`, `models`; interactive task; `fix` on tiny repo; `eval --subset 5 --dry-run`)
15. Tag v0.1.0 and push **only after all smoke tests pass**

## Definition of done

The project is done when **all** of the following hold:

- `cargo build --release --workspace` succeeds with no warnings on stable Rust 1.88+
- `cargo fmt --check` passes
- `cargo clippy --all-targets -- -D warnings` passes
- `cargo test --workspace` passes (target: ≥150 tests; was 135 at end of Phase 3)
- `agent-tui eval --subset 5 --dry-run` completes and produces a valid results JSON
- `agent-tui eval --compare` produces a markdown diff table from two result files
- All 10 documentation files exist and reference real config flags / commands / behaviour
- Both GitHub Actions workflows exist and have run at least once successfully
- The npm wrapper installs and resolves to the correct prebuilt binary
- A `v0.1.0` tag exists on `origin` and the release workflow has produced binaries
- A final status report has been appended to STATUS.md confirming each item above

## Constraints & regulations (Governed profile — strictly enforced)

These are non-negotiable. Workers and the orchestrator must obey them. They are repeated in
CLAUDE.md so every spawned worker inherits them.

1. **Stable Rust only.** No `#![feature(...)]`, no `cargo +nightly`, no unstable libraries.
2. **No Node or Python runtime dependency** in `agent-tui` itself. Ollama via HTTP is fine;
   shelling out to Python is not.
3. **All file paths use `camino::Utf8PathBuf`** in cross-platform code paths. Never
   `std::path::PathBuf`.
4. **Secrets only in `~/.agent-tui/config.toml` with mode 0600 on Unix.** Never written
   elsewhere. Never logged. Never committed.
5. **Tool execution that touches filesystem or shell must go through
   `agent-tui-execpolicy`.** No direct `std::process::Command` in tools.
6. **Streaming responses display incrementally.** Never buffer a full response before
   rendering.
7. **Tree-sitter syntax check on every edit.** Reject malformed edits at the tool layer
   before they propagate.
8. **`--scale N > 1` is headless-only.** Reject with a clear error if passed in interactive
   mode.
9. **Do not start Phase 5 until Phase 4 completes and its smoke tests pass.** Hard sequence.
10. **Do not tag v0.1.0 until every item in Definition of Done is checked.**
11. **Never auto-run irreversible AND destructive actions** — force-pushes over shared
    history, drops of production data, rotation of live credentials, deletion of branches
    with unmerged work. Route to a human pause.
12. **All work lands on `claude/agent-tui-phase-3-final-vE9RD`** until tagging. No PRs to
    `main` until v0.1.0 release.
13. **Cite arxiv 2603.00520 (SWE-ABS) in the mutation-strengthening module header.**
14. **Mutation operators must use tree-sitter AST rewrites, not regex.**
15. **Compaction breaks prefix cache; only run it when SeamManager has no slot remaining.**

## Out of scope (v0.1.0)

- Performance benchmarking against Claude Code / Cursor / Aider (post-launch work)
- Custom model fine-tuning
- Web UI or VS Code extension
- Cloud-hosted sandbox / runtime
- Homebrew tap (defer to v0.2)
- Windows seccomp equivalent beyond Job Objects (documented limitation)
- New tree-sitter grammars beyond the eight initial languages
