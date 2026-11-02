## What and why

<!-- One or two sentences. Link the issue if there is one. -->

## Checklist

- [ ] `npm run build` (type-check + bundled screens)
- [ ] `cd src-tauri && cargo test`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --check`
- [ ] New logic has a unit test; security-relevant changes follow `AGENTS.md` and `docs/SECURITY.md`
- [ ] `CHANGELOG.md` (`[Unreleased]`) and any doc this makes wrong are updated
- [ ] No secrets, keys, tokens or private server addresses in the diff
