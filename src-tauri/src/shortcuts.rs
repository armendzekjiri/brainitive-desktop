//! Global shortcuts (BRAI-359 quick capture, BRAI-401 quick switcher): defaults, parsing,
//! validation and labels. Pure logic; the registration with the OS is in `hotkeys.rs`.
//!
//! A global shortcut works in every app, so it must not steal a key the user needs elsewhere:
//! at least two modifiers (one of them Ctrl / Alt / Cmd) unless it is a function key, and none of
//! the system's own combinations (Spotlight, screenshots, input sources, Ctrl+Alt+Del …).

use std::str::FromStr;

use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

/// ⇧⌘Space on macOS, Ctrl+Shift+Space on Windows / Linux.
pub const DEFAULT_CAPTURE: &str = "CommandOrControl+Shift+Space";
/// ⌥⌘K on macOS, Ctrl+Alt+K on Windows / Linux ("K" like the web app's ⌘K search).
pub const DEFAULT_SWITCHER: &str = "CommandOrControl+Alt+K";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShortcutError {
    #[error("That is not a valid shortcut.")]
    Invalid,
    #[error("Use at least two modifier keys (for example ⇧ and ⌘ / Ctrl), so the shortcut doesn't block a key other apps use.")]
    TooSimple,
    #[error("That shortcut belongs to the system.")]
    Reserved,
    #[error("Another Brainitive shortcut already uses these keys.")]
    Duplicate,
}

const PRIMARY: Modifiers = Modifiers::CONTROL.union(Modifiers::ALT).union(Modifiers::SUPER);

/// Parses and checks a shortcut typed in Settings (or read from settings.json).
pub fn parse(accelerator: &str) -> Result<Shortcut, ShortcutError> {
    let accelerator = accelerator.trim();
    if accelerator.is_empty() || accelerator.len() > 64 {
        return Err(ShortcutError::Invalid);
    }
    let shortcut = Shortcut::from_str(accelerator).map_err(|_| ShortcutError::Invalid)?;
    if is_modifier_key(shortcut.key) {
        return Err(ShortcutError::Invalid);
    }
    let mods = shortcut.mods;
    if !mods.intersects(PRIMARY) && !is_function_key(shortcut.key) {
        return Err(ShortcutError::TooSimple);
    }
    if mods.bits().count_ones() < 2 && !is_function_key(shortcut.key) {
        return Err(ShortcutError::TooSimple);
    }
    if is_reserved(mods, shortcut.key) {
        return Err(ShortcutError::Reserved);
    }
    Ok(shortcut)
}

/// Checks every shortcut the app registers (`None` = off) and that no two are the same. The
/// error carries the name of the one at fault. Generic over any number of global shortcuts
/// (quick capture, quick switcher, later voice capture …).
pub fn parse_all<N: Copy>(
    shortcuts: &[(N, Option<&str>)],
) -> Result<Vec<(N, Shortcut)>, (N, ShortcutError)> {
    let mut out: Vec<(N, Shortcut)> = Vec::new();
    for (name, accelerator) in shortcuts {
        let Some(accelerator) = accelerator else { continue };
        let shortcut = parse(accelerator).map_err(|e| (*name, e))?;
        if out.iter().any(|(_, s)| s.id() == shortcut.id()) {
            return Err((*name, ShortcutError::Duplicate));
        }
        out.push((*name, shortcut));
    }
    Ok(out)
}

/// The stored form, e.g. `shift+super+Space` (stable, parses back to the same shortcut).
pub fn canonical(shortcut: &Shortcut) -> String {
    shortcut.into_string()
}

