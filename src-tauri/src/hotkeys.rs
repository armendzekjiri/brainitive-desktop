//! Registers the global shortcuts with the OS (`tauri-plugin-global-shortcut`, Rust side only:
//! no webview is granted the plugin's commands). Which keys are allowed: `shortcuts.rs`.
//!
//! Generic: every global shortcut is a [`HotkeyAction`] with an accelerator from the
//! preferences; adding one (e.g. voice capture) = a variant, its preference and its handler.

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
}

impl HotkeyAction {
    pub fn name(self) -> &'static str {
        match self {
            Self::Capture => "Quick capture",
            Self::Switcher => "Quick switcher",
        }
    }

    fn run(self, app: &AppHandle) {
        match self {
            Self::Capture => popup::toggle(app, Popup::Capture),
            Self::Switcher => popup::toggle(app, Popup::Switcher),
        }
    }
}

/// The shortcuts the preferences ask for, in registration order (`None` = off).
pub fn wanted(prefs: &Preferences) -> Vec<(HotkeyAction, Option<String>)> {
    vec![
        (HotkeyAction::Capture, prefs.capture_shortcut.clone()),
        (HotkeyAction::Switcher, prefs.switcher_shortcut.clone()),
    ]
}

/// The plugin's handler: a pressed shortcut runs its action.
pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state != ShortcutState::Pressed {
        return;
    }
    let action = state(app).lock().hotkeys.get(&shortcut.id()).copied();
    if let Some(action) = action {
        action.run(app);
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
