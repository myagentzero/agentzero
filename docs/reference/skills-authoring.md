# Skills Authoring Guide

Skills are user-defined or community-built capabilities that extend the agent.
Each skill lives in its own directory under `~/.zeroclaw/workspace/skills/<name>/`
and is defined by either a `SKILL.toml` manifest or a `SKILL.md` markdown file.

## Skill Formats

### SKILL.toml (structured)

Use SKILL.toml when your skill defines callable tools with typed parameters.

```toml
[skill]
name = "my-skill"
description = "What this skill does"
version = "1.0.0"
author = "your-name"
tags = ["productivity", "automation"]

[[tools]]
name = "run_lint"
description = "Run the linter on a file"
kind = "shell"
command = "lint --file {{file}} --format {{format}}"

[tools.args]
file = "The file to lint"
format = "Output format (json|text)"
```

### SKILL.md (prompt-based)

Use SKILL.md when your skill is primarily natural-language instructions for the
agent. Optional YAML frontmatter provides metadata.

```markdown
---
name: code-review
description: Review pull requests for common issues
version: 1.0.0
author: your-name
tags:
  - code-quality
  - review
---

# Code Review Skill

When asked to review code, follow these steps:
1. Check for security vulnerabilities
2. Look for performance issues
3. Verify error handling
```

When both `SKILL.toml` and `SKILL.md` exist in the same directory, `SKILL.toml`
takes priority.

## Callable Skill Tools

Tools defined in the `[[tools]]` section of SKILL.toml are registered as
first-class tools in the agent's tool registry. The agent can call them directly
with structured parameters instead of composing shell commands manually.

Each tool is registered with the name `<skill_name>.<tool_name>` to avoid
collisions with built-in tools (e.g. `my-skill.run_lint`).

### Tool Kinds

#### `shell` / `script`

Executes a shell command via `sh -c` in a sandboxed environment.

```toml
[[tools]]
name = "build"
description = "Build the project with the given target"
kind = "shell"
command = "cargo build --target {{target}} --release"

[tools.args]
target = "Rust target triple (e.g. x86_64-unknown-linux-gnu)"
```

Runtime behavior:
- Runs with a sanitized environment (only `PATH`, `HOME`, `TERM`, `LANG`,
  `LC_ALL`, `USER`, `SHELL`, `TMPDIR` are inherited)
- 60-second execution timeout
- Output capped at 1 MB (truncated with a notice if exceeded)
- Subject to security policy: rate limiting, forbidden path checks, command
  validation

#### `http`

Makes an HTTP GET request via reqwest. No shell involved.

```toml
[[tools]]
name = "get_weather"
description = "Fetch current weather for a city"
kind = "http"
command = "https://api.example.com/weather?city={{city}}&units={{units}}"

[tools.args]
city = "City name to look up"
units = "Temperature units (metric|imperial)"
```

Runtime behavior:
- Only `http://` and `https://` URLs are allowed
- 30-second request timeout
- Response body capped at 1 MB
- No shell execution, no injection risk

### Argument Substitution

Use `{{arg_name}}` placeholders in the `command` field. Each key in `[tools.args]`
becomes a required string parameter in the tool's JSON schema. The agent sees the
parameter names and descriptions and provides values when calling the tool.

If a placeholder has no matching argument value at call time, it is left as-is in
the command string.

```toml
[[tools]]
name = "deploy"
description = "Deploy a service to an environment"
kind = "shell"
command = "deploy.sh --service {{service}} --env {{env}} --tag {{tag}}"

[tools.args]
service = "Service name to deploy"
env = "Target environment (staging|production)"
tag = "Docker image tag to deploy"
```

### Tools Without Arguments

Tools with no arguments omit the `[tools.args]` section entirely.

```toml
[[tools]]
name = "status"
description = "Check deployment status"
kind = "http"
command = "https://api.example.com/deployments/status"
```

## Skill Self-Improvement

