# Brainitive Desktop

The desktop app for [Brainitive](https://brainitive.app) on macOS, Windows and Linux: a
[Tauri 2](https://tauri.app) shell around your Brainitive server (**Brainitive Cloud** or a
**self-hosted** instance) with sign-in through your system browser, the refresh token in the OS
keychain, `brainitive://` deep links, native notifications, global shortcuts, a menu-bar / tray
panel, an offline copy of your work and on-device voice capture.

Status: **v1.0.0** (see [CHANGELOG.md](CHANGELOG.md)). It needs a Brainitive server
with App API v1 (`/api/v1`, Brainitive v1.49.0 or later).

<!-- Screenshots: add them to docs/images/ and link them here before the 1.0.0 release. -->
_Screenshots: coming soon._

## Download

- **GitHub Releases**: <https://github.com/armendzekjiri/brainitive-desktop/releases> (macOS `.dmg`,
  Windows `-setup.exe`, Linux `.AppImage` and `.deb`, plus `SHA256SUMS`).
- **brainitive.app/download**: <https://brainitive.app/download> links the latest installers.

Installed apps update themselves (signed updates only; Settings → Updates has a beta channel).

### System requirements

| OS | Requirement |
|---|---|
| macOS | 14 (Sonoma) or later, Apple Silicon or Intel (universal build) |
| Windows | 10 or 11, x64, with [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) (the installer sets it up if missing) |
| Linux | x64, `.AppImage` or `.deb` (WebKitGTK 4.1, ALSA for the microphone). Global shortcuts do not work on Wayland; the tray and menus still do |

## Connecting to Brainitive

On first start choose **Brainitive Cloud** (one click) or **Self-hosted server** and type its
address (`brainitive.example.com` is enough: `https://` is added; `http://` only for `localhost`).
The app asks the server (`GET /api/v1/instance`) and shows its name and version before you
continue. Certificate problems, "not a Brainitive server", a too-old server or a too-old app each
get a clear message. Then you sign in in your system browser.

You can add up to 20 servers (for example work and private) and switch in a second: Servers menu,
**⌃⌘1…⌃⌘9** (macOS) / **Alt+1…Alt+9** (Windows, Linux), the tray menu or the panel header.
Each server keeps its own sign-in, webview data, offline copy and waiting captures.

## Native features and default shortcuts

| Action | macOS | Windows | Linux |
|---|---|---|---|
| Brainitive key (then **C** capture, **T** new task, **K** switcher, **V** voice, **O** open) | ⌃⌘K | Ctrl+Alt+K | Ctrl+Alt+K |
| Quick capture | ⇧⌘Space | Ctrl+Shift+Space | Ctrl+Shift+Space |
| New task | ⌃⌘T | Ctrl+Alt+T | off (Ctrl+Alt+T opens a terminal on Ubuntu) |
| Quick switcher | off, ⌃⌘K K (suggested direct key ⌥⌘K) | off, Ctrl+Alt+K K (suggested Ctrl+Alt+Shift+K) | same as Windows |
| Voice capture (off until you turn it on) | ⌃⌘K V (suggested ⇧⌘V) | Ctrl+Alt+K V (suggested Ctrl+Shift+V) | same as Windows |

Everything can be changed or turned off in **Settings → Shortcuts**. Also: a menu-bar / tray panel
(Today, My Day, Assigned, Inbox, Timer), a full app menu, offline mode with an outbox, voice capture
(Whisper runs on your computer, audio is never saved or sent), notifications with a badge, launch
at login, and download handling with the platform's quarantine marks. Details: [CHANGELOG.md](CHANGELOG.md)
and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Building from source

Requirements: Rust (stable, 1.85 or later), Node 20+, the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) of your OS (Xcode command line tools
on macOS; WebView2 and the Visual Studio C++ tools on Windows; `libwebkit2gtk-4.1-dev` etc. on
Linux) and, for voice capture (whisper.cpp), **CMake** (`brew install cmake`; included in Visual
Studio's C++ tools on Windows; `cmake libasound2-dev` on Linux). The first build compiles
whisper.cpp (about 20-30 s).

```bash
npm install
npm run tauri dev          # Vite on :1420 + the Rust app
npm run build              # type-check + build the bundled screens
cd src-tauri
cargo test                 # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
npx tauri build --debug --bundles app          # macOS: a .app with the brainitive:// scheme
```

Against a local server: run the Brainitive app (`composer run dev`, `http://localhost:8000`);
debug builds offer **Local (localhost:8000)** and **Pre-release (dev.brainitive.app)** on the
connect screen. On macOS the `brainitive://` scheme is only registered for a **bundled** app, so
open `src-tauri/target/debug/bundle/macos/Brainitive.app`. Unsigned debug builds may make macOS ask
for keychain access after each rebuild.

## Docs

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): modules, flows, per-server data
- [docs/SECURITY.md](docs/SECURITY.md): threat model, IPC, navigation guard, tokens, TLS
- [docs/RELEASING.md](docs/RELEASING.md): signing, updates, cutting a release
- [docs/QA-CHECKLIST.md](docs/QA-CHECKLIST.md): manual per-OS test list
- [CONTRIBUTING.md](CONTRIBUTING.md): how to contribute

## Security

Please report vulnerabilities privately, not in a public issue: see [SECURITY.md](SECURITY.md).

## License

[Business Source License 1.1](LICENSE), like Brainitive itself: free for production use below the
revenue threshold in the Additional Use Grant; each version becomes AGPL-3.0 four years after its
release.
