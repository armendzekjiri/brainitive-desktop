//! The app menu (BRAI-362): the macOS menu bar; on Windows and Linux the same menus open from the
//! main window's top bar (File, Edit, View, Go, Server, Help buttons; the OS menu bar is hidden,
//! its shortcuts still work), with ⌘ on macOS and Ctrl elsewhere (`CmdOrCtrl`). Menu actions go
//! to the **active tab**, never to the bundled popups.

use tauri::menu::{CheckMenuItem, ContextMenu, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, LogicalPosition, Window, Wry};

use crate::shell;
use crate::state::state;

/// What a click on an app menu item does. Ids are `MenuAction::id()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Settings,
    NewIssue,
    NewDoc,
    NewTab,
    CloseTab,
    ReopenTab,
    NextTab,
    PreviousTab,
    /// "Brainitive Key…" (BRAI-440): the overlay, the key shown as its accelerator.
    BrainitiveKey,
    QuickCapture,
    QuickSwitcher,
    VoiceCapture,
    Back,
    Forward,
    Reload,
    ZoomReset,
    ZoomIn,
    ZoomOut,
    Search,
    GoDashboard,
    GoMyWork,
    GoCaptures,
    GoNotifications,
    AccountSettings,
    ServerSwitch,
    ServerSignOut,
    HelpDocs,
    HelpCopyDiagnostics,
    HelpOpenLogs,
    /// "Check for Updates…", or "Restart to Update (x.y.z)" when one is ready (BRAI-373).
    CheckForUpdates,
    /// Servers ▸ Add Server…: the Choose screen, without signing out of the current server.
    AddServer,
    /// Servers ▸ Manage Servers…: Settings → Servers.
    ManageServers,
}

/// Servers ▸ one item per saved server: `server-pick:<id>` (`servers::server_id`).
pub const SERVER_PICK: &str = "server-pick:";
/// A tab's context menu: `tab-ctx:<action>:<label>`.
pub const TAB_MENU: &str = "tab-ctx:";

/// The menus the top bar shows as buttons on Windows / Linux: id and text.
pub const TOPBAR_MENUS: [(&str, &str); 6] = [
    ("file", "File"),
    ("edit", "Edit"),
    ("view", "View"),
    ("go", "Go"),
    ("server", "Server"),
    ("help", "Help"),
];

impl MenuAction {
    pub const ALL: [MenuAction; 32] = [
        Self::Settings,
        Self::NewIssue,
        Self::NewDoc,
        Self::NewTab,
        Self::CloseTab,
        Self::ReopenTab,
        Self::NextTab,
        Self::PreviousTab,
        Self::BrainitiveKey,
        Self::QuickCapture,
        Self::QuickSwitcher,
        Self::VoiceCapture,
        Self::Back,
        Self::Forward,
        Self::Reload,
        Self::ZoomReset,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::Search,
        Self::GoDashboard,
        Self::GoMyWork,
        Self::GoCaptures,
        Self::GoNotifications,
        Self::AccountSettings,
        Self::ServerSwitch,
        Self::ServerSignOut,
        Self::HelpDocs,
        Self::HelpCopyDiagnostics,
        Self::HelpOpenLogs,
        Self::CheckForUpdates,
        Self::AddServer,
        Self::ManageServers,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::NewIssue => "new-issue",
            Self::NewDoc => "new-doc",
            Self::NewTab => "new-tab",
            Self::CloseTab => "close-tab",
            Self::ReopenTab => "reopen-tab",
            Self::NextTab => "next-tab",
            Self::PreviousTab => "previous-tab",
            Self::BrainitiveKey => "brainitive-key",
            Self::QuickCapture => "quick-capture",
            Self::QuickSwitcher => "quick-switcher",
            Self::VoiceCapture => "voice-capture",
            Self::Back => "back",
            Self::Forward => "forward",
            Self::Reload => "reload",
            Self::ZoomReset => "zoom-reset",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
            Self::Search => "search",
            Self::GoDashboard => "go-dashboard",
            Self::GoMyWork => "go-my-work",
            Self::GoCaptures => "go-captures",
            Self::GoNotifications => "go-notifications",
            Self::AccountSettings => "account-settings",
            Self::ServerSwitch => "server-switch",
            Self::ServerSignOut => "server-sign-out",
            Self::HelpDocs => "help-docs",
            Self::HelpCopyDiagnostics => "help-copy-diagnostics",
            Self::HelpOpenLogs => "help-open-logs",
            Self::CheckForUpdates => "check-for-updates",
            Self::AddServer => "servers-add",
            Self::ManageServers => "servers-manage",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The keyboard shortcut (`CmdOrCtrl` = ⌘ on macOS, Ctrl on Windows / Linux; switching tabs is
    /// ⌃Tab everywhere, as in browsers).
    pub fn accelerator(self) -> Option<&'static str> {
        Some(match self {
            Self::Settings => "CmdOrCtrl+,",
            Self::NewIssue => "CmdOrCtrl+N",
            Self::NewDoc => "CmdOrCtrl+Alt+N",
            Self::NewTab => "CmdOrCtrl+T",
            Self::CloseTab => "CmdOrCtrl+W",
            Self::ReopenTab => "CmdOrCtrl+Shift+T",
            Self::NextTab => "Ctrl+Tab",
            Self::PreviousTab => "Ctrl+Shift+Tab",
            Self::Back => "CmdOrCtrl+[",
            Self::Forward => "CmdOrCtrl+]",
            Self::Reload => "CmdOrCtrl+R",
            Self::ZoomReset => "CmdOrCtrl+0",
            Self::ZoomIn => "CmdOrCtrl+=",
            Self::ZoomOut => "CmdOrCtrl+-",
            Self::Search => "CmdOrCtrl+K",
            Self::GoDashboard => "CmdOrCtrl+Shift+H",
            Self::GoMyWork => "CmdOrCtrl+Shift+M",
            _ => return None,
        })
    }

    /// A page of the server the action opens.
    pub fn web_path(self) -> Option<&'static str> {
        Some(match self {
            Self::GoDashboard => "/dashboard",
            Self::GoMyWork => "/my-work",
            Self::GoCaptures => "/captures",
            Self::GoNotifications => "/notifications",
            Self::AccountSettings => "/profile",
            _ => return None,
        })
    }

