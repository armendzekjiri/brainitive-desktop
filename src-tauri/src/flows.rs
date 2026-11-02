//! What happens when: start-up, connecting a server, signing in, deep links, a lost web
//! session, signing out, switching and forgetting servers (BRAI-354 / 355 / 356 / 357).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::api::{ApiError, InstanceInfo};
use crate::links::{self, DeepLink};
use crate::problem::Problem;
use crate::reconnect;
use crate::server::ServerOrigin;
use crate::session::SessionError;
use crate::shell;
use crate::state::{state, Notice, Phase, UpdateRequired};
use crate::version::{meets_minimum, APP_VERSION};

/// What the connect screen shows after "Continue": the server that answered.
#[derive(Debug, Clone, Serialize)]
pub struct ServerCheck {
    /// The origin that answered (after redirects): the one "Use this server" connects to.
    pub origin: ServerOrigin,
    /// The origin the user entered.
    pub entered: ServerOrigin,
    /// The server answered somewhere else than what was entered: the connect screen says so
    /// and the user confirms that address (BRAI-377).
    pub redirected: bool,
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
        redirected: final_origin != origin,
        entered: origin,
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

/// `use_server` gets the origin the user confirmed (the check's final origin, or a recent
/// server). When the server now answers somewhere else, that address was never shown: refuse,
/// and the user checks it again (the connect screen shows where it forwards and asks).
fn unconfirmed_redirect(confirmed: &ServerOrigin, answered: &ServerOrigin) -> Option<UiError> {
    (confirmed != answered).then(|| {
        UiError::new(
            "redirected",
            format!(
                "{} now forwards to {}. Enter the address again to check and confirm it.",
                confirmed.display_name(),
                answered.as_str()
            ),
        )
    })
}

/// "Use this server" (after a check) or a click on a recent server.
pub async fn use_server(app: &AppHandle, origin: ServerOrigin) -> Result<(), UiError> {
    // always ask the server again: the version rule must hold for recent servers too
    let check = check_server(app, origin.as_str()).await?;
    if let Some(error) = unconfirmed_redirect(&origin, &check.origin) {
        return Err(error);
    }
    if check.app_too_old {
        let update = UpdateRequired::new(
            &check.name,
            &check.origin,
            check.version.clone(),
            check.min_desktop_version.as_deref().unwrap_or_default(),
        );
        show_update_required(app, update);
        return Ok(());
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

/// The stored sign-in → a web session → the app. `to`: a path to land on. The server is asked
/// first (`/api/v1/instance`, concurrently with the session): a server that needs a newer app
/// never gets its web app loaded, and a failure is reported as what it is (see `problem.rs`).
pub async fn restore(app: &AppHandle, origin: ServerOrigin, to: Option<String>) {
    restore_inner(app, origin, to, false).await
}

/// `silent`: an automatic retry keeps the problem screen up instead of flashing "Connecting…".
async fn restore_inner(app: &AppHandle, origin: ServerOrigin, to: Option<String>, silent: bool) {
    if !silent {
        shell::show_local(app, Phase::Connecting, None);
    }
    if !state(app).session.has_stored_sign_in(&origin).await {
        shell::show_local(app, Phase::SignIn, None);
        return;
    }
    let host = origin.display_name();
    let app_state = state(app);
    let (gate, session) = tokio::join!(
        instance_gate(app, &origin),
        app_state.session.web_session_url(&origin, to.as_deref())
    );
    match gate {
        Err(problem) => return show_unreachable(app, problem),
        Ok(Some(update)) => return show_update_required(app, update),
        Ok(None) => {}
    }
    match session {
        Ok(url) => shell::enter_app(app, &origin, url),
        Err(SessionError::SignedOut(_)) => shell::show_local(
            app,
            Phase::SignIn,
            Some(Notice::info(
                "You were signed out of this server. Sign in again to continue.",
            )),
        ),
        Err(e) => {
            log::warn!("web session failed: {e}");
            show_unreachable(app, Problem::from_session(&e, &host));
        }
    }
}

/// `Ok(None)`: the server answered and this app is new enough.
async fn instance_gate(app: &AppHandle, origin: &ServerOrigin) -> Result<Option<UpdateRequired>, Problem> {
    let host = origin.display_name();
    let (info, final_origin) = state(app).session.api().instance(origin).await.map_err(|e| {
        log::warn!("instance check failed: {e:?}");
        Problem::from_api(&e, &host)
    })?;
    if &final_origin != origin {
        return Err(Problem::from_api(
            &ApiError::BadRedirect(final_origin.display_name()),
            &host,
        ));
    }
    let min = info.min_versions.as_ref().and_then(|m| m.desktop.clone());
    let update = min
        .as_deref()
        .filter(|m| !meets_minimum(APP_VERSION, m))
        .map(|m| UpdateRequired::new(&info.name, origin, info.version.clone(), m));
    state(app).lock().instance = Some((origin.clone(), info));
    Ok(update)
}

/// The poller's re-check of `min_versions.desktop` while the app runs (about every 6 hours and
/// after a wake, `version::Recheck`). A server that now needs a newer app gets the "Please
/// update" screen, like at connect. `false`: the answer didn't come (try again soon).
pub async fn recheck_minimum(app: &AppHandle, origin: &ServerOrigin) -> bool {
    match instance_gate(app, origin).await {
        Ok(Some(update)) => {
            // the user may have switched servers meanwhile
            if state(app).lock().current_origin().as_ref() == Some(origin) {
                show_update_required(app, update);
            }
            true
        }
        Ok(None) => true,
        Err(problem) => {
            log::info!("minimum version re-check failed: {:?}", problem.kind);
            false
        }
    }
}

/// Whether `origin` needs a newer app: the update screen is up, or its last `/api/v1/instance`
/// answer says so. Nothing is loaded or synced with such a server.
pub fn app_too_old(inner: &crate::state::Inner, origin: &ServerOrigin) -> bool {
    let min = inner
        .instance
        .as_ref()
        .filter(|(o, _)| o == origin)
        .and_then(|(_, info)| info.min_versions.as_ref()?.desktop.clone());
    inner.phase == Phase::UpdateRequired || min.is_some_and(|m| !meets_minimum(APP_VERSION, &m))
}

/// "Please update Brainitive Desktop": the web app is not loaded, nothing retries by itself.
pub fn show_update_required(app: &AppHandle, update: UpdateRequired) {
    log::info!(
        "server {} needs desktop {} (this is {APP_VERSION})",
        update.display_name,
        update.min_version
    );
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.reconnect_gen += 1;
        inner.update = Some(update);
    }
    shell::show_local(app, Phase::UpdateRequired, None);
}

/// The Unreachable screen with a classified problem. A transient one is tried again by itself
/// (backoff, and as soon as the network is back); certificate problems wait for the user.
pub fn show_unreachable(app: &AppHandle, mut problem: Problem) {
    log::info!("server unreachable: {:?}", problem.kind);
    let (generation, wait, origin) = {
        let s = state(app);
        let mut inner = s.lock();
        inner.reconnect_gen += 1;
        problem.attempts = inner.reconnect_attempts;
        let wait = reconnect::delay(inner.reconnect_attempts);
        if problem.retryable {
            inner.reconnect_attempts = inner.reconnect_attempts.saturating_add(1);
            problem.retry_at_ms = Some(now_ms() + wait.as_millis() as u64);
        }
        inner.problem = Some(problem.clone());
        (inner.reconnect_gen, wait, inner.current_origin())
    };
    shell::show_local(app, Phase::Unreachable, None);
    crate::offline::on_unreachable(app);
    if let (true, Some(origin)) = (problem.retryable, origin) {
        let handle = app.clone();
        let probe = problem.probes_network();
        tauri::async_runtime::spawn(async move { auto_reconnect(handle, generation, origin, probe, wait).await });
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Whether this loop is still the one the Unreachable screen is waiting for.
fn still_waiting(app: &AppHandle, generation: u64) -> bool {
    let s = state(app);
    let inner = s.lock();
    inner.phase == Phase::Unreachable && inner.reconnect_gen == generation
}

/// Waits out the backoff in short steps, then tries again. Earlier: when the server's name
/// resolves after it failed to (the network is back), or the computer woke from sleep.
async fn auto_reconnect(app: AppHandle, generation: u64, origin: ServerOrigin, probe: bool, wait: Duration) {
    let started = Instant::now();
    let wall_start = SystemTime::now();
    let mut lookup_failed = false;
    let mut after_wake = false;
    loop {
        let remaining = wait.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(reconnect::next_sleep(remaining)).await;
        if !still_waiting(&app, generation) {
            return;
        }
        let wall = wall_start.elapsed().unwrap_or_default();
        if crate::poll::woke_from_sleep(started.elapsed(), wall) {
            log::info!("woke from sleep: trying the server again");
            after_wake = true;
            break;
        }
        if probe {
            match resolves(&origin).await {
                false => lookup_failed = true,
                true if lookup_failed => {
                    log::info!("the network is back: trying the server again");
                    break;
                }
                true => {}
            }
        }
    }
    if after_wake {
        // Wi-Fi needs a moment after a wake
        tokio::time::sleep(crate::poll::WAKE_GRACE).await;
    }
    if still_waiting(&app, generation) {
        restore_inner(&app, origin, None, true).await;
    }
}

/// A local name lookup of the server's host: the cheapest "is the network back?" question.
async fn resolves(origin: &ServerOrigin) -> bool {
    let Some(target) = reconnect::lookup_target(&origin.url()) else {
        return false;
    };
    match tokio::time::timeout(Duration::from_secs(2), tokio::net::lookup_host(target)).await {
        Ok(Ok(mut addrs)) => addrs.next().is_some(),
        _ => false,
    }
}

/// "Retry" (screen button): ends the automatic loop and tries now. On the update screen:
/// asks the server again whether the app is new enough.
pub async fn retry_now(app: &AppHandle) -> Result<(), UiError> {
    let (update, current) = {
        let s = state(app);
        let mut inner = s.lock();
        inner.reconnect_gen += 1;
        inner.reconnect_attempts = 0;
        (inner.update.clone(), inner.current_origin())
    };
    if let Some(update) = update {
        let origin = ServerOrigin::parse(&update.origin).map_err(|e| UiError::new("invalid_url", e.to_string()))?;
        return use_server(app, origin).await;
    }
    if let Some(origin) = current {
        restore(app, origin, None).await;
    }
    Ok(())
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
            show_update_required(app, UpdateRequired::new(&info.name, &origin, info.version.clone(), min));
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
                Ok(origin) => {
                    // possibly another account than before
                    state(app).lock().account_epoch += 1;
                    restore(app, origin, None).await
                }
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
            // never the auth endpoints (`/app/session/<code>` would be login CSRF)
            let Some(page) = links::resolve_deep_link_target(&origin, &target) else {
                log::info!("deep link to another origin or an auth page ignored");
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
    crate::offline::wipe(app, &origin);
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
        inner.account_epoch += 1;
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
    crate::offline::wipe(app, &origin);
    let was_current = {
        let s = state(app);
        let mut inner = s.lock();
        let was_current = inner.current_origin().as_ref() == Some(&origin);
        inner.account_epoch += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_now_answers_elsewhere_needs_a_new_confirm() {
        let o = |s: &str| ServerOrigin::parse(s).unwrap();
        assert!(unconfirmed_redirect(&o("https://a.example.com"), &o("https://a.example.com")).is_none());
        let refused = unconfirmed_redirect(&o("https://a.example.com"), &o("https://b.example.com")).unwrap();
        assert_eq!(refused.kind, "redirected");
        assert!(refused.message.contains("https://b.example.com"), "{}", refused.message);
    }
}
