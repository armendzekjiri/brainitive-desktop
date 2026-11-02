# AGENTS.md

Guidance for AI coding agents (Claude, Codex, Copilot, Cursor) in **Brainitive Desktop**.

This is its **own git repository** (future remote `armendzekjiri/brainitive-desktop`, BSL 1.1). It is
checked out inside the Brainitive app repo at `brainitive/desktop/` (gitignored there) so local
agents share the app's docs and memory. **Never commit desktop files to the app repo, and never
commit app files here.** Releases and versions are this repo's own (0.x → Desktop 1.0).

## Read first

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — module map, flows, per-server data
- [docs/SECURITY.md](docs/SECURITY.md) — what remote content may do; read before touching
  `shell.rs`, `bridge.rs`, `links.rs`, capabilities or `build.rs`
- [docs/SPIKE.md](docs/SPIKE.md) — what is verified per OS
- The server contract: [`../docs/ai/API.md`](../docs/ai/API.md) (+ `../docs/api/openapi.json`) in the
  app repo. The web side of the bridge: `../resources/js/lib/desktop.ts`.

## Rules

- Every `/api/v1` call is made from **Rust** (`api.rs`, reqwest) — the API has no CORS, and tokens
  never enter the webview, localStorage or disk (refresh token: keychain only).
- Refreshes go through `Session` (one async mutex): a refresh token used twice revokes the device.
- Remote content gets only `brainitive_notify`, `brainitive_set_badge`, `brainitive_open_external`, `brainitive_sign_out`.
  A new bridge command needs: the command (with the current-origin check), `build.rs` manifest,
  `shell::BRIDGE_PERMISSIONS`, SECURITY.md, and the web side in the app repo.
- New local commands: `build.rs` manifest + the capability file of **the one bundled window** that
  uses it (`capabilities/local.json` = main window screens, `settings.json`, `capture.json`,
  `switcher.json`, `offline.json`, `voice.json`), a `popup::ensure_caller` check in the command,
  and SECURITY.md.
  `tests/capabilities.rs` fails when a file grants a remote origin, another window, a plugin or a
  bridge permission, or a command missing from `build.rs`.
- Native windows (`popup.rs`) are bundled pages: they reach the server only through Rust commands
  that use `Session::request` (`api::ApiRequest`: `/api/v1/…` paths only). Never give them a token.
- Plugins (global-shortcut, autostart, dialog, clipboard-manager, …) are used from Rust only; never
  grant their JS permissions to a webview.
- Don't build webviews inside main-thread handlers (menu, tray, global shortcut, sync commands):
  spawn on the async runtime (Windows deadlock).
- A new global shortcut = a `hotkeys::HotkeyAction` variant + its preference + `hotkeys::wanted`
  (`wants_release` if it needs the key-up, like voice capture's hold to talk).
- Voice capture (`voice/`): audio stays in memory (never a file, never the network, never a
  webview); log durations only, never audio or transcript text. The only extra network host is
  the pinned model download (`voice::model::MODEL`, size + SHA-256 checked).
- Pure logic goes in the pure modules (`server`, `links`, `pkce`, `version`, `settings`,
  `shortcuts`, `queue`, `offline_cache`, `outbox`, `poll`, `badge`, `voice::recorder`, and the pure
  parts of `tray` / `switcher` / `downloads` / `voice::model` / `voice::audio`) with unit tests.
- Offline data (`offline/<store id>/`) belongs to one server and one account: delete it wherever a
  sign-in ends (`offline::wipe`), never put a token in it.
- Pin exact dependency versions.
- When smoke-running the app, quit only the process you started: the app repo's `composer run dev`
  (vite, server) and a bundled Brainitive.app may be running — never `pkill` by a broad pattern.

## Gate (all green before a change is done)

```bash
npm run build
cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo build
```

Commits: conventional messages, author `armendzekjiri <armendzekjiri@outlook.de>`.
