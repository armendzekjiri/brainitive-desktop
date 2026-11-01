//! What happens when: start-up, connecting a server, signing in, deep links, a lost web
//! session, signing out, switching and forgetting servers (BRAI-354 / 355 / 356 / 357).

use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::api::{ApiError, InstanceInfo};
use crate::links::{self, DeepLink};
use crate::server::ServerOrigin;
use crate::session::SessionError;
use crate::shell;
use crate::state::{state, Notice, Phase};
use crate::version::{meets_minimum, APP_VERSION};

/// What the connect screen shows after "Continue": the server that answered.
#[derive(Debug, Clone, Serialize)]
pub struct ServerCheck {
    pub origin: ServerOrigin,
    pub display_name: String,
    pub name: String,
    pub version: Option<String>,
    pub sign_in: Option<serde_json::Value>,
    pub min_desktop_version: Option<String>,
    /// `min_versions.desktop` is newer than this app: it can't be used with this server.
    pub app_too_old: bool,
}

/// Errors shown on the bundled screens: a kind for the UI to branch on and a sentence.
#[derive(Debug, Clone, Serialize)]
pub struct UiError {
    pub kind: &'static str,
    pub message: String,
}

impl UiError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<ApiError> for UiError {
    fn from(e: ApiError) -> Self {
        let kind = match e {
            ApiError::Tls(_) => "tls",
            ApiError::Network(_) | ApiError::Timeout => "network",
            ApiError::NotBrainitive | ApiError::BadRedirect(_) => "not_brainitive",
            ApiError::ServerTooOld => "server_too_old",
            _ => "server_error",
        };
        Self::new(kind, e.to_string())
    }
}

impl From<SessionError> for UiError {
    fn from(e: SessionError) -> Self {
        let kind = match e {
            SessionError::SignedOut(_) => "signed_out",
            SessionError::Offline(_) => "network",
            SessionError::Keychain(_) => "keychain",
            _ => "sign_in",
        };
        Self::new(kind, e.to_string())
    }
}

/// App start: back into the last server (refresh → web session), else the connect screen.
pub fn startup(app: &AppHandle) {
    let current = state(app).lock().current_origin();
    match current {
        Some(origin) => {
            state(app).lock().phase = Phase::Connecting;
            // build the window with this server's data store right away: no rebuild later
            shell::open_main(app, origin.store_id(), tauri::WebviewUrl::App("index.html".into()));
            shell::refresh_title(app);
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                shell::run_pending_wipes(&handle).await;
                restore(&handle, origin, None).await;
            });
        }
        None => {
            state(app).lock().phase = Phase::Choose;
            shell::open_main(
                app,
                crate::server::local_store_id(),
                tauri::WebviewUrl::App("index.html".into()),
            );
            shell::refresh_title(app);
            let handle = app.clone();
            tauri::async_runtime::spawn(async move { shell::run_pending_wipes(&handle).await });
        }
    }
}

/// "Continue" on the connect screen: normalize, call `/api/v1/instance`, report what answered.
pub async fn check_server(app: &AppHandle, input: &str) -> Result<ServerCheck, UiError> {
    let origin = ServerOrigin::parse(input).map_err(|e| UiError::new("invalid_url", e.to_string()))?;
    if shell::local_origins(app).iter().any(|o| o == origin.as_str()) {
        return Err(UiError::new("invalid_url", "That is the address of this app itself."));
    }
    let (info, final_origin) = state(app).session.api().instance(&origin).await?;
    if shell::local_origins(app).iter().any(|o| o == final_origin.as_str()) {
        return Err(UiError::new("invalid_url", "That is the address of this app itself."));
    }
    let min = info.min_versions.as_ref().and_then(|m| m.desktop.clone());
    let check = ServerCheck {
        display_name: final_origin.display_name(),
        origin: final_origin.clone(),
        name: info.name.chars().take(120).collect(),
        version: info.version.clone(),
        sign_in: info.sign_in.clone(),
        app_too_old: min.as_deref().is_some_and(|m| !meets_minimum(APP_VERSION, m)),
        min_desktop_version: min,
    };
    state(app).lock().instance = Some((final_origin, info));
    Ok(check)
}

