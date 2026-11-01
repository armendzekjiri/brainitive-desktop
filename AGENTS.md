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
- New local commands: `build.rs` manifest + `capabilities/local.json`.
- Pure logic goes in the pure modules (`server`, `links`, `pkce`, `version`, `settings`) with unit tests.
- Pin exact dependency versions.

## Gate (all green before a change is done)

```bash
npm run build
cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings && cargo build
```

Commits: conventional messages, author `armendzekjiri <armendzekjiri@outlook.de>`.
