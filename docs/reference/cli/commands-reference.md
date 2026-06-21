# AgentZero Commands Reference

This reference is derived from the current CLI surface (`agentzero --help`).

Last verified: **March 26, 2026**.

## Top-Level Commands

| Command | Purpose |
|---|---|
| `onboard` | Initialize workspace/config quickly or interactively |
| `agent` | Run interactive chat or single-message mode |
| `gateway` | Start webhook and WhatsApp HTTP gateway |
| `acp` | Start ACP (Agent Control Protocol) server over stdio |
| `daemon` | Start supervised runtime (gateway + channels + optional heartbeat/scheduler) |
| `service` | Manage user-level OS service lifecycle |
| `doctor` | Run diagnostics and freshness checks |
| `status` | Print current configuration and system summary |
| `estop` | Engage/resume emergency stop levels and inspect estop state |
| `cron` | Manage scheduled tasks |
| `models` | Refresh provider model catalogs |
| `providers` | List provider IDs, aliases, and active provider |
| `channel` | Manage channels and channel health checks |
| `integrations` | View integration details and setup instructions |
| `skills` | List/install/remove skills |
| `config` | Manage configuration (view/set properties, export schema) |
| `completions` | Generate shell completion scripts to stdout |
| `hardware` | Discover and introspect USB hardware |
| `peripheral` | Configure and flash peripherals |

## Command Groups

### `onboard`

- `agentzero onboard`
- `agentzero onboard --channels-only`
- `agentzero onboard --force`
- `agentzero onboard --reinit`
- `agentzero onboard --api-key <KEY> --provider <ID> --memory <sqlite|lucid|markdown|none>`
- `agentzero onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none>`
- `agentzero onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none> --force`

`onboard` safety behavior:

- If `config.toml` already exists, onboarding offers two modes:
  - Full onboarding (overwrite `config.toml`)
  - Provider-only update (update provider/model/API key while preserving existing channels, tunnel, memory, hooks, and other settings)
- In non-interactive environments, existing `config.toml` causes a safe refusal unless `--force` is passed.
- Use `agentzero onboard --channels-only` when you only need to rotate channel tokens/allowlists.
- Use `agentzero onboard --reinit` to start fresh. This backs up your existing config directory with a timestamp suffix and creates a new configuration from scratch.

### `agent`

- `agentzero agent`
- `agentzero agent -m "Hello"`
- `agentzero agent --provider <ID> --model <MODEL> --temperature <0.0-2.0>`
- `agentzero agent --peripheral <board:path>`

Tip:

- To configure model routes and scenarios, edit the TOML configuration file under `[model_routes]` and `[query_classification]` sections.

### `acp`

- `agentzero acp`
- `agentzero acp --max-sessions <N>`
- `agentzero acp --session-timeout <SECONDS>`

Start the ACP (Agent Control Protocol) server for IDE and tool integration.

- Uses JSON-RPC 2.0 over stdin/stdout
- Supports methods: `initialize`, `session/new`, `session/prompt`, `session/stop`
- Streams agent reasoning, tool calls, and content in real-time as notifications
- Default max sessions: 10
- Default session timeout: 3600 seconds (1 hour)

### `gateway` / `daemon`

- `agentzero gateway [--host <HOST>] [--port <PORT>]`
- `agentzero daemon [--host <HOST>] [--port <PORT>]`

### `estop`

- `agentzero estop` (engage `kill-all`)
- `agentzero estop --level network-kill`
- `agentzero estop --level domain-block --domain "*.chase.com" [--domain "*.paypal.com"]`
- `agentzero estop --level tool-freeze --tool shell [--tool browser]`
- `agentzero estop status`
- `agentzero estop resume`
- `agentzero estop resume --network`
- `agentzero estop resume --domain "*.chase.com"`
- `agentzero estop resume --tool shell`
- `agentzero estop resume --otp <123456>`

Notes:

- `estop` commands require `[security.estop].enabled = true`.
- When `[security.estop].require_otp_to_resume = true`, `resume` requires OTP validation.
- OTP prompt appears automatically if `--otp` is omitted.

### `service`

- `agentzero service install`
- `agentzero service start`
- `agentzero service stop`
- `agentzero service restart`
- `agentzero service status`
- `agentzero service uninstall`

### `cron`

