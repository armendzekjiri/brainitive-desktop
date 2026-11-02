# Architecture

Brainitive Desktop is a **Tauri 2** shell: a main window that shows either a few bundled
screens (connect a server, sign in) or the user's Brainitive server — the full web app, loaded
remotely — plus extra app windows with pages of that server, and small **bundled native windows**
(quick capture, quick switcher, New task, settings, the offline view, the voice-capture indicator) that talk to the server only through Rust. The native
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
│   ├── panel.ts, panel.css     the menu-bar panel (panel.html, window "panel")
│   ├── task.ts, task.css       the New task window (task.html, window "task")
│   └── native.ts/.css, popup.css   shared by the native windows
├── public/logo.png
└── src-tauri/
    ├── tauri.conf.json         id app.brainitive.desktop, CSP, deep-link scheme, macOS ≥ 14
    ├── tauri.msstore.conf.json Microsoft Store overlay (BRAI-442): no updater config, no bundle
    ├── msix/AppxManifest.xml.tmpl  the Store package manifest (identity, protocol, startup task)
    ├── Info.plist              merged by Tauri: NSMicrophoneUsageDescription (voice capture)
    ├── .cargo/config.toml      GGML_NATIVE=OFF: a portable whisper.cpp build
    ├── build.rs                app ACL manifest (every command needs a permission)
    ├── capabilities/           one file per bundled window, local origin + its own window only:
    │                           local.json (main), settings.json, capture.json, switcher.json,
    │                           offline.json, voice.json, leader.json, panel.json, task.json
    ├── src/
    │   ├── lib.rs              plugins, commands, menu, deep-link + notification wiring, run loop
    │   ├── server.rs           ServerOrigin: typed address → origin, TLS rule, store ids   (pure)
    │   ├── servers.rs          saved servers: ids, default names, order, ⌃⌘1…9 / Alt+1…9  (pure)
    │   ├── links.rs            navigation + page-load decisions, deep-link parsing and target
    │   │                       guard (no auth pages), bundled origin per OS, URL validation (pure)
    │   ├── private_fs.rs       0600 files / 0700 folders, atomic writes (queue, offline, update)
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
    │   ├── leader.rs           the Brainitive key: letters, overlay commands       (pure + glue)
    │   ├── capture.rs          quick-capture commands, queue flush, open_quick_capture(prefill)
    │   ├── offline.rs          offline mode: files per server, sync, the offline window's commands
    │   ├── switcher.rs         quick-switcher commands (search, open, copy snippet, timer)
    │   ├── voice/              voice capture (BRAI-439): mod.rs glue + indicator commands,
    │   │                       recorder.rs (state machine, pure), model.rs (download / verify /
    │   │                       remove), audio.rs (cpal microphone, resampling), whisper.rs
    │   │                       (whisper-rs), permission.rs (macOS AVCaptureDevice)
    │   ├── tray.rs             tray menu model + icon, the poller, My Day / timer actions
    │   ├── new_task/           the New task window: mod.rs (state, commands), model.rs (destinations,
    │   │                       config, memory per server, payload, request id, errors, pure), tests.rs
    │   ├── panel/              the menu-bar panel: mod.rs (state, commands, tray clicks, window),
    │   │                       geometry.rs (where it opens, pure), view.rs (tabs' mapping, ids,
    │   │                       offline decisions, pure), tests.rs
    │   ├── updater.rs          auto-updates (BRAI-373): channels, manifest URLs, GitHub-only
    │   │                       client, key guard, schedule, labels (pure) + check / download /
    │   │                       Restart to Update with tauri-plugin-updater
    │   ├── downloads.rs        staging + Save As dialog, unique safe names, quarantine flag
    │   ├── prefs.rs            Settings window commands, launch at login
    │   ├── msstore/            the Microsoft Store build (BRAI-442): identity, Store URL, UA marks,
    │   │                       startup-task decision, AppData → package copy (mod.rs, pure);
    │   │                       packaged.rs: WinRT StartupTask / package / toast (Windows + msstore)
    │   ├── flows.rs            startup, connect, sign-in, deep links, sign-out, switch, forget
    │   ├── commands.rs         commands of the bundled screens
    │   └── bridge.rs           brainitive_notify / _set_badge / _open_external (remote origin)
    └── tests/
        ├── live_network.rs     `--ignored` checks against real hosts (TLS errors, 404 detection)
        ├── msix_manifest.rs    the MSIX template = MICROSOFT-STORE-LISTING.md = msstore constants
        └── capabilities.rs     every bundled window's capability: local, own window, own commands
scripts/
    ├── build-msix.ps1          Store variant → package folder → makepri → makeappx (BRAI-442)
    ├── msix-manifest.mjs       renders the MSIX manifest (X.Y.Z → X.Y.Z.0)
    └── msix-assets.mjs         the MSIX logos from src-tauri/icons/icon.png
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

