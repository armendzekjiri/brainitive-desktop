//! Commands of the bundled screens (`src/` → `index.html`). Only local content may call them:
//! `capabilities/local.json` grants them without a `remote` entry, and the app ACL manifest
//! (build.rs) makes every command need an explicit permission.

use serde::Serialize;
use tauri::AppHandle;

use crate::flows::{self, ServerCheck, UiError};
use crate::problem::Problem;
use crate::server::{ServerOrigin, CLOUD_ORIGIN, DEV_ORIGIN, LOCAL_ORIGIN};
use crate::settings::RecentServer;
use crate::state::{state, Notice, Phase, UpdateRequired};
use crate::version::{APP_VERSION, DOWNLOAD_URL};

#[derive(Serialize)]
pub struct CurrentServer {
    origin: ServerOrigin,
    display_name: String,
    name: String,
    version: Option<String>,
    is_cloud: bool,
}

#[derive(Serialize)]
pub struct RecentEntry {
    #[serde(flatten)]
    server: RecentServer,
    display_name: String,
    is_current: bool,
}

#[derive(Serialize)]
pub struct Preset {
    label: &'static str,
    origin: &'static str,
}

#[derive(Serialize)]
pub struct UiState {
    phase: Phase,
    notice: Option<Notice>,
    /// Phase `unreachable`: what is wrong and what to do about it.
    problem: Option<Problem>,
    /// Phase `update_required`: the server and both versions.
    update: Option<UpdateRequired>,
    current: Option<CurrentServer>,
    recent: Vec<RecentEntry>,
    app_version: &'static str,
    platform: &'static str,
    cloud_origin: &'static str,
    /// Debug builds only: the pre-release and local servers as one-click choices.
    presets: Vec<Preset>,
}

#[tauri::command]
pub fn get_state(app: AppHandle) -> UiState {
    let s = state(&app);
    let inner = s.lock();
    let current_origin = inner.current_origin();
    let current = current_origin.clone().map(|origin| {
        let recent = inner.settings.recent.iter().find(|r| r.origin == origin);
        CurrentServer {
            display_name: origin.display_name(),
            name: recent.map(|r| r.name.clone()).unwrap_or_else(|| origin.display_name()),
            version: recent.and_then(|r| r.version.clone()),
            is_cloud: origin.is_cloud(),
            origin,
        }
    });
    let recent = inner
        .settings
        .recent
        .iter()
        .map(|r| RecentEntry {
            display_name: r.origin.display_name(),
            is_current: Some(&r.origin) == current_origin.as_ref(),
            server: r.clone(),
        })
        .collect();
    let presets = if cfg!(debug_assertions) {
        vec![
            Preset {
                label: "Pre-release (dev.brainitive.app)",
                origin: DEV_ORIGIN,
            },
            Preset {
                label: "Local (localhost:8000)",
                origin: LOCAL_ORIGIN,
            },
        ]
    } else {
        vec![]
    };
    UiState {
        phase: inner.phase,
        notice: inner.notice.clone(),
        problem: inner.problem.clone(),
        update: inner.update.clone(),
        current,
        recent,
        app_version: APP_VERSION,
        platform: crate::api::platform(),
        cloud_origin: CLOUD_ORIGIN,
        presets,
    }
}

#[tauri::command]
pub async fn check_server(app: AppHandle, input: String) -> Result<ServerCheck, UiError> {
    flows::check_server(&app, &input).await
}

#[tauri::command]
pub async fn use_server(app: AppHandle, origin: String) -> Result<(), UiError> {
    let origin = ServerOrigin::parse(&origin).map_err(|e| UiError {
        kind: "invalid_url",
        message: e.to_string(),
    })?;
    flows::use_server(&app, origin).await
}

#[tauri::command]
pub async fn start_sign_in(app: AppHandle) -> Result<String, UiError> {
    flows::start_sign_in(&app).await
}

#[tauri::command]
pub fn cancel_sign_in(app: AppHandle) {
    flows::cancel_sign_in(&app);
}

#[tauri::command]
pub async fn retry_connect(app: AppHandle) -> Result<(), UiError> {
    flows::retry_now(&app).await
}

/// "Download the update": opens the fixed download page in the browser. The address is a
/// constant of this app, never an argument.
#[tauri::command]
pub fn open_download_page(app: AppHandle) -> Result<(), UiError> {
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_url(DOWNLOAD_URL, None::<&str>)
        .map_err(|e| UiError {
            kind: "browser",
            message: format!("The browser could not be opened: {e}"),
        })
}

fn too_old_error() -> UiError {
    UiError {
        kind: "app_too_old",
        message: "This server needs a newer Brainitive Desktop. Please update the app.".into(),
    }
}

/// The current server, unless it needs a newer app (then nothing of it is opened or synced).
fn usable_server(app: &AppHandle) -> Result<ServerOrigin, UiError> {
    let s = state(app);
    let inner = s.lock();
    let origin = inner.current_origin().ok_or(UiError {
        kind: "no_server",
        message: "Choose a server first.".into(),
    })?;
    if flows::app_too_old(&inner, &origin) {
        return Err(too_old_error());
    }
    Ok(origin)
}

/// "Open offline" (BRAI-366): the bundled offline window with this computer's copy of My Day,
/// My Work, the kept task lists and captures.
#[tauri::command]
pub fn open_offline(app: AppHandle) -> Result<(), UiError> {
    usable_server(&app)?;
    crate::popup::show(&app, crate::popup::Popup::Offline);
    Ok(())
}

/// "Use the web app offline": the server's pages from the webview cache / the web app's own
/// service worker (its opt-in offline mode), no new session.
#[tauri::command]
pub fn open_web_offline(app: AppHandle) -> Result<(), UiError> {
    let origin = usable_server(&app)?;
    crate::shell::enter_app(&app, &origin, origin.join("/dashboard"));
    Ok(())
}

#[tauri::command]
pub fn switch_server(app: AppHandle) {
    flows::switch_server(&app);
}

#[tauri::command]
pub async fn sign_out(app: AppHandle) {
    flows::sign_out(&app).await;
}

#[tauri::command]
pub async fn forget_server(app: AppHandle, origin: String) -> Result<(), UiError> {
    let origin = ServerOrigin::parse(&origin).map_err(|e| UiError {
        kind: "invalid_url",
        message: e.to_string(),
    })?;
    flows::forget_server(&app, origin).await;
    Ok(())
}

#[tauri::command]
pub fn dismiss_notice(app: AppHandle) {
    state(&app).lock().notice = None;
}

static CLIENT_ERRORS: std::sync::Mutex<crate::logging::RateLimit> =
    std::sync::Mutex::new(crate::logging::RateLimit::new(std::time::Duration::from_secs(60), 20));

/// A script error in a bundled screen (`window.onerror` of `src/main.ts`), into the log file.
/// Local content only (`capabilities/local.json`); the main window only, ≤ 20 per minute,
/// clipped and redacted like every other log line. Pages of the server can't call it.
#[tauri::command]
pub fn log_client_error(webview: tauri::Webview, message: String, source: Option<String>) {
    if webview.label() != crate::shell::MAIN {
        return;
    }
    let allowed = CLIENT_ERRORS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .allow(std::time::Instant::now());
    if allowed {
        let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
        log::error!(
            target: "brainitive_desktop_lib::webview",
            "{} ({})",
            clip(&message, 500),
            clip(source.as_deref().unwrap_or("unknown source"), 200)
        );
    }
}
