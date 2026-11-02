# Changelog

Brainitive Desktop has its own version numbers (0.x until Desktop 1.0). It needs a Brainitive
server with the App API v1 (Brainitive v1.49.0 or later).

## Unreleased

New task:

- **Create a task from any app**: press **⌃⌘T** (macOS) / **Ctrl+Alt+T** (Windows), or the
  Brainitive key and **T**. A small window asks for the project (type to search; it starts with the
  last project you used on this server), the board (when the project has several), the title, an
  optional description in Markdown, the priority and the type. **⌘↩ / Ctrl+↩** creates it; then
  **Open** shows it in Brainitive, or **New another**. Problems show next to the field (a missing
  title, a plan limit, a board you can no longer use).
- **Offline** (or when the server has a problem) you can **Save to Inbox instead**: the title and
  description go to your inbox (sent when you're back) and you turn them into a task later.
- **Linux**: no shortcut by default, because Ctrl+Alt+T opens the terminal on Ubuntu; set one in
  Settings → Shortcuts. **Updating** gives you the new shortcut only if none of your shortcuts
  uses those keys already; otherwise it starts off. If you had given **T** to another letter, New
  task gets **N** (or the first free letter).
- Also in the tray menu (**New Task…**) and the menu-bar panel's footer (**New Task**).

Menu-bar panel:

- **Click the menu-bar / tray icon** for a panel like the macOS Weather extra: frosted glass right
  under the icon (Windows: above the tray; Linux: tray menu → Open Panel…). **Today** (next events,
  work due or overdue), **My Day** and **Assigned** (grouped by project; tick to finish),
  **Inbox** (latest notifications, unread badge, open + mark read, Mark all read) and **Timer**
  (live clock, Stop & log with an inline activity choice, Discard, start a timer on one of your
  issues, today's entries and total). ⌘1–5, arrow keys, Esc; it remembers the last tab.
- **Right-click** (or ⌃-click) the icon for the menu with Quit, Settings and the rest.
- **Offline** the panel shows what was loaded last (or the offline copy); ticks wait in the offline
  outbox and sync later; timer and mark-read wait for the connection.
- The tray's timer now counts from when it really started.

Title bars (macOS 26):

- Settings, the offline view and the app windows no longer show scrolled content behind the
  see-through title bar of macOS 26 (Tahoe); the title bar matches the page in light and dark.

Brainitive key (BRAI-440):

- **One key, then a letter**: press **⌃⌘K** (macOS) / **Ctrl+Alt+K** (Windows, Linux) in any app and
  a small panel lists what comes next: **C** quick capture, **K** quick switcher (so ⌃⌘K K),
  **V** voice capture — hold V to talk, let go to finish — and **O** to bring Brainitive to the
  front. Esc or clicking elsewhere closes it. Only the key itself is taken
  from other apps; the letters are read by the panel. Ctrl+Alt+K types no AltGr character on German
  and most European layouts.
- **Settings → Shortcuts**: change or turn off the Brainitive key, pick another letter (A–Z) for
  each action, and set **direct shortcuts** if you prefer one combination per action. New
  defaults: quick capture ⇧⌘Space stays on, the quick switcher and voice capture have no direct
  shortcut until you set one (⌥⌘K and ⇧⌘V are suggested with **Default**; on Windows / Linux the
  switcher suggestion is now Ctrl+Alt+Shift+K).
- **Updating keeps your choices**: shortcuts you recorded yourself (or turned off) stay as they
  were; only the old untouched defaults of the switcher (⌥⌘K / Ctrl+Alt+K) and voice capture
  (⇧⌘V / Ctrl+Shift+V) are switched off. If you had recorded ⌃⌘K / Ctrl+Alt+K for something else,
  the Brainitive key starts off.
- Settings names the clash ("Quick capture already uses ⇧⌘Space.") and shows when the system or
  another app refuses a key; one refused shortcut no longer keeps the others from working.
- **Brainitive Key…** in the File menu and the tray (with the key next to it).

Security review (BRAI-377):

- **Links can't sign you in or out**: a `brainitive://open` link, a web notification or a server link
  in the tray, menus or switcher never opens the server's sign-in, sign-out, session, password, OAuth
  or API pages, however the address is encoded.
- **Foreign pages are sent back sooner**: when the app window starts showing another website (macOS,
  Linux), the app returns to your last page right away and opens the site in your browser.
- **Downloads are marked as downloaded**: saved files carry macOS's quarantine flag (Gatekeeper checks
  them on open) and Windows' Mark of the Web; file names lose hidden right-to-left characters,
  trailing dots and Windows device names (`CON`, `NUL`, …).
- **Safer server address check**: a server may no longer redirect the check from https to plain http
  (also not to `localhost`); when it answers at another address than the one you typed, the app
  shows that address and asks before using it.
- **Signing out leaves nothing behind**: a running voice recording is cancelled, a transcript waiting
  for Quick Capture is dropped, and the Quick Capture, Quick Switcher and offline windows are
  cleared. Tray data that arrives after a sign-out or server switch is ignored.
- **Updates**: changing the update channel during a download discards that download.
- Pages may open at most 10 links per minute in the browser; quick captures waiting to be sent, the
  offline copy and the downloaded update are created readable by your user only (folders included).
- **CI**: the update signing key no longer reaches the build (a separate step signs the update
  bundles), Apple / Azure secrets go only to the step that uses them, signing runs only for release
  tags (or a manual run on the default branch), and the Windows signing tool is pinned.

Auto-updates (BRAI-373, ready; switched on once the owner adds the update signing key):

- **Updates install themselves — when you say so**: the app checks 30 seconds after it starts and
  every 6 hours, downloads a new version in the background and tells you "Update ready — restart to
  install". It installs only when you choose **Restart to Update** (app menu, tray, Settings).
  "Check for Updates…" is in the app menu (macOS) / Help menu (Windows, Linux) and the tray.
- **Signed updates only**: every update is checked against the signing key built into the app and
  must be signed for exactly the version it claims; older or equal versions are never installed.
  Downloads come from GitHub over https only.
- **Beta channel**: Settings → Updates → "Get beta updates" for pre-release versions; stable is the
  default. Settings also shows the version, the last check and lets you turn automatic checks off.
- **"Please update" screen**: when your server needs a newer app, "Check for update now" fetches it
  right there.
- Builds without the signing key (and development builds) never check or install updates; the menu
  item is greyed out and Settings says why.

Build and distribution (phase 10, prepared; needs the owner's accounts to sign):

- **CI build** (`.github/workflows/build.yml`): macOS universal `.dmg`, Windows x64 NSIS installer,
  Linux `.deb` + AppImage on tags `v*` (draft release with `SHA256SUMS`) or by hand; tests only on pull
  requests. Signing steps run only when their secrets exist.
- **Signing config, inactive until the secrets are added**: macOS hardened runtime with
  `entitlements.plist` (microphone, network), minimum macOS 14; notarization through Apple ID or an API key;
  Windows through Azure Trusted Signing. Per-user NSIS install, WebView2 bootstrapper, `.deb` depends
  on ALSA. Step by step in `docs/RELEASING.md`.

Also:

- **"Please update" while the app is running**: the server's minimum desktop version is checked
  again about every 6 hours and after the computer wakes up, not only when connecting.

Sync v2 (BRAI-441):

- **Offline copy follows the server's v2 feed**: the lists you work in come from the server
  (`task_lists=mine`), only your inbox is kept (`captures=inbox`), and a deleted list takes its
  tasks with it. What the server answers to a change you made offline replaces the stored copy at
  once. Viewers no longer see "Add task" offline.

Voice capture (BRAI-439):

- **Speak instead of typing**: hold ⇧⌘V (macOS) / Ctrl+Shift+V (Windows, Linux), talk, let go — the
  text opens in quick capture to check and save. A short tap starts a recording that runs until you
  press the shortcut again or ↩. Also **Voice Capture…** in the File menu, the menu bar / tray and a
  microphone button in quick capture. A small indicator shows the level and the time (max 2
  minutes); Esc cancels.
- **On this computer**: the speech is turned into text locally with Whisper (whisper.cpp; Metal on
  Apple Silicon). The audio is kept in memory only — never saved, never sent anywhere — so it works
  the same for Cloud and self-hosted, offline too.
- **Off by default.** While it is off the shortcut is not registered (⇧⌘V / Ctrl+Shift+V stay
  "paste as plain text" in your other apps); File → Voice Capture…, the tray and quick capture's
  microphone button explain it and offer **Turn on**; the speech model (190 MB)
  is downloaded once, with progress and a checksum check. Settings → Voice capture: on / off, the
  shortcut, the language (detect automatically, English, German, Albanian, …) and **Remove voice
  model**.
- macOS asks for the microphone once; if it is blocked, the indicator says how to allow it and
  opens the privacy settings.

Offline mode (BRAI-366):

- **Work offline**: when the server can't be reached, **Open offline** (it also opens by itself once)
  shows this computer's copy of My Day, your open work, the task lists of your tasks and your capture
  inbox. Tick work done or undone, add work to My Day, add, tick and rename tasks, capture ideas —
  the changes wait on this computer and are sent in order when the server is back, never twice.
- Honest results: a change someone else made meanwhile wins and yours is shown next to it (copy your
  text); deleted, not allowed or not accepted changes are listed with your text instead of vanishing.
- "Offline — changes will sync" in the window title and the tray, **Sync Now** and **Open Offline
  View…** in the tray, a notification when queued changes were sent.
- The copy is kept per server and account, refreshed every minute while you're online, and deleted
  when you sign out, forget the server or another account signs in. The web app's own offline mode
  (if you turned it on) still opens with "Use the web app offline".

Phase 09, reliability and diagnostics:

- **Performance pass, desktop side** (BRAI-369): start-up phases are timed in the log
  (`startup: bundled shell loaded +…ms`, `remote page loaded`); the download-staging clean-up no
  longer blocks `setup()`. `docs/PERFORMANCE.md` has the procedure, budgets and suggestions for the
  web app.
- **Logs and diagnostics** (BRAI-368): Rust panics, app log output and script errors of the bundled
  screens go to a rotating local file (5 × 2 MB) with tokens, cookies, codes and query strings
  removed. Help → **Copy Diagnostics** (versions, OS, webview, server, signed in yes/no, last 50 log
  lines) and **Open Logs Folder**. No telemetry; nothing is sent.
- **Unreachable screen and version gate** (BRAI-367): "can't reach the server" now says why — no
  network, address not found, connection refused, no answer, self-signed / expired / wrong-host
  certificate (never bypassed), server error (HTTP 5xx), not a Brainitive server — with a suggested
  fix. It retries by itself (5 s → 60 s backoff, at once when the network is back or after sleep);
  Open offline keeps working. A server that needs a newer desktop app shows "Please update
  Brainitive Desktop" (server, both versions, download link) and the web app is not loaded.

Phase 05, native desktop experience:

- **Menus and shortcuts** (BRAI-362): full app menu with ⌘ on macOS / Ctrl on Windows and Linux:
  Settings (⌘,), New Issue (⌘N), New Doc (⌥⌘N), New Window (⇧⌘N), Back / Forward (⌘[ / ⌘]), Reload,
  zoom (⌘0 / ⌘= / ⌘-, remembered), Search (⌘K), Dashboard, My Work, Captures, Notifications,
  Account Settings. **Open in a new window**: pages of the server in extra windows with the same
  sign-in and the same security rules (also for links that open a new tab, e.g. attachments).
- **Quick capture** (BRAI-359): ⇧⌘Space / Ctrl+Shift+Space from any app opens a small capture
  window; captures go to your private inbox, or wait on this computer while you're offline and are
  sent later (never twice).
- **Menu bar / tray** (BRAI-360): quick capture, quick switcher, today's My Day (tick work done),
  the running timer (stop and log, or discard), unread notifications, Open, Settings, Quit. The app
  can keep running there when the window is closed (on by default).
- **Notifications and badge** (BRAI-361): the unread badge also on the Windows taskbar (overlay
  icon) and kept up to date while the window is closed; clicking a notification opens its page.
- **Files** (BRAI-363): downloads ask where to save them (native Save dialog, can be turned off);
  drag & drop of files from Finder / Explorer into the app keeps working; attachments open in an
  app window.
- **Launch at login** (BRAI-364): optional, off by default; can start in the menu bar / tray.
- **Quick switcher** (BRAI-401): ⌥⌘K / Ctrl+Alt+K opens a Spotlight-like search over the server:
  jump to an issue, doc or plan, copy a snippet, start the timer on an issue.
- **Settings window**: global shortcuts (record, default, off), tray, launch at login, downloads,
  zoom, account link.

## 0.1.0 (unreleased)

- Tauri 2 shell: the bundled connect / sign-in screens, then the server's web app in the main window.
- Connect **Brainitive Cloud** or a **self-hosted server** (address normalised, `https` only except
  localhost), checked with `GET /api/v1/instance` (name, version, minimum app version, TLS errors).
- **Switch Server…** in the menu, the last 5 servers for one-click switching, **Forget** per server;
  one keychain entry and one webview data store per server; the current server in the window title.
- Sign-in in the system browser (PKCE S256 + state) with the `brainitive://auth/callback` deep link;
  access token in memory, refresh token in the OS keychain, serialized refreshes; web session for
  the webview; sign out.
- Security model: the server's origin gets IPC for three bridge commands only (notify, badge, open
  external), navigation guard (other sites open in the browser), CSP for the bundled screens.
- Deep links `brainitive://open?url=…` and single instance.
- Native notifications (click opens the page), dock badge, downloads to the Downloads folder.