The host can be changed any time: **Server → Switch Server…** / **Servers ▸ Add Server…** (and a
"Switch server" button on the sign-in / unreachable screens) returns to the choice, whose "Back to
…" goes back without a new sign-in. Each server has **its own keychain entry** (account = origin)
and **its own webview data store**, so switching does not sign you out of the other one.
**Forget** (two clicks) signs that device out on the server, deletes its keychain entry and its
webview data, and removes it from the list.

### Saved servers and fast switching

- **List** (`settings.recent`, the JSON key from when it was a recent list): the user's order,
  up to `servers::MAX_SERVERS` = 20 (a 21st is refused, nothing is dropped silently). `remember`
  keeps a saved server's place and appends a new one; `touch` (a switch) only sets current + last
  used.
- **Names** (`RecentServer::label`): `custom_name` (Settings, `servers::clean_name`: no control /
  bidi characters, ≤ 60) else `servers::default_name`: "Brainitive Cloud" for the cloud, the
  instance's own name (`/api/v1/instance`, refreshed by every instance check; the stock
  "Brainitive" counts as none), else the host. Shown in the window title ("Brainitive — Work"),
  the Server menu line, Servers ▸, the tray, the panel header, the Choose screen, quick capture,
  the switcher, New task and the offline view.
- **Ids**: commands name a server by `servers::server_id` (16 hex characters of its store id),
  looked up in the list; menu ids `server-pick:<id>`, tray ids `tray-server:<id>`.
- **Switch** (`flows::switch_to`, from Servers ▸ / ⌃⌘1…9, the tray, the panel's switcher,
  Settings, the Choose screen): already current → nothing (or a restore from a bundled screen);
  else `leave_server(old)` → `Settings::touch(new)` → `servers_changed` (title, Servers menu, tray,
  panel) → `restore(new)` (instance gate, web session from the stored sign-in, `enter_app` →
  `open_main` rebuilds the main window on the new server's store; no / refused token → its
  sign-in screen). `use_server` (Choose screen) runs `leave_server` too.
- **`leave_server`**: the old server's outbox gets one bounded try (`offline::send_before_leaving`,
  ≤ 3 s, only while in the app there, online and with waiting changes, outbox only, to the old
  origin); then the voice recording is cancelled, the account windows (capture, switcher, offline,
  panel, New task) destroyed, and `drop_account_state`: `account_epoch` + 1, panel and New-task
  answers, tray data, ETags, capture prefill, instance, last page, and `OfflineData::unload`.
  Kept: keychain entry, webview store, offline files, capture queue (per origin) and New-task
  memory (per origin). The `app-<n>` windows close (`show_local` / `open_main`): guard, bridge
  check and store all follow the current origin.
- A restore whose server is no longer current when its answers arrive shows nothing
  (`restore_inner`), so two quick switches end on the last one.
- **Sign out** of a server that is not current (Settings): device signed out on the server,
  keychain entry, capture queue, offline files; its store is scheduled like Forget's
  (`Settings::schedule_wipe`) — it stays in the list. Forgetting a server that is not current
  leaves the app where it is.
- Only the current server is polled (tray, badge, panel); see the follow-up ideas in
  `docs/tasks/server-switching.md`.

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
  the link). Targets go through `links::resolve_deep_link_target` (also web notification URLs,
  notification clicks, tray / menu / switcher links): never `/app/*`, `/login*`, `/logout*`,
  `/register*`, `/password/*`, `/auth/*`, `/oauth/*`, `/api/*` and the other auth pages, compared
  after decoding and normalising the path (BRAI-377). Only the app's own web-session step opens
  `/app/session/<code>` (`resolve_on_instance`).
- **Connect**: `/api/v1/instance` follows ≤ 5 redirects, never https → http (`api::redirect_refusal`);
  when the server answers at another origin than the one entered, the confirm screen says so, and
  `use_server` refuses an origin that changed since it was confirmed (BRAI-377).
