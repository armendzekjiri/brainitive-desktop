# AGENTS.md

Guidance for AI coding agents (Claude, Codex, Copilot, Cursor) in **Brainitive Desktop**.

This is its **own git repository** (remote `armendzekjiri/brainitive-desktop`, public, BSL 1.1). On
Armend's machine it is checked out inside the Brainitive app repo at `brainitive/desktop/`
(gitignored there), so local agents share the app's docs and memory. In a standalone clone the
`../` paths below don't exist; the server contract is `docs/ai/API.md` in the app repo.

**Signing or building releases on someone else's Mac?** Start with
[docs/SIGNING-AGENT-BRIEF.md](docs/SIGNING-AGENT-BRIEF.md). **Never commit desktop files to the app repo, and never
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
  `switcher.json`, `offline.json`, `voice.json`, `leader.json`, `panel.json`, `topbar.json` = the main window's top bar, `tabs::topbar` as its check), a caller check as the
  command's **first line** (`popup::ensure_caller`, or `ensure_main_screen` for the main window's
  screens; `tests/capabilities.rs` checks every command of the manifest), and SECURITY.md.
- Work that awaits the network and then writes account data (outbox, capture queue, panel / tray
  answers) carries the `state::AccountTicket` / `tray::PollTicket` taken before the request and
  checks it under the state lock before writing (codex review 1 / 5 / 9). Tokens are stored only
  through `Session` (its epoch refuses a refresh / exchange that started before a sign-out).
- CI signing: no secret ever reaches a build step (`tauri build`, npm, cargo); platform signing runs
  after the build (`scripts/macos-sign.sh`, the Windows signing steps). `node --test scripts/`
  checks the workflow.
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
- Auto-updates (`updater.rs`, BRAI-373): `tauri-plugin-updater` from Rust only (never grant
  `updater:*`). Never commit a private update key or replace the placeholder public key with a key
  you made: the owner does (docs/RELEASING.md). Tests make throwaway keys in the test. Updater
  requests stay https on GitHub hosts (`updater::allowed_url`); nothing installs without the user's
  "Restart to Update" click.
- The Microsoft Store build (BRAI-442, `--features msstore` + `tauri.msstore.conf.json`,
  docs/MICROSOFT-STORE.md) has no updater: anything that checks, downloads or installs updates
  must go through `updater::Availability` (`Store` there). Its WinRT code (`msstore/packaged.rs`)
  only compiles on Windows; keep `msstore/mod.rs`'s stubs in step. The MSIX identity lives in
  `src-tauri/msix/AppxManifest.xml.tmpl`, `msstore` constants and MICROSOFT-STORE-LISTING.md:
  change all three or none (the tests compare them).
- Pure logic goes in the pure modules (`server`, `servers`, `links`, `pkce`, `version`, `settings`,
  `shortcuts`, `queue`, `offline_cache`, `outbox`, `poll`, `badge`, `voice::recorder`, and the pure
  parts of `tray` / `switcher` / `downloads` / `voice::model` / `voice::audio` / `updater`) with
  unit tests.
- A page link that comes from outside the app's own flow (deep link, web notification, server
  data in tray / menus / switcher) opens through `links::resolve_deep_link_target` (never the auth
  pages); only the web-session step uses `resolve_on_instance` for `/app/session/<code>` (BRAI-377).
- Files with user data are written through `private_fs` (0600 from creation, 0700 folders).
- Offline data (`offline/<store id>/`) belongs to one server and one account: delete it wherever a
  sign-in ends (`offline::wipe`), never put a token in it.
- Pin exact dependency versions.
- When smoke-running the app, quit only the process you started: the app repo's `composer run dev`
  (vite, server) and a bundled Brainitive.app may be running — never `pkill` by a broad pattern.

## Gate (all green before a change is done)

```bash
npm run build
cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo build
cargo clippy --all-targets --features msstore -- -D warnings   # the Store variant
node --test scripts/                                           # from the repo root
```

Commits: conventional messages, author `armendzekjiri <armendzekjiri@outlook.de>`.
