//! Commands of the Settings window (`settings.html`, menu → Settings… / ⌘,). Only that window's
//! bundled page may call them (`capabilities/settings.json` + [`popup::ensure_caller`]).

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Webview};
use tauri_plugin_autostart::ManagerExt;

use crate::popup::{self, Popup};
use crate::state::{state, Phase};
use crate::version::APP_VERSION;

/// The argument the login item starts the app with (BRAI-364).
pub const AUTOSTART_ARG: &str = "--autostart";

/// Started by the login item (not by the user): may stay in the tray.
pub fn launched_at_login(mut args: impl Iterator<Item = String>) -> bool {
    args.any(|a| a == AUTOSTART_ARG)
}

#[derive(Debug, Clone, Serialize)]
pub struct PrefsView {
    app_version: &'static str,
    platform: &'static str,
    /// "Cloud" / "brainitive.example.com", `None` before a server is chosen.
    server: Option<String>,
    signed_in: bool,
    zoom_percent: u32,
    /// "⇧⌘Space" etc.; empty = off.
    capture_shortcut: String,
    switcher_shortcut: String,
    keep_in_tray: bool,
    ask_where_to_save: bool,
    /// The OS login item is the truth (LaunchAgent / registry / autostart .desktop file).
    launch_at_login: bool,
    start_hidden: bool,
}

pub fn view(app: &AppHandle) -> PrefsView {
    let s = state(app);
    let inner = s.lock();
    let prefs = &inner.settings.preferences;
    PrefsView {
        app_version: APP_VERSION,
        platform: crate::api::platform(),
        server: inner.current_origin().map(|o| o.display_name()),
        signed_in: inner.phase == Phase::InApp,
        zoom_percent: (prefs.zoom * 100.0).round() as u32,
        capture_shortcut: crate::shortcuts::label_of(prefs.capture_shortcut.as_deref()),
        switcher_shortcut: crate::shortcuts::label_of(prefs.switcher_shortcut.as_deref()),
        keep_in_tray: prefs.keep_in_tray,
        ask_where_to_save: prefs.ask_where_to_save,
        launch_at_login: app.autolaunch().is_enabled().unwrap_or(false),
        start_hidden: prefs.start_hidden,
    }
}

#[tauri::command]
pub fn settings_get(app: AppHandle, webview: Webview) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    Ok(view(&app))
}

/// "Account settings" / "Notification settings": the web app's profile page in the app window.
#[tauri::command]
pub fn settings_open_account(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    crate::shell::open_page(&app, "/profile");
    Ok(())
}

#[tauri::command]
pub fn settings_reset_zoom(app: AppHandle, webview: Webview) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    crate::shell::zoom(&app, 0);
    Ok(view(&app))
}

/// Esc in the window.
#[tauri::command]
pub fn settings_close(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    popup::dismiss(&app, Popup::Settings);
    Ok(())
}

/// A new value for one of the global shortcuts.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", content = "keys", rename_all = "snake_case")]
pub enum ShortcutChoice {
    Off,
    Default,
    /// Recorded in the window: `Control+Shift+KeyJ` (KeyboardEvent.code names).
    Keys(String),
}

/// Changes a global shortcut. It is registered right away; when the OS refuses it (another app
/// has it) the previous shortcuts stay and the error says why.
#[tauri::command]
pub fn settings_set_shortcut(
    app: AppHandle,
    webview: Webview,
    which: String,
    choice: ShortcutChoice,
) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    let value = match choice {
        ShortcutChoice::Off => None,
        ShortcutChoice::Default => Some(match which.as_str() {
            "switcher" => crate::shortcuts::DEFAULT_SWITCHER.to_string(),
            _ => crate::shortcuts::DEFAULT_CAPTURE.to_string(),
        }),
        ShortcutChoice::Keys(keys) => Some(
            crate::shortcuts::parse(&keys)
                .map(|s| crate::shortcuts::canonical(&s))
                .map_err(|e| e.to_string())?,
        ),
    };
    let previous = state(&app).lock().settings.preferences.clone();
    let mut next = previous.clone();
    match which.as_str() {
        "capture" => next.capture_shortcut = value,
        "switcher" => next.switcher_shortcut = value,
        _ => return Err("unknown shortcut".into()),
    }
    state(&app).lock().settings.preferences = next;
    if let Err(e) = crate::hotkeys::apply(&app) {
        state(&app).lock().settings.preferences = previous;
        let _ = crate::hotkeys::apply(&app);
        return Err(e);
    }
    state(&app).lock().save_settings();
    crate::menu::refresh_shortcut_labels(&app);
    Ok(view(&app))
}

/// The window records a new shortcut: the current ones are paused so pressing them is seen by
/// the page instead of opening their window. `false` restores them.
#[tauri::command]
pub fn settings_record_shortcut(app: AppHandle, webview: Webview, active: bool) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    if active {
        crate::hotkeys::pause(&app);
        Ok(())
    } else {
        crate::hotkeys::apply(&app)
    }
}

/// An on / off preference of the window.
#[tauri::command]
pub fn settings_set_toggle(app: AppHandle, webview: Webview, key: String, value: bool) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    if key == "launch_at_login" {
        let autolaunch = app.autolaunch();
        let result = if value { autolaunch.enable() } else { autolaunch.disable() };
        result.map_err(|e| format!("The login item could not be changed: {e}"))?;
        return Ok(view(&app));
    }
    {
        let s = state(&app);
        let mut inner = s.lock();
        let prefs = &mut inner.settings.preferences;
        match key.as_str() {
            "start_hidden" => prefs.start_hidden = value,
            "keep_in_tray" => prefs.keep_in_tray = value,
            "ask_where_to_save" => prefs.ask_where_to_save = value,
            _ => return Err("unknown setting".into()),
        }
        inner.save_settings();
    }
    Ok(view(&app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_a_start_by_the_login_item() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        assert!(launched_at_login(args(&["/Applications/Brainitive.app/Contents/MacOS/brainitive-desktop", "--autostart"])));
        assert!(!launched_at_login(args(&["brainitive-desktop"])));
        assert!(!launched_at_login(args(&["brainitive-desktop", "--autostart=1"])));
    }
}