- **Sign out** (Server menu, or the web app's "Sign out" via `brainitive_sign_out`): `POST /auth/logout`, delete the keychain entry, clear this server's
  webview data; also (`flows::clear_account_traces`, BRAI-377) cancel a voice recording, drop a
  waiting transcript and destroy the quick-capture / switcher / offline windows. The account epoch
  (`Inner::account_epoch`) changes, so a tray poll still running writes nothing.

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
| `leader` | `leader.html` (BRAI-440, the Brainitive key overlay) | default | `capabilities/leader.json` |
| `panel` | `panel.html` (the menu-bar panel) | default | `capabilities/panel.json` |
| `task` | `task.html` (the New task window) | default | `capabilities/task.json` |

App windows share one navigation / page-load guard, downloads handler, user agent and zoom level;
switching server, signing out or forgetting closes the `app-<n>` windows. The bundled windows are
built on first use (from the async runtime: never inside a main-thread handler, which can deadlock
on Windows), hidden instead of closed, centred, and the floating ones (capture, switcher, New task, the
Brainitive key overlay) hide when they lose focus (the voice indicator floats too but stays: a recording goes on while the user
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

`menu.rs`: Brainitive (macOS) / File: Settings… ⌘, · **Servers ▸** (the saved servers as check
items, current checked, ⌃⌘1…⌃⌘9 on macOS / Alt+1…Alt+9 on Windows / Linux for the first nine —
menu accelerators only, never global; Ctrl+Alt+digit would be AltGr+digit on Windows, which types
`{ [ ] }` on many layouts, and the web editor uses Ctrl+Alt+1–6 / Ctrl+Shift+7–9; then Add
Server…, Manage Servers…; rebuilt by `menu::refresh_servers`; Settings refuses a global shortcut on
those keys) · New Issue ⌘N · New Doc ⌥⌘N · New Window ⇧⌘N
· Brainitive Key… / Quick Capture / Quick Switcher / Voice Capture (their global shortcut shown; New task is in the tray and the panel, not the File menu, whose ⌘N is the web's New Issue) · Close Window ⌘W · Quit; Edit;
View: Back ⌘[ · Forward ⌘] · Reload ⌘R · Actual Size ⌘0 · Zoom In ⌘= · Zoom Out ⌘-; Go: Search… ⌘K ·
Dashboard ⇧⌘H · My Work ⇧⌘M · Captures · Notifications · Account Settings; Server; Window (macOS);
Help. `CmdOrCtrl` = Ctrl on Windows / Linux. Actions go to the app window focused last.
New issue / doc / search are web commands: the page first gets a cancelable
`brainitive-desktop:command` event (`detail.command` = `new-issue` | `new-doc` | `search`); when
nothing claims it, the web app's own ⌘. (palette, commands) / ⌘K (search) handler runs.

## Global shortcuts, quick capture, quick switcher (BRAI-359 / BRAI-401)

`tauri-plugin-global-shortcut`, Rust side only. Each shortcut is a `hotkeys::HotkeyAction`
(registry: shortcut id → action) with an accelerator in the preferences: the **Brainitive key**
⌃⌘K / Ctrl+Alt+K (`Leader`, below), quick capture ⇧⌘Space / Ctrl+Shift+Space, **New task**
⌃⌘T / Ctrl+Alt+T (`Task`; off on Linux, where Ctrl+Alt+T opens the terminal: `shortcuts::DEFAULT_TASK`,
and `shortcuts::parse` refuses Ctrl+Alt+T on Linux only), and the optional
direct shortcuts quick switcher and voice capture (off by default since BRAI-440; **Default** sets
⌥⌘K / Ctrl+Alt+Shift+K and ⇧⌘V / Ctrl+Shift+V; voice is the only one that also gets the key-up).
Settings records new keys (the current shortcuts are paused while recording); `shortcuts.rs`
refuses one-modifier shortcuts (except F-keys), system combinations and duplicates
(`hotkeys::conflict` names the other one). `hotkeys::register` tries every shortcut, so one taken
by another app doesn't keep the others away; a change in Settings is rolled back only when its own
shortcut was refused, and the reason shows in Settings.

### The Brainitive key (BRAI-440)

One global shortcut, then one letter. `HotkeyAction::Leader` opens the overlay (`leader.html`,
window `leader`, floating, hides on blur, `capabilities/leader.json`); the overlay reads the next
key **in its own window** (no letter is registered globally) and sends it to `leader_press`; Rust
maps it (`Preferences.leader_letters`, default C / T / K / V / O, A–Z, unique, `leader.rs` pure) to
quick capture, New task, the quick switcher, voice capture or `shell::focus_main`. A file written
before New task gets T, or N / the first free letter when T is already another action's. The new window is shown
while the overlay still has the focus, so it inherits where the focus returns. It stays open until a letter, Esc
(`leader_close`, which ignores a hidden overlay) or a click elsewhere (blur) closes it. The key
pressed again while the overlay is in front counts as its own letter (⌃⌘K K → switcher), else
closes it.

- **Hold V**: key-down → `voice::start_from_leader` (hold mode, `release_letter` kept); the
  indicator takes the focus, so the key-up arrives in whichever window has it: the overlay
  (`leader_release`) or the indicator (`voice_release`; `VoiceView.release_key` tells it which
  key). Both feed `Input::HotkeyUp`; the same 0.5 s tap / hold rule applies. The press / release
  commands are synchronous so a quick tap's key-up can't overtake its key-down. While voice
  capture is off, V is greyed out and refused with a hint.
- **Letters**: the page sends what the layout types (`KeyboardEvent.key`, so Z / Y follow QWERTZ),
  else the key position (`code`) when modifiers change the character.
- **Preferences**: `leader_shortcut`, `leader_letters` (loaded leniently: a broken entry falls back
  to the defaults, never loses the file), `shortcut_scheme`. `Preferences::migrate` (scheme 0 → 1,
  at load, then saved) turns off only the **literal** old defaults (`CommandOrControl+Alt+K`,
  `CommandOrControl+Shift+KeyV`; recorded keys are stored canonical, e.g. `alt+super+KeyK`), keeps
  explicit Off, and starts the leader Off when a kept direct shortcut already uses its keys.
  Scheme 1 → 2 (New task): a file without `task_shortcut` reads the default, which starts Off when
  the Brainitive key or a direct shortcut already uses those keys.
- **Settings → Shortcuts**: the key (Change / Default / Off), a letter field + Default per action
  (`settings_set_leader_letter`), then "Direct shortcuts (optional)". Menus: **Brainitive Key…** in
  File and the tray (`tray::add_leader_entry`, only while the key is on), the key as its
  accelerator (display only). The voice indicator says "Press ⌃⌘K, then hold V" when no direct
  shortcut is set (`leader::voice_how_to`).

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

## New task window

`HotkeyAction::Task` (⌃⌘T / Ctrl+Alt+T, Linux off), the Brainitive key's **T**, the tray's **New
Task…** (under Quick Capture, `tray::add_task_entry`) and the panel footer open the `task` window
(`task.html`, `new_task/`, floating, centred, hides on blur with the text kept, destroyed on
sign-out). Pressed while it is open, the shortcut focuses it (no toggle). The height follows the
page (`task_resize`, 180…620 pt, the top stays put).

- **Where to**: `GET /api/v1/capture-destinations` — the projects with at least one board the user
  may write to, every workspace in one call (`model::parse_destinations`). Kept per server +
  `account_epoch` (`TaskState.kept`, with the answer's ETag), shown at once on open and asked again
  with `If-None-Match` on every open (304 → the kept list and configs stay); a server without ETags
  is asked again only when the list is older than 60 s (`model::stale`).
  The project field is a searchable select (name, key, workspace; grouped by workspace when there
  are several). Board only when the project has more than one.
- **Starting choice**: `Preferences.new_task` (origin → `{project_id, board_id, type_id, at}`, ≤ 10
  servers, lenient load, removed on Forget) — the last project, its last board, the last type when
  the project has it; else the first project / board and the project's default type. The server
  keeps no "recent destinations" (`/me/navigation`'s recent list lives on the device).
- **Project config**: `GET /api/v1/projects/{id}/config` → `types` (shown when there are any) and
  `priorities` (chips: None + the project's), kept per project until the list is reloaded.
- **Create**: `POST /api/v1/boards/{id}/items` `{title (trimmed), description? (Markdown, only when
  not empty), priority_id?, type_id?, request_id}` with `ApiRequest::error_body()` so a 4xx comes
  back with its body. `model::draft` takes only ids of the kept answers. **request_id**: one per
  create; when the call got no answer (offline, 5xx) the same input reuses it
  (`model::request_id_for`, `TaskState.pending`), so the server answers 200 with the issue it may
  already have made; another input or any answer → a new one. 201/200 → the created state (key,
  title, **Open** / **New another**), the memory is saved, the issue `url` stays in Rust
  (`TaskState.created`) for `task_open` → `shell::open_page`.
- **Errors** (`model::error_answer` / `call_error`): 422 per field (`title`, `description`,
  `type_id` → type, `priority_id` → priority, others → the form), 402 the server's message, 403, 404
  (the list is reloaded), 429 / offline / 5xx → "Save to Inbox instead" (`task_save_to_inbox` →
  `capture::save`: the title and description as a capture with `intended_type: issue`, queued in
  `capture-queue.json` when offline). Signed out / no server → "Sign in to Brainitive" / "Connect
  to Brainitive" + Open Brainitive.
- **Offline issues**: on a server with the client-gaps sync types, a create whose request finds no
  server goes to the outbox as `issue.create {board_id, title, description?, item_type_id?,
  priority_id?}` (`model::offline_change`; the REST `request_id` is not reused, the outbox change
  has its own) and the window shows "Task saved — created once Brainitive is back online"
  (`CreatedView.pending`, nothing to open yet). An older server, or a full outbox, keeps **Save to
  Inbox instead**. The created issue isn't put into the offline copy (it isn't assigned work by
  itself; the feed brings it when it is).

## Voice capture (BRAI-439)

Press the Brainitive key and hold V (BRAI-440), or hold the optional direct shortcut (off by
default, ⇧⌘V / Ctrl+Shift+V when set, `HotkeyAction::Voice`), speak, let go: the speech becomes text
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
  `voice_turn_on`, `voice_download`, `voice_open_privacy`, `voice_close`, `voice_release` (the held
  Brainitive-key letter went up in this window).
- **Build**: whisper.cpp needs CMake and a C++ compiler (and libclang for bindgen, with a bundled
  fallback); `GGML_NATIVE=OFF` (`src-tauri/.cargo/config.toml`) keeps release builds portable
  (Apple Silicon baseline M1; x86-64 AVX2 + FMA + F16C).

## Title bars (macOS 26)

Tauri's default macOS title-bar style (`Visible`) adds `NSWindowStyleMask::FullSizeContentView`, so
the webview spans the title bar; macOS 26 (Tahoe) draws that title bar see-through, and scrolled
content showed behind the window title. Every decorated window (`shell::build_app_window`,
`popup::build` for Settings / Offline) uses `shell::TITLE_BAR` = `TitleBarStyle::Transparent`: no
full-size content view, the webview starts below the title bar (checked with an AppKit probe on
macOS 26.2: webview 640 pt in a 672 pt window, no overlap). The title bar shows the window's
background colour: the system's for app windows (nothing is injected into the web app), the top
colour of the page for Settings (`--bg`) and the offline view (`--panel`), light / dark
(`popup::title_bar_color`, again on `WindowEvent::ThemeChanged`). Floating windows are borderless.
Windows / Linux: unchanged (macOS-only API).

## Menu-bar panel

Left click on the tray icon toggles the **panel** (`panel/`, window `panel`, `panel.html`); right
click (macOS also ⌃-click, `NSEvent.modifierFlags`, then `TrayIcon::show_menu` from a blocking task)
opens the native menu below. macOS reacts on mouse-down like the system's extras, Windows on
mouse-up; a click within 0.4 s after the panel hid on blur is the click that blurred it
(`panel::toggle_decision`). Linux (AppIndicator: no click events): **Open Panel…** in the menu.

- **Window**: `Popup::Panel` (floating, hides on blur, holds account data → destroyed on sign-out).
  macOS: transparent window + `Effect::Popover` (`EffectState::Active`, radius 12: the system's
  NSVisualEffectView, follows light / dark), the page has no background. Tauri 2.12.5 always
  enables the transparent-window private API (`macos-private-api` is a no-op since 2.12.1), so no
  `macOSPrivateApi` flag. Windows / Linux: solid page with a border + shadow.
- **Placement** (`panel::geometry`, pure): the screen under the icon (else the nearest), its work
  area; the bar's edge from where the icon lies outside the work area (top / bottom / left /
  right; inside it, e.g. an auto-hidden bar: the nearest edge); under / above / beside the icon,
  centred on it, clamped with 6 pt to the bar and 8 pt to the other edges. macOS in points (each
  monitor's physical rect / its scale), Windows / Linux in physical pixels. No icon rect → the
  bar's corner (top-right macOS / Linux, bottom-right Windows). Width 360, height from the page
  (`panel_resize`, 220…560), re-placed so the bar side stays put.
- **Tabs** (`panel::view`, pure mapping): **Today** `GET /me/today` (ETag via the poller's ETag
  cache: events not over yet, all-day first, ≤ 8; open work due today / overdue, ≤ 30), **My Day**
  `GET /me/my-day` (ETag, also updates the tray), **Assigned** `GET /me/work?status=open&limit=100`
  (ETag) grouped by project, **Inbox** `GET /notifications?limit=25` (ETag on the first page) +
  `unread-count`: each row's text is the server's `summary` and its link the server's `url`
  (`null` = no link); a row without those fields (older server) is worded like the web bell
  (`view::describe`, mirrors `NotificationMessage.tsx`) with the payload's link. **Timer**
  `GET /me/timer` (ETag; elapsed from `started_at`; `timer.description` under the issue); today's
  total = `today_logged_seconds` + the running time (ticking) — an older server without the field
  gets `GET /work-logs?from=D&to=D` (D = the user's date of `/me/today`) with the entries; start
  candidates = open issues of My Day + Assigned (offline without a live answer: the offline copy's),
  with an optional "What are you working on?" (`description`, newer servers). Last tab:
  `Preferences.panel_tab`. ETags: `tray::get_cached_request` (key = server + path + query; a server
  without ETags is asked in full, nothing kept), the decision is the pure `api::revalidate`.
- **Data**: `PanelState.kept` holds the last answers for one server + sign-in (`account_epoch`;
  another one drops them, a write after a switch is dropped by `PollTicket`). Opening shows the
  kept data at once, then refreshes (answers < 15 s old are kept). Changes: `PATCH /me/work/{id}`,
  `POST /notifications/{id}/read` (on open), `/notifications/read-all`, `/me/timer/start|stop|discard`;
  they update the tray data / badge, and the page reloads on `panel://changed` (sent by
  `tray::refresh`). Stop: activity = picked one (of the timer's `activities`), else the default,
  else the page shows the choice; a 422 shows it too (timer keeps running).
- **Offline** (server unreachable / not in the app): the kept data, else the offline copy (My Day,
  assigned work, Today = due work without events). Done ticks → the outbox (`work.done`,
  `offline::queue_work_done`); a tick whose request finds no server goes there too. On a server
  with the client-gaps sync types (`offline::extended_sync`, below) timer start / stop / discard,
  opening a notification (read) and **Mark all read** go to the outbox too (`timer.start` with the
  device clock, `timer.stop {stopped_at}`, `timer.discard`, `notification.read`,
  `notifications.read_all {before}` with `before` = the newest `created_at` the panel showed,
  `view::read_all_before`), also when the request finds no server (`queue_instead`). The page shows
  them at once (`view::TimerOverlay`, `view::PendingReads`): a timer started offline runs with a
  "Waiting to sync" badge (discard takes the unsent start back; stop waits for its sync, it needs
  the server's id), a stopped / discarded one goes with a note, read rows and the unread count
  follow. A sync result's live state (`record_type` `timer` / `notification` / `unread_count`)
  updates the tray, badge and panel (`panel::apply_sync_states`): a start refused because another
  timer runs (`conflict`) shows that timer and lists a problem; a stop refused for want of an
  activity (`invalid` + `activities`) is kept as `Outbox.activity_needed` and the Timer tab shows the
  inline choice (while that timer still runs); a pick resends the stop with `activity_id` and the
  same `stopped_at`. On an older server (flag false / unknown) these stay disabled with the hint.
  Header dot: `view::sync_status`.
- **Signed out / no server**: "Sign in to Brainitive" / "Connect to Brainitive" + Open Brainitive,
  and "Switch to <name>" for the other saved servers.
- **Server switcher**: the header's name is a button ("Work ▾") with the saved servers
  (`Head.servers`: ids, names, hosts, shortcuts) and Manage Servers…; a pick calls
  `panel_switch_server {id}` → `menu::pick_server` (the panel hides and is destroyed by the switch).

## Tray / menu bar (BRAI-360) and badge (BRAI-361)

A native menu (right click on the icon; left click opens the menu-bar panel above; `tray.rs`, no webview): server line, **Servers ▸** (the saved servers as check items, Add Server…, Manage Servers…; no shortcuts), Brainitive Key… (while the key is on), Quick
Capture, New Task…, Quick Switcher, Voice Capture, My Day (check
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
  `capture.create`, and from the panel / New task window (servers with the client-gaps types only):
  `timer.start`, `timer.stop`, `timer.discard`, `notification.read`, `notifications.read_all`,
  `issue.create` (+ description, type) — each with its own `request_id`. A read twice is one; a
  later "mark all read" moves the unsent one's `before` forward. Unsent changes fold together (tick + untick =
  nothing; an edit of an unsent new task changes its create); a change that went out once is never
  edited, the next one is based on its value. Sent in order, ≤ 50 per `POST /api/v1/sync`; per
  result: applied / conflict → done (a conflict is listed with mine / theirs: the server's value
  stayed), deleted / forbidden / invalid → dropped and listed **with the user's text**, waiting →
  kept (dropped after 5 tries), rate_limited / error → kept, stop. A batch refused with a 4xx is
  sent one by one; a single change refused is dropped and listed. Nothing 4xx is retried forever.
  An applied / conflict result carries `entity` + `record_type` (the feed's record): it replaces the
  cached one (`entity: null` keeps the local patch until the next pull). `gone` (a timer that no
  longer runs, a deleted notification) is dropped without a problem (logged); `timer`,
  `notification` and `unread_count` records go to the panel / tray, not the cache.
- **What the server takes** (`Cache.extended_sync`): `today_logged_seconds` in a `GET /me/timer`
  answer (tray poll or panel, `offline::note_timer_answer`) means the server has the client-gaps
  sync types (they came in the same server release; `/instance`'s `version` can't tell a build
  before it from one after). Kept in `cache.json` so it is known offline; `None` = not known = no.
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
  servers keeps each server's files; the old server's data is unloaded (`OfflineData::unload`) and
  loading another server (`ensure`) bumps `epoch` and `load`, so a sync of the old server that is
  still running stops at its next step (it can never take the new server's outbox to the old
  origin) and leaves `syncing` / `last_error` / `online` of the new one alone.

## Downloads and files (BRAI-363)

`downloads.rs`: with "Ask where to save" (default on) a download goes to
`<app cache>/downloads/<uuid>/<name>`; when complete a Save As dialog (`tauri-plugin-dialog`) moves
it, cancel deletes it; the staging folder is cleared at start. Off: Downloads folder, unique name.
Every saved file gets `com.apple.quarantine` (macOS, `0081;<hex time>;Brainitive;<UUID>`) /
`Zone.Identifier` (Windows) (`mark_downloaded`, BRAI-377); `clean_name` drops bidi / format
characters, trailing dots and spaces, and Windows device names.
A notification click reveals the file. Tauri's drag & drop handler is disabled on every window, so
OS file drops reach the page; `file:` navigations stay blocked.

## Launch at login (BRAI-364)

`tauri-plugin-autostart` (LaunchAgent / registry / autostart file) behind Settings, off by
default; the OS login item is read back as the truth. The login item passes `--autostart`; with
"Start in the menu bar / tray" the main window stays hidden until the user opens it.

The Microsoft Store build, packaged (BRAI-442): the plugin's Run key would be copied into the
package's private registry, which Windows never reads at login, so `prefs::launch_at_login` /
`set_launch_at_login` use the manifest's startup task `BrainitiveStartup` instead
(`msstore::packaged`: WinRT `StartupTask`, waited for on its own thread; `msstore::startup_step`
decides: enable / disable / nothing / refuse with a message when the user turned it off in
Task Manager or a policy decides). A startup task passes no arguments: "start hidden" reads the
activation kind (`AppInstance::GetActivatedEventArgs` = `StartupTask`).

## Build and distribution (phase 10)

`.github/workflows/build.yml`: matrix macOS-14 (`--target universal-apple-darwin`, app + dmg),
Windows 2022 (NSIS), Ubuntu 22.04 (deb + AppImage); each job runs `npm run build`, `cargo test`,
`cargo clippy -D warnings`, then `tauri build` (not on pull requests). Tags `v*` add a draft GitHub
release with `SHA256SUMS`. Signing is conditional: only for a `v*` tag push or a manual run on the
default branch (`SIGNING_ALLOWED`), and a detect step checks for the secrets (booleans only).
Secrets go through step `env:` only, never `$GITHUB_ENV` (BRAI-377): `tauri build` gets `APPLE_*`
on macOS (Tauri signs + notarizes; `signingIdentity` is null in the config; an API-key `.p8` is a
temp file deleted after the build) or `AZURE_*` on Windows, where a step writes the `signCommand`
overlay for Azure Trusted Signing (`trusted-signing-cli` 0.11.0, pinned) passed with `--config`.
Actions use exact tags marked `# TODO pin SHA` until pinned (release blocker, `docs/RELEASING.md`).
`bundle` config: `entitlements.plist` (audio-input, network client; no JIT), min macOS 14, NSIS
`currentUser` + WebView2 download bootstrapper, `.deb` depends `libasound2`. The `brainitive://`
scheme is registered by the installers / `.desktop` file from `plugins.deep-link`, and by
`register_all()` in debug builds. Owner steps: `docs/RELEASING.md`.

Updater bundles (BRAI-373): signing builds with the `TAURI_SIGNING_PRIVATE_KEY` secret (and a real
public key in the config) build without the key; the step "Updater signing" after `tauri build` (the
only one with the key, BRAI-377) packs the macOS `.app` into `Brainitive.app.tar.gz` and signs it /
the NSIS `-setup.exe` / the `.AppImage` and `.deb` with `tauri signer sign --app-version`. The ignored test `ci_signed_bundle_verifies_with_the_committed_key`
then checks one signed bundle with the plugin against the committed key. The release job writes
`latest.json` (`scripts/updater-manifest.mjs`: `darwin-{aarch64,x86_64}[-app]` → the universal
`.app.tar.gz`, `windows-x86_64[-nsis]`, `linux-x86_64[-appimage]`, `linux-x86_64-deb`) and makes
`v*-*` tags pre-releases. The job `beta-manifest` runs when a pre-release is published and copies its
`latest.json` to `beta.json` of the rolling `updater-beta` pre-release.

### The Microsoft Store package (BRAI-442)

Job `msstore` in `build.yml` (windows-latest; `v*` tags without a pre-release part and manual runs;
no secrets): `node --test scripts/`, `cargo test` / `clippy` with `--features msstore`, then
`scripts/build-msix.ps1 -SignForTest`: `tauri build --no-bundle --features msstore --config
src-tauri/tauri.msstore.conf.json` → `src-tauri/target/msix/package/` (`Brainitive.exe`, the
rendered `AppxManifest.xml`, `Assets/` from `scripts/msix-assets.mjs`, `resources.pri` from
`makepri`) → `makeappx pack` (schema-validated) → `Brainitive_<X.Y.Z>_x64.msix`, **unsigned** (the
Store signs it). It is uploaded as `installers-msstore`, so the draft release attaches it; the
self-signed sideload copy + `.cer` go to `msstore-sideload-test`; WACK runs when the image has
`appcert.exe` (informational, report artifact `msstore-wack-report`). `ci.yml` runs the msstore
clippy / tests on Windows for every PR.

What the `msstore` feature changes (`msstore/`; without it every function there is inert):

| | NSIS / other builds | Store build |
| --- | --- | --- |
| Updates | `tauri-plugin-updater` (BRAI-373) | plugin not registered, `plugins.updater` removed by the overlay; `Availability::Store`: "Check for Updates…", Settings → Updates ("Open Microsoft Store") and the "Please update" screen open `ms-windows-store://pdp/?productid=9N08DHPCF7WB` |
| User agent | ` BrainitiveDesktop/<v>`; API `(windows)` | ` BrainitiveDesktop/<v> (MSStore)`; API `(windows; MSStore)` |
| Open at login | autostart plugin (HKCU Run) | packaged: startup task `BrainitiveStartup` |
| Notifications | plugin, AUMID = identifier | packaged: `tauri-winrt-notification` with `Ndize.al.Brainitive_x9qbckpnnsjh8!Brainitive`; click → `bridge::on_notification_action` |
| Open Logs Folder | the folder | packaged: `msstore::outside_view` → `%LOCALAPPDATA%\Packages\<family>\LocalCache\Local\…` when that copy exists |
| `brainitive://` | HKCU from the NSIS installer | manifest `windows.protocol`, `Parameters="%1"`: the same single-argument launch the deep-link / single-instance plugins read |

Unchanged in a package: the data folders (same identifier; MSIX writes new files under AppData to
the package's LocalCache, which the app sees at the usual paths and Windows deletes on uninstall),
`private_fs` (the profile's ACLs cover the package copy too), WebView2 (its user data folder is in
that copy), the keyring (Credential Manager is not virtualized), downloads (a real folder the user
or Windows chose; staging → move falls back to copy + delete), global shortcuts, tray, the voice
model download (in the package copy) and the microphone (declared `microphone` capability).

## Auto-updates (BRAI-373)

`updater.rs`, `tauri-plugin-updater` from Rust only (no JS permission anywhere).
- **Guard** (`build_availability`): the Microsoft Store build is always `Store` (no plugin; every
  update action opens the Store page). Otherwise (`availability`) `plugins.updater.pubkey` must decode to a minisign Ed25519 public key
  (the committed placeholder doesn't) and the build must not be a development build; otherwise
  `init` logs why and nothing runs (menu item greyed, no tray line, Settings explains).
- **Channels** (`Channel`, `settings.preferences.update_channel`, default stable): stable reads
  `github.com/armendzekjiri/brainitive-desktop/releases/latest/download/latest.json`; beta reads
  `…/releases/download/updater-beta/beta.json` and the stable one, `find_update` keeps the highest
  offered version (an error only when no manifest answered).
- **Client** (`github_only`, through `configure_client`): https only, ≤ 5 redirects, each on
  `github.com` / `objects.githubusercontent.com` / `release-assets.githubusercontent.com`; the bundle
  URL is checked against the same list. Comparator: strictly newer only. `requireSignedVersion`.
- **Schedule** (`Schedule`, wall clock): 30 s after start, every 6 h, 1 h after a failure; a loop
  ticks every 30 s; skipped while a check runs or an update waits, or with automatic checks off
  (`auto_update_check`). The last answered check is `settings.last_update_check`.
- **Flow / status** (`Status`): Checking → UpToDate / Failed, or Downloading (progress in 10 % steps)
  → the plugin verifies → `<app cache>/update/pending.update` (0600) → Ready + notification "Update
  ready — restart to install". `restart_to_update` (user click only) installs off the main thread
  (signature checked again) and `request_restart()`s; Windows: the NSIS installer (passive) quits and
  restarts the app.
- **UI**: app menu item `check-for-updates` (macOS app menu under About, Help menu elsewhere) whose
  text follows `menu_item` ("Restart to Update (x.y.z)" when ready); the tray line above "Open
  Brainitive" (`tray::add_update_entry`); Settings → Updates (polls `settings_get` while busy); the
  "Please update" screen (minimum version gate) shows "Check for update now" / "Restart to Update"
  and follows `desktop://changed`. Notifications only for an automatic "ready" and for menu clicks.

## Tests

`cargo test` runs the pure logic (server URL rules, PKCE incl. the RFC 7636 vector, deep links,
navigation decisions incl. the bundled windows', versions, settings / preferences / zoom, token /
error parsing, API request paths, call-error classification, shortcut rules and labels, the
Brainitive key (letters, conflicts, migration of old defaults, labels per OS), the capture
queue, the voice recorder state machine, model download verification / removal, resampling and the
meter, transcript clean-up, polling backoff / wake / plan, tray menu model and API parsing, the menu-bar panel (placement per bar edge / monitor / clamping,
tray clicks, each tab's mapping, ids it may act on, offline decisions, the server's notification
summary / url vs. the older-server fallback, today's total, the waiting timer / read overlays,
`read_all`'s `before`, the activity choice of a refused stop), ETag reuse (`api::revalidate`, keys
with the query), the client-gaps outbox changes (payloads, folding, ok / conflict / invalid +
activities / gone), the older-server detection (`extended_sync_of`), saved servers (default
names, name cleaning, order / moves, the limit, ⌃⌘1…9 / Alt+1…9 vs. every menu, global and editor
shortcut, the switch dropping every in-memory account answer while queue / memory stay per origin,
sign-out vs. forget, a running sync stopped by a switch, per-server ETags, tray Servers ▸), New task (defaults per OS,
migration, letter T, memory per server, destinations / config parsing, payload, request id reuse,
error mapping, window height, the offline `issue.create`, ETag vs. 1-minute throttle), switcher result parsing,
download names / moves, the overlay badge, menu ids / accelerators, updater channels / URLs /
redirects / key guard / schedule / labels, and the updater plugin with a throwaway minisign key:
bad signature, foreign key, signed-version mismatch, equal / older versions, a loopback-served
manifest + download) and `tests/capabilities.rs`
(capability scoping). `cargo test --test live_network -- --ignored`
checks discovery against real hosts (badssl.com certificate errors, Cloud 404 detection).
`BRAINITIVE_VOICE_MODEL_DIR=<dir> cargo test --lib real_transcription -- --ignored --nocapture`
downloads the real model once into `<dir>` and transcribes clips spoken by macOS `say` (Metal and
CPU timings).

The Microsoft Store build (BRAI-442): `cargo test --features msstore` runs the same suite as that
build (the update guard is `Store`, the user agents carry `(MSStore)`, the "Please update" link is
the Store page); `msstore::tests` cover the startup-task decision, the AppData → package copy
mapping and the identity; `tests/msix_manifest.rs` and `node --test scripts/` check the manifest
template against `docs/MICROSOFT-STORE-LISTING.md`, the version mapping (`1.2.3` → `1.2.3.0`,
pre-releases refused) and the generated logos. The WinRT calls (`msstore/packaged.rs`) compile only
on Windows: `ci.yml` runs clippy / tests with `--features msstore` there.
