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
