//! Registers the global shortcuts with the OS (`tauri-plugin-global-shortcut`, Rust side only:
//! no webview is granted the plugin's commands). Which keys are allowed: `shortcuts.rs`.
//!
//! Generic: every global shortcut is a [`HotkeyAction`] with an accelerator from the
//! preferences; adding one = a variant, its preference and its handler. Voice capture (BRAI-439)
//! is the one action that also gets the key-up (hold to talk).

use std::collections::HashMap;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::popup::{self, Popup};
use crate::settings::Preferences;
use crate::shortcuts;
use crate::state::state;

/// What a global shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    Capture,
    Switcher,
    /// Hold to talk (BRAI-439): press starts recording, release stops it.
    Voice,
}

impl HotkeyAction {
    pub fn name(self) -> &'static str {
        match self {
            Self::Capture => "Quick capture",
            Self::Switcher => "Quick switcher",
            Self::Voice => "Voice capture",
        }
    }

    /// Whether the action wants the key-up too (hold to talk), not only the key-down.
    pub fn wants_release(self) -> bool {
        matches!(self, Self::Voice)
    }

    fn run(self, app: &AppHandle, pressed: bool) {
        match self {
            Self::Capture => popup::toggle(app, Popup::Capture),
            Self::Switcher => popup::toggle(app, Popup::Switcher),
            Self::Voice => crate::voice::on_hotkey(app, pressed),
        }
    }
}

/// The shortcuts the preferences ask for, in registration order (`None` = off).
pub fn wanted(prefs: &Preferences) -> Vec<(HotkeyAction, Option<String>)> {
    vec![
        (HotkeyAction::Capture, prefs.capture_shortcut.clone()),
        (HotkeyAction::Switcher, prefs.switcher_shortcut.clone()),
        // registered while voice capture is off too: the first press explains it and offers to
        // turn it on
        (HotkeyAction::Voice, prefs.voice_shortcut.clone()),
    ]
}

/// The plugin's handler: a pressed shortcut runs its action (a released one only for actions
/// that hold, see [`HotkeyAction::wants_release`]).
pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    let pressed = event.state == ShortcutState::Pressed;
    let action = state(app).lock().hotkeys.get(&shortcut.id()).copied();
    if let Some(action) = action.filter(|a| pressed || a.wants_release()) {
        action.run(app, pressed);
    }
}

/// Registers the shortcuts of the preferences (replacing the previous ones). The error says
/// which one could not be registered: invalid, or taken by another app.
pub fn apply(app: &AppHandle) -> Result<(), String> {
    let prefs = state(app).lock().settings.preferences.clone();
    let wanted = wanted(&prefs);
    let list: Vec<(HotkeyAction, Option<&str>)> = wanted.iter().map(|(a, s)| (*a, s.as_deref())).collect();
    let result = register(app, &list);
    state(app).lock().hotkey_error = result.as_ref().err().cloned();
    result
}

pub fn register(app: &AppHandle, list: &[(HotkeyAction, Option<&str>)]) -> Result<(), String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    state(app).lock().hotkeys = HashMap::new();
    let parsed = shortcuts::parse_all(list).map_err(|(action, e)| format!("{}: {e}", action.name()))?;
    for (action, shortcut) in parsed {
        gs.register(shortcut).map_err(|e| taken(action, &shortcut, e))?;
        state(app).lock().hotkeys.insert(shortcut.id(), action);
    }
    Ok(())
}

/// While the Settings window records a new shortcut, the current ones must not fire.
pub fn pause(app: &AppHandle) {
    let _ = app.global_shortcut().unregister_all();
    state(app).lock().hotkeys = HashMap::new();
}

fn taken(action: HotkeyAction, s: &Shortcut, e: impl std::fmt::Display) -> String {
    log::warn!("global shortcut not registered: {e}");
    format!(
        "{}: {} is already used by another app or the system. Choose another one.",
        action.name(),
        shortcuts::label(s, cfg!(target_os = "macos"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_capture_is_in_the_registry_with_its_preference() {
        let prefs = Preferences::default();
        let list = wanted(&prefs);
        assert_eq!(
            list.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
            [HotkeyAction::Capture, HotkeyAction::Switcher, HotkeyAction::Voice]
        );
        assert_eq!(list[2].1.as_deref(), Some(shortcuts::DEFAULT_VOICE));
        // registered even while voice capture is off (the first press offers to turn it on)
        assert!(!prefs.voice_enabled);
        // the defaults are valid and different
        let refs: Vec<(HotkeyAction, Option<&str>)> = list.iter().map(|(a, s)| (*a, s.as_deref())).collect();
        assert_eq!(shortcuts::parse_all(&refs).unwrap().len(), 3);

        // off = not registered; rebound = its own keys
        let mut p = prefs.clone();
        p.voice_shortcut = None;
        assert_eq!(wanted(&p)[2], (HotkeyAction::Voice, None));
        p.voice_shortcut = Some("Control+Alt+KeyV".into());
        assert_eq!(wanted(&p)[2].1.as_deref(), Some("Control+Alt+KeyV"));
        // the same keys as quick capture are refused, naming voice capture
        p.voice_shortcut = p.capture_shortcut.clone();
        let list = wanted(&p);
        let refs: Vec<(HotkeyAction, Option<&str>)> = list.iter().map(|(a, s)| (*a, s.as_deref())).collect();
        assert_eq!(
            shortcuts::parse_all(&refs).unwrap_err(),
            (HotkeyAction::Voice, shortcuts::ShortcutError::Duplicate)
        );
    }

    #[test]
    fn only_voice_capture_gets_the_key_up() {
        assert!(HotkeyAction::Voice.wants_release());
        assert!(!HotkeyAction::Capture.wants_release());
        assert!(!HotkeyAction::Switcher.wants_release());
        assert_eq!(HotkeyAction::Voice.name(), "Voice capture");
    }
}
