# Fast server switching

Switch quickly between several Brainitive servers (accounts), e.g. a company's self-hosted
instance for work and Brainitive Cloud for private. The pieces were there (keychain entry, webview
data store, offline files and capture queue per origin; a recent list; Switch Server… → Choose;
Forget); this makes switching fast and obvious. No server API change.

## Decisions

- **Saved servers** = `settings.recent` (JSON key kept for old files), now in the **user's order**
  (no more "most recent first"): an existing server keeps its place when used, a new one goes to
  the end. Up to **20** (was 5, silently truncated, which left the dropped server's keychain entry
  behind); a 21st is refused with "Forget a server first".
- **Names**: `RecentServer.custom_name` (rename in Settings, ≤ 60 characters, control characters
  dropped, empty = back to the default). Default (`servers::default_name`): **Brainitive Cloud** for
  the cloud origin; else the `/api/v1/instance` name when it has one of its own; else the host.
  The stock name "Brainitive" counts as none (a fresh install is called that, and two servers
  both called "Brainitive" can't be told apart) → the host. The instance name is refreshed on
  every connect. The name shows in the window title ("Brainitive — Work"), the Server menu line,
  the Servers submenu, the tray, the panel header and the Choose screen.
- **Ids**: commands take a server **id** = 16 hex characters of the origin's store id
  (`servers::server_id`), looked up in the saved list (unknown id → refused). Never an origin from a
  page; adding a server stays the Choose → check → confirm flow (`check_server` / `use_server`).
- **Menu**: Brainitive (macOS app menu) / File (Windows, Linux) → **Servers** ▸ one check item
  per saved server (current checked), then Add Server… and Manage Servers…. Rebuilt whenever the
  list, a name or the current server changes (`menu::refresh_servers`).
- **Shortcuts**: macOS **⌃⌘1…⌃⌘9** (`Control+Super+Digit<n>`), local menu accelerators only (no
  global registration). Free: the app menu uses ⌘ combos only (⌘0 / ⌘= / ⌘- zoom, ⌘[ / ⌘], ⌘K,
  ⇧⌘H / ⇧⌘M …), the web editor's (TipTap) headings are ⌥⌘1–6 and lists ⇧⌘7–9, the global
  defaults are ⌃⌘K / ⌃⌘T / ⇧⌘Space, and macOS uses ⌃1–9 (Spaces), not ⌃⌘digits.
  **Windows / Linux: Alt+1…Alt+9** instead of Ctrl+Alt+digit: Ctrl+Alt is AltGr on Windows, and
  AltGr+7/8/9/0/2/3/Q… type `{ [ ] } ² ³ @` on German and many European layouts — a menu
  accelerator there would make those characters impossible to type in the app; TipTap also maps
  Ctrl+Alt+1–6 (headings) and Ctrl+Shift+7–9 (lists) there. Alt+digit is used by neither the web app
  nor the OS (AltGr = Ctrl+Alt does not match a plain Alt accelerator). A test checks the nine
  against every menu accelerator and the global defaults on both patterns.
- **Tray menu**: a **Servers** submenu under the server line (same check items, Add Server…,
  Manage Servers…; no shortcuts).
- **Panel header**: the host becomes a button "Work ▾" with a small list (current checked, then
  Manage Servers…); picking one switches. The signed-out view lists the other servers too.
- **Settings → Servers**: per server the name (editable field), host, signed in yes / no (keychain),
  last used; **Switch to**, **Sign out** (keeps it in the list), **Forget** (two clicks, the
  existing flow), ↑ / ↓ (order = ⌃⌘1…9); **Add Server…** below. "Manage Servers…" opens Settings
  scrolled to this section.
- **The switch** (`flows::switch_to`): saved server only →
  1. the old server's **outbox** gets one bounded try (≤ 3 s, only when it has changes and the
     app is signed in there): `POST /api/v1/sync` to the **old** origin; whatever is left stays in
     that server's `outbox.json` and goes out the next time it is current. Never to another server:
     the offline data is unloaded and `OfflineData::ensure` bumps its epoch when the origin changes,
     so a sync of the old server that is still running stops at its next step (it used to keep
     going with the newly loaded server's outbox — a real cross-server leak, fixed here);
  2. in-memory account data of the old server goes (`flows::leave_server`): `account_epoch` + 1,
     panel answers, New task state, tray data, ETag cache (now keyed by origin + path), capture
     prefill, a voice recording, and the account windows (quick capture, switcher, offline, panel,
     New task) are destroyed — like sign-out, **but the keychain token, webview store, offline
     files and capture queue stay**;
  3. the server becomes current (`Settings::touch`: last used, order unchanged), title / menus /
     tray follow, badge 0;
  4. `flows::restore` — the existing logic: instance check (version gate, unreachable screen with
     Open offline), web session from the stored sign-in, `shell::enter_app` → `shell::open_main`
     rebuilds the main window on that server's data store. No token / refresh refused → its
     sign-in screen ("Sign in to Work").
  `use_server` (the Choose screen) runs the same `leave_server` when the server changes.
- **app-<n> windows of the old server are closed** (as today on every switch): the navigation
  guard, the bridge check and the store model all follow the *current* origin, so a window kept
  on the old server would have its own links treated as foreign and its bridge calls refused, and
  its store would count as unused (a Forget could wipe it under it).
- **Sign out** of a server that is not current: `POST /auth/logout` + keychain entry, its capture
  queue and offline files, and its webview store (scheduled like Forget's and deleted at once when
  unused); it stays in the list. The current one: the existing `flows::sign_out`.
- **Polling**: only the current server is polled (tray, badge, panel). Background polling of the
  others is a follow-up idea (below), not built.

## Steps

1. Plan (this file).
2. Pure: `servers.rs` (ids, default names, name cleaning, order moves, accelerators, choices);
   `settings.rs` (custom name, order-keeping `remember`, `touch`, limit 20).
3. Flows: `switch_to`, `leave_server`, `sign_out_server`; offline epoch / unload / bounded
   flush; ETag key per origin; names in title / tray / panel / Choose.
4. Menu Servers submenu + accelerators, tray submenu.
5. Commands + capabilities: `switch_to_server` (local.json), `panel_switch_server` (panel.json),
   `settings_servers` / `_rename_server` / `_move_server` / `_switch_server` / `_sign_out_server` /
   `_forget_server` / `_add_server` (settings.json); `ensure_caller` first in each; build.rs.
6. Pages: Settings → Servers, panel switcher, Choose screen names + "Back to …".
7. Docs: ARCHITECTURE, SECURITY, README, CHANGELOG, QA-CHECKLIST; gate; `tauri build --debug`.

## Tests (Rust)

Default names (cloud, instance name, stock name, empty, host with port), name cleaning; order
kept on use, new at the end, limit, moves up / down at the edges; accelerators 1–9 on both
patterns, none for the 10th, no clash with menu accelerators / global defaults; switch state
(`leave_server` on an `Inner`: panel answers, New task state, tray data, ETags, epoch, offline
unloaded and epoch bumped; capture queue and New-task memory stay per origin); sign-out vs.
forget on the settings (sign-out keeps the entry, forget removes it and its memory); capability
files (the new commands only in their window's file, every one checks its caller).

## Follow-up ideas

- Background polling of the other signed-in servers (unread count per server in the Servers
  menu / panel switcher, a badge sum), at a lower rate and with its own `PollTicket` per origin.
- Keep the old server's main webview alive (one hidden main window per server) for an instant
  switch back, at the cost of memory.

## Result

**Built**

- `servers.rs` (pure): ids, default names (Brainitive Cloud / the instance's own name, the stock
  "Brainitive" counts as none / the host), name cleaning (control + bidi characters, ≤ 60), up /
  down, ⌃⌘1…9 (macOS) / Alt+1…9 (Windows, Linux) accelerators, choices, `takes_server_keys`.
- `settings.rs`: `custom_name`, the user's order (`remember` keeps a saved server's place, appends
  a new one), `touch`, `update_instance`, `rename`, `schedule_wipe`, `label_of`, limit 20
  (`is_full_for`; `use_server` refuses a 21st).
- `flows.rs`: `switch_to`, `leave_server` (bounded outbox try, voice cancelled, account windows
  destroyed, `drop_account_state`), `servers_changed`, `sign_out_server` (not current: stays in the
  list), `use_server` leaves the old server too, a restore of a server that is no longer current
  shows nothing, the instance check refreshes the saved name, Forget of a server that is not
  current leaves the app where it is.
- `offline.rs`: `ensure` bumps `epoch` / `load` when the server changes, `unload`,
  `send_before_leaving` (≤ 3 s, outbox only); a sync of a previous load no longer touches the
  current one's state. **Bug fixed**: a sync running during a switch could take the newly loaded
  server's outbox and post it to the old origin. ETag cache keyed by origin + path.
- Menu: Brainitive (macOS) / File → **Servers ▸** (check items, accelerators, Add Server…, Manage
  Servers…), rebuilt by `menu::refresh_servers`. Tray: **Servers ▸** under the server line (no
  shortcuts). Names in the title, the Server menu line, tray, panel, quick capture, switcher, New
  task and offline view.
- Commands (ids only, caller checked first): `switch_to_server` (local.json),
  `panel_switch_server` (panel.json), `settings_servers` / `_rename_server` / `_move_server` /
  `_switch_server` / `_sign_out_server` / `_forget_server` / `_add_server` (settings.json); Settings
  refuses a global shortcut on a server key.
- Pages: Settings → Servers (name field, host, signed in, last used, shortcut, Switch to / Sign out
  / Forget, ↑ / ↓, Add Server…; Manage Servers… scrolls there); the panel header's switcher
  ("Work ▾", ↑↓ / Esc) and "Switch to …" links when signed out; the connect screen's "Your servers"
  (by name, switched by id) and "Back to <current>".

**Choices**

- Windows / Linux shortcuts are **Alt+1…9**, not Ctrl+Alt+1…9: Ctrl+Alt is AltGr on Windows
  (AltGr+7/8/9/0 type `{ [ ] }` on German layouts: a menu accelerator would swallow them) and the
  web editor (TipTap) uses Ctrl+Alt+1–6 / Ctrl+Shift+7–9. macOS keeps ⌃⌘1…9 (free: menus use ⌘,
  TipTap ⌥⌘ / ⇧⌘, Spaces ⌃1–9).
- **app-<n> windows close** on a switch: the navigation guard, the bridge's caller check and the
  store bookkeeping follow the current origin, so a window left on the old server would see its
  own links treated as foreign, its bridge calls refused, and its store could be wiped by a Forget.
- Outbox: **before** the switch, bounded (3 s, only when there is something and the app is in the
  old server online); the rest when that server is current again. From the connect screen (not
  in the app) nothing is tried before; the changes wait on disk for that server.

**Tests**: `cargo test`: 303 unit tests (16 new: default names, name cleaning, ids, moves, the
nine accelerators, no clash with any menu / global / editor shortcut on both patterns, a global
shortcut on a server key, choices, the saved order + touch, the limit, rename / reconnect / old
files, the switch dropping every in-memory account answer while queue / New-task memory / the
saved entry stay per origin, an old poll ticket dropped after a switch and back, sign-out vs.
forget, a running sync stopped by another server / unload, per-server ETags, tray Servers ▸;
updated: tray ids, menu ids) + 14 capability tests (2 new: the switch commands only in their
window's file, every one checks its caller first and none takes an origin / URL). `cargo clippy
--all-targets -D warnings`, `cargo fmt --check`, `npm run build`, `npx tauri build --debug
--bundles app` green (`src-tauri/target/debug/bundle/macos/Brainitive.app`).

**Checked visually**: the built Settings and panel pages in headless Chrome with mocked commands
(dark): Settings → Servers scrolled into view, the open header switcher, the signed-out links.

**Commits**: `47c896d` plan · `ac853ef` Rust (servers, settings, flows, offline, menu, tray,
commands, capabilities) · `623f5d1` pages · docs (this one).

**Not tested**

- The running app was not launched (as asked): a real switch (main window rebuild on the other
  store, no new sign-in), ⌃⌘1…9 reaching the menu while the web page has the focus (WKWebView
  passes ⌃⌘ key equivalents to the menu as far as known; WebView2 / WebKitGTK Alt+digit
  unverified), the check marks after a click, the tray submenu, the bounded outbox send — QA 2A.
- Settings reads the keychain once per saved server to show "Signed in": unsigned debug builds
  may ask for keychain access per server (signed builds don't).
- Windows / Linux not built or run here.

## Open questions

- **Signed in** means "a refresh token is in the keychain", not "the server still accepts it"
  (a device revoked on the web shows as signed in until the next switch / restore). Checking each
  would cost a refresh per server.
- **Unread per server** for the switcher (a dot next to "Private") needs polling every signed-in
  server (follow-up idea above); there is no cross-server endpoint, and none is needed in the API.
- The stock instance name "Brainitive" is treated as "no name". A self-hosted admin can set
  `APP_NAME`; if the server had a separate "instance display name" setting, `/api/v1/instance`
  could carry it — not needed now (no API change made).