    /// A command of the web app (new issue, new doc, search): first offered to the page as a
    /// cancelable `brainitive-desktop:command` event, else its own ⌘K / ⌘. palette shortcut.
    pub fn web_command(self) -> Option<(&'static str, char)> {
        match self {
            Self::NewIssue => Some(("new-issue", '.')),
            Self::NewDoc => Some(("new-doc", '.')),
            Self::Search => Some(("search", 'k')),
            _ => None,
        }
    }
}

/// The script that runs a web command in the page. Only static values go in (no user input).
pub fn web_command_script(command: &str, fallback_key: char) -> String {
    format!(
        "(() => {{ const e = new CustomEvent('brainitive-desktop:command', {{ detail: {{ command: '{command}' }}, cancelable: true }}); \
         if (window.dispatchEvent(e)) {{ window.dispatchEvent(new KeyboardEvent('keydown', {{ key: '{fallback_key}', metaKey: true, ctrlKey: true, bubbles: true, cancelable: true }})); }} }})()"
    )
}

fn item(app: &AppHandle, action: MenuAction, text: &str) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, action.id(), text, true, action.accelerator())
}

/// Builds and installs the menu; keeps the items whose text / shortcut changes later.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let mac = cfg!(target_os = "macos");
    let sep = || PredefinedMenuItem::separator(app);

    let settings = item(app, MenuAction::Settings, "Settings…")?;
    let leader = item(app, MenuAction::BrainitiveKey, "Brainitive Key…")?;
    let capture = item(app, MenuAction::QuickCapture, "Quick Capture…")?;
    let switcher = item(app, MenuAction::QuickSwitcher, "Quick Switcher…")?;
    let voice = item(app, MenuAction::VoiceCapture, "Voice Capture…")?;
    let updates = item(app, MenuAction::CheckForUpdates, "Check for Updates…")?;
    // the saved servers, filled by `refresh_servers`
    let servers = Submenu::with_id(app, "servers", "Servers", true)?;

    let menu = Menu::new(app)?;

    #[cfg(target_os = "macos")]
    {
        let app_menu = Submenu::with_items(
            app,
            "Brainitive",
            true,
            &[
                &PredefinedMenuItem::about(app, Some("About Brainitive"), None)?,
                &updates,
                &sep()?,
                &settings,
                &servers,
                &sep()?,
                &PredefinedMenuItem::services(app, None)?,
                &sep()?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::show_all(app, None)?,
                &sep()?,
                &PredefinedMenuItem::quit(app, None)?,
            ],
        )?;
        menu.append(&app_menu)?;
    }

    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item(app, MenuAction::NewIssue, "New Issue")?,
            &item(app, MenuAction::NewDoc, "New Doc")?,
            &sep()?,
            &item(app, MenuAction::NewTab, "New Tab")?,
            &item(app, MenuAction::CloseTab, "Close Tab")?,
            &item(app, MenuAction::ReopenTab, "Reopen Closed Tab")?,
            &sep()?,
            &leader,
            &capture,
            &switcher,
            &voice,
        ],
    )?;
    if !mac {
        file.append(&sep()?)?;
        file.append(&servers)?;
        file.append(&settings)?;
        file.append(&sep()?)?;
        file.append(&PredefinedMenuItem::quit(app, Some("Quit Brainitive"))?)?;
    }
    menu.append(&file)?;

    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &sep()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    menu.append(&edit)?;

    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item(app, MenuAction::Back, "Back")?,
            &item(app, MenuAction::Forward, "Forward")?,
            &item(app, MenuAction::Reload, "Reload")?,
            &sep()?,
            &item(app, MenuAction::NextTab, "Next Tab")?,
            &item(app, MenuAction::PreviousTab, "Previous Tab")?,
            &sep()?,
            &item(app, MenuAction::ZoomReset, "Actual Size")?,
            &item(app, MenuAction::ZoomIn, "Zoom In")?,
            &item(app, MenuAction::ZoomOut, "Zoom Out")?,
        ],
    )?;
    if mac {
        view.append(&sep()?)?;
        view.append(&PredefinedMenuItem::fullscreen(app, None)?)?;
    }
    menu.append(&view)?;

    let go = Submenu::with_items(
        app,
        "Go",
        true,
        &[
            &item(app, MenuAction::Search, "Search…")?,
            &sep()?,
            &item(app, MenuAction::GoDashboard, "Dashboard")?,
            &item(app, MenuAction::GoMyWork, "My Work")?,
            &item(app, MenuAction::GoCaptures, "Captures")?,
            &item(app, MenuAction::GoNotifications, "Notifications")?,
            &sep()?,
            &item(app, MenuAction::AccountSettings, "Account Settings")?,
        ],
    )?;
    menu.append(&go)?;

    let current = MenuItem::with_id(app, "server-current", "Not connected", false, None::<&str>)?;
    let server = Submenu::with_items(
        app,
        "Server",
        true,
        &[
            &current,
            &sep()?,
            &item(app, MenuAction::ServerSwitch, "Switch Server…")?,
            &item(app, MenuAction::ServerSignOut, "Sign Out")?,
        ],
    )?;
    menu.append(&server)?;

    #[cfg(target_os = "macos")]
    menu.append(&Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )?)?;

    let help = Submenu::with_items(
        app,
        "Help",
        true,
        &[
            &item(app, MenuAction::HelpDocs, "Brainitive Help")?,
            &sep()?,
            &item(app, MenuAction::HelpCopyDiagnostics, "Copy Diagnostics")?,
            &item(app, MenuAction::HelpOpenLogs, "Open Logs Folder")?,
        ],
    )?;
    if !mac {
        help.append(&sep()?)?;
        help.append(&updates)?;
        help.append(&PredefinedMenuItem::about(app, Some("About Brainitive"), None)?)?;
    }
    menu.append(&help)?;

    app.set_menu(menu)?;
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.server_menu_item = Some(current);
        inner.servers_menu = Some(servers);
        inner.leader_menu_item = Some(leader);
        inner.capture_menu_item = Some(capture);
        inner.switcher_menu_item = Some(switcher);
        inner.voice_menu_item = Some(voice);
        inner.update_menu_item = Some(updates);
        inner.topbar_menus = vec![
            ("file", file),
            ("edit", edit),
            ("view", view),
            ("go", go),
            ("server", server),
            ("help", help),
        ];
    }
    refresh_shortcut_labels(app);
    refresh_update_item(app);
    refresh_servers(app);
    Ok(())
}

