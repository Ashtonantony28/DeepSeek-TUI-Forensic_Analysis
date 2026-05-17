# Configuration

`agent-tui` uses a five-layer TOML configuration cascade. Settings are
merged from lowest to highest precedence. A higher layer overrides a lower
one for each individual key; unknown keys are ignored.

---

## Config file locations

| Layer | Path | Precedence |
|---|---|---|
| 5 (lowest) | Built-in defaults | always applied |
| 4 | `~/.agent-tui/config.toml` | user-level settings |
| 3 | `.agent-tui/config.toml` (workspace root) | project-level settings |
| 2 | CLI flags (`--provider`, `--model`, `--yolo`) | per-invocation overrides |
| 1 (highest) | Environment variables | runtime overrides |

The user config is created by `agent-tui login`. On Unix its permissions are
set to `0600` automatically.

---

## Security: forbidden project keys

To prevent accidental credential leakage, the project overlay (layer 3) is
**not allowed** to set the following keys. The process exits with an error
if they appear:

```
api_key
base_url
provider
mcp_config_path
providers.*.api_key
providers.*.base_url
```

Use the user config or environment variables for credentials.

---

## Environment variables

| Variable | Overrides |
|---|---|
| `AGENT_TUI_API_KEY_ANTHROPIC` | `providers.anthropic.api_key` |
| `AGENT_TUI_API_KEY_OPENAI` | `providers.openai.api_key` |
| `AGENT_TUI_API_KEY_DEEPSEEK` | `providers.deepseek.api_key` |
| `AGENT_TUI_API_KEY_GROQ` | `providers.groq.api_key` |
| `AGENT_TUI_API_KEY_XAI` | `providers.xai.api_key` |
| `AGENT_TUI_API_KEY_OLLAMA` | `providers.ollama.api_key` |
| `AGENT_TUI_API_KEY_OPENAI_COMPAT` | `providers.openai_compat.api_key` |
| `AGENT_TUI_BASE_URL_<PROVIDER>` | `providers.<provider>.base_url` |
| `AGENT_TUI_PROFILE` | `profile` |
| `AGENT_TUI_MOCK` | Forces mock LLM client (for testing) |
| `AGENT_TUI_YOLO` | Forces YOLO mode (auto-approve all tools) |

Provider names in env vars are the uppercase form of the `Provider::as_str()`
value: `ANTHROPIC`, `OPENAI`, `DEEPSEEK`, `GROQ`, `XAI`, `OLLAMA`,
`OPENAI_COMPAT`.

---

## Full TOML schema

### Top-level keys

```toml
# Which provider to use when no explicit --provider flag is given.
# Values: "anthropic" | "openai" | "deepseek" | "groq" | "xai" | "ollama" | "openai_compat"
# Default: "anthropic"
provider = "anthropic"

# Which model to use. If absent, falls back to providers.<p>.default_model,
# then the built-in per-provider default.
model = "claude-opus-4-7"

# Path to the MCP server config file (see docs/MCP.md).
# Default: absent (no MCP servers loaded).
mcp_config_path = "~/.agent-tui/mcp.toml"

# Named profile (reserved; not yet used by the engine).
profile = "work"

# Auto-approve all tool calls (equivalent to --yolo flag).
# WARNING: tools run without confirmation.
yolo = false
```

### `[providers.<name>]`

`<name>` must be one of the provider strings above.

```toml
[providers.anthropic]
# API key. Prefer the env var AGENT_TUI_API_KEY_ANTHROPIC.
api_key = "sk-ant-..."

# Override the provider's base URL.
# Default: provider's own API endpoint.
base_url = "https://api.anthropic.com"

# Default model for this provider.
default_model = "claude-opus-4-7"

# Extra HTTP headers sent with every request to this provider.
[providers.anthropic.extra_headers]
"X-Custom" = "value"
```

### `[extensions]`

All keys are optional. Defaults are listed.

```toml
[extensions]
# 3.1 — Per-turn workspace snapshots and /rollback command.
checkpoint_enabled = true

# 3.2 — Tree-sitter syntax check on every edit.
# This is always enforced at the tool layer regardless of this flag.
# The flag exists only for future UI integration.
lint_on_edit = true

# 3.3 — Detect and run the project test suite after file mutations.
auto_test = true

# 3.4 — Retrieval mode.
# "ripgrep"  — grep-only, no dependencies
# "graph"    — repo dependency graph (PageRank), no Ollama required
# "semantic" — tree-sitter + Ollama embeddings only
# "hybrid"   — all three fused with RRF (recommended)
retrieval_mode = "hybrid"

# 3.5 — Cross-session lesson memory.
# Lessons stored at ~/.agent-tui/lessons.json.
# Injected into the system prompt on each turn.
memory_enabled = true

# 3.6 — Auto model routing.
# "auto"  — cheap pre-turn call classifies complexity and picks model
# "off"   — use the configured model for every turn
routing = "auto"

# 3.7 — Structured <plan> and <reflect> blocks + side panel.
plan_blocks = true

# 3.8 — DARS parallel branching at high-uncertainty decision points.
dars_branching = true

# 3.8 — Number of verifier sub-agents in the DARS vote.
verifier_count = 3

# 3.9 — Flash-tier summarization for seam/cycle archived blocks.
# When false, archives emit "[seam summary disabled]" instead of a real summary.
# Useful for fully-offline testing with a mock client.
compaction_enabled = true

# 3.10 — Stateful /bin/sh REPL tools (repl_open, repl_eval, repl_close).
# Default off because REPL sessions inherit the process environment and
# are not yet routed through execpolicy.
repl_tools = false

# 3.11 — Agentless hierarchical pipeline for `agent-tui fix`.
# Default off until the pipeline is fully validated.
pipeline_enabled = false
```

---

## Examples

### Minimal user config (Anthropic only)

```toml
[providers.anthropic]
api_key = "sk-ant-..."
```

Set via `agent-tui login --provider anthropic` which writes this
automatically.

### Switch default provider to DeepSeek

```toml
provider = "deepseek"

[providers.deepseek]
api_key = "sk-..."
default_model = "deepseek-chat"
```

### Use a local Ollama server

```toml
provider = "ollama"

[providers.ollama]
base_url = "http://localhost:11434"
default_model = "llama3.1"
```

No `api_key` needed for Ollama.

### Disable non-essential extensions for offline testing

```toml
[extensions]
retrieval_mode = "ripgrep"
memory_enabled = false
routing = "off"
dars_branching = false
compaction_enabled = false
```

### Project overlay (`.agent-tui/config.toml` in repo root)

```toml
# project overlays may NOT set api_key, base_url, provider, mcp_config_path

model = "claude-haiku-4-5"

[extensions]
auto_test = true
lint_on_edit = true
```

### Run with OpenAI-compatible local server

```toml
provider = "openai_compat"

[providers.openai_compat]
base_url = "http://localhost:8080/v1"
default_model = "mistral-7b"
```

---

## CLI flags

CLI flags override layer 3 and below but can be overridden by env vars.

| Flag | Equivalent config key |
|---|---|
| `--provider <p>` | `provider` |
| `--model <m>` | `model` |
| `--yolo` | `yolo = true` |

Global flags work on all subcommands (`agent-tui --provider openai models`).