When `skills.skill_improvement.enabled = true` (the default), ZeroClaw can
automatically refine skill files after successful usage. Improvements are
throttled by `skills.skill_improvement.cooldown_secs` (default: 3600, i.e. one
hour between improvements for the same skill).

```toml
# config.toml
[skills]
skill_improvement.enabled = true
skill_improvement.cooldown_secs = 3600
```

This requires the `skill-creation` compile-time feature.

## Pipeline Tool

The pipeline tool (`execute_pipeline`) chains multiple tool calls in sequence,
passing outputs between steps. Enable it in config:

```toml
# config.toml
[pipeline]
enabled = true
max_steps = 20
allowed_tools = ["shell", "file_read", "content_search"]
```

- `max_steps` limits the number of steps per pipeline invocation (default: 20)
- `allowed_tools` restricts which tools can appear in pipeline steps; an empty
  list allows all tools

## Skill Testing

Skills can include a `TEST.sh` file for automated validation. Each line defines
a test case in the format:

```
command | expected_exit_code | expected_output_pattern
```

Example `TEST.sh`:

```sh
# Verify the greeting command works
echo hello | 0 | hello

# Check that missing args fail
false | 1 |

# Regex pattern matching
echo "version 2.5.1" | 0 | version \d+\.\d+\.\d+
```

Lines starting with `#` are comments. Empty lines are skipped. The output
pattern can be a substring match or a regular expression.

## Security Audit

All skills are audited on load. The audit blocks:

- Symlinks inside the skill package
- Script files (`.sh`, `.bash`, `.zsh`, `.ps1`, `.bat`, `.cmd`) unless
  `skills.allow_scripts = true`
- High-risk command snippets (pipe-to-shell payloads)
- Markdown links that escape the skill root or target remote/script files

To allow script files in skills:

```toml
# config.toml
[skills]
allow_scripts = true
```

To manually audit a skill before installing:

```bash
zeroclaw skills audit <path_or_name>
```

## Prompt Injection Modes

Control how skills appear in the agent's system prompt:

```toml
# config.toml
[skills]
prompt_injection_mode = "compact"  # or "full"
```

- **`compact`** (default): Injects only skill names, descriptions, and
  locations. Full instructions are loaded on demand via the `read_skill` tool.
  Keeps context small.
- **`full`**: Injects complete skill instructions and tool metadata into the
  system prompt. Legacy behavior, available as opt-in.

## Complete Example

A skill that provides deployment tools with both shell and HTTP capabilities:

```
~/.zeroclaw/workspace/skills/deploy-tools/
  SKILL.toml
  TEST.sh
```

**SKILL.toml:**

```toml
[skill]
name = "deploy-tools"
description = "Deploy and monitor services"
version = "1.0.0"
author = "ops-team"
tags = ["devops", "deployment"]

[[tools]]
name = "deploy"
description = "Deploy a service to the specified environment"
kind = "shell"
command = "kubectl set image deployment/{{service}} app={{image}}:{{tag}} -n {{namespace}}"

[tools.args]
service = "Kubernetes deployment name"
image = "Container image repository"
tag = "Image tag to deploy"
namespace = "Kubernetes namespace (e.g. staging, production)"

[[tools]]
name = "rollback"
description = "Roll back a deployment to the previous revision"
kind = "shell"
command = "kubectl rollout undo deployment/{{service}} -n {{namespace}}"

[tools.args]
service = "Kubernetes deployment name"
namespace = "Kubernetes namespace"

[[tools]]
name = "health"
description = "Check service health endpoint"
kind = "http"
command = "https://{{host}}/healthz"

[tools.args]
host = "Service hostname (e.g. api.example.com)"
```

**TEST.sh:**

```sh
# Verify kubectl is available
which kubectl | 0 | kubectl

# Health check tool uses valid URL format
echo "https://localhost/healthz" | 0 | https://
```

The agent sees these as `deploy-tools.deploy`, `deploy-tools.rollback`, and
`deploy-tools.health` in its tool registry.
