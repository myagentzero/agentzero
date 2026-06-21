# AgentZero Operations Runbook

This runbook is for operators who maintain availability, security posture, and incident response.

Last verified: **February 18, 2026**.

## Scope

Use this document for day-2 operations:

- starting and supervising runtime
- health checks and diagnostics
- safe rollout and rollback
- incident triage and recovery

For first-time installation, start from [one-click-bootstrap.md](../setup-guides/one-click-bootstrap.md).

## Runtime Modes

| Mode | Command | When to use |
|---|---|---|
| Foreground runtime | `agentzero daemon` | local debugging, short-lived sessions |
| Foreground gateway only | `agentzero gateway` | webhook endpoint testing |
| User service | `agentzero service install && agentzero service start` | persistent operator-managed runtime |
| Docker / Podman | `docker compose up -d` | containerized deployment |

## Docker / Podman Runtime

If you installed via `./install.sh --docker`, the container exits after onboarding. To run
AgentZero as a long-lived container, use the repository `docker-compose.yml` or start a
container manually against the persisted data directory.

### Recommended: docker-compose

```bash
# Start (detached, auto-restarts on reboot)
docker compose up -d

# Stop
docker compose down

# Restart
docker compose up -d
```

Replace `docker` with `podman` if using Podman.

### Manual container lifecycle

```bash
# Start a new container from the bootstrap image
docker run -d --name agentzero \
  --restart unless-stopped \
  -v "$PWD/.agentzero-docker/.agentzero:/agentzero-data/.agentzero" \
  -v "$PWD/.agentzero-docker/workspace:/agentzero-data/workspace" \
  -e HOME=/agentzero-data \
  -e AGENTZERO_WORKSPACE=/agentzero-data/workspace \
  -p 42617:42617 \
  agentzero-bootstrap:local \
  gateway

# Stop (preserves config and workspace)
docker stop agentzero

# Restart a stopped container
docker start agentzero

# View logs
docker logs -f agentzero

# Health check
docker exec agentzero agentzero status
```

For Podman, add `--userns keep-id --user "$(id -u):$(id -g)"` and append `:Z` to volume mounts.

### Key detail: do not re-run install.sh to restart

Re-running `install.sh --docker` rebuilds the image and re-runs onboarding. To simply
restart, use `docker start`, `docker compose up -d`, or `podman start`.

For full setup instructions, see [one-click-bootstrap.md](../setup-guides/one-click-bootstrap.md#stopping-and-restarting-a-dockerpodman-container).

## Baseline Operator Checklist

1. Validate configuration:

```bash
agentzero status
```

2. Verify diagnostics:

```bash
agentzero doctor
agentzero channel doctor
```

3. Start runtime:

```bash
agentzero daemon
```

4. For persistent user session service:

```bash
agentzero service install
agentzero service start
agentzero service status
```

## Health and State Signals

| Signal | Command / File | Expected |
|---|---|---|
| Config validity | `agentzero doctor` | no critical errors |
| Channel connectivity | `agentzero channel doctor` | configured channels healthy |
| Runtime summary | `agentzero status` | expected provider/model/channels |
| Daemon heartbeat/state | `~/.agentzero/daemon_state.json` | file updates periodically |

## Logs and Diagnostics

### macOS / Windows (service wrapper logs)

- `~/.agentzero/logs/daemon.stdout.log`
- `~/.agentzero/logs/daemon.stderr.log`

### Linux (systemd user service)

```bash
journalctl --user -u agentzero.service -f
```

## Incident Triage Flow (Fast Path)

1. Snapshot system state:

```bash
agentzero status
agentzero doctor
agentzero channel doctor
```

2. Check service state:

```bash
agentzero service status
```

3. If service is unhealthy, restart cleanly:

```bash
agentzero service stop
agentzero service start
```

4. If channels still fail, verify allowlists and credentials in `~/.agentzero/config.toml`.

5. If gateway is involved, verify bind/auth settings (`[gateway]`) and local reachability.

## Safe Change Procedure

Before applying config changes:

1. backup `~/.agentzero/config.toml`
2. apply one logical change at a time
3. run `agentzero doctor`
4. restart daemon/service
5. verify with `status` + `channel doctor`

## Rollback Procedure

If a rollout regresses behavior:

1. restore previous `config.toml`
2. restart runtime (`daemon` or `service`)
3. confirm recovery via `doctor` and channel health checks
4. document incident root cause and mitigation

## Related Docs

- [one-click-bootstrap.md](../setup-guides/one-click-bootstrap.md)
- [troubleshooting.md](./troubleshooting.md)
- [config-reference.md](../reference/api/config-reference.md)
- [commands-reference.md](../reference/cli/commands-reference.md)
