# Channels Reference

This document is the canonical reference for channel configuration in ZeroClaw.

For encrypted Matrix rooms, also read the dedicated runbook:
- [Matrix E2EE Guide](../../security/matrix-e2ee-guide.md)

## Quick Paths

- Need a full config reference by channel: jump to [Per-Channel Config Examples](#4-per-channel-config-examples).
- Need a no-response diagnosis flow: jump to [Troubleshooting Checklist](#6-troubleshooting-checklist).
- Need deployment/network assumptions (polling vs webhook): use [Network Deployment](../../ops/network-deployment.md).

## FAQ: Matrix setup passes but no reply

This is the most common symptom (same class as issue #499). Check these in order:

1. **Allowlist mismatch**: `allowed_users` does not include the sender (or is empty).
2. **Wrong room target**: bot is not joined to the configured `room_id` / alias target room.
3. **Token/account mismatch**: token is valid but belongs to another Matrix account.
4. **E2EE device identity gap**: `whoami` does not return `device_id` and config does not provide one.
5. **Key sharing/trust gap**: room keys were not shared to the bot device, so encrypted events cannot be decrypted.
6. **Stale runtime state**: config changed but `zeroclaw daemon` was not restarted.

---

## 1. Configuration Namespace

All channel settings live under `channels_config` in `~/.zeroclaw/config.toml`.

```toml
[channels_config]
cli = true
```

Each channel is enabled by creating its sub-table (for example, `[channels_config.telegram]`).

## In-Chat Runtime Model Switching (Telegram / Discord)

When running `zeroclaw channel start` (or daemon mode), Telegram and Discord now support sender-scoped runtime switching:

- `/models` — show available providers and current selection
- `/models <provider>` — switch provider for the current sender session
- `/model` — show current model and cached model IDs (if available)
- `/model <model-id>` — switch model for the current sender session
- `/new` — clear conversation history and start a fresh session

Notes:

- Switching provider or model clears only that sender's in-memory conversation history to avoid cross-model context contamination.
- `/new` clears the sender's conversation history without changing provider or model selection.
- Model cache previews come from `zeroclaw models refresh --provider <ID>`.
- These are runtime chat commands, not CLI subcommands.

## Inbound Image Marker Protocol

ZeroClaw supports multimodal input through inline message markers:

- Syntax: ``[IMAGE:<source>]``
- `<source>` can be:
  - Local file path
  - Data URI (`data:image/...;base64,...`)
  - Remote URL only when `[multimodal].allow_remote_fetch = true`

Operational notes:

- Marker parsing applies to user-role messages before provider calls.
- Provider capability is enforced at runtime: if the selected provider does not support vision, the request fails with a structured capability error (`capability=vision`).
- Linq webhook `media` parts with `image/*` MIME type are automatically converted to this marker format.


## 2. Delivery Modes at a Glance

| Channel | Receive mode | Public inbound port required? |
|---|---|---|
| CLI | local stdin/stdout | No |
| Discord | gateway/websocket | No |
| Slack | events API | No (token-based channel flow) |
| Webhook | gateway endpoint (`/webhook`) | Usually yes |
| Email | IMAP polling + SMTP send | No |
| IRC | IRC socket | No |


## 3. Allowlist Semantics

For channels with inbound sender allowlists:

- Empty allowlist: deny all inbound messages.
- `"*"`: allow all inbound senders (use for temporary verification only).
- Explicit list: allow only listed senders.

Field names differ by channel:

- `allowed_users` (Discord/Slack)
- `allowed_from` (Signal)
- `allowed_numbers` (WhatsApp)
- `allowed_senders` (Email/Linq)
- `allowed_contacts` (iMessage)
- `allowed_pubkeys` (Nostr)

---

## 4. Per-Channel Config Examples

### 4.1 Discord

```toml
[channels_config.discord]
bot_token = "discord-bot-token"
guild_id = "123456789012345678"   # optional
allowed_users = ["*"]
listen_to_bots = false
mention_only = false
stream_mode = "multi_message"     # optional: off | partial | multi_message (default: multi_message via wizard)
draft_update_interval_ms = 1000   # optional: edit throttle for partial streaming
multi_message_delay_ms = 800      # optional: delay between paragraph sends in multi_message mode
```

Discord notes:

- `stream_mode = "partial"` sends an editable draft message that updates token-by-token as the LLM streams its response, then finalizes with the complete text.
- `stream_mode = "multi_message"` delivers the response incrementally as separate messages, splitting at paragraph boundaries (`\n\n`) as tokens arrive from the provider. Each paragraph appears in Discord as soon as it completes.
- `draft_update_interval_ms` controls edit throttling in partial mode (default: 1000ms).
- `multi_message_delay_ms` controls minimum delay between paragraph sends in multi_message mode to avoid Discord rate limits (default: 800ms).
- Code fences are never split across messages in multi_message mode.

### 4.2 Slack

```toml
[channels_config.slack]
bot_token = "xoxb-..."
app_token = "xapp-..."             # optional
channel_id = "C1234567890"         # optional: single channel; omit or "*" for all accessible channels
channel_ids = ["C1234567890"]      # optional: explicit channel list; takes precedence over channel_id
allowed_users = ["*"]
```

Slack listen behavior:

- `channel_ids = ["C123...", "D456..."]`: listen only on the listed channels/DMs.
- `channel_id = "C123..."`: listen only on that channel.
- `channel_id = "*"` or omitted: auto-discover and listen across all accessible channels.

### 4.3 Webhook Channel Config (Gateway)

`channels_config.webhook` enables webhook-specific gateway behavior.

```toml
[channels_config.webhook]
port = 8080
secret = "optional-shared-secret"
```

Run with gateway/daemon and verify `/health`.

### 4.4 Email

```toml
[channels_config.email]
imap_host = "imap.example.com"
imap_port = 993
imap_folder = "INBOX"
smtp_host = "smtp.example.com"
smtp_port = 465
smtp_tls = true
username = "bot@example.com"
password = "email-password"
from_address = "bot@example.com"
poll_interval_secs = 60
allowed_senders = ["*"]
```

### 4.5 IRC

```toml
[channels_config.irc]
server = "irc.libera.chat"
port = 6697
nickname = "zeroclaw-bot"
username = "zeroclaw"              # optional
channels = ["#zeroclaw"]
allowed_users = ["*"]
server_password = ""                # optional
nickserv_password = ""              # optional
sasl_password = ""                  # optional
verify_tls = true
```

---

## 5. Validation Workflow

1. Configure one channel with permissive allowlist (`"*"`) for initial verification.
2. Run:

```bash
zeroclaw onboard --channels-only
zeroclaw daemon
```

1. Send a message from an expected sender.
2. Confirm a reply arrives.
3. Tighten allowlist from `"*"` to explicit IDs.

---

## 6. Troubleshooting Checklist

If a channel appears connected but does not respond:

1. Confirm the sender identity is allowed by the correct allowlist field.
2. Confirm bot account membership/permissions in target room/channel.
3. Confirm tokens/secrets are valid (and not expired/revoked).
4. Confirm transport mode assumptions:
   - polling/websocket channels do not need public inbound HTTP
   - webhook channels do need reachable HTTPS callback
5. Restart `zeroclaw daemon` after config changes.

For Matrix encrypted rooms specifically, use:
- [Matrix E2EE Guide](../../security/matrix-e2ee-guide.md)

---

## 7. Operations Appendix: Log Keywords Matrix

Use this appendix for fast triage. Match log keywords first, then follow the troubleshooting steps above.

### 7.1 Runtime supervisor keywords

If a specific channel task crashes or exits, the channel supervisor in `channels/mod.rs` emits:

- `Channel <name> exited unexpectedly; restarting`
- `Channel <name> error: ...; restarting`
- `Channel message worker crashed:`

These messages indicate automatic restart behavior is active, and you should inspect preceding logs for root cause.
