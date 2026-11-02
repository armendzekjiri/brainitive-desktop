# Performance (BRAI-369, desktop side)

Targets: cold start < 2 s to first paint of the bundled shell; remote page as fast as the server;
idle memory within a per-OS budget. Large boards, whiteboards and Gantt are rendered by the web app
(other repo) — see "Suggestions for the web app".

## What was changed in the desktop

- `setup()` does no network work: `flows::startup` builds the window and spawns the connect on the
  async runtime. The window is created with the bundled `index.html`, so the first paint does not
  wait for the server.
- `downloads::clean_staging` (a directory removal) now runs on the blocking pool, not in `setup()`.
- Settings are loaded once (`AppState::new`); the capture queue file once. Nothing else reads disk.
- `startup.rs` logs each phase once, as ms since `run()` began: `setup entered`,
  `settings and queue loaded`, `menu, tray and shortcuts ready`, `main window created`,
  `bundled shell loaded` (first page load of the bundled screen, the first-paint proxy), and
  `remote page loaded` (first load of the server's page in the main window).
- Not changed on purpose: the two `reqwest` clients are built in `ApiClient::new` (microseconds,
  no I/O); the keychain is only read inside the async connect.

## Measuring

1. Release build: `cd src-tauri && cargo build --release` (binary `target/release/brainitive-desktop`).
2. Start it (quit any running Brainitive first: the single-instance plugin hands a second launch to
   the running app, which then exits at once, and nothing is measured).
3. Cold start: read `brainitive.log` (macOS `~/Library/Logs/app.brainitive.desktop/`, other OSes: Tauri's `app_log_dir`, untested),
   lines `startup: … +Nms`. "Cold" = after a reboot or `sudo purge` on macOS; "warm" = second launch.
   `bundled shell loaded` is a few ms before the pixels appear (WKWebView paints right after
   `didFinish`); for pixel-accurate numbers record the screen at 60 fps.
4. Idle memory (macOS): launch, wait 30 s, then
   `ps -axo pid,rss,comm | grep -E "brainitive|com.apple.WebKit"`. The WebKit helpers
   (WebContent, Networking, GPU) belong to the app: they start right after it, so compare the PID
   list before and after launch. Sum `rss` (KB). Better: Activity Monitor → "Memory" column, or
   `footprint <pid>` for each. Windows: Task Manager → Details, `msedgewebview2.exe` children of the
   app. Linux: `ps --ppid` for `WebKitWebProcess`.

## Numbers

**Not measured in this run.** The automation could not start a clean second instance: a debug
bundle of Brainitive Desktop belonging to the developer was running, the single-instance plugin
forwarded the new launch to it and exited, and I did not stop a process I had not started. Run the
procedure above on a quiet machine and fill in:

| | macOS (arm64) | Windows | Linux |
|---|---|---|---|
| Cold start → bundled shell loaded | _todo_ | _todo_ | _todo_ |
| Cold start → remote page loaded | _todo_ | _todo_ | _todo_ |
| Idle RSS, app + webview helpers (Choose screen) | _todo_ | _todo_ | _todo_ |
| Idle RSS, signed in on the dashboard | _todo_ | _todo_ | _todo_ |

Reference points that were seen: release binary 8.4 MB (stripped). The developer's *debug* build
signed in to a live dev server showed about 445 MB in total (app 128 MB, WebContent 245 MB,
GPU 47 MB, Networking 25 MB): not a budget measurement (debug build, dev server, many open
views), but it shows the web content dominates: the Rust process is a small share.

**Proposed budgets** (to confirm with real numbers): Choose screen ≤ 120 MB total; signed in,
dashboard idle ≤ 350 MB macOS, ≤ 400 MB Windows (WebView2 is heavier), ≤ 350 MB Linux.

## Release profile (`Cargo.toml`)

| Setting | Why |
|---|---|
| `opt-level = "s"` | size over speed: the work is in the webview; less code to page in at launch |
| `lto = true` | cross-crate dead-code removal and inlining (~10–20 % smaller) |
| `codegen-units = 1` | best optimisation; only costs build time |
| `panic = "abort"` | no unwinding tables; the panic hook still logs first |
| `strip = true` | no symbols; the debug build and CI artefacts keep them for debugging |

All were already set; they were reviewed and are kept. `opt-level = 3` or `thin` LTO would only
shorten builds / change speed in code that isn't hot. Don't enable `debug = true` in release.

## Suggestions for the web app (not changed here)

- **Boards:** virtualise columns and cards (render only what is in the viewport + overscan);
  `content-visibility: auto` on cards; keep card components memoised so a drag only re-renders the
  moved card and its columns; paginate / lazy-load long columns.
- **Whiteboards:** draw on `canvas` (or batch SVG updates) instead of one DOM node per element;
  cull off-screen elements; use `requestAnimationFrame` batching and `will-change: transform` only
  while panning; debounce persistence; move heavy hit-testing to a worker.
- **Gantt:** window the rows and the time axis; render bars with CSS transforms; compute dependency
  lines lazily for visible rows; avoid layout reads inside scroll handlers (use
  `IntersectionObserver`).
- **General:** ship a service-worker / HTTP-cached app shell so the remote page loads from cache
  (the desktop loads it from the server each start); code-split heavy editors; send `Cache-Control:
  immutable` for hashed assets; `prefers-reduced-motion` support.
