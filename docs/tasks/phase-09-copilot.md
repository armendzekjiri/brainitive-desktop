# Phase 09 — reachability, diagnostics, performance (BRAI-367 / 368 / 369)

Built in this order, one commit per card. Gate after each card:
`cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings`, `npm run build` when `src/` changed.

## BRAI-367 — unreachable / reconnect screens, version gate

- `problem.rs` (pure): `Problem` = kind + title + message + suggested fix + `retryable`. Classifies an
  `ApiError` (and a `SessionError`) honestly: no network, DNS, refused, timeout, TLS (self-signed /
  expired / wrong host / other), HTTP 5xx, not a Brainitive server, moved. TLS reasons become an enum
  (`TlsReason`) instead of free text.
- `reconnect.rs` (pure): backoff schedule (5 s → 10 → 20 → 40 → 60 s cap) and the "network is back"
  rule (DNS / offline problems are probed every 3 s with a local name lookup, a success retries now).
- `flows::restore` asks `/api/v1/instance` first: error → Unreachable with the `Problem`; too old →
  new phase `UpdateRequired` (server name, both versions, download button); only then the web session.
  `open_offline` refuses while the server is known to need a newer app.
- Auto-reconnect loop (one at a time, generation counter), manual Retry resets it, a wake from sleep
  retries at once.
- UI (`index.html`, `main.ts`): problem title / message / fix, countdown, "Retrying…", the update screen.
- Tests: classification of every kind, backoff, version comparison with pre-release tags.

## BRAI-368 — logs, panic hook, Copy Diagnostics

- `redact.rs` (pure): URLs lose userinfo, query, fragment and opaque path segments; `bra_` / `brr_` tokens,
  JWTs, `Bearer …`, cookies and `token=`, `code=`, `state=`, `password=` … values are replaced.
- `logging.rs`: our own `log::Log` writer (no new dependency): `brainitive.log` in the app log dir,
  5 files × 2 MB, every line redacted at write time, UTC timestamps; a panic hook that logs message,
  location and backtrace (the app is `panic = "abort"`, the hook runs first). Webview errors of the
  bundled screens arrive through one command (`log_client_error`, main window only, rate-limited).
- `diagnostics.rs`: text builder (pure) + Help → Copy Diagnostics (clipboard plugin, Rust side) and
  Help → Open Logs Folder. No network code of any kind.
- Tests: redaction, rotation, timestamp, diagnostics text, menu ids.

## BRAI-369 — performance pass (desktop side)

- `startup.rs`: `mark("phase")` logs `startup: <phase> +<ms>` once per phase from `run()` on; phases
  from process entry to the bundled shell's first load and the server's first page load.
- `setup()`: nothing blocking that can wait (staging clean-up, wipes, capture queue flush run on the
  async runtime); settings read once.
- Measure cold start and idle memory on macOS (release build), write `docs/PERFORMANCE.md` with the numbers,
  how to measure, per-OS budgets, and suggestions for the web app (not changed here).
- `Cargo.toml` release profile: review and document.
