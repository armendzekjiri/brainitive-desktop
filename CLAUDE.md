# CLAUDE.md

Brainitive Desktop (Tauri 2). Everything agents need is in **[AGENTS.md](AGENTS.md)** — read it
first; this file stays thin so all agents share one set of rules.

- Own repo, own releases; checked out at `brainitive/desktop/` inside the app repo. Don't commit
  across the two repos.
- Server contract: `../docs/ai/API.md` (app repo). Security model: `docs/SECURITY.md`.
- Gate: `npm run build` and, in `src-tauri/`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo build`.
