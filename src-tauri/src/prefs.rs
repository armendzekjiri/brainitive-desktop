//! Commands of the Settings window (`settings.html`, menu → Settings… / ⌘,). Only that window's
//! bundled page may call them (`capabilities/settings.json` + [`popup::ensure_caller`]).

use serde::Serialize;
use tauri::{AppHandle, Webview};

use crate::popup::{self, Popup};
use crate::state::{state, Phase};
use crate::version::APP_VERSION;

#[derive(Debug, Clone, Serialize)]
pub struct PrefsView {
    app_version: &'static str,
    platform: &'static str,
    /// "Cloud" / "brainitive.example.com", `None` before a server is chosen.
    server: Option<String>,
    signed_in: bool,
    zoom_percent: u32,
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
