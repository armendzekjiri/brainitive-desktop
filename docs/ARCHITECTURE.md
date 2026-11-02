# Architecture

Brainitive Desktop is a **Tauri 2** shell: a main window that shows either a few bundled
screens (connect a server, sign in) or the user's Brainitive server — the full web app, loaded
remotely — plus extra app windows with pages of that server, and small **bundled native windows**
(quick capture, quick switcher, settings, the offline view, the voice-capture indicator) that talk to the server only through Rust. The native
layer adds what a browser tab can't: system-browser sign-in with keychain storage, deep links,
native menus, global shortcuts, a tray / menu-bar icon, native notifications and the badge, a
Save dialog for downloads and launch at login. Server contract:
[`../../docs/ai/API.md`](../../docs/ai/API.md) in the app repo (when checked out at `brainitive/desktop/`).

```
desktop/
├── index.html, src/            bundled pages (Vite multi-page + TypeScript, no framework)
│   ├── main.ts, styles.css     connect / sign-in screens (index.html, main window)
│   ├── capture.ts              quick capture (capture.html, window "capture")
│   ├── switcher.ts             quick switcher (switcher.html, window "switcher")
│   ├── settings.ts             Settings (settings.html, window "settings")
│   ├── offline.ts, offline.css the offline view (offline.html, window "offline", BRAI-366)
│   ├── voice.ts                the voice-capture indicator (voice.html, window "voice", BRAI-439)
│   └── native.ts/.css, popup.css   shared by the native windows
├── public/logo.png
└── src-tauri/
    ├── tauri.conf.json         id app.brainitive.desktop, CSP, deep-link scheme, macOS ≥ 14
    ├── Info.plist              merged by Tauri: NSMicrophoneUsageDescription (voice capture)
    ├── .cargo/config.toml      GGML_NATIVE=OFF: a portable whisper.cpp build
    ├── build.rs                app ACL manifest (every command needs a permission)
    ├── capabilities/           one file per bundled window, local origin + its own window only:
    │                           local.json (main), settings.json, capture.json, switcher.json,
    │                           offline.json, voice.json
    ├── src/
    │   ├── lib.rs              plugins, commands, menu, deep-link + notification wiring, run loop
    │   ├── server.rs           ServerOrigin: typed address → origin, TLS rule, store ids   (pure)
    │   ├── links.rs            navigation decisions, deep-link parsing, URL validation      (pure)
    │   ├── pkce.rs             PKCE S256 + state                                             (pure)
    │   ├── version.rs          min_versions.desktop check, download link                     (pure)
    │   ├── problem.rs          why a server can't be used: kind, message, fix, retryable      (pure)
    │   ├── redact.rs           removes tokens, cookies, codes, query strings from log text   (pure)
    │   ├── logging.rs          rotating log file (5 × 2 MB), `log` backend, panic hook       (rotation pure)
    │   ├── diagnostics.rs      Help → Copy Diagnostics text + Open Logs Folder
    │   ├── startup.rs          cold-start phase timing (`startup: <phase> +Nms` in the log)
    │   ├── reconnect.rs        backoff 5 s → 60 s, lookup target for the "network is back" probe (pure)
    │   ├── settings.rs         settings.json: servers, pending wipes, Preferences, zoom steps (pure)
    │   ├── shortcuts.rs        global shortcut defaults, parsing, rules, labels              (pure)
    │   ├── queue.rs            offline quick-capture queue (capture-queue.json)              (pure)
    │   ├── offline_cache.rs    offline copy from the sync feed: runs, tombstones, view model (pure)
    │   ├── outbox.rs           offline changes for POST /api/v1/sync, result handling       (pure)
    │   ├── poll.rs             tray polling: backoff, wake detection, plan, clock            (pure)
    │   ├── badge.rs            Windows overlay badge bitmap                                  (pure)
    │   ├── api.rs              reqwest client for /api/v1 (instance, auth, and ApiRequest for any
    │   │                       authenticated call: /api/v1/ paths only, ETag / 304, no redirects)
    │   ├── secrets.rs          refresh token per origin in the OS keychain (keyring)
    │   ├── session.rs          sign-in state, in-memory access tokens, serialized refresh,
    │   │                       Session::request (refresh + retry on 401) and CallError
    │   ├── state.rs            AppState managed by Tauri (phase, notice, window store, …)
    │   ├── shell.rs            app windows (main + app-<n>): data store, guards, UA, capability, zoom
    │   ├── menu.rs             the app menu and its actions (BRAI-362)
    │   ├── popup.rs            bundled windows: build / show / dismiss, caller check
    │   ├── hotkeys.rs          global shortcut registration (action registry)
    │   ├── capture.rs          quick-capture commands, queue flush, open_quick_capture(prefill)
    │   ├── offline.rs          offline mode: files per server, sync, the offline window's commands
    │   ├── switcher.rs         quick-switcher commands (search, open, copy snippet, timer)
    │   ├── voice/              voice capture (BRAI-439): mod.rs glue + indicator commands,
    │   │                       recorder.rs (state machine, pure), model.rs (download / verify /
    │   │                       remove), audio.rs (cpal microphone, resampling), whisper.rs
    │   │                       (whisper-rs), permission.rs (macOS AVCaptureDevice)
    │   ├── tray.rs             tray menu model + icon, the poller, My Day / timer actions
    │   ├── downloads.rs        staging + Save As dialog, unique names
    │   ├── prefs.rs            Settings window commands, launch at login
    │   ├── flows.rs            startup, connect, sign-in, deep links, sign-out, switch, forget
    │   ├── commands.rs         commands of the bundled screens
    │   └── bridge.rs           brainitive_notify / _set_badge / _open_external (remote origin)
    └── tests/
        ├── live_network.rs     `--ignored` checks against real hosts (TLS errors, 404 detection)
        └── capabilities.rs     every bundled window's capability: local, own window, own commands
```

