# macOS Update and Uninstall Guide

This page documents supported update and uninstall procedures for AgentZero on macOS (OS X).

Last verified: **February 22, 2026**.

## 1) Check current install method

```bash
which agentzero
agentzero --version
```

Typical locations:

- Homebrew: `/opt/homebrew/bin/agentzero` (Apple Silicon) or `/usr/local/bin/agentzero` (Intel)
- Cargo/bootstrap/manual: `~/.cargo/bin/agentzero`

If both exist, your shell `PATH` order decides which one runs.

## 2) Update on macOS

### A) Homebrew install

```bash
brew update
brew upgrade agentzero
agentzero --version
```

### B) Clone + bootstrap install

From your local repository checkout:

```bash
git pull --ff-only
./install.sh --prefer-prebuilt
agentzero --version
```

If you want source-only update:

```bash
git pull --ff-only
cargo install --path . --force --locked
agentzero --version
```

### C) Manual prebuilt binary install

Re-run your download/install flow with the latest release asset, then verify:

```bash
agentzero --version
```

## 3) Uninstall on macOS

### A) Stop and remove background service first

This prevents the daemon from continuing to run after binary removal.

```bash
agentzero service stop || true
agentzero service uninstall || true
```

Service artifacts removed by `service uninstall`:

- `~/Library/LaunchAgents/com.agentzero.daemon.plist`

### B) Remove the binary by install method

Homebrew:

```bash
brew uninstall agentzero
```

Cargo/bootstrap/manual (`~/.cargo/bin/agentzero`):

```bash
cargo uninstall agentzero || true
rm -f ~/.cargo/bin/agentzero
```

### C) Optional: remove local runtime data

Only run this if you want a full cleanup of config, auth profiles, logs, and workspace state.

```bash
rm -rf ~/.agentzero
```

## 4) Verify uninstall completed

```bash
command -v agentzero || echo "agentzero binary not found"
pgrep -fl agentzero || echo "No running agentzero process"
```

If `pgrep` still finds a process, stop it manually and re-check:

```bash
pkill -f agentzero
```

## Related docs

- [One-Click Bootstrap](one-click-bootstrap.md)
- [Commands Reference](../reference/cli/commands-reference.md)
- [Troubleshooting](../ops/troubleshooting.md)
