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
    /// When the webview last went to the server's `/` and `/login` (logout / loop detection).
    pub home_at: Option<Instant>,
    pub reauth_at: Option<Instant>,
    /// Origins that already got their remote capability (Tauri can add, not remove, them).
    pub capabilities: HashSet<String>,
    /// Native notification id → page to open on click.
    pub notifications: HashMap<i32, Url>,
    pub next_notification: i32,
    pub notify_times: VecDeque<Instant>,
    pub last_external: Option<(Url, Instant)>,
    pub server_menu_item: Option<MenuItem<Wry>>,
}

pub struct AppState {
    inner: Mutex<Inner>,
    pub session: Session,
}

impl AppState {
    pub fn new(settings_path: PathBuf) -> Self {
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
                home_at: None,
                reauth_at: None,
                capabilities: HashSet::new(),
                notifications: HashMap::new(),
                next_notification: 1,
                notify_times: VecDeque::new(),
                last_external: None,
                server_menu_item: None,
            }),
            session: Session::default(),
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

    pub fn save_settings(&self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            log::warn!("could not save settings: {e}");
        }
    }
}

pub fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}
