//! The app menu (BRAI-362): macOS menu bar / the window menu on Windows and Linux, with ⌘ on
//! macOS and Ctrl elsewhere (`CmdOrCtrl`). Menu actions go to the last focused **app window**
//! (the main window or an `app-<n>` window), never to the bundled popups.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Wry};

use crate::shell;
use crate::state::state;

/// What a click on an app menu item does. Ids are `MenuAction::id()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Settings,
    NewIssue,
    NewDoc,
    NewWindow,
    QuickCapture,
    QuickSwitcher,
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
}

impl MenuAction {
    pub const ALL: [MenuAction; 21] = [
        Self::Settings,
        Self::NewIssue,
        Self::NewDoc,
        Self::NewWindow,
        Self::QuickCapture,
        Self::QuickSwitcher,
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
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::NewIssue => "new-issue",
            Self::NewDoc => "new-doc",
            Self::NewWindow => "new-window",
            Self::QuickCapture => "quick-capture",
            Self::QuickSwitcher => "quick-switcher",
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
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// The keyboard shortcut (`CmdOrCtrl` = ⌘ on macOS, Ctrl on Windows / Linux).
    pub fn accelerator(self) -> Option<&'static str> {
        Some(match self {
            Self::Settings => "CmdOrCtrl+,",
            Self::NewIssue => "CmdOrCtrl+N",
            Self::NewDoc => "CmdOrCtrl+Alt+N",
            Self::NewWindow => "CmdOrCtrl+Shift+N",
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
    let capture = item(app, MenuAction::QuickCapture, "Quick Capture…")?;
    let switcher = item(app, MenuAction::QuickSwitcher, "Quick Switcher…")?;

    let menu = Menu::new(app)?;

    #[cfg(target_os = "macos")]
    {
        let app_menu = Submenu::with_items(
            app,
            "Brainitive",
            true,
            &[
                &PredefinedMenuItem::about(app, Some("About Brainitive"), None)?,
                &sep()?,
                &settings,
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
            &item(app, MenuAction::NewWindow, "New Window")?,
            &sep()?,
            &capture,
            &switcher,
            &sep()?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    if !mac {
        file.append(&sep()?)?;
        file.append(&settings)?;
        file.append(&sep()?)?;
        file.append(&PredefinedMenuItem::quit(app, Some("Quit Brainitive"))?)?;
    }
    menu.append(&file)?;

    menu.append(&Submenu::with_items(
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
    )?)?;

    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item(app, MenuAction::Back, "Back")?,
            &item(app, MenuAction::Forward, "Forward")?,
            &item(app, MenuAction::Reload, "Reload")?,
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

    menu.append(&Submenu::with_items(
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
    )?)?;

    let current = MenuItem::with_id(app, "server-current", "Not connected", false, None::<&str>)?;
    menu.append(&Submenu::with_items(
        app,
        "Server",
        true,
        &[
            &current,
            &sep()?,
            &item(app, MenuAction::ServerSwitch, "Switch Server…")?,
            &item(app, MenuAction::ServerSignOut, "Sign Out")?,
        ],
    )?)?;

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

    let help = Submenu::with_items(app, "Help", true, &[&item(app, MenuAction::HelpDocs, "Brainitive Help")?])?;
    if !mac {
        help.append(&sep()?)?;
        help.append(&PredefinedMenuItem::about(app, Some("About Brainitive"), None)?)?;
    }
    menu.append(&help)?;

    app.set_menu(menu)?;
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.server_menu_item = Some(current);
        inner.capture_menu_item = Some(capture);
        inner.switcher_menu_item = Some(switcher);
    }
    refresh_shortcut_labels(app);
    Ok(())
}

/// Shows the global shortcuts next to Quick Capture / Quick Switcher (display only: the OS
/// delivers the global shortcut before the menu would see it).
pub fn refresh_shortcut_labels(app: &AppHandle) {
    let (prefs, capture, switcher) = {
        let s = state(app);
        let inner = s.lock();
        (
            inner.settings.preferences.clone(),
            inner.capture_menu_item.clone(),
            inner.switcher_menu_item.clone(),
        )
    };
    let accel = |a: Option<&str>| {
        a.and_then(|a| crate::shortcuts::parse(a).ok())
            .map(|s| crate::shortcuts::canonical(&s))
    };
    if let Some(item) = capture {
        let _ = item.set_accelerator(accel(prefs.capture_shortcut.as_deref()));
    }
    if let Some(item) = switcher {
        let _ = item.set_accelerator(accel(prefs.switcher_shortcut.as_deref()));
    }
}

/// Every menu event of the app menu (tray items are handled in `tray.rs`).
pub fn on_menu_event(app: &AppHandle, id: &str) {
    let Some(action) = MenuAction::from_id(id) else {
        return;
    };
    match action {
        MenuAction::Settings => crate::popup::show(app, crate::popup::Popup::Settings),
        MenuAction::QuickCapture => crate::capture::open_quick_capture(app, None),
        MenuAction::QuickSwitcher => crate::popup::show(app, crate::popup::Popup::Switcher),
        MenuAction::NewWindow => {
            let current = shell::target_window(app).and_then(|w| w.url().ok());
            let origin = state(app).lock().current_origin();
            if let Some(origin) = origin {
                let url = current
                    .filter(|u| origin.contains(u))
                    .unwrap_or_else(|| origin.join("/dashboard"));
                if let Err(e) = shell::open_app_window(app, url) {
                    log::info!("no new window: {e}");
                }
            }
        }
        MenuAction::Back | MenuAction::Forward => {
            if let Some(w) = shell::target_window(app).filter(|_| shell::in_app(app)) {
                let _ = w.eval(if action == MenuAction::Back {
                    "history.back()"
                } else {
                    "history.forward()"
                });
            }
        }
        MenuAction::Reload => {
            if let Some(w) = shell::target_window(app) {
                let _ = w.reload();
            }
        }
        MenuAction::ZoomReset => shell::zoom(app, 0),
        MenuAction::ZoomIn => shell::zoom(app, 1),
        MenuAction::ZoomOut => shell::zoom(app, -1),
        MenuAction::NewIssue | MenuAction::NewDoc | MenuAction::Search => {
            let Some((command, key)) = action.web_command() else { return };
            if let Some(w) = shell::target_window(app).filter(|_| shell::in_app(app)) {
                let _ = w.set_focus();
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
    }

    #[test]
    fn accelerators_are_unique_and_use_cmd_or_ctrl() {
        let mut seen = Vec::new();
        for a in MenuAction::ALL {
            if let Some(acc) = a.accelerator() {
                assert!(acc.starts_with("CmdOrCtrl+"), "{acc}");
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
