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

/// "Open Brainitive when you log in": the MSIX startup task when the Microsoft Store build runs
/// packaged (BRAI-442; a Run key would land in the package's private registry, which Windows
/// never reads at login), otherwise the autostart plugin's login item.
fn launch_at_login(app: &AppHandle) -> bool {
    match crate::msstore::startup_task_state() {
        Some(task) => task.is_enabled(),
        None => app.autolaunch().is_enabled().unwrap_or(false),
    }
}

fn set_launch_at_login(app: &AppHandle, on: bool) -> Result<(), String> {
    if let Some(result) = crate::msstore::set_startup_task(on) {
        return result;
    }
    let autolaunch = app.autolaunch();
    let result = if on { autolaunch.enable() } else { autolaunch.disable() };
    result.map_err(|e| format!("The login item could not be changed: {e}"))
}

#[derive(Debug, Clone, Serialize)]
pub struct PrefsView {
    app_version: &'static str,
    platform: &'static str,
    /// The current server's name ("Work", "Brainitive Cloud"), `None` before a server is chosen.
    server: Option<String>,
    signed_in: bool,
    zoom_percent: u32,
    /// The Brainitive key (BRAI-440): "⌃⌘K"; empty = off.
    leader_shortcut: String,
    /// The letter of each action after the Brainitive key.
    leader_letters: Vec<LetterView>,
    /// "⇧⌘Space" etc.; empty = off.
    capture_shortcut: String,
    switcher_shortcut: String,
    /// Voice capture (BRAI-439).
    voice_shortcut: String,
    /// The New task window; empty = off.
    task_shortcut: String,
    /// What "Default" gives New task here ("" on Linux: off).
    task_default: String,
    voice_enabled: bool,
    voice_language: String,
    voice_languages: Vec<Language>,
    voice_model: crate::voice::ModelStatus,
    keep_in_tray: bool,
    ask_where_to_save: bool,
    /// The OS login item is the truth (LaunchAgent / registry / autostart .desktop file).
    launch_at_login: bool,
    start_hidden: bool,
    /// A global shortcut the OS refused (taken by another app).
    shortcut_problem: Option<String>,
    /// Auto-updates (BRAI-373): on / off, channel, last check, status.
    updates: crate::updater::View,
    /// A section to scroll to once ("servers" after Manage Servers…).
    focus: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LetterView {
    id: &'static str,
    name: &'static str,
    letter: char,
    default: char,
}

#[derive(Debug, Clone, Serialize)]
pub struct Language {
    code: &'static str,
    name: &'static str,
}

pub fn view(app: &AppHandle) -> PrefsView {
    let voice_model = crate::voice::model_status(app);
    let updates = crate::updater::view(app);
    let s = state(app);
    let inner = s.lock();
    let prefs = &inner.settings.preferences;
    PrefsView {
        app_version: APP_VERSION,
        platform: crate::api::platform(),
        server: inner.current_origin().map(|o| inner.settings.label_of(&o)),
        signed_in: inner.phase == Phase::InApp,
        zoom_percent: (prefs.zoom * 100.0).round() as u32,
        leader_shortcut: crate::shortcuts::label_of(prefs.leader_shortcut.as_deref()),
        leader_letters: crate::leader::LeaderAction::ALL
            .into_iter()
            .map(|a| LetterView {
                id: a.id(),
                name: a.name(),
                letter: prefs.leader_letters.get(a),
                default: a.default_letter(),
            })
            .collect(),
        capture_shortcut: crate::shortcuts::label_of(prefs.capture_shortcut.as_deref()),
        switcher_shortcut: crate::shortcuts::label_of(prefs.switcher_shortcut.as_deref()),
        voice_shortcut: crate::shortcuts::label_of(prefs.voice_shortcut.as_deref()),
        task_shortcut: crate::shortcuts::label_of(prefs.task_shortcut.as_deref()),
        task_default: crate::shortcuts::label_of(crate::shortcuts::DEFAULT_TASK),
        voice_enabled: prefs.voice_enabled,
        voice_language: prefs.voice_language.clone(),
        voice_languages: crate::voice::LANGUAGES
            .iter()
            .map(|(code, name)| Language { code, name })
            .collect(),
        voice_model,
        keep_in_tray: prefs.keep_in_tray,
        ask_where_to_save: prefs.ask_where_to_save,
        launch_at_login: launch_at_login(app),
        start_hidden: prefs.start_hidden,
        shortcut_problem: inner.hotkey_error.clone(),
        updates,
        focus: None,
    }
}

#[tauri::command]
pub fn settings_get(app: AppHandle, webview: Webview) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    let mut view = view(&app);
    view.focus = state(&app).lock().settings_focus.take();
    Ok(view)
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

/// Changes a global shortcut (`which`: `leader`, `capture`, `switcher`, `voice`, `task`). It is
/// registered right away; when the OS refuses it (another app has it) the previous shortcuts stay
/// and the error says why.
#[tauri::command]
pub fn settings_set_shortcut(
    app: AppHandle,
    webview: Webview,
    which: String,
    choice: ShortcutChoice,
) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    use crate::hotkeys::HotkeyAction;
    let action = match which.as_str() {
        "leader" => HotkeyAction::Leader,
        "capture" => HotkeyAction::Capture,
        "switcher" => HotkeyAction::Switcher,
        "voice" => HotkeyAction::Voice,
        "task" => HotkeyAction::Task,
        _ => return Err("unknown shortcut".into()),
    };
    let value = match choice {
        ShortcutChoice::Off => None,
        // New task has no default on Linux (the terminal's keys): Default = Off there
        ShortcutChoice::Default => match action {
            HotkeyAction::Leader => Some(crate::shortcuts::DEFAULT_LEADER),
            HotkeyAction::Capture => Some(crate::shortcuts::DEFAULT_CAPTURE),
            HotkeyAction::Switcher => Some(crate::shortcuts::DEFAULT_SWITCHER),
            HotkeyAction::Voice => Some(crate::shortcuts::DEFAULT_VOICE),
            HotkeyAction::Task => crate::shortcuts::DEFAULT_TASK,
        }
        .map(str::to_string),
        ShortcutChoice::Keys(keys) => {
            let shortcut = crate::shortcuts::parse(&keys).map_err(|e| e.to_string())?;
            if crate::servers::takes_server_keys(&shortcut, cfg!(target_os = "macos")) {
                return Err("These keys switch servers in the app's menu (Servers ▸). Choose other keys.".into());
            }
            Some(crate::shortcuts::canonical(&shortcut))
        }
    };
    let label = crate::shortcuts::label_of(value.as_deref());
    let previous = state(&app).lock().settings.preferences.clone();
    let mut next = previous.clone();
    match action {
        HotkeyAction::Leader => next.leader_shortcut = value,
        HotkeyAction::Capture => next.capture_shortcut = value,
        HotkeyAction::Switcher => next.switcher_shortcut = value,
        HotkeyAction::Voice => next.voice_shortcut = value,
        HotkeyAction::Task => next.task_shortcut = value,
    }
    if let Some(other) = crate::hotkeys::conflict(&next, action) {
        return Err(format!("{} already uses {label}.", other.name()));
    }
    crate::hotkeys::validate(&next)?;
    state(&app).lock().settings.preferences = next;
    if let Err(errors) = crate::hotkeys::apply_each(&app) {
        // only a refusal of the changed shortcut undoes the change (another one may have been
        // refused since the start, e.g. taken by an app that is still running)
        if let Some((_, e)) = errors.into_iter().find(|(a, _)| *a == action) {
            state(&app).lock().settings.preferences = previous;
            let _ = crate::hotkeys::apply(&app);
            return Err(e);
        }
    }
    state(&app).lock().save_settings();
    crate::menu::refresh_shortcut_labels(&app);
    crate::tray::refresh(&app);
    Ok(view(&app))
}

