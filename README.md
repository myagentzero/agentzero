<p align="center">
  <img src="docs/assets/banner.png" alt="ZeroClaw — Zero overhead. Zero compromise. 100% Rust. 100% Agnostic." width="800" />
</p>

# ZeroClaw 🦀

<p align="center">
  <strong>Zero overhead. Zero compromise. 100% Rust. 100% Agnostic.</strong><br>
  ⚡️ <strong>Runs on Raspberry Pi hardware with <20MB RAM: That's 99% less memory than OpenClaw and 98% cheaper than a Mac mini!</strong>
</p>

<p align="center">
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/rust-edition%202024-orange?logo=rust" alt="Rust Edition 2024" /></a>
</p>

ZeroClaw is an agent runtime — a single Rust binary you configure and run. It talks to LLM providers (Anthropic, OpenAI, Ollama, and ~20 others). It answers you on the channels you already use ( Slack, Discord, IRC, Email, and more). It has a web dashboard for real-time control and can connect to hardware peripherals (ESP32, STM32, Arduino, Raspberry Pi). The Gateway is the control plane — the product is the assistant.

If you want a personal, single-user assistant that feels local, fast, and always-on, this is it.

<p align="center">
  <a href="docs/README.md">Docs</a> ·
  <a href="docs/architecture.md">Architecture</a> ·
  <a href="#quick-start">Getting Started</a> ·
  <a href="#migrating-from-openclaw">Migrating from OpenClaw</a> ·
  <a href="docs/ops/troubleshooting.md">Troubleshoot</a> ·
</p>

The installer asks whether you want a prebuilt binary (fast, ~seconds) or a source build (slower, customisable). Both end the same way — `zeroclaw onboard` kicks off automatically.

Flags:

```
./install.sh --prebuilt              # always prebuilt; don't ask
./install.sh --source                # always build from source
./install.sh --minimal               # kernel only (~6.6 MB)
./install.sh --source --features agent-runtime,channel-discord  # custom feature set
./install.sh --skip-onboard          # install only, run `zeroclaw onboard` later
./install.sh --list-features         # print available feature flags
```

Platform-specific notes: [Linux](docs/book/src/setup/linux.md) · [macOS](docs/book/src/setup/macos.md) · [Windows](docs/book/src/setup/windows.md) · [Docker](docs/book/src/setup/container.md)

## Quick start

```bash
zeroclaw onboard                  # interactive onboard: provider, channels, agents, etc.
zeroclaw agent -a <alias>         # interactive chat using the [agents.<alias>] entry
zeroclaw service install          # register as systemd/launchctl/Windows Service
zeroclaw service start            # run it always-on in the background
```

Full walkthrough: [Quick start](docs/book/src/getting-started/quick-start.md) — or skip the safety gates with [YOLO mode](docs/book/src/getting-started/yolo.md) for dev boxes.

## What ZeroClaw does

- **Multi-channel** — one agent answering you across [every channel you configure](docs/book/src/channels/overview.md). Inbound messages from Discord, Telegram, Matrix, email, webhooks, CLI — all delivered to the same agent loop.
- **Provider-agnostic** — [model providers](docs/book/src/providers/overview.md) are pluggable. Configure Anthropic, OpenAI, local Ollama, or any OpenAI-compatible endpoint. [Fallback chains and routing](docs/book/src/providers/fallback-and-routing.md) keep the agent running when a provider flakes.
- **Security-first, with escape hatches** — default autonomy is `supervised`: medium-risk ops require approval, high-risk blocked. Workspace boundaries, command policy, OS-level sandboxes (Landlock / Bubblewrap / Seatbelt / Docker), and cryptographic [tool receipts](docs/book/src/security/tool-receipts.md) on every action. [YOLO mode](docs/book/src/getting-started/yolo.md) exists for trusted dev environments.
- **Hardware-capable** — GPIO / I2C / SPI / USB on Raspberry Pi, STM32, Arduino, and ESP32 via the `Peripheral` trait. See [Hardware](docs/book/src/hardware/index.md).
- **Gateway + dashboard** — HTTP / WebSocket gateway for clients, with a web dashboard for chat, memory browsing, config editing, cron management, and tool inspection.
- **SOP engine** — event-triggered [Standard Operating Procedures](docs/book/src/sop/index.md) (MQTT / webhook / cron / peripheral) with approval gates and resumable runs.
- **ACP** — IDE / editor integration via [Agent Client Protocol](docs/book/src/channels/acp.md) (JSON-RPC 2.0 over stdio).

