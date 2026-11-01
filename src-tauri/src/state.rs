//! The app's runtime state, managed by Tauri (`app.state::<AppState>()`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;
use tauri::menu::MenuItem;
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
    /// The data store of the existing main window.
    pub window_store: Option<[u8; 16]>,
    pub rebuild: Option<Rebuild>,
    /// Set while the main window is destroyed for a rebuild: the app must not quit then.
    pub rebuilding: bool,
    /// Last page of the server the main frame showed, to return to after a blocked load.
    pub last_good: Option<Url>,
    /// When the webview last reached the server's `/login` (sign-in loop detection).
    pub reauth_at: Option<Instant>,
    /// Origins that already got their remote capability (Tauri can add, not remove, them).
    pub capabilities: HashSet<String>,
    /// Native notification id → what a click opens.
    pub notifications: HashMap<i32, NotificationTarget>,
    pub next_notification: i32,
    pub notify_times: VecDeque<Instant>,
    pub last_external: Option<(Url, Instant)>,
    pub server_menu_item: Option<MenuItem<Wry>>,
    pub capture_menu_item: Option<MenuItem<Wry>>,
    pub switcher_menu_item: Option<MenuItem<Wry>>,
    /// Launched at login with "start hidden": the main window stays hidden until the user
    /// opens it (tray, dock, menu, a second launch).
    pub start_hidden: bool,
    /// The app window (main / `app-<n>`) that had the focus last: where menu actions go.
    pub focused_app_window: Option<String>,
    /// Counter for `app-<n>` labels.
    pub app_window_seq: u32,
    /// Popup label → whether an app window had the focus before it opened.
    pub popup_return: HashMap<String, bool>,
    /// Registered global shortcuts: shortcut id → what it does.
    pub hotkeys: HashMap<u32, crate::hotkeys::HotkeyAction>,
    /// Text the quick-capture window starts with (`capture::open_quick_capture`), taken once.
    pub capture_prefill: Option<String>,
    pub tray: Option<tauri::tray::TrayIcon<Wry>>,
    pub tray_data: crate::tray::TrayData,
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
}

/// What a click on one of our notifications opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationTarget {
    /// A page of the server (checked again on click: the server may have been switched).
    Page(Url),
    /// A downloaded file: shown in Finder / Explorer.
    File(PathBuf),
}

pub struct AppState {
    inner: Mutex<Inner>,
    pub session: Session,
    /// Wakes the tray poller (sign-in, a tray action).
    pub poll_now: tokio::sync::Notify,
}

impl AppState {
    pub fn new(settings_path: PathBuf, queue_path: PathBuf) -> Self {
        let settings = Settings::load(&settings_path);
        Self {
            inner: Mutex::new(Inner {
                settings,
                settings_path,
                instance: None,
                phase: Phase::Choose,
                notice: None,
                window_store: None,
                rebuild: None,
                rebuilding: false,
                last_good: None,
                reauth_at: None,
                capabilities: HashSet::new(),
                notifications: HashMap::new(),
                next_notification: 1,
                notify_times: VecDeque::new(),
                last_external: None,
                server_menu_item: None,
                capture_menu_item: None,
                switcher_menu_item: None,
                start_hidden: false,
                focused_app_window: None,
                app_window_seq: 0,
                popup_return: HashMap::new(),
                hotkeys: HashMap::new(),
                capture_prefill: None,
                tray: None,
                tray_data: Default::default(),
                tray_model: None,
                hotkey_error: None,
                etags: HashMap::new(),
                queue: CaptureQueue::load(&queue_path),
                queue_path,
                flushing: false,
                downloads: Vec::new(),
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

pub fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}
