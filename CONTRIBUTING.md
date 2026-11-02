# Contributing

Thanks for helping. Brainitive Desktop is licensed under BSL 1.1 (see [LICENSE](LICENSE)); by
contributing you agree your changes are released under it.

1. **Branch** from `main`: `feat/<topic>` or `fix/<topic>`. Keep a change focused.
2. **Read** [AGENTS.md](AGENTS.md) (rules for the Rust / IPC / security boundaries) and
   [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Pure logic gets a unit test.
3. **Run the gate** before opening a PR; all must be green:

   ```bash
   npm ci && npm run build
   cd src-tauri
   cargo test
   cargo clippy --all-targets -- -D warnings
   cargo fmt --check
   ```

4. **Commit style**: short imperative subject, conventional prefix (`feat:`, `fix:`, `docs:`,
   `chore:`, `ci:`), optional scope: `fix(updater): ignore older versions`.
5. **Docs**: update `CHANGELOG.md` (under `[Unreleased]`, plain sentences for users) and the doc
   your change makes wrong.
6. **No secrets**: never commit tokens, signing or update keys, `.env` files, certificates or real
   server addresses. Security problems go through [SECURITY.md](SECURITY.md), not a public issue.