/// "Use this server" (after a check) or a click on a recent server.
pub async fn use_server(app: &AppHandle, origin: ServerOrigin) -> Result<(), UiError> {
    // always ask the server again: the version rule must hold for recent servers too
    let check = check_server(app, origin.as_str()).await?;
    if check.app_too_old {
        return Err(UiError::new(
            "app_too_old",
            format!(
                "This server needs Brainitive Desktop {} or later (this is {APP_VERSION}). Please update the app.",
                check.min_desktop_version.unwrap_or_default()
            ),
        ));
    }
    let origin = check.origin;
    {
        let s = state(app);
        let mut inner = s.lock();
        let previous = inner.current_origin();
        inner.settings.remember(&origin, &check.name, check.version.clone());
        inner.save_settings();
        if previous.as_ref() != Some(&origin) {
            inner.last_good = None;
            inner.reauth_at = None;
        }
    }
    shell::refresh_title(app);
    // a server switch must not leave the other server's badge on the dock
    crate::bridge::set_badge(app, 0);

    if state(app).session.has_stored_sign_in(&origin).await {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move { restore(&handle, origin, None).await });
        state(app).lock().phase = Phase::Connecting;
    } else {
        shell::show_local(app, Phase::SignIn, None);
    }
    Ok(())
}

/// The stored sign-in → a web session → the app. `to`: a path to land on.
pub async fn restore(app: &AppHandle, origin: ServerOrigin, to: Option<String>) {
    shell::show_local(app, Phase::Connecting, None);
    if !state(app).session.has_stored_sign_in(&origin).await {
        shell::show_local(app, Phase::SignIn, None);
        return;
    }
    match state(app).session.web_session_url(&origin, to.as_deref()).await {
        Ok(url) => shell::enter_app(app, &origin, url),
        Err(SessionError::SignedOut(_)) => shell::show_local(
            app,
            Phase::SignIn,
            Some(Notice::info(
                "You were signed out of this server. Sign in again to continue.",
            )),
        ),
        Err(e @ SessionError::Offline(_)) => {
            shell::show_local(app, Phase::Unreachable, Some(Notice::error(e.to_string())))
        }
        Err(e) => shell::show_local(app, Phase::Unreachable, Some(Notice::error(e.to_string()))),
    }
}

/// "Sign in with your browser": PKCE + state, then the system browser.
pub async fn start_sign_in(app: &AppHandle) -> Result<String, UiError> {
    let origin = state(app)
        .lock()
        .current_origin()
        .ok_or_else(|| UiError::new("no_server", "Choose a server first."))?;
    let info = fresh_instance(app, &origin).await?;
    if let Some(min) = info.min_versions.as_ref().and_then(|m| m.desktop.as_deref()) {
        if !meets_minimum(APP_VERSION, min) {
            return Err(UiError::new(
                "app_too_old",
                format!("This server needs Brainitive Desktop {min} or later."),
            ));
        }
    }
    let url = state(app).session.begin_sign_in(&origin, &info);
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|e| UiError::new("browser", format!("The browser could not be opened: {e}")))?;
    shell::show_local(app, Phase::Waiting, None);
    Ok(url.to_string())
}

async fn fresh_instance(app: &AppHandle, origin: &ServerOrigin) -> Result<InstanceInfo, UiError> {
    let (info, final_origin) = state(app).session.api().instance(origin).await?;
    if &final_origin != origin {
        return Err(UiError::new(
            "not_brainitive",
            format!("The server now answers at {final_origin}. Choose it again."),
        ));
    }
    state(app).lock().instance = Some((origin.clone(), info.clone()));
    Ok(info)
}

pub fn cancel_sign_in(app: &AppHandle) {
    state(app).session.cancel_sign_in();
    shell::show_local(app, Phase::SignIn, None);
}

