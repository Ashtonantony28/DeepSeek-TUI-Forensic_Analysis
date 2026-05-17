# Providers

`agent-tui` supports seven LLM providers. Providers share the same
`LlmClient` trait; only the wire format and authentication differ.

---

## Quick reference

| Provider | `--provider` value | Auth env var | Default model |
|---|---|---|---|
| Anthropic | `anthropic` | `AGENT_TUI_API_KEY_ANTHROPIC` | `claude-opus-4-7` |
| OpenAI | `openai` | `AGENT_TUI_API_KEY_OPENAI` | `gpt-4o` |
| DeepSeek | `deepseek` | `AGENT_TUI_API_KEY_DEEPSEEK` | `deepseek-chat` |
| Groq | `groq` | `AGENT_TUI_API_KEY_GROQ` | `llama-3.1-70b-versatile` |
| xAI | `xai` | `AGENT_TUI_API_KEY_XAI` | `grok-2` |
| Ollama | `ollama` | — | `llama3.1` |
| OpenAI-compat | `openai_compat` | `AGENT_TUI_API_KEY_OPENAI_COMPAT` | `default` |

---

## Anthropic

**Endpoint:** `https://api.anthropic.com/v1/messages`

**Authentication:** API key in `Authorization: Bearer <key>` header
(set via `AGENT_TUI_API_KEY_ANTHROPIC` or `agent-tui login --provider anthropic`).

**Notable capabilities:**
- Extended thinking blocks (`Thinking` content blocks in the stream)
- Long context windows (up to 200K tokens depending on model)
- Prefix cache: 90% discount at 128-token granularity; `SeamManager` is
  tuned around this

**Known quirks:**
- The `claude-opus-4-7` and `claude-sonnet-4-6` models use a different
  API version header (`anthropic-version: 2023-06-01`). The adapter sets
  this automatically.
- Tool use requires `tools` array in the request even for a single tool.

**Example config:**

```toml
provider = "anthropic"

[providers.anthropic]
api_key = "sk-ant-..."
default_model = "claude-opus-4-7"
```

---

## OpenAI

**Endpoint:** `https://api.openai.com/v1/chat/completions`

**Authentication:** `Authorization: Bearer <key>`.

**Notable capabilities:**
- Structured outputs and tool calling via the Chat Completions API
- `gpt-4o` recommended for coding tasks

**Known quirks:**
- Streaming tool calls accumulate in `delta.tool_calls[].function.arguments`
  across multiple chunks. The adapter handles this transparently.

**Example config:**

```toml
provider = "openai"

[providers.openai]
api_key = "sk-..."
default_model = "gpt-4o"
```

---

## DeepSeek

**Endpoint:** `https://api.deepseek.com/beta/chat/completions`

**Authentication:** `Authorization: Bearer <key>`.

**Notable capabilities:**
- `deepseek-chat` and `deepseek-reasoner` (with thinking blocks)
- OpenAI-compatible wire format

**Known quirks:**
- The beta endpoint (`/beta/chat/completions`) is required for full tool
  use support. The stable endpoint has limited tool support.
- `deepseek-reasoner` emits `reasoning_content` blocks that are mapped to
  `ContentBlock::Thinking`.

**Example config:**

```toml
provider = "deepseek"

[providers.deepseek]
api_key = "sk-..."
default_model = "deepseek-chat"
```

---

## Groq

**Endpoint:** `https://api.groq.com/openai/v1/chat/completions`

**Authentication:** `Authorization: Bearer <key>`.

**Notable capabilities:**
- Very fast inference (hardware-accelerated via Groq silicon)
- Good for routing/small-tier calls (3.6 auto-routing)
- `llama-3.1-70b-versatile` is a strong default

**Known quirks:**
- Groq does not support streaming tool calls in all model variants.
  The adapter falls back to non-streaming tool result collection when
  a model reports `finish_reason: tool_calls` without streaming deltas.
- Rate limits are strict on the free tier.

**Example config:**

```toml
[providers.groq]
api_key = "gsk_..."
default_model = "llama-3.1-70b-versatile"
```

---

## xAI

**Endpoint:** `https://api.x.ai/v1/chat/completions`

**Authentication:** `Authorization: Bearer <key>`.

**Notable capabilities:**
- `grok-2` is competitive on coding tasks
- OpenAI-compatible wire format

**Example config:**

```toml
[providers.xai]
api_key = "xai-..."
default_model = "grok-2"
```

---

## Ollama

**Endpoint:** `http://localhost:11434/api/chat` (default)

**Authentication:** None required for local Ollama.

**Notable capabilities:**
- Fully local inference — no API key, no network required
- Doubles as the embedding server for Phase 3.4 semantic retrieval
  (`nomic-embed-text` model)
- Supports any model available in the Ollama model library

**Setup:**
1. Install Ollama: https://ollama.com
2. Pull a model: `ollama pull llama3.1`
3. Pull the embedding model: `ollama pull nomic-embed-text`
4. Start Ollama: `ollama serve` (or let it run as a system service)

**Example config:**

```toml
provider = "ollama"

[providers.ollama]
base_url = "http://localhost:11434"
default_model = "llama3.1"
```

**Known quirks:**
- Tool use support varies by model. `llama3.1` and `qwen2.5-coder` have
  reasonable tool support. Many models do not.
- `agent-tui doctor` detects if Ollama is reachable and prints a hint if not.
- When Ollama is not running, semantic retrieval falls back to ripgrep
  and a one-line warning is emitted.

---

## OpenAI-compatible (generic)

For any server that speaks the OpenAI Chat Completions API: local models
served by `llama.cpp`, `vllm`, `text-generation-inference`, LM Studio, etc.

**Authentication:** Optional — set `api_key` if the server requires it.

**Example config:**

```toml
provider = "openai_compat"

[providers.openai_compat]
base_url = "http://localhost:8080/v1"
default_model = "mistral-7b-instruct"
# api_key = "..." # only if the server requires it
```

**Known quirks:**
- Capability detection (`supports_thinking`, `supports_tools`) is disabled for
  OpenAI-compat servers because there is no standard way to query it.
  Tool use is attempted and falls back gracefully if the server rejects it.

---

## Switching providers at runtime

```bash
# For the current session
agent-tui --provider deepseek
agent-tui --provider openai --model gpt-4o

# Inside the TUI (if routing is off)
/provider anthropic
/model claude-haiku-4-5
```

Auto-routing (`routing = "auto"`) selects provider and model per turn, but
only within the single provider configured in `[providers]`. Cross-provider
routing is a planned future feature.
