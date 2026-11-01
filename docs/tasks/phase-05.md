# Phase 05: native desktop experience

Cards (board `desktop-app`): BRAI-362, BRAI-359, BRAI-360, BRAI-361, BRAI-363, BRAI-364, BRAI-401.
Built in this order, one commit (or more) per card on `main`.

## Shared foundation (lands with BRAI-362 / BRAI-359)

- **Preferences** in `settings.json` (`settings::Preferences`, serde defaults so old files still load):
  capture / switcher shortcut, launch at login, start hidden, keep running in the tray when the window
  closes, ask where to save downloads, zoom.
- **Authenticated API calls** (`api::ApiClient::request` + `Session::request`): bearer token from
  `Session`, one refresh + one retry on 401, `If-None-Match` / 304 when an ETag is known. Errors keep
  their HTTP status (`remote::CallError`) so the capture queue can tell "retry later" from "rejected".
- **Bundled windows** (`popup.rs`): `capture`, `switcher`, `settings` load `capture.html`,
  `switcher.html`, `settings.html` (Vite multi-page). Built lazily, hidden instead of closed, a
  navigation guard that keeps them on the bundled origin, no `window.open`. Each has its **own
  capability file** (`capabilities/<label>.json`, `windows: [<label>]`, `local: true`) with only its
  commands, and every command also checks the caller's label + local origin in Rust.

## BRAI-362: menus, shortcuts, window state, multi-window

- Full menu: app menu (macOS) / File (Win/Linux) with Settings… (⌘, / Ctrl+,) and Quit; File: New
  Issue (⌘N), New Doc (⌥⌘N / Ctrl+Alt+N), New Window (⇧⌘N = the current page in a new window), Quick
  Capture…, Quick Switcher…, Close Window (⌘W); Edit (predefined); View: Back (⌘[), Forward (⌘]),
  Reload (⌘R), Actual Size (⌘0), Zoom In (⌘=), Zoom Out (⌘-), Full Screen; Go: Search… (⌘K),
  Dashboard, My Work, Captures, Notifications; Server (existing); Window (macOS); Help.
- Actions go to the last focused **app window** (main or `app-<n>`). New issue / doc / search: a
  `brainitive-desktop:command` DOM event first (the web can claim it later), else the web's own ⌘K /
  ⌘. palette handler (no web URL exists to create an issue or doc: API gap).
- Zoom is one global level (persisted), applied to every app window.
- **Multi-window**: `app-<n>` windows load a page of the current server with the **same data store**,
  the same navigation / page-load guard, downloads handler, user agent and bridge capability
  (`remote.urls = [origin]`, windows `main` + `app-*`). Same-origin `window.open` / `target=_blank`
  now opens such a window. Switching server, signing out or forgetting closes them.
- Window state: the window-state plugin keeps main / app windows; popups are denylisted.
- Tests: menu accelerators parse, menu-id → action mapping, app-window label rules, zoom clamp.

## BRAI-359: global quick capture

- `tauri-plugin-global-shortcut` (Rust side only, no JS permission). Default ⇧⌘Space /
  Ctrl+Shift+Space, rebindable / off in Settings; validated (`shortcuts.rs`: needs Ctrl / Alt / Cmd,
  not a reserved system combo, both shortcuts different).
- Capture window (always on top, no decorations, hides on blur / Esc): text, optional link, type;
  `capture_submit` → `POST /api/v1/captures` (`request_id` uuid, `source` "Brainitive Desktop").
- Offline / server error → `capture-queue.json` (app data dir, per origin), flushed by the poller and
  after every successful capture with the same `request_id` (the API replays, no duplicates).
  Rejected (4xx) entries are dropped with a notification. Sign out / forget deletes that origin's queue.
- Tests: shortcut parsing / validation / labels, queue push / cap / flush classification / persistence.

## BRAI-360: tray / menu bar

- Native tray menu (no webview): server + account line, Quick Capture, Quick Switcher, My Day (items as
  checkboxes → `PATCH /me/my-day/items/{id}`), running timer (label + Stop and log / Discard), unread
  notifications (opens `/notifications`), Open Brainitive, Settings…, Quit.
- Poller: every 60 s while signed in (phase `InApp`): unread count + timer; My Day every 5th tick
  or after a change. Errors back off (60 s → 15 min); a wake from sleep waits a short grace first.
  `If-None-Match` whenever an endpoint returns an ETag.
- Close-to-tray (preference, default on): closing the main window hides it; dock click / tray reopens.
- Tests: backoff, wake detection, tray menu ids, label formatting.

## BRAI-361: notifications + badge (finish)

- Badge also from the poller's unread count (works while the web app is closed / offline).
- Windows: overlay icon with the count (`set_overlay_icon`, rendered in Rust); macOS tray title shows
  the count; tooltip everywhere.
- Notification clicks: pages (existing) and downloaded files (reveal in Finder / Explorer).
- Tests: overlay bitmap rendering.

## BRAI-363: files

- Downloads go to a staging folder, then a native **Save As** dialog (`tauri-plugin-dialog`, Rust
  side) moves them; cancel deletes the staged file. Preference off → straight to Downloads (old way).
- OS drag & drop: Tauri's drop handler stays disabled on every app window so HTML5 drops reach the
  page; a file dropped outside a drop zone (`file:` navigation) stays blocked.
- Attachments: same-origin new-tab links open in an app window; other origins in the browser.
- Tests: staging names, move fallback.

## BRAI-364: launch at login

- `tauri-plugin-autostart` (LaunchAgent on macOS), `--minimized` arg; Settings toggle (off by default)
  + "start hidden" toggle. The OS state is the truth for the toggle.

## BRAI-401: quick switcher

- Second global shortcut (default ⌥⌘K / Ctrl+Alt+K) → bundled switcher window: `GET /search`
  (debounced, `limit=12`, optional `type=snippets` filter) → Enter jumps to the page in the main
  window; on a snippet ⌘C / button copies its content (`GET /snippets/{id}` → clipboard in Rust);
  on an issue ⌘T starts the timer (`POST /me/timer/start`).

## Docs at the end

ARCHITECTURE.md, SECURITY.md (threat model: bundled windows, global shortcuts, autostart, queue on
disk, clipboard), AGENTS.md, CHANGELOG (Unreleased), SPIKE matrix rows.