/// Every `brainitive://` link: from the OS (deep-link plugin, a second launch forwarded by
/// single-instance) or clicked inside the webview.
pub async fn handle_deep_link(app: &AppHandle, url: Url) {
    shell::focus_main(app);
    let Some(link) = links::parse_deep_link(&url) else {
        log::info!("ignored deep link");
        return;
    };
    match link {
        DeepLink::AuthCallback { code, state: st } => {
            if state(app).session.sign_in_pending_for().is_none() {
                return; // stale or forged: nothing to finish
            }
            shell::show_local(app, Phase::Connecting, None);
            match state(app).session.complete_sign_in(&code, &st).await {
                Ok(origin) => restore(app, origin, None).await,
                Err(SessionError::StateMismatch) => {
                    // keep waiting for the real callback
                    shell::show_local(
                        app,
                        Phase::Waiting,
                        Some(Notice::error(SessionError::StateMismatch.to_string())),
                    );
                }
                Err(e) => shell::show_local(app, Phase::SignIn, Some(Notice::error(e.to_string()))),
            }
        }
        DeepLink::AuthError { error, state: st } => {
            if state(app).session.fail_sign_in(st.as_deref()) {
                let message = if error == "access_denied" {
                    "Sign-in was cancelled in the browser.".to_string()
                } else {
                    format!("Sign-in failed ({error}).")
                };
                shell::show_local(app, Phase::SignIn, Some(Notice::info(message)));
            }
        }
        DeepLink::Open { target } => {
            let (origin, phase) = {
                let s = state(app);
                let inner = s.lock();
                (inner.current_origin(), inner.phase)
            };
            let Some(origin) = origin else { return };
            let Some(page) = links::resolve_on_instance(&origin, &target) else {
                log::info!("deep link to another origin ignored");
                return;
            };
            if phase == Phase::InApp {
                if let Some(w) = shell::main_window(app) {
                    let _ = w.navigate(page);
                }
            } else if state(app).session.has_stored_sign_in(&origin).await {
                let to = path_of(&page);
                restore(app, origin, Some(to)).await;
            }
        }
    }
}

fn path_of(url: &Url) -> String {
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    path
}

/// The webview reached the server's `/login`: its session cookie is gone (the web app's own
/// "Sign out" calls `brainitive_sign_out` instead, so this is an expired session).
/// - a second time within 30 s (the one-time session link failed): show the sign-in screen;
/// - otherwise: a fresh web session from the stored sign-in, back to the last page.
pub async fn on_web_session_lost(app: &AppHandle, origin: ServerOrigin) {
    let (looping, last) = {
        let s = state(app);
        let mut inner = s.lock();
        let looping = inner.reauth_at.is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        inner.reauth_at = Some(Instant::now());
        (looping, inner.last_good.clone())
    };
    if looping {
        shell::show_local(
            app,
            Phase::SignIn,
            Some(Notice::info("Your session on this server ended. Sign in again.")),
        );
    } else {
        let to = last.filter(|u| origin.contains(u)).map(|u| path_of(&u));
        restore(app, origin, to).await;
    }
}

/// "Sign out" (menu or the web app's logout): revoke this device, delete the keychain entry,
/// clear the webview data of this server, show the sign-in screen.
pub async fn sign_out(app: &AppHandle) {
    let Some(origin) = state(app).lock().current_origin() else {
        return;
    };
    state(app).session.sign_out(&origin).await;
    crate::capture::drop_queue(app, &origin);
    crate::bridge::set_badge(app, 0);
    if let Some(w) = shell::main_window(app) {
        // the window uses this server's own data store, so this clears only its data
        if state(app).lock().window_store == Some(origin.store_id()) {
            let _ = w.clear_all_browsing_data();
        }
    }
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.last_good = None;
    }
    shell::show_local(app, Phase::SignIn, Some(Notice::info("You are signed out.")));
}

/// "Switch server…": back to the choice, without signing out of the current server.
pub fn switch_server(app: &AppHandle) {
    state(app).session.cancel_sign_in();
    shell::show_local(app, Phase::Choose, None);
}

/// "Forget this server": sign out there, delete its keychain entry and its webview data,
/// remove it from the recent list.
pub async fn forget_server(app: &AppHandle, origin: ServerOrigin) {
    state(app).session.sign_out(&origin).await;
    crate::capture::drop_queue(app, &origin);
    let was_current = {
        let s = state(app);
        let mut inner = s.lock();
        let was_current = inner.current_origin().as_ref() == Some(&origin);
        inner.settings.forget(&origin);
        inner.save_settings();
        if was_current {
            inner.instance = None;
            inner.last_good = None;
        }
        was_current
    };
    shell::refresh_title(app);
    let uses_store = state(app).lock().window_store == Some(origin.store_id());
    if uses_store {
        if was_current {
            crate::bridge::set_badge(app, 0);
        }
        // move the window to the neutral store; the old one is deleted once it is released
        state(app).lock().phase = Phase::Choose;
        state(app).lock().notice = Some(Notice::info(format!(
            "{} was removed from this app.",
            origin.display_name()
        )));
        shell::open_main(
            app,
            crate::server::local_store_id(),
            tauri::WebviewUrl::App("index.html".into()),
        );
    } else {
        shell::show_local(
            app,
            Phase::Choose,
            Some(Notice::info(format!(
                "{} was removed from this app.",
                origin.display_name()
            ))),
        );
        shell::run_pending_wipes(app).await;
    }
}