/// The letter of one action after the Brainitive key (`letter`: `None` = its default). A–Z only,
/// different from the other actions' letters.
#[tauri::command]
pub fn settings_set_leader_letter(
    app: AppHandle,
    webview: Webview,
    action: String,
    letter: Option<String>,
) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    let action = crate::leader::LeaderAction::from_id(&action).ok_or("unknown action")?;
    let letter = letter.unwrap_or_else(|| action.default_letter().to_string());
    {
        let s = state(&app);
        let mut inner = s.lock();
        let next = inner
            .settings
            .preferences
            .leader_letters
            .with(action, &letter)
            .map_err(|e| e.to_string())?;
        inner.settings.preferences.leader_letters = next;
        inner.save_settings();
    }
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
        set_launch_at_login(&app, value)?;
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
            "voice_enabled" => prefs.voice_enabled = value,
            // the loop reads it on its next tick; nothing to start or stop
            "auto_update_check" => prefs.auto_update_check = value,
            _ => return Err("unknown setting".into()),
        }
        inner.save_settings();
    }
    if key == "voice_enabled" {
        if !value {
            crate::voice::turned_off(&app);
        }
        crate::voice::enabled_changed(&app);
    }
    Ok(view(&app))
}

/// "Get beta updates" (BRAI-373): `stable` or `beta`.
#[tauri::command]
pub fn settings_set_update_channel(
    app: AppHandle,
    webview: Webview,
    channel: crate::updater::Channel,
) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    crate::updater::set_channel(&app, channel)?;
    Ok(view(&app))
}

/// "Check now": runs in the background; the window polls `settings_get` for the status.
#[tauri::command]
pub fn settings_check_for_updates(app: AppHandle, webview: Webview) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    crate::updater::check_now(&app, crate::updater::Trigger::Window)?;
    Ok(view(&app))
}

/// "Restart to Update": the user's click installs the waiting update and restarts.
#[tauri::command]
pub fn settings_restart_to_update(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    crate::updater::restart_to_update(&app)
}

/// The language voice capture listens for ("auto" or one of `voice::LANGUAGES`).
#[tauri::command]
pub fn settings_set_voice_language(app: AppHandle, webview: Webview, language: String) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    if !crate::voice::is_language(&language) {
        return Err("unknown language".into());
    }
    {
        let s = state(&app);
        let mut inner = s.lock();
        inner.settings.preferences.voice_language = language;
        inner.save_settings();
    }
    Ok(view(&app))
}