/// What menus and Settings show: `⇧⌘Space` on macOS, `Ctrl+Shift+Space` elsewhere.
pub fn label(shortcut: &Shortcut, mac: bool) -> String {
    let mods = shortcut.mods;
    let key = key_label(shortcut.key);
    if mac {
        let mut out = String::new();
        for (flag, sign) in [
            (Modifiers::CONTROL, "⌃"),
            (Modifiers::ALT, "⌥"),
            (Modifiers::SHIFT, "⇧"),
            (Modifiers::SUPER, "⌘"),
        ] {
            if mods.contains(flag) {
                out.push_str(sign);
            }
        }
        out.push_str(&key);
        out
    } else {
        let mut parts: Vec<String> = Vec::new();
        for (flag, name) in [
            (Modifiers::CONTROL, "Ctrl"),
            (Modifiers::ALT, "Alt"),
            (Modifiers::SHIFT, "Shift"),
            (Modifiers::SUPER, "Win"),
        ] {
            if mods.contains(flag) {
                parts.push(name.into());
            }
        }
        parts.push(key);
        parts.join("+")
    }
}

/// Label of a stored accelerator string for this platform ("" when invalid / off).
pub fn label_of(accelerator: Option<&str>) -> String {
    accelerator
        .and_then(|a| parse(a).ok())
        .map(|s| label(&s, cfg!(target_os = "macos")))
        .unwrap_or_default()
}

fn key_label(code: Code) -> String {
    let raw = code.to_string();
    if let Some(letter) = raw.strip_prefix("Key") {
        return letter.to_string();
    }
    if let Some(digit) = raw.strip_prefix("Digit") {
        return digit.to_string();
    }
    match code {
        Code::ArrowUp => "↑".into(),
        Code::ArrowDown => "↓".into(),
        Code::ArrowLeft => "←".into(),
        Code::ArrowRight => "→".into(),
        Code::Enter => "Return".into(),
        Code::Escape => "Esc".into(),
        Code::Backquote => "`".into(),
        Code::Minus => "-".into(),
        Code::Equal => "=".into(),
        Code::BracketLeft => "[".into(),
        Code::BracketRight => "]".into(),
        Code::Semicolon => ";".into(),
        Code::Quote => "'".into(),
        Code::Comma => ",".into(),
        Code::Period => ".".into(),
        Code::Slash => "/".into(),
        Code::Backslash => "\\".into(),
        _ => raw,
    }
}

fn is_modifier_key(code: Code) -> bool {
    matches!(
        code,
        Code::ShiftLeft
            | Code::ShiftRight
            | Code::ControlLeft
            | Code::ControlRight
            | Code::AltLeft
            | Code::AltRight
            | Code::MetaLeft
            | Code::MetaRight
            | Code::CapsLock
            | Code::Fn
    )
}

fn is_function_key(code: Code) -> bool {
    code.to_string()
        .strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n))
}

