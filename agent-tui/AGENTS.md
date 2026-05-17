# AGENTS.md — Contributing to agent-tui

This file is read by AI coding agents (Claude Code, agent-tui itself, etc.) before
making any change. Human contributors should read it too — it captures the
non-obvious rules that keep the codebase buildable on stable Rust with zero
external runtime dependencies.

---

## Hard rules

### Stable Rust only

The workspace specifies `rust-version = "1.88"` in `Cargo.toml`. **No
`#![feature(...)]`, no `cargo +nightly`, no unstable flags, no unstable
crates.** If a desired pattern requires a nightly feature, find the stable
equivalent or mark the item with a `// TODO(stabilisation)` comment.

The one permitted exception is `let_chains`, which stabilised in Rust 1.88.
You may use chained `let` in `if` guards (`if let Some(a) = x && let Ok(b) = f(a)`).

### No Python or Node.js runtime dependency

The `agent-tui` binary must run with no external interpreter on the `PATH`.
Ollama (a separate process the user runs) is acceptable for the embedding
pipeline, but you must not `std::process::Command::new("python")` or
`std::process::Command::new("node")` anywhere in the codebase.

### All cross-platform paths use `camino::Utf8PathBuf`

Never use `std::path::PathBuf` in code that crosses a crate boundary or
touches the filesystem API. `camino::Utf8PathBuf` gives cross-platform UTF-8
paths without `OsString` pain. Convert with `Utf8PathBuf::from_path_buf()`
at the OS boundary and propagate the `Utf8Path` type inward.

### Secrets never written to disk except `~/.agent-tui/config.toml`

API keys stay in the user config file, which `agent-tui-config::save_user()`
writes with mode `0600` on Unix. Do not write keys to project overlays,
environment files, or temp files. The project overlay (`FORBIDDEN_PROJECT_KEYS`
in `crates/config/src/lib.rs`) explicitly rejects `api_key`, `base_url`,
`provider`, and `mcp_config_path` at load time.

### All tool execution goes through `agent-tui-execpolicy`

`std::process::Command` is forbidden inside tool implementations. Use
`agent_tui_execpolicy::ExecPolicyEngine::check()` → `SandboxedCommand` for
anything that forks a subprocess. This enforces the approval gate and egress
policy.

### Streaming must be incremental

`LlmClient::stream()` returns a `ChatStream`. Render `Delta` events as they
arrive. Never buffer the complete response before writing to the TUI or
stdout. The `cmd_oneshot` function in `crates/cli/src/main.rs` is the
reference.

### Tree-sitter syntax check on every edit

Every `write_file`, `edit_file`, and `apply_patch` tool invocation runs
`agent_tui_tools::syntax::check_syntax()` on the result before returning
success. If the language is recognised and the parse fails, return
`ToolError::SyntaxError { position, message }` so the model retries.
Do not bypass this check.

---

## Common pitfalls and how to fix them

### `if let` guard in a `match` arm

**Wrong (stable Rust does not permit this):**

```rust
match x {
    Some(v) if let Ok(n) = v.parse::<i64>() => use(n),
    _ => {}
}
```

**Right — move the inner `let` into the arm body:**

```rust
match x {
    Some(v) => {
        if let Ok(n) = v.parse::<i64>() {
            use(n);
        }
    }
    _ => {}
}
```

`let_chains` (stable since 1.88) work in `if` expressions but not inside
`match` guards. Keep complex guard logic in the arm body.

### `async fn` in a trait without `async_trait`

Add `#[async_trait]` from the `async-trait` crate to both the trait
definition and the implementation. Every trait in this project that has `async
fn` already uses it; do not remove it.

### `thiserror` vs `anyhow`

- Library crates (`protocol`, `config`, `llm`, `tools`, …): define typed
  errors with `#[derive(thiserror::Error)]`.
- Binary entrypoint (`cli/src/main.rs`) and test harnesses: use `anyhow::Result`.

Do not introduce `anyhow` into library crate public APIs.

### `Utf8PathBuf::from_path_buf` can fail

The `from_path_buf` conversion fails on non-UTF-8 paths. At the boundary
(e.g. reading `std::env::current_dir()`), propagate the error — do not
`.unwrap()`.

### MockClient in tests

Use `agent_tui_llm::MockClient` for any test that needs an `LlmClient`.
Push scripted responses with `mock.push_text(...)` or
`mock.push_tool_call(name, args)` before running the engine. Tests that
need a real network call should be gated behind `#[ignore]` and documented.

### Tree-sitter grammar versions

Grammar versions are pinned in the workspace `Cargo.toml`. Do not upgrade
them individually — they must all be updated together to avoid ABI
mismatches. Adding a new language grammar is a deliberate change, not
automatic.

---

## PR shape expectations

1. **One logical change per PR.** A refactor and a feature go in separate
   PRs.
2. **All tests pass.** `cargo test --workspace` must succeed. If a test is
   flaky, fix the flakiness or gate behind `#[ignore]`.
3. **`cargo fmt --check` passes.** Run `cargo fmt` before pushing.
4. **`cargo clippy --all-targets -- -D warnings` passes.** Fix or explicitly
   allow (`#[allow(...)]` with a comment) every warning.
5. **New public API gets doc comments.** One-line `///` is enough. Do not
   add multi-paragraph docstrings; the code should be readable without them.
6. **Extension toggles are respected.** Every Phase 3 feature is guarded by
   its `Extensions` flag. New behaviours must respect `cfg.extensions.*`
   and not activate unconditionally.
7. **No secrets in commits.** The CI will reject commits that contain
   API-key-shaped strings.

Estimated review time: **1–2 days** for changes within a single crate; **3–5
days** for cross-crate changes or changes to `agent-tui-agent`'s turn loop.

---

## Adding a new provider

1. Add a variant to `Provider` in `crates/protocol/src/lib.rs`.
2. Add the `as_str()` mapping.
3. Create `crates/llm/src/adapters/<name>.rs` implementing `LlmClient`.
4. Re-export from `crates/llm/src/adapters/mod.rs` and from `crates/llm/src/lib.rs`.
5. Add the env-var loop entry in `crates/config/src/lib.rs::apply_env()`.
6. Add a `parse_provider()` arm in `crates/cli/src/main.rs`.
7. Add a `resolve_model()` default arm in `crates/cli/src/main.rs`.
8. Add a `build_client()` arm in `crates/cli/src/lib.rs`.
9. Add unit tests covering round-trip serialisation and `list_models()` with a
   mock HTTP server.

---

## Adding a new built-in tool

1. Create `crates/tools/src/tools/<name>.rs` implementing the `Tool` trait.
2. Register it in `ToolRegistry::with_builtins()` in `crates/tools/src/registry.rs`.
3. If the tool modifies files, call `syntax::check_syntax()` before returning
   success.
4. If the tool shells out, route through `execpolicy`.
5. Add at least two unit tests: one happy path and one error path.

---

## Crate dependency rules

The workspace dependency graph must remain a DAG. The allowed dependency
edges are fixed; do not add new cross-crate dependencies without updating
`crate_map.dot`. The build order is:

```
protocol → config → llm → execpolicy → tools → context → retrieval
         → agent → subagent → pipeline → mcp → acp → tui → cli → eval
```

`eval` may depend on `llm`, `agent`, `protocol`, and `pipeline` but must not
be depended on by any other crate.
