# agent-tui

A multi-provider terminal coding agent. Keyboard-driven TUI, streaming
responses, structured context management, and twelve research-backed
extensions — built in stable Rust with no external runtime dependencies.

Modelled on [DeepSeek-TUI](https://github.com/Hmbown/DeepSeek-TUI) and
extended with state-of-the-art techniques from 2024–2026.

---

## Install

### Cargo

```bash
cargo install --git https://github.com/ashtonantony28/deepseek-tui-enhanced agent-tui-cli
```

### npm (downloads prebuilt binary)

```bash
npm install -g agent-tui
```

### Prebuilt binary

Download from [GitHub Releases](https://github.com/ashtonantony28/deepseek-tui-enhanced/releases/latest):

| Platform | Binary |
|---|---|
| Linux x86_64 | `agent-tui-x86_64-unknown-linux-gnu.tar.gz` |
| Linux aarch64 | `agent-tui-aarch64-unknown-linux-gnu.tar.gz` |
| macOS x86_64 | `agent-tui-x86_64-apple-darwin.tar.gz` |
| macOS arm64 | `agent-tui-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `agent-tui-x86_64-pc-windows-msvc.zip` |

Each archive includes a SHA-256 checksum file.

---

## Quick start

```bash
# Save your API key
agent-tui login --provider anthropic

# Check the setup
agent-tui doctor

# Launch the interactive TUI
agent-tui

# One-shot prompt
agent-tui -p "Explain this codebase"

# Fix a GitHub issue (hierarchical pipeline)
agent-tui fix issue.md

# Run in YOLO mode (auto-approve all tools)
agent-tui --yolo
```

---

## Feature matrix

### Replicated from DeepSeek-TUI

| Feature | Status |
|---|---|
| Streaming engine (`Op` / `Event` ABI) | ✓ |
| Three-tier context management (Seam / Cycle / Compaction) | ✓ |
| CapacityController (RiskBand, VerifyAndReplan) | ✓ |
| Tool registry with multi-stage input recovery | ✓ |
| Tree-sitter syntax check on every edit | ✓ |
| Plan mode (read-only tool catalog) | ✓ |
| MCP client integration | ✓ |
| LSP post-edit diagnostic injection | ✓ |
| Sub-agent sessions | ✓ |
| Execpolicy approval gate | ✓ |
| ratatui TUI with transcript, status bar, command palette | ✓ |
| ACP server (`serve --acp`) | ✓ |
| HTTP/SSE server (`serve --http`) | ✓ |

### New in agent-tui

| Extension | Phase | Config flag | Default |
|---|---|---|---|
| Checkpoint + rollback | 3.1 | `checkpoint_enabled` | on |
| Linter-on-edit (ACI) | 3.2 | `lint_on_edit` | on |
| Auto-test validation loop | 3.3 | `auto_test` | on |
| Hybrid retrieval (AST + PageRank + embeddings) | 3.4 | `retrieval_mode` | `"hybrid"` |
| Hierarchical memory + Reflexion | 3.5 | `memory_enabled` | on |
| Auto model routing | 3.6 | `routing` | `"auto"` |
| Plan + reflect dual blocks | 3.7 | `plan_blocks` | on |
| DARS-style branching | 3.8 | `dars_branching` | on |
| Flash-tier compaction | 3.9 | `compaction_enabled` | on |
| Stateful REPL tools | 3.10 | `repl_tools` | off |
| Agentless hierarchical pipeline | 3.11 | `pipeline_enabled` | off |
| MCP tool integration | 3.12 | (set `mcp_config_path`) | — |

### Evaluation harness

| Capability | Status |
|---|---|
| SWE-bench Verified subset evaluation | ✓ |
| Mutation-strengthened `semantic_pass@1` | ✓ |
| RTV test-time scaling (`--scale N`) | ✓ |
| Run comparison (`--compare a.json b.json`) | ✓ |

---

## Supported providers

| Provider | Key doc | Notes |
|---|---|---|
| Anthropic | [PROVIDERS.md#anthropic](docs/PROVIDERS.md) | Extended thinking, long context |
| OpenAI | [PROVIDERS.md#openai](docs/PROVIDERS.md) | GPT-4o, structured outputs |
| DeepSeek | [PROVIDERS.md#deepseek](docs/PROVIDERS.md) | Reasoning model, low cost |
| Groq | [PROVIDERS.md#groq](docs/PROVIDERS.md) | Very fast inference |
| xAI | [PROVIDERS.md#xai](docs/PROVIDERS.md) | Grok-2 |
| Ollama | [PROVIDERS.md#ollama](docs/PROVIDERS.md) | Local inference, also used for embeddings |
| OpenAI-compat | [PROVIDERS.md#openai-compatible-generic](docs/PROVIDERS.md) | llama.cpp, vllm, LM Studio |

---

## Documentation

- [ARCHITECTURE.md](docs/ARCHITECTURE.md) — crate map, engine ABI, turn lifecycle
- [CONFIGURATION.md](docs/CONFIGURATION.md) — full TOML schema, env vars, five-layer cascade
- [EXTENSIONS.md](docs/EXTENSIONS.md) — all 12 extensions with research citations
- [EVAL.md](docs/EVAL.md) — evaluation harness, mutation strengthening, comparing runs
- [MCP.md](docs/MCP.md) — adding MCP servers
- [ACP.md](docs/ACP.md) — editor integration (Zed, JetBrains, Neovim, Emacs)
- [PROVIDERS.md](docs/PROVIDERS.md) — supported providers, authentication, quirks
- [AGENTS.md](AGENTS.md) — contributing, stable Rust rules, pitfalls
- [CHANGELOG.md](CHANGELOG.md) — release history

---

## Editor integration (ACP)

Use `agent-tui` as an external coding agent in your editor. See
[docs/ACP.md](docs/ACP.md) for setup guides for Zed, JetBrains, Neovim,
and Emacs.

**Zed quick setup:**

```json
{
  "agent_servers": [
    { "name": "agent-tui", "command": "agent-tui", "args": ["serve", "--acp"] }
  ]
}
```

---

## License

MIT. See [LICENSE](LICENSE).