/// System combinations a global shortcut must never take (on any OS, so a synced preference
/// can't break another machine).
fn is_reserved(mods: Modifiers, key: Code) -> bool {
    const CTRL: Modifiers = Modifiers::CONTROL;
    const ALT: Modifiers = Modifiers::ALT;
    const SHIFT: Modifiers = Modifiers::SHIFT;
    const CMD: Modifiers = Modifiers::SUPER;
    let reserved: &[(Modifiers, Code)] = &[
        // macOS
        (CMD.union(ALT), Code::Space),          // Finder search
        (CMD.union(CTRL), Code::Space),         // emoji & symbols
        (CTRL.union(ALT), Code::Space),         // next input source
        (CMD.union(SHIFT), Code::Digit3),       // screenshots
        (CMD.union(SHIFT), Code::Digit4),
        (CMD.union(SHIFT), Code::Digit5),
        (CMD.union(CTRL), Code::KeyQ),          // lock screen
        (CMD.union(ALT), Code::Escape),         // force quit
        (CMD.union(SHIFT), Code::KeyQ),         // log out
        (CMD.union(CTRL), Code::KeyF),          // full screen
        // Windows / Linux
        (CTRL.union(ALT), Code::Delete),
        (CTRL.union(SHIFT), Code::Escape),      // task manager
        (CTRL.union(ALT), Code::KeyT),          // terminal (GNOME, Ubuntu)
        (CTRL.union(ALT), Code::KeyL),          // lock (some desktops)
        (CTRL.union(ALT), Code::ArrowLeft),     // workspaces
        (CTRL.union(ALT), Code::ArrowRight),
        (CTRL.union(ALT), Code::ArrowUp),
        (CTRL.union(ALT), Code::ArrowDown),
        (CMD.union(SHIFT), Code::KeyS),         // Windows snipping tool
        (ALT, Code::F4),
        (ALT, Code::Tab),
        (CMD, Code::Tab),
    ];
    reserved.iter().any(|(m, k)| *m == mods && *k == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_different() {
        let all = parse_all(&[("capture", Some(DEFAULT_CAPTURE)), ("switcher", Some(DEFAULT_SWITCHER))]).unwrap();
        let (c, s) = (all[0].1, all[1].1);
        assert_eq!(c.key, Code::Space);
        assert!(c.mods.contains(Modifiers::SHIFT));
        assert_ne!(c.id(), s.id());
    }

    #[test]
    fn canonical_round_trips() {
        for raw in [DEFAULT_CAPTURE, DEFAULT_SWITCHER, "Control+Alt+KeyB", "shift+super+Digit7", "Ctrl+F5"] {
            let a = parse(raw).unwrap();
            let b = parse(&canonical(&a)).unwrap();
            assert_eq!(a.id(), b.id(), "{raw}");
        }
        // the settings UI sends KeyboardEvent.code names
        assert_eq!(parse("Control+Shift+KeyJ").unwrap().key, Code::KeyJ);
    }

    #[test]
    fn rejects_shortcuts_that_would_steal_keys() {
        assert_eq!(parse(""), Err(ShortcutError::Invalid));
        assert_eq!(parse("Ctrl+Shift+NotAKey"), Err(ShortcutError::Invalid));
        assert_eq!(parse("Ctrl+Shift"), Err(ShortcutError::Invalid));
        assert_eq!(parse("Shift+ShiftLeft"), Err(ShortcutError::Invalid));
        assert_eq!(parse("Space"), Err(ShortcutError::TooSimple));
        assert_eq!(parse("Shift+A"), Err(ShortcutError::TooSimple));
        assert_eq!(parse("CommandOrControl+C"), Err(ShortcutError::TooSimple));
        assert_eq!(parse("Super+Space"), Err(ShortcutError::TooSimple));
        assert!(parse("Ctrl+F5").is_ok(), "a function key needs only one modifier");
        assert!(parse("F13").is_ok());
        assert_eq!(parse("Super+Alt+Space"), Err(ShortcutError::Reserved));
        assert_eq!(parse("Control+Alt+Delete"), Err(ShortcutError::Reserved));
        assert_eq!(parse("Shift+Super+Digit4"), Err(ShortcutError::Reserved));
        assert_eq!(parse("Alt+F4"), Err(ShortcutError::Reserved));
    }

    #[test]
    fn shortcuts_must_differ() {
        assert_eq!(
            parse_all(&[("capture", Some("Control+Shift+KeyJ")), ("switcher", Some("ctrl+shift+j"))]).unwrap_err(),
            ("switcher", ShortcutError::Duplicate)
        );
        assert!(parse_all::<&str>(&[("capture", None), ("switcher", None)]).unwrap().is_empty());
        assert_eq!(parse_all(&[("capture", Some("x")), ("switcher", None)]).unwrap_err().0, "capture");
        // off ones are skipped, a third one is checked against both
        let three = [("a", Some("Control+Shift+KeyJ")), ("b", None), ("c", Some("Control+Alt+KeyJ"))];
        assert_eq!(parse_all(&three).unwrap().len(), 2);
    }

    #[test]
    fn labels_per_platform() {
        let s = parse("Control+Alt+Shift+Super+KeyK").unwrap();
        assert_eq!(label(&s, true), "⌃⌥⇧⌘K");
        assert_eq!(label(&s, false), "Ctrl+Alt+Shift+Win+K");
        let s = parse("Shift+Super+Space").unwrap();
        assert_eq!(label(&s, true), "⇧⌘Space");
        let s = parse("Control+Shift+Space").unwrap();
        assert_eq!(label(&s, false), "Ctrl+Shift+Space");
        let s = parse("Control+Alt+Digit1").unwrap();
        assert_eq!(label(&s, false), "Ctrl+Alt+1");
        assert_eq!(label_of(None), "");
        assert_eq!(label_of(Some("bogus")), "");
    }
}