/// Menu text of a server name: `&` is a mnemonic marker in Windows / GTK menus.
fn menu_text(name: &str, mac: bool) -> String {
    if mac {
        name.to_string()
    } else {
        name.replace('&', "&&")
    }
}

/// Fills Servers ▸ with the saved servers (the current one checked, ⌃⌘1…9 / Alt+1…9 on the
/// first nine — menu accelerators only, never global), then Add Server… and Manage Servers….
pub fn refresh_servers(app: &AppHandle) {
    let mac = cfg!(target_os = "macos");
    let (submenu, choices) = {
        let s = state(app);
        let inner = s.lock();
        (
            inner.servers_menu.clone(),
            crate::servers::choices(&inner.settings, mac),
        )
    };
    let Some(submenu) = submenu else { return };
    let result = (|| -> tauri::Result<()> {
        for old in submenu.items()? {
            submenu.remove(&old)?;
        }
        for (i, c) in choices.iter().enumerate() {
            let accelerator = crate::servers::accelerator(i, mac);
            submenu.append(&CheckMenuItem::with_id(
                app,
                format!("{SERVER_PICK}{}", c.id),
                menu_text(&c.name, mac),
                true,
                c.current,
                accelerator.as_deref(),
            )?)?;
        }
        if !choices.is_empty() {
            submenu.append(&PredefinedMenuItem::separator(app)?)?;
        }
        submenu.append(&item(app, MenuAction::AddServer, "Add Server…")?)?;
        submenu.append(&item(app, MenuAction::ManageServers, "Manage Servers…")?)?;
        Ok(())
    })();
    if let Err(e) = result {
        log::warn!("servers menu: {e}");
    }
}

