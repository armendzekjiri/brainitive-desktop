//! The app's runtime state, managed by Tauri (`app.state::<AppState>()`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;
use tauri::menu::{MenuItem, Submenu};
use tauri::{AppHandle, Manager, WebviewUrl, Wry};
use url::Url;

use crate::api::InstanceInfo;
use crate::queue::CaptureQueue;
use crate::server::ServerOrigin;
use crate::session::Session;
use crate::settings::Settings;

/// Which bundled screen the main window shows (the web app itself is `InApp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// First run / "Switch server…": Brainitive Cloud or a self-hosted server.
    Choose,
    /// A server is chosen, nobody is signed in: "Sign in with your browser".
    SignIn,
    /// The browser is open at `/app/authorize`, waiting for `brainitive://auth/callback`.
    Waiting,
    /// Refreshing the stored sign-in / exchanging the code.
    Connecting,
    /// The server can't be reached (or answered with an error): Retry / open offline.
    Unreachable,
    /// The server needs a newer desktop app (`min_versions.desktop`): the web app is not loaded.
    UpdateRequired,
    /// The main window shows the server.
    InApp,
}

#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    /// "info" | "error"
    pub level: &'static str,
    pub message: String,
}

impl Notice {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            level: "info",
            message: message.into(),
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: "error",
            message: message.into(),
        }
    }
}

/// What the "Please update Brainitive Desktop" screen shows.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateRequired {
    pub server_name: String,
    pub origin: String,
    pub display_name: String,
    pub server_version: Option<String>,
    pub min_version: String,
    pub app_version: &'static str,
    pub download_url: &'static str,
}

impl UpdateRequired {
    pub fn new(server_name: &str, origin: &ServerOrigin, server_version: Option<String>, min_version: &str) -> Self {
        Self {
            server_name: server_name.chars().take(120).collect(),
            origin: origin.as_str().to_string(),
            display_name: origin.display_name(),
            server_version,
            min_version: min_version.chars().take(40).collect(),
            app_version: crate::version::APP_VERSION,
            // the Store build: the app's Store page (BRAI-442)
            download_url: crate::msstore::update_page(crate::msstore::STORE_BUILD, crate::version::DOWNLOAD_URL),
        }
    }
}

/// A main window waiting to be rebuilt with another webview data store.
pub struct Rebuild {
    pub store: [u8; 16],
    pub url: WebviewUrl,
}

