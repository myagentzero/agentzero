# Getting Started Docs

For first-time setup and quick orientation.

## Start Path

1. Main overview and quick start: [../../README.md](../../README.md)
2. One-click setup and dual bootstrap mode: [one-click-bootstrap.md](one-click-bootstrap.md)
3. Update or uninstall on macOS: [macos-update-uninstall.md](macos-update-uninstall.md)
4. Find commands by tasks: [../reference/cli/commands-reference.md](../reference/cli/commands-reference.md)

## Choose Your Path

| Scenario | Command |
|----------|---------|
| I have an API key, want fastest setup | `agentzero onboard --api-key sk-... --provider openrouter` |
| I want guided prompts | `agentzero onboard` |
| Config exists, just fix channels | `agentzero onboard --channels-only` |
| Config exists, I intentionally want full overwrite | `agentzero onboard --force` |
| Using subscription auth | See [Subscription Auth](../../README.md#subscription-auth-openai-codex--claude-code) |

## Onboarding and Validation

- Quick onboarding: `agentzero onboard --api-key "sk-..." --provider openrouter`
- Guided onboarding: `agentzero onboard`
- Existing config protection: reruns require explicit confirmation (or `--force` in non-interactive flows)
- Ollama cloud models (`:cloud`) require a remote `api_url` and API key (for example `api_url = "https://ollama.com"`).
- Validate environment: `agentzero status` + `agentzero doctor`

## Next

- Runtime operations: [../ops/README.md](../ops/README.md)
- Reference catalogs: [../reference/README.md](../reference/README.md)
- macOS lifecycle tasks: [macos-update-uninstall.md](macos-update-uninstall.md)
