# Changelog

Brainitive Desktop has its own version numbers (0.x until Desktop 1.0). It needs a Brainitive
server with the App API v1 (Brainitive v1.49.0 or later).

## Unreleased

Phase 09, reliability and diagnostics:

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