pub struct Inner {
    pub settings: Settings,
    pub settings_path: PathBuf,
    /// The current server and its last `/api/v1/instance` answer.
    pub instance: Option<(ServerOrigin, InstanceInfo)>,
    pub phase: Phase,
    pub notice: Option<Notice>,
    /// Why the server can't be used (phase `Unreachable`).
    pub problem: Option<crate::problem::Problem>,
    /// The server that needs a newer app (phase `UpdateRequired`).
    pub update: Option<UpdateRequired>,
    /// Identifies the running auto-reconnect loop; a new value ends the old loop.
    pub reconnect_gen: u64,
    /// Automatic attempts since the server last answered.
    pub reconnect_attempts: u32,
    /// The data store of the existing main window.
    pub window_store: Option<[u8; 16]>,
    pub rebuild: Option<Rebuild>,
    /// Set while the main window is destroyed for a rebuild: the app must not quit then.
    pub rebuilding: bool,
    /// Last page of the server the main frame showed, to return to after a blocked load.
    pub last_good: Option<Url>,
    /// App window label → the page its main frame committed last (codex review 6: who may
    /// navigate it to the bundled screens).
    pub committed: HashMap<String, Url>,
    /// App window label → when a native transition to a bundled page was announced (one-shot).
    pub native_navigation: HashMap<String, Instant>,
    /// When the webview last reached the server's `/login` (sign-in loop detection).
    pub reauth_at: Option<Instant>,
    /// Bumped whenever a sign-in begins or ends (completed sign-in, sign-out, forget): work that
    /// started for the previous account (a tray poll) drops its answer (BRAI-377).
    pub account_epoch: u64,
    /// Origins that already got their remote capability (Tauri can add, not remove, them).
    pub capabilities: HashSet<String>,
    /// Native notification id → what a click opens.
    pub notifications: HashMap<i32, NotificationTarget>,
    pub next_notification: i32,
    pub notify_times: VecDeque<Instant>,
    /// `brainitive_open_external` calls of the last minute (BRAI-377).
    pub external_times: VecDeque<Instant>,
    pub last_external: Option<(Url, Instant)>,
    pub server_menu_item: Option<MenuItem<Wry>>,
    /// The app menu's "Servers" submenu (rebuilt when the saved servers change).
    pub servers_menu: Option<Submenu<Wry>>,
    /// The Settings section to show when it opens next ("Manage Servers…"), taken once.
    pub settings_focus: Option<&'static str>,
    /// "Brainitive Key…" (BRAI-440).
    pub leader_menu_item: Option<MenuItem<Wry>>,
    pub capture_menu_item: Option<MenuItem<Wry>>,
    pub switcher_menu_item: Option<MenuItem<Wry>>,
    pub voice_menu_item: Option<MenuItem<Wry>>,
    /// "Check for Updates…" / "Restart to Update (x.y.z)" (BRAI-373).
    pub update_menu_item: Option<MenuItem<Wry>>,
    /// Launched at login with "start hidden": the main window stays hidden until the user
    /// opens it (tray, dock, menu, a second launch).
    pub start_hidden: bool,
    /// The app window (main / `app-<n>`) that had the focus last: where menu actions go.
    pub focused_app_window: Option<String>,
    /// Counter for `app-<n>` labels.
    pub app_window_seq: u32,
    /// The tabs of the main window (`tabs.rs`).
    pub tabs: crate::tabs::Tabs,
    /// The app menus the top bar opens (Windows / Linux): id → submenu.
    pub topbar_menus: Vec<(&'static str, Submenu<Wry>)>,
    /// Popup label → whether an app window had the focus before it opened.
    pub popup_return: HashMap<String, bool>,
    /// Registered global shortcuts: shortcut id → what it does.
    pub hotkeys: HashMap<u32, crate::hotkeys::HotkeyAction>,
    /// Text the quick-capture window starts with (`capture::open_quick_capture`), taken once.
    pub capture_prefill: Option<String>,
    pub tray: Option<tauri::tray::TrayIcon<Wry>>,
    pub tray_data: crate::tray::TrayData,
    /// Server notifications already seen / shown as OS notifications (reset with `tray_data`).
    pub alerts: crate::alerts::Seen,
    /// The tray menu last built (rebuilt only when it changes).
    pub tray_model: Option<Vec<crate::tray::Entry>>,
    /// Why a global shortcut could not be registered at start (shown in Settings).
    pub hotkey_error: Option<String>,
    pub etags: crate::tray::EtagCache,
    /// Quick captures waiting to be sent, and where they are stored.
    pub queue: CaptureQueue,
    pub queue_path: PathBuf,
    /// A flush of the capture queue is running.
    pub flushing: bool,
    /// Downloads in progress (BRAI-363).
    pub downloads: Vec<crate::downloads::Pending>,
    /// Offline mode (BRAI-366): the current server's cache and outbox.
    pub offline: crate::offline::OfflineData,
    /// Auto-updates (BRAI-373): whether they may run, status, schedule, the waiting update.
    pub updater: crate::updater::UpdaterState,
    /// The menu-bar panel: its last answers, where it opened.
    pub panel: crate::panel::PanelState,
    /// The New task window: its last answers, a create waiting for a retry, the created issue.
    pub task: crate::new_task::TaskState,
}

/// What a click on one of our notifications opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationTarget {
    /// A page of the server (checked again on click: the server may have been switched).
    Page(Url),
    /// A downloaded file: shown in Finder / Explorer.
    File(PathBuf),
    /// The offline window (BRAI-366: changes sent / not applied).
    OfflineView,
}

pub struct AppState {
    inner: Mutex<Inner>,
    pub session: Session,
    /// Wakes the tray poller (sign-in, a tray action).
    pub poll_now: tokio::sync::Notify,
}

