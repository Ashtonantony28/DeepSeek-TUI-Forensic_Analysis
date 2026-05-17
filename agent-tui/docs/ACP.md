# ACP integration

`agent-tui` acts as an ACP (Agent Client Protocol) **server**: editors that
speak ACP can use `agent-tui` as their external coding agent. ACP is a
JSON-RPC 2.0 protocol over stdio, analogous to LSP but for AI agents.

Start the server with:

```bash
agent-tui serve --acp
```

The process reads requests from stdin and writes responses to stdout, one
JSON object per line.

---

## Capabilities advertised

`agent-tui` advertises the following capabilities at `initialize`:

```json
{
  "capabilities": {
    "session": { "load": true, "new": true },
    "prompt": { "stream": true },
    "fs": { "read": true, "write": true },
    "image": true
  }
}
```

---

## Supported methods

| Method | Description |
|---|---|
| `initialize` | Negotiate capabilities |
| `session/new` | Create a new agent session |
| `session/load` | Resume a session by ID |
| `session/prompt` | Send a prompt; stream text and tool events back |
| `fs/read_text_file` | Read a file from the workspace |
| `fs/write_text_file` | Write a file to the workspace |

---

## Editor setup

### Zed

Add `agent-tui` as a custom agent in your Zed settings
(`~/.config/zed/settings.json`):

```json
{
  "agent_servers": [
    {
      "name": "agent-tui",
      "command": "agent-tui",
      "args": ["serve", "--acp"],
      "env": {}
    }
  ]
}
```

Restart Zed. The agent appears in the AI panel as "agent-tui". Open any
file, switch to the agent panel, and start a conversation. Zed streams
text and tool events in real time.

To use a specific provider:

```json
{
  "agent_servers": [
    {
      "name": "agent-tui (deepseek)",
      "command": "agent-tui",
      "args": ["serve", "--acp", "--provider", "deepseek"],
      "env": {
        "AGENT_TUI_API_KEY_DEEPSEEK": "sk-..."
      }
    }
  ]
}
```

### JetBrains (IntelliJ IDEA, PyCharm, GoLand, etc.)

Install the **AI Assistant** plugin (bundled in JetBrains IDEs 2024.2+).
Go to **Settings → Tools → AI Assistant → Custom AI Agent** and fill in:

| Field | Value |
|---|---|
| Name | agent-tui |
| Protocol | ACP |
| Command | `agent-tui serve --acp` |
| Working directory | `$PROJECT_DIR$` |

Apply and restart the IDE. The agent appears in the AI Assistant panel.

### Neovim

With `nvim-lspconfig` or a compatible plugin that supports ACP, add to
your `init.lua`:

```lua
-- Minimal ACP setup for Neovim
-- Requires a plugin that implements the ACP client side.
-- Example: https://github.com/some-user/nvim-acp (hypothetical)

require("nvim-acp").setup({
  servers = {
    {
      name = "agent-tui",
      cmd = { "agent-tui", "serve", "--acp" },
      root_dir = vim.fn.getcwd(),
    }
  }
})
```

Once a plugin with ACP client support is available, `agent-tui` will work
out of the box — it speaks standard ACP.

### Emacs

With `eglot` or `lsp-mode` extended to support ACP, add:

```elisp
;; Example using a hypothetical acp.el package
(require 'acp)
(acp-register-server
 '(agent-tui
   :command ("agent-tui" "serve" "--acp")
   :activation-fn acp-activate-if-project))
```

The ACP protocol is JSON-RPC over stdio, so any Emacs package that
implements an ACP client will work with `agent-tui serve --acp` as the
server command.

---

## Multiple agents side-by-side

You can run multiple `agent-tui` instances with different providers in the
same editor session. Each gets its own server entry with a unique name:

```json
{
  "agent_servers": [
    {
      "name": "agent-tui (anthropic)",
      "command": "agent-tui",
      "args": ["serve", "--acp", "--provider", "anthropic"]
    },
    {
      "name": "agent-tui (local ollama)",
      "command": "agent-tui",
      "args": ["serve", "--acp", "--provider", "ollama"]
    }
  ]
}
```

Switch between them in the AI panel.

---

## HTTP/SSE API

For integrations that prefer HTTP over stdio, use:

```bash
agent-tui serve --http --addr 127.0.0.1:7777
```

This exposes a minimal HTTP+SSE surface:

- `POST /v1/sessions` — create a session and stream `Event` objects as
  SSE frames. The request body may include `{ "prompt": "..." }`.
- Each SSE frame is `data: <event-json>\n\n`.
- The stream ends after `TurnComplete` or `Error`.

Example with `curl`:

```bash
curl -N -X POST http://127.0.0.1:7777/v1/sessions \
  -H "Content-Type: application/json" \
  -d '{"prompt": "summarise README.md"}'
```

---

## Troubleshooting

**Agent starts but returns no text**
Check that the API key is set: `agent-tui doctor`. The ACP server will log
errors to stderr which most editors surface in their output panel.

**Streaming is slow**
`agent-tui serve --acp` streams as fast as the provider allows. For local
models, use Ollama with a quantised model for faster throughput.

**Session state is lost between editor restarts**
Each `serve --acp` invocation starts a fresh in-memory session. Persistent
session state across editor restarts is not yet implemented.
