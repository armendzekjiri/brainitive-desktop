# Brainitive Desktop

The desktop app of [Brainitive](https://brainitive.app) for macOS, Windows and Linux: a
[Tauri 2](https://tauri.app) shell around your Brainitive server — **Brainitive Cloud** or a
**self-hosted** instance — with sign-in through your system browser, the refresh token in the OS
keychain, `brainitive://` deep links and native notifications.

Status: **0.1.0 (unreleased)**. The server side (App API v1, `/api/v1`) ships with Brainitive
v1.49.0.

## Choosing a server

On first start you choose **Brainitive Cloud** (one click) or **Self-hosted server**: type the
address (`brainitive.example.com` is enough — `https://` is added, paths and trailing slashes are
dropped; `http://` only for `localhost` / `127.0.0.1`). The app asks the server
(`GET /api/v1/instance`) and shows its name and version before you continue; certificate problems,
"not a Brainitive server", a too-old server and a too-old app are shown as clear errors.

You can change the server any time: **Server → Switch Server…** in the menu (or "Switch server" on
the sign-in screen). The last 5 servers are listed for one-click switching. Each server keeps its
own sign-in (keychain entry) and its own webview data, so switching does not sign you out.
**Forget** next to a recent server signs this computer out there and deletes its data in the
app. The window title shows where you are: "Brainitive — Cloud", "Brainitive — brainitive.example.com".

## Native features

- **Brainitive key** — ⌃⌘K (macOS) / Ctrl+Alt+K (Windows, Linux) from any app, then one letter:
  **C** quick capture, **K** quick switcher, **V** voice capture (hold to talk), **O** open Brainitive.
  A small panel lists the letters; Esc, clicking elsewhere or 3 seconds without a key close it.
  Change the key and the letters in Settings → Shortcuts.
- **Quick capture** — ⇧⌘Space (macOS) / Ctrl+Shift+Space from any app: type, ⌘↩ / Ctrl+↩ to save
  to your inbox. Works offline (sent when you're back).
- **Voice capture** — ⌃⌘K, then hold V (or a direct shortcut you set, e.g. ⇧⌘V), speak, let go: the text opens in quick capture to
  check and save (a tap starts it, ↩ or the shortcut again finishes; Esc cancels; max 2 minutes).
  Turn it on in Settings, or from File → Voice Capture… / the tray / quick capture's microphone button (a direct shortcut is only taken from other apps while it is on); the speech model (190 MB) is downloaded once.
  Speech is turned into text **on your computer** (Whisper); the audio is never saved or sent.
  Pick the language (or auto-detect) and remove the model in Settings → Voice capture.
- **Quick switcher** — ⌃⌘K K (or a direct shortcut you set): search everything, ↩ opens, ⌘C copies a snippet, ⌘T starts
  the timer on an issue.
- **Menu bar / tray** — My Day (tick work done), the running timer, unread notifications.
- **Menus** — ⌘N new issue, ⌥⌘N new doc, ⇧⌘N new window, ⌘K search, ⌘[ / ⌘] back / forward,
  ⌘= / ⌘- / ⌘0 zoom, ⌘, settings (Ctrl on Windows / Linux).
- **Settings** (⌘,) — Shortcuts: the Brainitive key and its letters, and optional direct shortcuts
  (quick capture ⇧⌘Space is on, quick switcher and voice capture are off until you set them); keep running in the tray, open at
  login, ask where to save downloads, voice capture.

## Development

Requirements: Rust (stable, ≥ 1.85), Node 20+, the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) of your OS (Xcode command line tools
on macOS; WebView2 on Windows; `libwebkit2gtk-4.1-dev` etc. on Linux) and, for voice capture's
whisper.cpp, **CMake** (`brew install cmake`; Visual Studio's C++ tools include it on Windows;
`cmake libasound2-dev` on Linux). The first build compiles whisper.cpp (≈ 20–30 s).

```bash
npm install
npm run tauri dev          # Vite on :1420 + the Rust app
npm run build              # type-check + build the bundled screens
cd src-tauri
cargo test                 # unit tests (URL rules, PKCE, deep links, guard, versions, settings)
cargo test --test live_network -- --ignored   # live TLS / discovery checks (network)
BRAINITIVE_VOICE_MODEL_DIR=/tmp/voice cargo test --lib real_transcription -- --ignored --nocapture
                           # macOS: downloads the speech model once, transcribes `say` clips
cargo clippy --all-targets -- -D warnings
npx tauri build --debug --bundles app          # macOS: a .app with the brainitive:// scheme
```

### Against a local server

In the app repo run `composer run dev` (the server on `http://localhost:8000`). Debug builds offer
**Local (localhost:8000)** and **Pre-release (dev.brainitive.app)** on the connect screen.

Sign-in returns through `brainitive://auth/callback`. On macOS the scheme is only registered for a
**bundled** app: build it with `npx tauri build --debug --bundles app` and open
`src-tauri/target/debug/bundle/macos/Brainitive.app` (on Windows / Linux, `tauri dev` registers the
scheme at runtime). Unsigned debug builds may make macOS ask for keychain access after each rebuild.

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — modules, flows, per-server data
- [docs/SECURITY.md](docs/SECURITY.md) — threat model, IPC, navigation guard, tokens, TLS
- [docs/SPIKE.md](docs/SPIKE.md) — per-OS webview matrix and go / no-go
- Server contract: `docs/ai/API.md` of the app repo (`../docs/ai/API.md` when this repo is checked
  out at `brainitive/desktop/`)

## License

[Business Source License 1.1](LICENSE), like Brainitive itself: free for production use below the
revenue threshold of the Additional Use Grant; each version becomes AGPL-3.0 four years after its
release.