## Servers: Cloud or self-hosted

A server is identified by its **origin** (`ServerOrigin`, `server.rs`). The connect screen offers
**Brainitive Cloud** (`https://www.brainitive.app`, one click) and **Self-hosted server** (a URL
field). Normalisation of the typed address:

- trimmed; no scheme → `https://` (`http://` when the host is `localhost` / `127.0.0.1` / `[::1]`);
- path, query, fragment and trailing slashes dropped; host lowercased, IDN → punycode, default port dropped;
- `http` rejected except for those loopback hosts; `user:pass@`, other schemes, bare words rejected.

Then Rust calls `GET {origin}/api/v1/instance` (redirects: ≤ 5, each to an allowed origin; the
final origin is what gets used). The screen shows the name + version before continuing, and
these errors: not a Brainitive server, TLS problem (expired / unknown issuer / wrong host —
never auto-trusted), **server too old** (404 on `/api/v1/instance` but `/site.webmanifest` has
Brainitive's theme colour), **app too old** (`min_versions.desktop`). Debug builds also offer
the pre-release (`dev.brainitive.app`) and local (`localhost:8000`) servers.

The host can be changed any time: **Server → Switch Server…** in the menu (and a "Switch server"
button on the sign-in / unreachable screens) returns to the choice. Up to **5 recent servers**
(name + URL) are remembered for one-click switching. Each server has **its own keychain entry**
(account = origin) and **its own webview data store**, so switching does not sign you out of the
other one. **Forget** (two clicks, recent list) signs that device out on the server, deletes its
keychain entry and its webview data, and removes it from the list. The window title and the
Server menu show the current server ("Brainitive — Cloud", "Brainitive — brainitive.example.com").

## Webview data per server

The main window is created with a data store derived from the origin
(`ServerOrigin::store_id`, 16 bytes of sha256): `data_store_identifier` on macOS 14+,
`data_directory` (`<app local data>/webview/<hex>`) on Windows / Linux. A WebView's store is
fixed at creation, so entering another server **rebuilds** the main window (`shell::open_main`:
destroy → `Destroyed` event → build; `ExitRequested` is prevented meanwhile; the window-state
plugin restores size/position by label). The bundled screens use whatever store the window has;
before any server is chosen a neutral "local" store is used. Forgotten stores are deleted once no
window uses them (`settings.pending_wipes`, retried at start-up).

## Flows (`flows.rs`)

- **Start**: current server + keychain token → `Connecting` → `GET /api/v1/instance` **and** the
  refresh → web-session in parallel → main window on `/app/session/<code>` → `/dashboard`. No token
  → sign-in screen.
  - **Unreachable** (BRAI-367): the failure is classified by `problem.rs`: no network, DNS, refused,
    timeout, TLS (self-signed / expired / wrong host / other), HTTP 5xx / 408 / 429, not a Brainitive
    server, redirect not followed. The screen shows a title, what it means and a suggested fix, with
    Retry now / Open offline (the webview cache and service worker) / Switch server. Transient
    problems retry by themselves (`flows::auto_reconnect`): 5 s, 10 s, 20 s, 40 s, then every 60 s,
    with a visible countdown; for DNS / no-network problems a local name lookup every 3 s retries the
    moment it starts to resolve, and a wake from sleep retries after a 10 s grace. Certificate and
    wrong-address problems are not retried by themselves. One loop at a time (`reconnect_gen`).
  - **Update required**: `min_versions.desktop` above this app (`version::meets_minimum`, semver with
    pre-release tags, `v` prefix and build metadata tolerated) → phase `update_required`: server name,
    both versions, the download link (opened from Rust), "Check again". The web app is not loaded, and
    "Open offline" refuses. The same screen replaces the old error in `use_server` and `start_sign_in`.
    While the app runs the tray poller asks again (`flows::recheck_minimum`, scheduled by the pure
    `version::Recheck`): about every 6 hours after a successful poll, at once after a wake from
    sleep, 15 min after a failed try; a server that raised its minimum meanwhile gets this screen.
- **Sign in**: PKCE + state → system browser at `{origin}{authorize_path}` → `brainitive://auth/callback`
  → state check → exchange → refresh token to the keychain → web-session → app.
- **Session lost in the webview** (`/login` reached): a fresh web session from the stored sign-in,
  back to the last page (`?to=`). Twice within 30 s → sign-in screen.
- **Deep links** (BRAI-356): `brainitive://open?url=<path or URL on the current origin>` focuses the
  app and navigates (signed out → after sign-in); `brainitive://auth/callback` finishes a sign-in.
  A second launch focuses the running window (single-instance; on Windows / Linux it also carries
  the link).
- **Sign out** (Server menu, or the web app's "Sign out" via `brainitive_sign_out`): `POST /auth/logout`, delete the keychain entry, clear this server's
  webview data.

## Remote content and the bridge

See [SECURITY.md](./SECURITY.md). In short: the current origin gets a runtime capability
(`Manager::add_capability`, `remote.urls = ["<origin>/*"]`) for exactly `brainitive_notify {title, body, url?}`,
`brainitive_set_badge {count}`, `brainitive_open_external {url}`, `brainitive_sign_out {}` (called by the web app's
`resources/js/lib/desktop.ts` via `window.__TAURI_INTERNALS__.invoke`). The navigation guard keeps
the main frame on the bundled screens and the current origin. The user agent ends with
` BrainitiveDesktop/<version>`.

## Windows

| Label | Shows | Data store | IPC |
|---|---|---|---|
| `main` | bundled screens, then the server | the server's own (`store_id`) | local screens: `capabilities/local.json`; server origin: the 4 bridge commands |
| `app-<n>` | a page of the current server (File → New Window, a same-origin `window.open` / `target=_blank`, attachments) | the main window's (same cookies) | the 4 bridge commands (notify / badge are ignored from these: no duplicates) |
| `capture` | `capture.html` | default | `capabilities/capture.json` |
| `switcher` | `switcher.html` | default | `capabilities/switcher.json` |
| `settings` | `settings.html` | default | `capabilities/settings.json` |
| `offline` | `offline.html` (BRAI-366) | default | `capabilities/offline.json` |
| `voice` | `voice.html` (BRAI-439) | default | `capabilities/voice.json` |

App windows share one navigation / page-load guard, downloads handler, user agent and zoom level;
switching server, signing out or forgetting closes the `app-<n>` windows. The bundled windows are
built on first use (from the async runtime: never inside a main-thread handler, which can deadlock
on Windows), hidden instead of closed, centred, and the floating ones (capture, switcher) hide when
they lose focus (the voice indicator floats too but stays: a recording goes on while the user
clicks elsewhere). A bundled window opened from another one that has the focus (voice → quick
capture) inherits where the focus goes back to. Dismissing one with Esc gives the focus back to the app the user came from (macOS:
`app.hide()` when another app was in front). The window-state plugin keeps app windows only.

## Logs and diagnostics (BRAI-368)

`logging::init` runs first in `setup()`: a `log` backend writes to `brainitive.log` in the app log
dir (macOS `~/Library/Logs/<identifier>/`, Windows/Linux the platform log/data dir), rotated at 2 MB
into `.1`–`.4` (5 files, mode 0600). Our own modules log at info (debug in debug builds), other
crates at warn. A panic hook logs the message, location and up to 40 backtrace lines before the
`panic = "abort"` of the release profile ends the process. Script errors of the bundled screens
arrive through `log_client_error` (main window, ≤ 20/min). Every line passes `redact.rs` first.
Help → **Copy Diagnostics** puts version, OS, webview version, the server **origin**, signed in
yes/no, the screen and the last 50 redacted lines on the clipboard (Rust side); **Open Logs Folder**
opens the directory. Nothing is ever sent anywhere.

## Menus and shortcuts (BRAI-362)

`menu.rs`: Brainitive (macOS) / File: Settings… ⌘, · New Issue ⌘N · New Doc ⌥⌘N · New Window ⇧⌘N
· Quick Capture / Quick Switcher (their global shortcut shown) · Close Window ⌘W · Quit; Edit;
View: Back ⌘[ · Forward ⌘] · Reload ⌘R · Actual Size ⌘0 · Zoom In ⌘= · Zoom Out ⌘-; Go: Search… ⌘K ·
Dashboard ⇧⌘H · My Work ⇧⌘M · Captures · Notifications · Account Settings; Server; Window (macOS);
Help. `CmdOrCtrl` = Ctrl on Windows / Linux. Actions go to the app window focused last.
New issue / doc / search are web commands: the page first gets a cancelable
`brainitive-desktop:command` event (`detail.command` = `new-issue` | `new-doc` | `search`); when
nothing claims it, the web app's own ⌘. (palette, commands) / ⌘K (search) handler runs.

## Global shortcuts, quick capture, quick switcher (BRAI-359 / BRAI-401)

`tauri-plugin-global-shortcut`, Rust side only. Each shortcut is a `hotkeys::HotkeyAction`
(registry: shortcut id → action) with an accelerator in the preferences: quick capture ⇧⌘Space /
Ctrl+Shift+Space, quick switcher ⌥⌘K / Ctrl+Alt+K, voice capture ⇧⌘V / Ctrl+Shift+V (the only one
that also gets the key-up). Settings records new keys (the current
shortcuts are paused while recording); `shortcuts.rs` refuses one-modifier shortcuts (except
F-keys), system combinations and duplicates; the OS refusing a shortcut keeps the previous ones
and the reason shows in Settings.

- **Quick capture**: `capture_submit` → `POST /api/v1/captures {text, url?, intended_type?,
  source: "Brainitive Desktop", request_id}`. Offline / 5xx / 429 → `capture-queue.json` (app data
  dir, per origin, ≤ 500), sent oldest first by the poller, after sign-in and after the next
  successful capture, with the same `request_id` (the server answers 200 for a repeat). 4xx →
  dropped with a notification. Sign out / forget deletes the origin's queue.
  `capture::open_quick_capture(app, prefill)` opens the window with text (voice capture's result,
  added to what is already typed).
- **Quick switcher**: `GET /api/v1/search?q=&limit=12[&type=issues|docs|plans|snippets]`; Enter →
  `shell::navigate_to` (the app window, or after the stored sign-in is restored); ⌘C on a snippet →
  `GET /snippets/{id}` → clipboard (`tauri-plugin-clipboard-manager`, Rust side); ⌘T on an issue →
  `POST /me/timer/start`.

## Voice capture (BRAI-439)

Hold a global shortcut (default ⇧⌘V / Ctrl+Shift+V, `HotkeyAction::Voice`, rebindable or off;
**not final**: it may move under a "Brainitive key" chord), speak, let go: the speech becomes text
on this computer and opens in quick capture (`capture::open_quick_capture(app, Some(text))`, added to
what is already typed) to check and save as usual. Also **Voice Capture…** in the File menu, the
tray and a microphone button in quick capture (those start a recording that runs until Enter /
Done / the shortcut again).

- **Hold vs. tap**: `tauri-plugin-global-shortcut` (`global-hotkey` 0.8) reports the key-up on
  macOS (Carbon `kEventHotKeyReleased`), Windows (the key is polled every 50 ms after `WM_HOTKEY`)
  and X11 (detectable auto-repeat). `HotkeyAction::wants_release` passes it to `voice::on_hotkey`
  only. Held ≥ 0.5 s → the release finishes; a shorter tap latches toggle mode.
- **State machine** (`voice/recorder.rs`, pure): idle → recording → transcribing → done /
  cancelled / error; Esc cancels (also while transcribing: the late result is dropped by session
  id); the recording is cut off and transcribed at 2 minutes.
- **Audio** (`voice/audio.rs`): `cpal` default input on its own thread (the stream is not `Send`),
  mixed to mono f32 **in memory only** (≤ 2 min), wiped after use; a meter level per block;
  `resample_16k` (two box filters as a low-pass, then linear interpolation).
- **Transcription** (`voice/whisper.rs`): `whisper-rs =0.16.0` (whisper.cpp built from source by
  CMake; Metal on macOS, CPU elsewhere). The model is loaded on a background thread as soon as the
  microphone opens, so after the release only the transcription runs (another background thread);
  greedy decoding, the language from Settings or auto-detect. Clips < 0.3 s or without any sound
  above −40 dBFS are not transcribed (whisper invents text on silence); segments that are only a
  marker ("[BLANK_AUDIO]", "(music)") are dropped. The model is dropped after each recording.
  Timing is logged (`voice: 4.8 s recorded, model load 140 ms, resample 1 ms, transcription 210 ms
  (metal)`), never audio or text. whisper.cpp / ggml output goes to the `log` crate (warn+ only).
- **Model** (`voice/model.rs`): whisper.cpp's multilingual **small q5_1** (190 MB), downloaded on
  first use from a pinned commit of `huggingface.co/ggerganov/whisper.cpp` into
  `<app local data>/voice/ggml-small-q5_1.bin`, via `.part` with a running SHA-256; only the exact
  size + checksum is renamed into place. Redirects only to Hugging Face hosts over https (the
  file is served by their CDN). Partial files and old models are deleted at start; **Remove voice
  model** in Settings deletes the folder (and stops a download).
- **Permission** (`voice/permission.rs`): macOS `AVCaptureDevice` status / request (objc2), the
  prompt text is `NSMicrophoneUsageDescription` in `src-tauri/Info.plist`; Windows reports a
  blocked microphone as the stream's `PermissionDenied`; denied → the indicator says how to allow
  it and opens the OS privacy page (fixed URLs).
- **Off by default** (`Preferences.voice_enabled`); the shortcut is registered **only while it is
  on** (`Preferences::active_voice_shortcut`; turning it on / off re-registers via
  `voice::enabled_changed`), so it never takes ⇧⌘V / Ctrl+Shift+V from other apps while off. The File
  menu, the tray and quick capture's mic button show what it does and **Turn on** (asks for the microphone, downloads the model with
  progress). Settings: on / off, the shortcut, the language (auto, English, German, Albanian, …),
  the model and Remove.
- **Indicator** (`voice.html`, window `voice`, always on top, frameless): intro, permission,
  denied, downloading, ready, recording (level meter, timer up to 2:00, Done / Cancel), transcribing,
  errors. Rust sends it `voice://view` (the screen) and `voice://level` (level + elapsed, every
  80 ms), to that window only; it calls `voice_view`, `voice_start`, `voice_stop`, `voice_cancel`,
  `voice_turn_on`, `voice_download`, `voice_open_privacy`, `voice_close`.
- **Build**: whisper.cpp needs CMake and a C++ compiler (and libclang for bindgen, with a bundled
  fallback); `GGML_NATIVE=OFF` (`src-tauri/.cargo/config.toml`) keeps release builds portable
  (Apple Silicon baseline M1; x86-64 AVX2 + FMA + F16C).

## Tray / menu bar (BRAI-360) and badge (BRAI-361)

A native menu (`tray.rs`, no webview): server line, Quick Capture, Quick Switcher, My Day (check
items → `PATCH /me/my-day/items/{id}`), the running timer (Stop and Log Time → `POST /me/timer/stop`
with the default activity when one is required; Discard), unread notifications (→ `/notifications`),
Open Brainitive, Settings…, Quit. The menu is a pure model (`tray::model`) rebuilt only when it
changes. The poller (`tray::start_poller`, `poll.rs`) runs while the phase is `InApp`: unread count
+ timer every 60 s, My Day every 5th poll or after a change; errors back off 2 → 4 → 8 → 15 min;
the monotonic sleep pauses during system sleep and a wake waits 10 s for the network; every GET
sends `If-None-Match` when the last answer had an ETag. The poller also sets the badge and flushes
the capture queue. Badge: macOS dock, Linux Unity launcher, Windows taskbar overlay icon
(`badge.rs`); macOS also shows the count next to the menu-bar icon. "Keep running in the menu bar /
tray" (default on): closing the main window hides it; the dock / tray brings it back.

## Offline mode (BRAI-366)

**The web's own offline mode** (BRAI-82: `public/sw.js` serves a cached `/offline` shell that reads
IndexedDB; opt-in per browser profile, "Work offline on this device") **works inside WKWebView**:
probed on macOS 26 with a `WKWebsiteDataStore(forIdentifier:)` store on the Cloud origin and on
`http://localhost:8000` (`navigator.serviceWorker` present, `/sw.js` registers, activates and controls
a reload, Cache API + IndexedDB work). Each server's data store is its own profile, so a user who
turns it on in the desktop app gets it there ("Use the web app offline"). WebView2 supports service
workers; WebKitGTK depends on the build (untested).