/// A click on a server (menu, tray, panel): switch in the background (not on the main thread).
pub fn pick_server(app: &AppHandle, id: &str) {
    let h = app.clone();
    let id = id.to_string();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::flows::switch_to(&h, &id).await {
            log::info!("switch refused: {}", e.message);
        }
        // a check item flips itself on click: put the marks back to the truth
        refresh_servers(&h);
    });
}

/// Servers ▸ Manage Servers… (also the tray and the panel): Settings, scrolled to Servers.
pub fn manage_servers(app: &AppHandle) {
    state(app).lock().settings_focus = Some("servers");
    crate::popup::show(app, crate::popup::Popup::Settings);
}

/// Servers ▸ Add Server…: the Choose screen in the main window; the current server stays signed
/// in (and one click away).
pub fn add_server(app: &AppHandle) {
    shell::focus_main(app);
    crate::flows::switch_server(app);
}

/// The update item follows the updater: disabled while updates are off for this build or a check /
/// download runs, "Restart to Update (x.y.z)" when an update waits.
pub fn refresh_update_item(app: &AppHandle) {
    let (item, text, enabled) = {
        let s = state(app);
        let inner = s.lock();
        let (text, enabled) = crate::updater::menu_item(inner.updater.availability, &inner.updater.status);
        (inner.update_menu_item.clone(), text, enabled)
    };
    if let Some(item) = item {
        let _ = item.set_text(text);
        let _ = item.set_enabled(enabled);
    }
}

/// Shows the global shortcuts next to Brainitive Key / Quick Capture / Quick Switcher / Voice
/// Capture (display only: the OS delivers the global shortcut before the menu would see it).
pub fn refresh_shortcut_labels(app: &AppHandle) {
    let (prefs, leader, capture, switcher, voice) = {
        let s = state(app);
        let inner = s.lock();
        (
            inner.settings.preferences.clone(),
            inner.leader_menu_item.clone(),
            inner.capture_menu_item.clone(),
            inner.switcher_menu_item.clone(),
            inner.voice_menu_item.clone(),
        )
    };
    let accel = |a: Option<&str>| {
        a.and_then(|a| crate::shortcuts::parse(a).ok())
            .map(|s| crate::shortcuts::canonical(&s))
    };
    if let Some(item) = leader {
        let _ = item.set_accelerator(accel(prefs.leader_shortcut.as_deref()));
    }
    if let Some(item) = capture {
        let _ = item.set_accelerator(accel(prefs.capture_shortcut.as_deref()));
    }
    if let Some(item) = switcher {
        let _ = item.set_accelerator(accel(prefs.switcher_shortcut.as_deref()));
    }
    if let Some(item) = voice {
        let _ = item.set_accelerator(accel(prefs.active_voice_shortcut().as_deref()));
    }
}

