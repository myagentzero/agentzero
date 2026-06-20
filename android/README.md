# AgentZero Android Client

Native Android client for the [AgentZero](https://github.com/agentzero) gateway API. Connect over Tailscale VPN using the same pairing flow as the web UI and TUI.

## Features

- **Dashboard** — version, uptime, provider/model, cost pulse, token statistics, component health
- **Agent Chat** — WebSocket chat with streaming, tool calls, and history restore
- **Mission Control** — live SSE event stream with pause, filters, and event details
- **Memory** — browse, search, add, and delete memory entries
- **Workspace** — file tree viewer with text preview and binary download
- **Devices** — list paired devices, generate invite codes, revoke access

## Requirements

- Android 8.0+ (API 26); tested target is recent Pixel devices (API 35)
- [Android Studio](https://developer.android.com/studio) Ladybug (2024.2) or newer recommended
- JDK 17+
- Android SDK with API 35 platform and build-tools installed
- AgentZero gateway reachable over Tailscale (HTTP)

## First-time setup

1. Install Android Studio and open the **`android/`** directory as a project.
2. When prompted, install the Android SDK Platform 35 and Android SDK Build-Tools.
3. Create `android/local.properties` if Android Studio does not create it automatically:

```properties
sdk.dir=/Users/YOUR_USER/Library/Android/sdk
```

On Linux, use `sdk.dir=/home/YOUR_USER/Android/Sdk`.

## Build a debug APK (recommended for sideloading)

From the `android/` directory:

```bash
./gradlew assembleDebug
```

Output:

```
android/app/build/outputs/apk/debug/app-debug.apk
```

Install on a connected device:

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

## Build a release APK

Release builds must be signed. For local/testing use, create a debug-style keystore or use your own signing config.

### Option A — unsigned release APK (quick local build)

```bash
./gradlew assembleRelease
```

Some devices require signing; prefer the debug APK for personal sideloading.

### Option B — signed release APK

1. Create a keystore (one-time):

```bash
keytool -genkey -v -keystore agentzero-release.keystore -alias agentzero \
  -keyalg RSA -keysize 2048 -validity 10000
```

2. Add to `android/gradle.properties` (do **not** commit secrets):

```properties
RELEASE_STORE_FILE=../agentzero-release.keystore
RELEASE_STORE_PASSWORD=your-store-password
RELEASE_KEY_ALIAS=agentzero
RELEASE_KEY_PASSWORD=your-key-password
```

3. Add signing config to `app/build.gradle.kts` (see Android docs), then:

```bash
./gradlew assembleRelease
```

Output: `android/app/build/outputs/apk/release/app-release.apk`

## Using the app

1. Connect your phone to Tailscale.
2. Open **AgentZero**.
3. Enter the gateway **host** (Tailscale IP like `100.x.x.x` or MagicDNS name) and **port** (default `42617`).
4. If pairing is enabled on the gateway, enter the **6-digit code** shown in the gateway terminal (same as the web UI).
5. Use the navigation drawer to switch between screens.

## Authentication

Matches the web UI:

| Step | Endpoint | Notes |
|------|----------|-------|
| Health check | `GET /health` | Determines if pairing is required |
| Pair | `POST /pair` | Header `X-Pairing-Code: <code>` → bearer token |
| API calls | `Authorization: Bearer <token>` | Stored in EncryptedSharedPreferences |
| WebSocket | `/ws/chat` | Subprotocols `zeroclaw.v1`, `bearer.<token>` |
| SSE | `/api/events` | Bearer token in `Authorization` header |

## Network

The app allows cleartext HTTP for private Tailscale networks via `network_security_config.xml`. Do not expose the gateway to the public internet without TLS.

## Project structure

```
android/
  app/src/main/kotlin/com/agentzero/client/
    data/           # Gateway REST, WebSocket, SSE clients
    ui/screens/     # Compose screens
    ui/theme/       # Material 3 theme
  README.md
```

## Troubleshooting

| Issue | Fix |
|-------|-----|
| `SDK location not found` | Create `local.properties` with `sdk.dir=...` |
| Connection refused | Confirm Tailscale is connected; verify host/port; gateway bound to Tailscale interface or `0.0.0.0` |
| 401 Unauthorized | Sign out and re-pair with a fresh 6-digit code |
| WebSocket disconnects | Check gateway logs; ensure pairing token is valid |

## Development commands

```bash
# Format/lint via Android Studio, or:
./gradlew :app:assembleDebug
./gradlew :app:lint
```