/// "Remove voice model": frees the disk space; the next voice capture downloads it again.
#[tauri::command]
pub async fn settings_remove_voice_model(app: AppHandle, webview: Webview) -> Result<PrefsView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    crate::voice::remove_model(&app)?;
    Ok(view(&app))
}

// ── Settings → Servers ─────────────────────────────────────────────────────────

/// One saved server in Settings → Servers.
#[derive(Debug, Clone, Serialize)]
pub struct ServerRow {
    #[serde(flatten)]
    choice: crate::servers::Choice,
    /// What the name is when the field is emptied.
    default_name: String,
    /// A refresh token is in the keychain (signed in on this computer).
    signed_in: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServersView {
    servers: Vec<ServerRow>,
    max: usize,
}

/// The saved servers in their order; "signed in" is one keychain read per server.
async fn servers_view(app: &AppHandle) -> ServersView {
    let rows: Vec<(crate::servers::Choice, crate::server::ServerOrigin, String)> = {
        let s = state(app);
        let inner = s.lock();
        let choices = crate::servers::choices(&inner.settings, cfg!(target_os = "macos"));
        choices
            .into_iter()
            .zip(inner.settings.recent.iter())
            .map(|(c, r)| (c, r.origin.clone(), crate::servers::default_name(&r.origin, &r.name)))
            .collect()
    };
    let mut servers = Vec::with_capacity(rows.len());
    for (choice, origin, default_name) in rows {
        let signed_in = state(app).session.has_stored_sign_in(&origin).await;
        servers.push(ServerRow {
            choice,
            default_name,
            signed_in,
        });
    }
    ServersView {
        servers,
        max: crate::servers::MAX_SERVERS,
    }
}

fn saved(app: &AppHandle, id: &str) -> Result<crate::server::ServerOrigin, String> {
    crate::flows::saved_server(app, id).map_err(|e| e.message)
}

#[tauri::command]
pub async fn settings_servers(app: AppHandle, webview: Webview) -> Result<ServersView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    Ok(servers_view(&app).await)
}

/// A name of the user's own (empty: back to the default).
#[tauri::command]
pub async fn settings_rename_server(
    app: AppHandle,
    webview: Webview,
    id: String,
    name: String,
) -> Result<ServersView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    if name.chars().count() > 500 {
        return Err("That name is too long.".into());
    }
    {
        let s = state(&app);
        let mut inner = s.lock();
        if !inner.settings.rename(&id, &name) {
            return Err("That server is not in your list any more.".into());
        }
        inner.save_settings();
    }
    crate::flows::servers_changed(&app);
    Ok(servers_view(&app).await)
}

/// ↑ / ↓: the order is the ⌃⌘1…9 / Alt+1…9 order.
#[tauri::command]
pub async fn settings_move_server(
    app: AppHandle,
    webview: Webview,
    id: String,
    direction: crate::servers::Move,
) -> Result<ServersView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    let moved = {
        let s = state(&app);
        let mut inner = s.lock();
        let moved = crate::servers::move_server(&mut inner.settings.recent, &id, direction);
        if moved {
            inner.save_settings();
        }
        moved
    };
    if moved {
        crate::flows::servers_changed(&app);
    }
    Ok(servers_view(&app).await)
}

/// "Switch to": the main window comes to the front on that server (or its sign-in screen).
#[tauri::command]
pub async fn settings_switch_server(app: AppHandle, webview: Webview, id: String) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    saved(&app, &id)?;
    crate::menu::pick_server(&app, &id);
    Ok(())
}

/// "Sign out": that server's device, keychain entry, queue, offline files and webview data go;
/// it stays in the list.
#[tauri::command]
pub async fn settings_sign_out_server(app: AppHandle, webview: Webview, id: String) -> Result<ServersView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    let origin = saved(&app, &id)?;
    crate::flows::sign_out_server(&app, origin).await;
    Ok(servers_view(&app).await)
}

/// "Forget" (second click): the existing flow — signed out, its data deleted, off the list.
#[tauri::command]
pub async fn settings_forget_server(app: AppHandle, webview: Webview, id: String) -> Result<ServersView, String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    let origin = saved(&app, &id)?;
    crate::flows::forget_server(&app, origin).await;
    Ok(servers_view(&app).await)
}

/// "Add Server…": the connect screen in the main window (the current server stays signed in).
#[tauri::command]
pub fn settings_add_server(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Settings)?;
    popup::dismiss(&app, Popup::Settings);
    crate::menu::add_server(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_a_start_by_the_login_item() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        assert!(launched_at_login(args(&[
            "/Applications/Brainitive.app/Contents/MacOS/brainitive-desktop",
            "--autostart"
        ])));
        assert!(!launched_at_login(args(&["brainitive-desktop"])));
        assert!(!launched_at_login(args(&["brainitive-desktop", "--autostart=1"])));
    }
}