/// A menu opened from the top bar at `at` (the bar's CSS pixels = the window's logical pixels):
/// one of [`TOPBAR_MENUS`], `servers` (the saved servers) or `tab` (a tab's context menu).
pub fn popup_topbar_menu(
    app: &AppHandle,
    window: &Window,
    which: &str,
    tab: Option<&str>,
    at: LogicalPosition<f64>,
) -> Result<(), String> {
    let err = |e: tauri::Error| e.to_string();
    match which {
        "servers" => {
            refresh_servers(app);
            let menu = state(app).lock().servers_menu.clone().ok_or("no menu")?;
            menu.popup_at(window.clone(), at).map_err(err)
        }
        "tab" => {
            let tab = tab.ok_or("no tab")?;
            let several = state(app).lock().tabs.list.len() > 1;
            let id = |action: &str| format!("{TAB_MENU}{action}:{tab}");
            let new =
                MenuItem::with_id(app, id("new"), "New Tab", true, MenuAction::NewTab.accelerator()).map_err(err)?;
            let duplicate =
                MenuItem::with_id(app, id("duplicate"), "Duplicate Tab", true, None::<&str>).map_err(err)?;
            let reload = MenuItem::with_id(app, id("reload"), "Reload", true, None::<&str>).map_err(err)?;
            let sep = PredefinedMenuItem::separator(app).map_err(err)?;
            let close = MenuItem::with_id(app, id("close"), "Close Tab", several, None::<&str>).map_err(err)?;
            let others =
                MenuItem::with_id(app, id("close-others"), "Close Other Tabs", several, None::<&str>).map_err(err)?;
            let menu = Menu::with_items(app, &[&new, &duplicate, &reload, &sep, &close, &others]).map_err(err)?;
            menu.popup_at(window.clone(), at).map_err(err)
        }
        // a narrow window: every app menu in one
        "all" => {
            let menus: Vec<Submenu<Wry>> = state(app).lock().topbar_menus.iter().map(|(_, m)| m.clone()).collect();
            let items: Vec<&dyn tauri::menu::IsMenuItem<Wry>> =
                menus.iter().map(|m| m as &dyn tauri::menu::IsMenuItem<Wry>).collect();
            let menu = Menu::with_items(app, &items).map_err(err)?;
            menu.popup_at(window.clone(), at).map_err(err)
        }
        _ => {
            let menu = state(app)
                .lock()
                .topbar_menus
                .iter()
                .find(|(id, _)| *id == which)
                .map(|(_, m)| m.clone())
                .ok_or("no such menu")?;
            menu.popup_at(window.clone(), at).map_err(err)
        }
    }
}

/// `tab-ctx:<action>:<label>`: a tab's context menu (the label is checked against the tabs).
fn on_tab_menu(app: &AppHandle, rest: &str) {
    let Some((action, label)) = rest.split_once(':') else {
        return;
    };
    if state(app).lock().tabs.index(label).is_none() {
        return;
    }
    let (h, action, label) = (app.clone(), action.to_string(), label.to_string());
    // not on the main thread: a tab's webview may be built
    tauri::async_runtime::spawn(async move {
        match action.as_str() {
            "new" => crate::tabs::open_new(&h),
            "duplicate" => crate::tabs::duplicate(&h, &label),
            "reload" => {
                if let Some(w) = tauri::Manager::get_webview(&h, &label).filter(|w| shell::is_app_window(w.label())) {
                    let _ = w.reload();
                }
            }
            "close" => crate::tabs::close(&h, &label),
            "close-others" => crate::tabs::close_others(&h, &label),
            _ => {}
        }
    });
}

