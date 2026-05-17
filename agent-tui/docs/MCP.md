# MCP integration

`agent-tui` acts as an MCP **client**: it spawns external MCP servers as
child processes and makes their tools available to the agent alongside the
built-in tools. Tool names are qualified as `server:tool` to avoid
collisions.

---

## Config file

MCP servers are configured in a separate TOML file. Point to it from your
user config:

```toml
# ~/.agent-tui/config.toml
mcp_config_path = "~/.agent-tui/mcp.toml"
```

The `mcp.toml` file contains one `[[servers]]` section per MCP server:

```toml
[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
enabled = true

[[servers]]
name = "brave-search"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-brave-search"]
enabled = true
[servers.env]
BRAVE_API_KEY = "your-key-here"

[[servers]]
name = "sqlite"
command = "/usr/local/bin/mcp-server-sqlite"
args = ["--db-path", "/home/user/data.db"]
enabled = true
```

Fields per server:

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Unique server identifier. Used as the qualifier in `server:tool`. |
| `command` | string | yes | Executable to spawn. |
| `args` | string array | no | Arguments passed to the command. |
| `env` | table | no | Extra environment variables for the child process. |
| `enabled` | bool | no | Default `true`. Set to `false` to disable without removing the entry. |

---

## Listing available tools

```bash
agent-tui mcp list
```

Loads `mcp_config_path` from the merged config, spawns all enabled servers,
and prints their tools:

```
filesystem:read_file — Read the contents of a file from the filesystem
filesystem:write_file — Write content to a file
brave-search:web_search — Performs a web search using the Brave Search API
sqlite:query — Execute a SQL query against a SQLite database
```

Servers that fail to spawn print a warning and are skipped. The rest of
the tools are still registered.

---

## Probing a server ad-hoc

```bash
agent-tui mcp probe --command npx --args -y @modelcontextprotocol/server-filesystem /tmp
```

Spawns a single server without a config file and prints its tools. Useful
for testing a new server before adding it to `mcp.toml`.

```bash
# Custom server name in the output
agent-tui mcp probe --command ./my-server --name my-server
```

---

## How tools reach the agent

At startup (interactive TUI, oneshot `-p`, and `serve --acp`), `agent-tui`
calls `McpManager::spawn_and_register_all()`, then
`ToolRegistry::register_mcp_tools()`. Every tool the manager reports is
wrapped in a `McpToolAdapter` and inserted into the registry.

The model then has access to `server:tool` names in its tool catalog,
exactly like built-in tools. The input schema advertised to the model is
the schema the MCP server reports.

Failed tool calls return `is_error: true` in `Event::ToolCallFinished` —
the same as a built-in tool error — so the model can retry.

---

## Security considerations

- MCP server processes run as the same user as `agent-tui`.
- `env` entries in `mcp.toml` are passed directly to the child process;
  do not put secrets there if `mcp.toml` is shared or committed to a repo.
- The `execpolicy` layer is not applied to MCP tool calls — MCP servers
  handle their own sandboxing. Treat an MCP server like any trusted
  subprocess.
- The `mcp_config_path` key is forbidden in project overlays
  (`.agent-tui/config.toml`). Only the user config may set it, preventing
  a project from injecting arbitrary servers.

---

## Multiple servers side-by-side

You can run as many servers as needed. If two servers expose a tool with the
same name, they are distinguished by the qualifier:

```
serverA:read_file
serverB:read_file
```

The model sees both names and can call either.

---

## Troubleshooting

**"no MCP servers configured"**
Set `mcp_config_path` in `~/.agent-tui/config.toml` and create the file.

**Server fails to spawn**
Run `agent-tui mcp probe --command <cmd>` directly to see the raw error.
Common causes: command not on `PATH`, wrong arguments, missing env vars.

**Server times out during initialization**
The JSON-RPC `initialize` handshake has a 10 s timeout. Slow servers (e.g.
`npx` downloading packages on first run) may fail on the first invocation
and succeed on subsequent ones once the npm package is cached.

**Tools not appearing in the agent**
Check `agent-tui mcp list` — if tools appear there but not in the agent
session, the `mcp_config_path` may not be picked up from the correct config
layer. Run `agent-tui doctor` to confirm which config file is active.