## Configuration

One TOML file at `~/.zeroclaw/config.toml`. Pointers:

- [Provider configuration](docs/book/src/providers/configuration.md) — the universal `[providers.models.<type>.<alias>]` schema
- [Channels overview](docs/book/src/channels/overview.md) — per-channel `[channels.<type>.<alias>]` blocks
- [Security overview](docs/book/src/security/overview.md) — autonomy, sandboxing, tool receipts
- [Full config reference](docs/book/src/reference/config.md) — generated from the live schema; every key documented

A V3 config has at minimum four section headers (`<type>.<alias>` shaped) — a provider entry, an agent that references it, and a risk profile the agent gates against. See [Provider Configuration → Minimal working example](docs/book/src/providers/configuration.md#minimal-working-example) for the canonical four-section form with inline type/alias commentary.

For standard OpenAI Codex subscription auth, swap the provider entry to:

```toml
[providers.models.openai.coding]   # type = openai; alias = coding (you choose)
model = "gpt-5-codex"
wire_api = "responses"
requires_openai_auth = true
```

…and point your agent at it with `model_provider = "openai.coding"`.

Notes:

- Normal OpenAI Codex subscription auth uses stored auth profiles, not an `api_key` on the provider entry.
- Only set `api_key` / `uri` on `[providers.models.openai.<alias>]` when intentionally targeting a custom OpenAI-compatible gateway or endpoint.
- If you see `provider streaming failed, falling back to non-streaming chat`, ZeroClaw retries the same request in non-streaming mode. Check `zeroclaw auth status` before changing provider config.

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│            channels       gateway        ACP                 │
│          (30+ adapters)   (REST/WS)    (JSON-RPC)            │
│                        ↓                                     │
│                   ZeroClaw runtime                           │
│         ┌──────────┬──────────┬──────────┐                   │
│         │  agent   │ security │   SOP    │                   │
│         │   loop   │  policy  │  engine  │                   │
│         └──────────┴──────────┴──────────┘                   │
│              ↓          ↓           ↓                        │
│          providers    tools      memory                      │
│         (Anthropic,  (shell,    (SQLite,                     │
│          OpenAI,     browser,    embeddings)                 │
│          Ollama,     HTTP,                                   │
│          ~20 more)   hardware)                               │
└──────────────────────────────────────────────────────────────┘
```

Full detail with Mermaid diagrams: [Architecture overview](docs/book/src/architecture/overview.md) · [Request lifecycle](docs/book/src/architecture/request-lifecycle.md) · [Crates](docs/book/src/architecture/crates.md).


## Agent workspace + skills

Workspace root: `~/.zeroclaw/workspace/` (configurable via config).

Injected prompt files:
- `IDENTITY.md` — agent personality and role
- `USER.md` — user context and preferences
- `MEMORY.md` — long-term facts and lessons
- `AGENTS.md` — session conventions and initialization rules
- `SOUL.md` — core identity and operating principles

Skills: `~/.zeroclaw/workspace/skills/<skill>/SKILL.md` or `SKILL.toml`.


## License

ZeroClaw is dual-licensed for maximum openness and contributor protection:

| License | Use case |
|---|---|
| [MIT](LICENSE-MIT) | Open-source, research, academic, personal use |
| [Apache 2.0](LICENSE-APACHE) | Patent protection, institutional, commercial deployment |

You may choose either license. **Contributors automatically grant rights under both** — see [CLA.md](docs/contributing/cla.md) for the full contributor agreement.

### Trademark

The **ZeroClaw** name and logo are trademarks of ZeroClaw Labs. This license does not grant permission to use them to imply endorsement or affiliation. See [TRADEMARK.md](docs/maintainers/trademark.md) for permitted and prohibited uses.

### Contributor Protections

- You **retain copyright** of your contributions
- **Patent grant** (Apache 2.0) shields you from patent claims by other contributors
- Your contributions are **permanently attributed** in commit history and [NOTICE](NOTICE)
- No trademark rights are transferred by contributing