/// Every menu event of the app menu (tray items are handled in `tray.rs`).
pub fn on_menu_event(app: &AppHandle, id: &str) {
    if let Some(server) = id.strip_prefix(SERVER_PICK) {
        pick_server(app, server);
        return;
    }
    if let Some(rest) = id.strip_prefix(TAB_MENU) {
        on_tab_menu(app, rest);
        return;
    }
    let Some(action) = MenuAction::from_id(id) else {
        return;
    };
    match action {
        MenuAction::Settings => crate::popup::show(app, crate::popup::Popup::Settings),
        MenuAction::BrainitiveKey => crate::leader::show(app),
        MenuAction::QuickCapture => crate::capture::open_quick_capture(app, None),
        MenuAction::QuickSwitcher => crate::popup::show(app, crate::popup::Popup::Switcher),
        MenuAction::VoiceCapture => crate::voice::start_from_menu(app),
        // not on the main thread (a webview built there can deadlock on Windows)
        MenuAction::NewTab if shell::in_app(app) => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move { crate::tabs::open_new(&h) });
        }
        MenuAction::ReopenTab if shell::in_app(app) => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move { crate::tabs::reopen_closed(&h) });
        }
        MenuAction::CloseTab | MenuAction::NextTab | MenuAction::PreviousTab if shell::in_app(app) => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move {
                match action {
                    MenuAction::CloseTab => {
                        let active = state(&h).lock().tabs.active.clone();
                        crate::tabs::close(&h, &active);
                    }
                    MenuAction::NextTab => crate::tabs::cycle(&h, 1),
                    _ => crate::tabs::cycle(&h, -1),
                }
            });
        }
        MenuAction::NewTab
        | MenuAction::CloseTab
        | MenuAction::ReopenTab
        | MenuAction::NextTab
        | MenuAction::PreviousTab => {}
        MenuAction::Back | MenuAction::Forward => {
            if let Some(w) = shell::active_tab(app).filter(|_| shell::in_app(app)) {
                let _ = w.eval(if action == MenuAction::Back {
                    "history.back()"
                } else {
                    "history.forward()"
                });
            }
        }
        MenuAction::Reload => {
            if let Some(w) = shell::active_tab(app) {
                let _ = w.reload();
            }
        }
        MenuAction::ZoomReset => shell::zoom(app, 0),
        MenuAction::ZoomIn => shell::zoom(app, 1),
        MenuAction::ZoomOut => shell::zoom(app, -1),
        MenuAction::NewIssue | MenuAction::NewDoc | MenuAction::Search => {
            let Some((command, key)) = action.web_command() else {
                return;
            };
            if let Some(w) = shell::active_tab(app).filter(|_| shell::in_app(app)) {
                shell::focus_main(app);
                let _ = w.eval(web_command_script(command, key));
            }
        }
        MenuAction::GoDashboard
        | MenuAction::GoMyWork
        | MenuAction::GoCaptures
        | MenuAction::GoNotifications
        | MenuAction::AccountSettings => {
            if let Some(path) = action.web_path() {
                shell::open_page(app, path);
            }
        }
        MenuAction::ServerSwitch => {
            shell::focus_main(app);
            crate::flows::switch_server(app);
        }
        MenuAction::ServerSignOut => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move { crate::flows::sign_out(&h).await });
        }
        MenuAction::HelpCopyDiagnostics => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move { crate::diagnostics::copy(&h).await });
        }
        MenuAction::HelpOpenLogs => crate::diagnostics::open_logs_folder(app),
        MenuAction::CheckForUpdates => crate::updater::on_menu_click(app),
        MenuAction::AddServer => add_server(app),
        MenuAction::ManageServers => manage_servers(app),
        MenuAction::HelpDocs => {
            if let Ok(url) = url::Url::parse("https://docs.brainitive.app/") {
                shell::open_external(app, &url);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut ids: Vec<&str> = MenuAction::ALL.iter().map(|a| a.id()).collect();
        for a in MenuAction::ALL {
            assert_eq!(MenuAction::from_id(a.id()), Some(a));
            assert!(!a.id().starts_with("tray-"), "tray ids are routed to tray.rs");
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MenuAction::ALL.len());
        assert_eq!(MenuAction::from_id("nope"), None);
        // a server pick is routed before the fixed ids
        for a in MenuAction::ALL {
            assert!(!a.id().starts_with(SERVER_PICK));
        }
        assert_eq!(MenuAction::AddServer.accelerator(), None);
        assert_eq!(MenuAction::ManageServers.accelerator(), None);
    }

    #[test]
    fn accelerators_are_unique_and_use_cmd_or_ctrl() {
        let mut seen = Vec::new();
        for a in MenuAction::ALL {
            if let Some(acc) = a.accelerator() {
                // switching tabs is ⌃Tab on every platform (⌘Tab is the macOS app switcher)
                let tab_switch = matches!(a, MenuAction::NextTab | MenuAction::PreviousTab);
                assert!(
                    acc.starts_with("CmdOrCtrl+") || (tab_switch && acc.starts_with("Ctrl+")),
                    "{acc}"
                );
                assert!(!seen.contains(&acc), "{acc} used twice");
                seen.push(acc);
            }
        }
        // the usual ones
        assert_eq!(MenuAction::Settings.accelerator(), Some("CmdOrCtrl+,"));
        assert_eq!(MenuAction::Search.accelerator(), Some("CmdOrCtrl+K"));
        assert_eq!(MenuAction::Reload.accelerator(), Some("CmdOrCtrl+R"));
    }

    #[test]
    fn web_pages_are_plain_paths() {
        for a in MenuAction::ALL {
            if let Some(p) = a.web_path() {
                assert!(p.starts_with('/') && !p.starts_with("//"), "{p}");
            }
        }
    }

    #[test]
    fn web_command_script_offers_the_event_first() {
        let js = web_command_script("search", 'k');
        assert!(js.contains("'brainitive-desktop:command'"));
        assert!(js.contains("command: 'search'"));
        assert!(js.contains("cancelable: true"));
        assert!(js.contains("key: 'k'"));
    }
}
