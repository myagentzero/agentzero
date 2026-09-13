# MCP Server Registration

AgentZero supports the **Model Context Protocol (MCP)**, allowing you to extend the agent's capabilities with external tools and context providers. This guide explains how to register and configure MCP servers.

When `[mcp] enabled = true` and at least one server is configured, AgentZero connects those servers while building the tool registry. Discovered tools are registered as prefixed names (`<server>__<tool>`) and dispatched through the existing agent tool loop on:

- CLI / interactive agent loop
- `Agent::turn()` (`Agent::from_config`)
- Channel listeners (Telegram, Discord, Slack, and others)
- Gateway / webhook agent loop

## Overview

MCP servers can be connected via three transport types:
- **stdio**: Long-running local processes (e.g., Node.js or Python scripts).
- **sse**: Remote servers via Server-Sent Events.
- **http**: Simple HTTP POST-based servers.

## Configuration

MCP servers are configured in the `[mcp]` section of your `config.toml`.

```toml
[mcp]
enabled = true

[[mcp.servers]]
name = "my_local_tool"
transport = "stdio"
command = "node"
args = ["/path/to/server.js"]
env = { "API_KEY" = "secret_value" }

[[mcp.servers]]
name = "my_remote_tool"
transport = "sse"
url = "https://mcp.example.com/sse"
```

### Server Configuration Fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | String | **Required**. Display name used as a tool prefix (`name__tool_name`). |
| `transport` | String | `stdio`, `sse`, or `http`. Default: `stdio`. |
| `command` | String | (stdio only) Executable to run. |
| `args` | List | (stdio only) Command line arguments. |
| `env` | Map | (stdio only) Environment variables. |
| `url` | String | (sse/http only) Server endpoint URL. |
| `headers` | Map | (sse/http only) Custom HTTP headers (e.g., for auth). |
| `tool_timeout_secs` | Integer | Per-call timeout for tools from this server. |

Failed server connections are logged and skipped; remaining servers still register.

## Security and Auto-Approval

By default, any tool execution from an MCP server requires manual approval unless your autonomy level is set to `full`.

To automatically approve tools from a specific MCP server, add exact prefixed names or a single `*` wildcard to the `auto_approve` list in the `[autonomy]` section:

```toml
[autonomy]
auto_approve = [
  "my_local_tool__read_file", # Allow a specific tool from 'my_local_tool'
  "my_remote_tool__*",        # Allow every tool from 'my_remote_tool'
]
```

Primary-agent visibility uses the same wildcard rules in `[agent] allowed_tools` / `denied_tools` (for example `denied_tools = ["untrusted__*"]`).