**The native offline view** covers everyone else (the web's mode is off by default), a cold start
without network and the tray. Rust keeps a copy of the user's own data per server and user, and the
bundled `offline` window shows it:

- **Cache** (`offline_cache.rs`, `<app local data>/offline/<store id>/cache.json`): records `work`,
  `task_list`, `task`, `capture`, `my_day` from `GET /api/v1/sync/changes` (sync v2). No cursor → a
  full run (`scopes=work,task_lists,my_day,captures`, `task_lists=mine` — the server resolves the
  lists the user works in and answers which it keeps (`task_lists`) —, `captures=inbox`,
  `limit=100`; the same filters go with a cursor, so an old v1 cursor moves over): pages while
  `has_more`, then everything the run didn't send goes and the last `next_cursor` is kept. Later:
  delta runs (upserts by `updated_at`, the overlap of 10 s is harmless; `op: delete` tombstones
  drop; a `task_list` delete drops its tasks; new lists come in full with `reset: false`). After
  every page, records of projects not in `project_ids` drop (lost access). Only the newest My Day is
  kept. `task_list` records carry `can.manage_items`: false hides "Add task". ≤ 200 pages per sync.
- **Outbox** (`outbox.rs`, `outbox.json` next to it, ≤ 1000): `work.done` (with `base.fields.done`),
  `my_day.add`, `list_task.create`, `list_task.update` (`base.fields` title / status),
  `capture.create`, each with its own `request_id`. Unsent changes fold together (tick + untick =
  nothing; an edit of an unsent new task changes its create); a change that went out once is never
  edited, the next one is based on its value. Sent in order, ≤ 50 per `POST /api/v1/sync`; per
  result: applied / conflict → done (a conflict is listed with mine / theirs: the server's value
  stayed), deleted / forbidden / invalid → dropped and listed **with the user's text**, waiting →
  kept (dropped after 5 tries), rate_limited / error → kept, stop. A batch refused with a 4xx is
  sent one by one; a single change refused is dropped and listed. Nothing 4xx is retried forever.
  An applied / conflict result carries `entity` + `record_type` (the feed's record): it replaces the
  cached one (`entity: null` keeps the local patch until the next pull).
- **Sync** (`offline.rs`): `GET /me` once per session — another account's cache / outbox are deleted
  first — then the outbox, then the feed. Runs after every tray poll while in the app, on **Sync now**
  (tray, window; when the server answers while the unreachable screen is up, the app reconnects too)
  and right after each change made in the window. One at a time (a request during a run runs it once
  more); a sign-out during a run stops it from writing anything back (`epoch`). Files are written
  outside the state lock, newest generation wins, 0600, atomic.
- **Status**: the window title gets " (offline)" (unreachable screen, or the poller can't reach the
  server); the tray shows "Offline — N changes will sync" / "N changes waiting to sync", **Open
  Offline View…** and **Sync Now**; a notification says when queued changes were sent and when some
  could not be applied (click → the offline window).
- **Deleted** on sign-out, forget, and when another account signs in on the same server. Switching
  servers keeps each server's files.

## Downloads and files (BRAI-363)

`downloads.rs`: with "Ask where to save" (default on) a download goes to
`<app cache>/downloads/<uuid>/<name>`; when complete a Save As dialog (`tauri-plugin-dialog`) moves
it, cancel deletes it; the staging folder is cleared at start. Off: Downloads folder, unique name.
A notification click reveals the file. Tauri's drag & drop handler is disabled on every window, so
OS file drops reach the page; `file:` navigations stay blocked.

## Launch at login (BRAI-364)

`tauri-plugin-autostart` (LaunchAgent / registry / autostart file) behind Settings, off by
default; the OS login item is read back as the truth. The login item passes `--autostart`; with
"Start in the menu bar / tray" the main window stays hidden until the user opens it.

## Tests

`cargo test` runs the pure logic (server URL rules, PKCE incl. the RFC 7636 vector, deep links,
navigation decisions incl. the bundled windows', versions, settings / preferences / zoom, token /
error parsing, API request paths, call-error classification, shortcut rules and labels, the capture
queue, the voice recorder state machine, model download verification / removal, resampling and the
meter, transcript clean-up, polling backoff / wake / plan, tray menu model and API parsing, switcher result parsing,
download names / moves, the overlay badge, menu ids / accelerators) and `tests/capabilities.rs`
(capability scoping). `cargo test --test live_network -- --ignored`
checks discovery against real hosts (badssl.com certificate errors, Cloud 404 detection).
`BRAINITIVE_VOICE_MODEL_DIR=<dir> cargo test --lib real_transcription -- --ignored --nocapture`
downloads the real model once into `<dir>` and transcribes clips spoken by macOS `say` (Metal and
CPU timings).
