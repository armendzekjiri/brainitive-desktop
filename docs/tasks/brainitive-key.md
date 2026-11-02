# Brainitive key (BRAI-440)

One easy global shortcut (the **Brainitive key**), then one letter for the action.

## Decisions

- **Brainitive key**: macOS **⌃⌘K** (left hand ⌃⌘, right hand K), Windows / Linux **Ctrl+Alt+K**
  (not an AltGr letter on German and most European layouts). One global shortcut,
  `HotkeyAction::Leader`, registered like the others; Record / Default / Off in Settings.
- Pressing it opens the **Brainitive key overlay** (`leader.html`, window `leader`, own
  `capabilities/leader.json`): a small centred panel that takes the focus, lists the letters and
  reads the next key **in that window** (no global per-letter registration). Closes on Esc, on
  losing focus, or after 3 s without a key.
- **Letters** (Settings, A–Z, unique): **C** quick capture · **K** quick switcher (⌃⌘K K) ·
  **V** voice capture, held: key-down starts, key-up stops (greyed with a hint while voice capture
  is off) · **O** open the main window.
- Pressing the Brainitive key again while the overlay is in front (modifiers still held) counts as
  its letter (K → switcher); when that letter is not an action it closes the overlay.
- **Hold V**: the overlay sends `leader_press` → `voice::start_from_leader` (hold mode, the release
  letter remembered). The voice indicator takes the focus, so the key-up arrives in **whichever**
  window has the focus: the overlay (`leader_release`) or the indicator (`voice_release`, it gets the
  letter in its view). Both feed `Input::HotkeyUp`; the recorder ignores a second one. Same tap vs.
  hold rule as the direct shortcut (< 0.5 s = toggle mode).
- **Direct shortcuts** stay, optional: quick capture ⇧⌘Space stays **on**; quick switcher and
  voice capture default to **Off**. Their "Default" button sets the suggested keys (switcher
  ⌥⌘K on macOS, Ctrl+Alt+Shift+K on Windows / Linux so it never equals the Brainitive key; voice
  ⇧⌘V / Ctrl+Shift+V).
- **Migration** (`Preferences::migrate`, `shortcut_scheme` 0 → 1, runs once at load, then saved):
  the file always stores every field, so "default" vs. "explicit" is told apart by the **exact
  string**: untouched / "Default" values are the literal constants `CommandOrControl+Alt+K` and
  `CommandOrControl+Shift+KeyV`; recorded keys are stored canonical (`alt+super+KeyK`). Only the
  literal defaults become Off; recorded keys and an explicit Off stay. A file without the leader
  gets the default leader, unless one of the kept direct shortcuts already uses those keys (then the
  leader starts Off).
- **Settings → Shortcuts**: "Brainitive key" row (Record / Default / Off), under it one row per
  action letter (one-letter field + Default), inline errors; then "Direct shortcuts (optional)"
  with quick capture, quick switcher, voice capture (moved there from the Voice section). The OS
  refusal of the Brainitive key shows inline and at the top.
- A conflict names the other shortcut ("Quick capture already uses ⇧⌘Space."). Registration now
  tries every shortcut (one refused shortcut no longer leaves the later ones unregistered) and a
  change is only rolled back when **its** shortcut was refused.
- **Menus / tray**: "Brainitive Key…" in the File menu and the tray, the key shown as its
  accelerator (display only, like the others: the OS delivers the global shortcut first). No new
  static menu accelerator.
- Voice indicator / notification say how to start: "Hold ⇧⌘V", else "Press ⌃⌘K, then hold V".

## Steps

1. `leader.rs` (pure: actions, letters, validation, key-of-letter, labels; glue: overlay commands,
   hotkey handler), `shortcuts.rs` (DEFAULT_LEADER, suggested direct keys), `settings.rs`
   (fields, lenient letters, migration), `hotkeys.rs` (Leader action, per-action errors,
   conflicts), `prefs.rs` (leader + letters), `popup.rs` (Popup::Leader), voice glue.
2. Overlay page + capability + build.rs + tauri.conf + vite input; capability tests.
3. Settings UI; voice indicator key-up; menu / tray entries.
4. Docs: ARCHITECTURE, SECURITY, README, CHANGELOG, QA-CHECKLIST.
5. Gate: cargo test / clippy / fmt, npm run build, `npx tauri build --debug --bundles app`.

## Tests (Rust)

Letter validation (A–Z, case, unique, lenient load), migration default vs. explicit, labels per
OS for the leader, leader ≠ any direct shortcut (defaults and recorded), overlay key handling
(letter → action, leader key again), voice how-to text, tray entry.

## Result

**Built**

- Brainitive key ⌃⌘K (macOS) / Ctrl+Alt+K (Windows, Linux), `HotkeyAction::Leader`, opening the
  overlay `leader.html` (window `leader`, `capabilities/leader.json`: 4 commands, local only, each
  with `ensure_caller`). Letters C / K / V (hold) / O; Esc, blur and a 3 s timer close it; the key
  again = its letter (⌃⌘K K → switcher). V greyed with a hint while voice capture is off.
- Hold V: `voice::start_from_leader` / `leader_released`; key-up from the overlay
  (`leader_release`) or the voice indicator (new `voice_release`, voice capability); voice texts
  say "Press ⌃⌘K, then hold V" when no direct shortcut is set.
- Direct shortcuts optional: capture on, switcher / voice Off by default; Default suggests ⌥⌘K
  (Ctrl+Alt+Shift+K on Windows / Linux, so it never equals the key) and ⇧⌘V / Ctrl+Shift+V.
- `Preferences::migrate` (`shortcut_scheme`), lenient letters, `hotkeys::conflict` (names the
  other shortcut), registration of every shortcut even if one is refused; Settings rolls back only
  the changed one.
- Settings → Shortcuts (key, letters, "Direct shortcuts (optional)"; voice's direct row moved
  there). File menu and tray: **Brainitive Key…** with the key.
- Docs: ARCHITECTURE, SECURITY, README, CHANGELOG (Unreleased), QA-CHECKLIST (section 4A + updated
  rows in 4, 7, 8, 13).

**Tests**: `cargo test` 240 passed (232 lib + 8 capabilities; 6 ignored as before), 15 of them new:
letters (A–Z, unique, lenient load), migration (old defaults off, recorded keys / explicit Off
kept, leader off on a clash, runs once, saved at load), leader ≠ any direct shortcut, labels per
OS, key-again → letter, voice how-to text, tray entry, overlay capability. `cargo clippy
--all-targets -D warnings`, `cargo fmt --check`, `npm run build` green;
`npx tauri build --debug --bundles app` built `src-tauri/target/debug/bundle/macos/Brainitive.app`.

**Not tested / open**

- The app was not launched (by request): the overlay, focus hand-over and hold-V are untested
  on every OS — QA-CHECKLIST 4A.
- Hold V's key-up after the focus moves to the voice indicator relies on the webview delivering a
  key-up without its key-down (WKWebView / WebView2 / WebKitGTK); the first use (indicator built
  then) is the riskiest (4A.12). Fallback: Done / ↩ / the 2-minute cut-off.
- Windows / Linux not built or run here: focus-stealing rules (Windows foreground lock, GNOME) may
  keep the overlay from getting the keys; Wayland has no global shortcuts (known gap).
- Ctrl+Alt+K is AltGr+K (`ł`) on Czech, Slovak and Hungarian layouts (documented in SECURITY.md,
  QA 4A.22); those users must change the key.