impl AppState {
    pub fn new(settings_path: PathBuf, queue_path: PathBuf, offline_dir: PathBuf) -> Self {
        let settings = Settings::load(&settings_path);
        Self {
            inner: Mutex::new(Inner {
                settings,
                settings_path,
                instance: None,
                phase: Phase::Choose,
                notice: None,
                problem: None,
                update: None,
                reconnect_gen: 0,
                reconnect_attempts: 0,
                window_store: None,
                rebuild: None,
                rebuilding: false,
                last_good: None,
                committed: HashMap::new(),
                native_navigation: HashMap::new(),
                reauth_at: None,
                account_epoch: 0,
                capabilities: HashSet::new(),
                notifications: HashMap::new(),
                next_notification: 1,
                notify_times: VecDeque::new(),
                external_times: VecDeque::new(),
                last_external: None,
                server_menu_item: None,
                servers_menu: None,
                settings_focus: None,
                leader_menu_item: None,
                capture_menu_item: None,
                switcher_menu_item: None,
                voice_menu_item: None,
                update_menu_item: None,
                start_hidden: false,
                focused_app_window: None,
                app_window_seq: 0,
                tabs: crate::tabs::Tabs::default(),
                topbar_menus: Vec::new(),
                popup_return: HashMap::new(),
                hotkeys: HashMap::new(),
                capture_prefill: None,
                tray: None,
                tray_data: Default::default(),
                alerts: Default::default(),
                tray_model: None,
                hotkey_error: None,
                etags: HashMap::new(),
                queue: CaptureQueue::load(&queue_path),
                queue_path,
                flushing: false,
                downloads: Vec::new(),
                offline: crate::offline::OfflineData::new(offline_dir),
                updater: Default::default(),
                panel: Default::default(),
                task: Default::default(),
            }),
            session: Session::default(),
            poll_now: tokio::sync::Notify::new(),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Inner {
    pub fn current_origin(&self) -> Option<ServerOrigin> {
        self.settings.current.clone()
    }

    /// Loads `origin`'s offline data for the sign-in stored for it (see `OfflineData::load`).
    pub fn load_offline(&mut self, origin: &ServerOrigin) {
        let sign_in = self.settings.sign_in_of(origin);
        self.offline.load(origin, sign_in.as_deref());
    }

    pub fn save_queue(&self) {
        if let Err(e) = self.queue.save(&self.queue_path) {
            log::warn!("could not save the capture queue: {e}");
        }
    }

    pub fn save_settings(&self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            log::warn!("could not save settings: {e}");
        }
    }
}

/// Who a piece of work was started for: the server and the sign-in (`Inner::account_epoch`,
/// bumped by every sign-in, sign-out and switch). Work that awaited the network checks it again
/// under the state lock before it writes anything (a queued capture, an outbox entry, a panel
/// answer), so nothing of one sign-in lands in another's data (codex review 1, 5, 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountTicket {
    pub origin: ServerOrigin,
    pub account: u64,
}

impl AccountTicket {
    /// The current server and sign-in (`None`: no server chosen).
    pub fn now(inner: &Inner) -> Option<Self> {
        Some(Self {
            origin: inner.current_origin()?,
            account: inner.account_epoch,
        })
    }

    /// Still the same server and the same sign-in.
    pub fn holds(&self, inner: &Inner) -> bool {
        inner.current_origin().as_ref() == Some(&self.origin) && inner.account_epoch == self.account
    }
}

pub fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_screen_carries_both_versions_and_the_download_link() {
        let origin = ServerOrigin::parse("brain.example.com").unwrap();
        let u = UpdateRequired::new("Acme Brain", &origin, Some("1.52.0".into()), "9.0.0");
        assert_eq!(u.server_name, "Acme Brain");
        assert_eq!(u.display_name, "brain.example.com");
        assert_eq!(u.min_version, "9.0.0");
        assert_eq!(u.app_version, crate::version::APP_VERSION);
        // the website's download page; the app's Store page in the Microsoft Store build (BRAI-442)
        if cfg!(feature = "msstore") {
            assert_eq!(u.download_url, crate::msstore::STORE_URL);
        } else {
            assert!(u.download_url.starts_with("https://"));
        }
        // a server can't make the screen huge
        let long = "x".repeat(500);
        assert_eq!(UpdateRequired::new(&long, &origin, None, &long).server_name.len(), 120);
    }
}