- `agentzero cron list`
- `agentzero cron add <expr> [--tz <IANA_TZ>] <command>`
- `agentzero cron add-at <rfc3339_timestamp> <command>`
- `agentzero cron add-every <every_ms> <command>`
- `agentzero cron once <delay> <command>`
- `agentzero cron remove <id>`
- `agentzero cron pause <id>`
- `agentzero cron resume <id>`

Notes:

- Mutating schedule/cron actions require `cron.enabled = true`.
- Shell command payloads for schedule creation (`create` / `add` / `once`) are validated by security command policy before job persistence.

### `models`

- `agentzero models refresh`
- `agentzero models refresh --provider <ID>`
- `agentzero models refresh --force`

`models refresh` currently supports live catalog refresh for provider IDs: `openrouter`, `openai`, `anthropic`, `groq`, `mistral`, `deepseek`, `xai`, `together-ai`, `gemini`, `ollama`, `llamacpp`, `sglang`, `vllm`, `astrai`, `venice`, `fireworks`, `cohere`, `moonshot`, `glm`, `zai`, `qwen`, and `nvidia`.

### `doctor`

- `agentzero doctor`
- `agentzero doctor models [--provider <ID>] [--use-cache]`
- `agentzero doctor traces [--limit <N>] [--event <TYPE>] [--contains <TEXT>]`
- `agentzero doctor traces --id <TRACE_ID>`

`doctor traces` reads runtime tool/model diagnostics from `observability.runtime_trace_path`.

### `channel`

- `agentzero channel list`
- `agentzero channel start`
- `agentzero channel doctor`
- `agentzero channel bind-telegram <IDENTITY>`
- `agentzero channel add <type> <json>`
- `agentzero channel remove <name>`

Runtime in-chat commands (Telegram/Discord while channel server is running):

- `/models`
- `/models <provider>`
- `/model`
- `/model <model-id>`
- `/new`

Channel runtime also watches `config.toml` and hot-applies updates to:
- `default_provider`
- `default_model`
- `default_temperature`
- `api_key` / `api_url` (for the default provider)
- `reliability.*` provider retry settings

`add/remove` currently route you back to managed setup/manual config paths (not full declarative mutators yet).

### `integrations`

- `agentzero integrations info <name>`

### `skills`

- `agentzero skills list`
- `agentzero skills audit <source_or_name>`
- `agentzero skills install <source>`
- `agentzero skills remove <name>`

`<source>` accepts git remotes (`https://...`, `http://...`, `ssh://...`, and `git@host:owner/repo.git`) or a local filesystem path.

`skills install` always runs a built-in static security audit before the skill is accepted. The audit blocks:
- symlinks inside the skill package
- script-like files (`.sh`, `.bash`, `.zsh`, `.ps1`, `.bat`, `.cmd`) unless `skills.allow_scripts = true`
- high-risk command snippets (for example pipe-to-shell payloads)
- markdown links that escape the skill root, point to remote markdown, or target script files

Use `skills audit` to manually validate a candidate skill directory (or an installed skill by name) before sharing it.

Skill manifests (`SKILL.toml`) support `prompts` and `[[tools]]`; both are injected into the agent system prompt at runtime, so the model can follow skill instructions without manually reading skill files.

Tools defined with `kind = "shell"` or `kind = "http"` in `[[tools]]` sections are registered as first-class callable tools in the agent's tool registry (prefixed as `skill_name.tool_name`). See [Skills Authoring Guide](../skills-authoring.md) for full format documentation, argument substitution, security model, self-improvement, pipeline tool, and TEST.sh validation.

### `config`

- `agentzero config schema`

`config schema` prints a JSON Schema (draft 2020-12) for the full `config.toml` contract to stdout.

### `completions`

- `agentzero completions bash`
- `agentzero completions fish`
- `agentzero completions zsh`
- `agentzero completions powershell`
- `agentzero completions elvish`

`completions` is stdout-only by design so scripts can be sourced directly without log/warning contamination.

### `hardware`

- `agentzero hardware discover`
- `agentzero hardware introspect <path>`
- `agentzero hardware info [--chip <chip_name>]`

### `peripheral`

- `agentzero peripheral list`
- `agentzero peripheral add <board> <path>`
- `agentzero peripheral flash [--port <serial_port>]`
- `agentzero peripheral setup-uno-q [--host <ip_or_host>]`
- `agentzero peripheral flash-nucleo`

## Validation Tip

To verify docs against your current binary quickly:

```bash
agentzero --help
agentzero <command> --help
```
