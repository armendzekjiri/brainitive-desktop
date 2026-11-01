//! Registers the global shortcuts with the OS (`tauri-plugin-global-shortcut`, Rust side only:
//! no webview is granted the plugin's commands). Which shortcut does what: `shortcuts.rs`.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::popup::{self, Popup};
use crate::shortcuts::{self, ShortcutError};
use crate::state::state;

/// The plugin's handler: a pressed shortcut opens (or closes again) its window.
pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state != ShortcutState::Pressed {
        return;
    }
    let (capture, switcher) = state(app).lock().hotkey_ids;
    let id = shortcut.id();
    if Some(id) == capture {
        popup::toggle(app, Popup::Capture);
    } else if Some(id) == switcher {
        popup::toggle(app, Popup::Switcher);
    }
}

/// Registers the shortcuts of the preferences (replacing the previous ones). The error says
/// which one could not be registered: invalid, or taken by another app.
pub fn apply(app: &AppHandle) -> Result<(), String> {
    let prefs = state(app).lock().settings.preferences.clone();
    register(app, prefs.capture_shortcut.as_deref(), None)
}

pub fn register(app: &AppHandle, capture: Option<&str>, switcher: Option<&str>) -> Result<(), String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    state(app).lock().hotkey_ids = (None, None);
    let (capture, switcher) = shortcuts::parse_pair(capture, switcher).map_err(|(which, e)| describe(which, &e))?;
    if let Some(s) = capture {
        gs.register(s).map_err(|e| taken("quick capture", &s, e))?;
        state(app).lock().hotkey_ids.0 = Some(s.id());
    }
    if let Some(s) = switcher {
        gs.register(s).map_err(|e| taken("the quick switcher", &s, e))?;
        state(app).lock().hotkey_ids.1 = Some(s.id());
    }
    Ok(())
}

/// While the Settings window records a new shortcut, the current ones must not fire.
pub fn pause(app: &AppHandle) {
    let _ = app.global_shortcut().unregister_all();
    state(app).lock().hotkey_ids = (None, None);
}

fn describe(which: &str, e: &ShortcutError) -> String {
    let name = if which == "capture" {
        "Quick capture"
    } else {
        "Quick switcher"
    };
    format!("{name}: {e}")
}

fn taken(name: &str, s: &Shortcut, e: impl std::fmt::Display) -> String {
    log::warn!("global shortcut not registered: {e}");
    format!(
        "{} is already used by another app or the system, so it can't open {name}. Choose another one.",
        shortcuts::label(s, cfg!(target_os = "macos"))
    )
}
