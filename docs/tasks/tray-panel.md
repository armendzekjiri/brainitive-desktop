# Title bars on macOS 26 + the menu-bar panel

Two changes: (1) the see-through title bars of the decorated windows on macOS 26 (Tahoe), (2) a
glass panel under the tray / menu-bar icon (like the macOS Weather menu-bar extra).

## 1. Title bars (macOS 26)

**Cause** (checked with a small AppKit probe on macOS 26.2, `docs/tasks` has no screenshots
because `screencapture` is not permitted in the agent's session): Tauri's default macOS title-bar
style (`TitleBarStyle::Visible`) sets `NSWindowStyleMask::FullSizeContentView` (a workaround for
devtools, tauri#10225), so the webview spans the whole window, title bar included (window
640 pt, `contentLayoutRect` 608 pt, webview 640 pt). Until macOS 15 the title bar was opaque and
hid what scrolled under it; on macOS 26 it is see-through, so the page shows behind the title.

**Fix** (Rust only, no CSS in any page, nothing injected into the web app):

- Decorated windows use `TitleBarStyle::Transparent` = no full-size content view: the webview
  starts **below** the title bar (probe: webview 640 pt, window 672 pt, no overlap) and the title
  bar shows the window's background colour.
- Bundled decorated windows (Settings, Offline) get that background colour = the top of their page
  (`--bg` of Settings, `--panel` of the offline status bar), light or dark, re-set on
  `WindowEvent::ThemeChanged`: one solid strip that looks like part of the page.
- The main window / `app-<n>` windows (the remote web app, and the bundled connect screens) are
  affected the same way: they get `TitleBarStyle::Transparent` too and keep the system window
  background (follows light / dark by itself).
- Floating windows (capture, switcher, Brainitive key, voice) are undecorated (`Borderless`): no
  title bar, nothing to fix (checked in `tao`: decorations false → borderless mask).
- Windows / Linux: `title_bar_style` is macOS only; nothing changes there.

## 2. The menu-bar panel

**Window** `panel` (`panel.html`, `src/panel.ts`, `src/panel.css`), a `Popup::Panel`: undecorated,
always on top, skip taskbar, on every space, hides on blur and Esc. macOS: transparent window +
`Effect::Popover` (`EffectState::Active`, radius 12) — the real NSVisualEffectView material that
follows light / dark; the page background is transparent. Windows / Linux: opaque `--bg`, shadow.
360 pt wide, height = content (220…560), then the list scrolls. Tauri 2.12.5 always enables the
transparent-window private API (`macos-private-api` is a no-op since 2.12.1), so no config flag.

**Tray**: left click toggles the panel (macOS on mouse-down like native extras, Windows on
mouse-up), right click and ⌃-click open the native menu (`with_inner_tray_icon(show_menu)`), a
click within 0.4 s after the panel hid on blur doesn't reopen it (the click that blurred it).
Linux (AppIndicator has no click events): "Open Panel…" in the tray menu.

**Position** (`panel::geometry`, pure): the monitor under the icon, its work area; the side of the
bar from work area vs. monitor (top / bottom / left / right, else the icon's half); under / above /
beside the icon, centred on it, clamped into the work area with an 8 pt margin; no icon rect →
the corner where the bar is (top-right on macOS / Linux, bottom-right on Windows). macOS computes
in points (each monitor's rect / its scale), Windows / Linux in physical pixels.

**Tabs** (header: host + sync dot; 5 icon tabs with labels; footer: Quick capture · Open
Brainitive · Settings), last tab in `Preferences.panel_tab`:

1. **Today** — `GET /me/today` (ETag): upcoming events (ended ones dropped) and open work due
   today / overdue. Click → the item / event URL in the main window (`shell::open_page`, which goes
   through `links::resolve_deep_link_target`).
2. **My Day** — `GET /me/my-day` (ETag, shared with the poller's ETag cache) with checkboxes →
   `PATCH /me/my-day/items/{id}`.
3. **Assigned** — `GET /me/work?status=open&limit=100`, grouped by project; checkbox →
   `PATCH /me/work/{id}`; "more in My Work" when `next_cursor`.
4. **Notifications** — `GET /notifications?limit=25`, wording like the web bell
   (`NotificationMessage.tsx`); click → open + `POST /notifications/{id}/read`; "Mark all read" →
   `POST /notifications/read-all`; unread badge on the tab (the poller's count).
5. **Timer** — `GET /me/timer` (ETag) ticking from `started_at`, Stop (activity required without
   default / a 422 → inline activity picker from the timer's `activities`, retry with
   `activity_id`), Discard (two clicks); today's entries `GET /work-logs?from=D&to=D` (D =
   `/me/today`'s `date`) with the total; Start: search among My Day + assigned **issues** →
   `POST /me/timer/start`.

**Data**: every call is a Rust command with `Session::request` (no token in the page). Opening the
panel shows what is cached at once (panel memory, the poller's ETag bodies, the offline copy), then
refreshes (ETag where the API has one; skipped when the data is < 15 s old). Answers are kept only
for the same server + sign-in (`tray::PollTicket`). Changes update the tray data (badge, My Day,
timer) and the native menu.

**Ids**: commands take only ids the server sent in the panel's last answers for this sign-in
(work `issue:<n>` / `task:<n>`, notification uuids, activity ids of the timer, the timer id is
never taken from the page). Pages are opened by reference (tab + id), the URL comes from Rust's
copy of the answer.

**Offline**: the last data (panel memory, else the offline copy: My Day, assigned work; Today is
built from it without events), marked "Offline — last synced …". Done ticks go to the existing
outbox as `work.done` (supported by `POST /api/v1/sync`), shown as pending. Timer start / stop /
discard and mark read have no sync type: disabled with an "offline" hint.

**Signed out / no server**: "Sign in to Brainitive" + a button that opens the main window.

**Security**: `capabilities/panel.json` (`local: true`, `windows: ["panel"]`, only `allow-panel-*`
+ listening to `panel://changed`, sent to this window only); every command `ensure_caller(Panel)`;
the panel is destroyed on sign-out (holds account data).

## Steps

1. Title bars (`popup.rs`, `shell.rs`, `lib.rs` theme event) → commit.
2. `panel/` module: `geometry.rs` (pure), `view.rs` (pure mapping + decisions), `mod.rs` (state,
   commands, tray glue); `Popup::Panel`; tray click handling; preference; capability + build.rs +
   tauri.conf + vite input + capability tests → commit.
3. Page (`panel.html`, `panel.ts`, `panel.css`) + static preview screenshots → commit.
4. Docs (ARCHITECTURE, SECURITY, CHANGELOG, README, QA-CHECKLIST) → commit.
5. Gate: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`,
   `npm run build`, `npx tauri build --debug --bundles app`.

## Tests (Rust)

Geometry (menu bar top, taskbar bottom / left / right, auto-hide, multi-monitor with negative
coordinates, clamping at screen edges, no icon rect, resize keeps the anchor); tray click mapping
per OS (left / right / ⌃-click, blur guard); each tab's mapping (today events / due work, my day,
assigned grouping, notifications wording + safe urls, timer + entries + total, start candidates);
id checks; offline decisions (API / outbox / disabled) and sync status; capability rules.
