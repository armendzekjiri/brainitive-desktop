# Spike: remote-URL Tauri shell per OS (BRAI-349)

Date: 2026-11-01 · Tauri 2.12.5 / wry 0.57.1 · tested on macOS 26 (Darwin 25.2), Apple Silicon.

**Legend:** ✅ verified on this Mac · 🟡 written, not yet exercised · ⏳ pending (needs the OS / CI) · ⚠️ known risk

## What was verified on macOS

| Check | Result |
|---|---|
| `cargo build` (debug) + `tauri build --debug --bundles app` | ✅ builds; the bundle's Info.plist registers `brainitive://`, `LSMinimumSystemVersion 14.0` |
| The shell starts, the bundled connect screen loads and runs its JS (Vite dev server, IPC) | ✅ |
| Per-server webview data store (`WKWebsiteDataStore(forIdentifier:)`) | ✅ a store with the expected UUID is created in `~/Library/WebKit/brainitive-desktop/WebsiteDataStore/` |
| Connect a local server (`localhost:8000`, `release/v1.49.0`): `/api/v1/instance` → name + version shown → saved as current + recent | ✅ (`settings.json` written) |
| TLS: self-signed / expired / wrong host certificates are refused with a readable reason | ✅ `cargo test --test live_network -- --ignored` (badssl.com) |
| Cloud before v1.49.0: 404 on `/api/v1/instance` recognised as "server too old" | ✅ live test |
| `https://brainitive.app` redirect | ✅ refused, as designed: it 301s to **http**://www… (an http hop is never followed) — fix nginx to redirect straight to https://www |

## Matrix

| Area | macOS (WKWebView) | Windows (WebView2) | Linux (WebKitGTK) |
|---|---|---|---|
| Build + start | ✅ | ⏳ CI (`ci.yml`) | ⏳ CI |
| Login: system browser + `brainitive://auth/callback` | 🟡 needs the bundled app + a server with v1.49.0 (lead's e2e) | ⏳ (scheme via installer / `register_all` in dev) | ⏳ (`.desktop` file; dev: `register_all`) |
| Web session in the webview (`/app/session/<code>`) | 🟡 | ⏳ | ⏳ |
| Collaborative doc editor (Yjs over `/collab` websocket) | 🟡 ⚠️ check websocket + `SharedArrayBuffer`-free path | ⏳ | ⏳ ⚠️ WebKitGTK version differences |
| Whiteboard | 🟡 | ⏳ | ⏳ ⚠️ canvas performance on WebKitGTK |
| Gantt drag, board drag & drop | 🟡 (Tauri's drop handler disabled so HTML5 DnD reaches the page) | ⏳ ⚠️ the classic WebView2 DnD issue — handler disabled for this reason | ⏳ |
| File upload (`<input type=file>`, drop files) | 🟡 | ⏳ | ⏳ |
| File download | 🟡 saved to Downloads (unique name) + a notification; ⚠️ `blob:` / `data:` downloads to verify | ⏳ | ⏳ |
| Clipboard (`navigator.clipboard`, ⌘C/⌘V) | 🟡 Edit menu present (needed for shortcuts) | ⏳ `enable_clipboard_access` set | ⏳ same |
| Reverb realtime (websocket to the instance) | 🟡 same origin, no CSP change by the app | ⏳ | ⏳ |
| Service worker + IndexedDB offline | ✅ (BRAI-366 probe, 2026-11-02, macOS 26: WKWebView with a `forIdentifier:` store registers `/sw.js` on the Cloud origin and on `http://localhost:8000`, the worker controls a reload, Cache API + IndexedDB work); the native offline view covers users without the web's opt-in | ⏳ supported; native offline view untested | ⚠️ ⏳ WebKitGTK SW support depends on version; native offline view untested |
| Native offline view + outbox (BRAI-366) | 🟡 unit tested; not clicked through in a running app, no end-to-end sync (dev DB lacks `sync_tombstones`) | ⏳ | ⏳ |
| Web push | replaced by native notifications (`brainitive_notify`) 🟡; click → focus + open the page | 🟡 click reported only while the toast is on screen | 🟡 |
| Dock / taskbar badge | 🟡 `set_badge_count` | ⏳ not implemented (needs an overlay icon) | 🟡 Unity launcher API only |
| User agent ` BrainitiveDesktop/<v>` | 🟡 WKWebView default + marker | ⚠️ a fixed Chrome version is spelled out (wry can only replace the UA) — check UA sniffing | 🟡 |
| Iframe embeds (YouTube, Figma) under the navigation guard | 🟡 allowed (the handler also sees iframes on macOS; top-level loads are cancelled on page load) | ⏳ handler is top-level only | ⏳ |
| Window state, single instance | 🟡 | ⏳ | ⏳ |
| App menu + accelerators (BRAI-362) | ✅ menu builds at start; 🟡 actions | ⏳ ⚠️ accelerators while WebView2 has focus | ⏳ |
| Extra app windows sharing the data store | 🟡 | ⏳ same `data_directory` in one process | ⏳ |
| Global shortcuts (BRAI-359 / 401) | 🟡 registered from Rust | ⏳ | ⏳ ⚠️ X11 only (no global shortcuts on Wayland) |
| Bundled popups (capture / switcher): focus, hide on blur, focus back | 🟡 `app.hide()` returns focus | ⏳ | ⏳ ⚠️ always-on-top / focus vary per WM |
| Tray / menu bar (BRAI-360) | ✅ installs at start; 🟡 menu actions | ⏳ | ⏳ ⚠️ needs libayatana-appindicator; left click shows the menu |
| Badge (BRAI-361) | 🟡 dock badge + count next to the menu-bar icon | 🟡 overlay icon (written, not run) | 🟡 Unity launcher only |
| Save dialog for downloads (BRAI-363) | 🟡 staging + dialog | ⏳ | ⏳ (xdg portal / GTK dialog) |
| Launch at login (BRAI-364) | 🟡 LaunchAgent | ⏳ Run registry key | ⏳ autostart `.desktop` file |
| Voice capture (BRAI-439): whisper.cpp build, model download, transcription | ✅ builds (cmake), real model download + checksum, transcription with Metal (M4 Pro: 13.7 s clip in 0.35 s; CPU 0.5 s); 🟡 microphone, permission prompt, indicator and hold-to-talk not run in the app | ⏳ MSVC build of whisper.cpp, WASAPI capture, "denied" error mapping, key-up by polling | ⏳ needs `libasound2-dev`; key-up via X11 detectable auto-repeat; ⚠️ no global shortcuts on Wayland (tray / menu still work) |

## Preliminary go / no-go

**macOS: GO (preliminary)** for the Tauri remote-URL shell. Build, bundle, URL scheme, IPC from
the bundled screens, per-server data stores, TLS policy and the server discovery work. The
service worker + IndexedDB check is cleared (BRAI-366): the web's own offline mode works in
WKWebView, and a native offline view (Rust cache + outbox over the App API sync) covers the rest. Then the editor, whiteboard and Gantt/board drag checks
with a signed-in session (needs v1.49.0 on a server).

**Windows / Linux: pending.** Nothing verified yet; run the CI build first, then a manual pass on
real machines (WebView2 drag & drop, WebKitGTK editor / whiteboard / service worker). Fallback
per the card: Electron for Linux only, if WebKitGTK fails the editor or whiteboard.
