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
use crate::session::{Remote, SessionError};
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
            let unfinished = close_unfinished_sign_outs(app);
            state(app).lock().phase = Phase::Connecting;
            // build the window with this server's data store right away: no rebuild later
            shell::open_main(app, origin.store_id(), tauri::WebviewUrl::App("index.html".into()));
            shell::refresh_title(app);
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                shell::run_pending_wipes(&handle).await;
                finish_sign_outs(&handle, unfinished).await;
                restore(&handle, origin, None).await;
            });
        }
        None => {
            let unfinished = close_unfinished_sign_outs(app);
            state(app).lock().phase = Phase::Choose;
            shell::open_main(
                app,
                crate::server::local_store_id(),
                tauri::WebviewUrl::App("index.html".into()),
            );
            shell::refresh_title(app);
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                shell::run_pending_wipes(&handle).await;
                finish_sign_outs(&handle, unfinished).await;
            });
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
    let previous = {
        let s = state(app);
        let inner = s.lock();
        if inner.settings.is_full_for(&origin) {
            return Err(UiError::new(
                "too_many",
                format!(
                    "Brainitive keeps up to {} servers. Forget one in Settings → Servers first.",
                    crate::servers::MAX_SERVERS
                ),
            ));
        }
        inner.current_origin()
    };
    if let Some(previous) = previous.filter(|p| p != &origin) {
        leave_server(app, &previous).await;
    }
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.settings.remember(&origin, &check.name, check.version.clone());
        inner.save_settings();
    }
    servers_changed(app);
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
    // switched to another server while this one was asked: its answer shows nothing
    if state(app).lock().current_origin().as_ref() != Some(&origin) {
        log::info!("restore of a server that is no longer current dropped");
        return;
    }
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
    let renamed = {
        let s = state(app);
        let mut inner = s.lock();
        // the server's own name (the default display name) may have changed
        let renamed = inner.settings.update_instance(origin, &info.name, info.version.clone());
        if renamed {
            inner.save_settings();
        }
        inner.instance = Some((origin.clone(), info));
        renamed
    };
    if renamed {
        servers_changed(app);
    }
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
                    {
                        // possibly another account than before
                        let s = state(app);
                        let mut inner = s.lock();
                        inner.account_epoch += 1;
                        // a sign-out cleanup still waiting for this server is moot: this
                        // sign-in's keychain entry replaced the old one
                        if inner.settings.pending_sign_outs.contains(&origin) {
                            inner.settings.sign_out_done(&origin);
                            inner.save_settings();
                        }
                    }
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
/// clear the webview data of this server, show the sign-in screen — and say honestly what could
/// not be done (codex review 4).
pub async fn sign_out(app: &AppHandle) {
    let Some(origin) = state(app).lock().current_origin() else {
        return;
    };
    // first: a capture, panel or New-task call still running for this sign-in writes nothing
    // back from now on (codex review 5)
    state(app).lock().account_epoch += 1;
    clear_account_traces(app);
    let mut cleanup = end_sign_in(app, &origin).await;
    crate::bridge::set_badge(app, 0);
    if let Some(w) = shell::main_window(app) {
        // the window uses this server's own data store, so this clears only its data
        if state(app).lock().window_store == Some(origin.store_id()) {
            if let Err(e) = w.clear_all_browsing_data() {
                log::warn!("webview data of the signed-out server not cleared: {e}");
                cleanup.browsing_data = false;
            }
        }
    }
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.last_good = None;
        inner.account_epoch += 1;
    }
    let notice = sign_out_notice(&cleanup, &origin.display_name());
    shell::show_local(app, Phase::SignIn, Some(notice));
}

/// What a sign-out managed to do (codex review 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleanup {
    pub remote: Remote,
    pub keychain: bool,
    pub offline_files: bool,
    pub queue_file: bool,
    pub browsing_data: bool,
}

impl Cleanup {
    /// Nothing of the sign-in is left on this computer.
    pub fn local_done(&self) -> bool {
        self.keychain && self.offline_files && self.queue_file
    }
}

/// The parts every sign-out shares (current server, Settings → Servers, Forget): the device on
/// the server, the keychain entry, the queued captures, the offline files. The server is marked
/// in settings first (`pending_sign_outs`) and unmarked only when everything local is gone, so a
/// refused deletion or a crash is retried at the next start and never restored meanwhile.
async fn end_sign_in(app: &AppHandle, origin: &ServerOrigin) -> Cleanup {
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.settings.sign_out_started(origin);
        inner.save_settings();
    }
    let report = state(app).session.sign_out(origin).await;
    let queue_file = crate::capture::drop_queue(app, origin);
    let offline_files = crate::offline::wipe(app, origin);
    let cleanup = Cleanup {
        remote: report.remote,
        keychain: report.keychain_deleted,
        offline_files,
        queue_file,
        browsing_data: true,
    };
    if cleanup.local_done() {
        let s = state(app);
        let mut inner = s.lock();
        inner.settings.sign_out_done(origin);
        inner.save_settings();
    }
    log::info!(
        "signed out: server {:?}, keychain {}, offline files {}, queue {}",
        cleanup.remote,
        if cleanup.keychain { "deleted" } else { "LEFT" },
        if cleanup.offline_files { "deleted" } else { "LEFT" },
        if cleanup.queue_file { "deleted" } else { "LEFT" },
    );
    cleanup
}

/// The sentence after a sign-out: "You are signed out." only when it is true; otherwise what is
/// left and how to fix it (pure, tested).
pub fn sign_out_notice(c: &Cleanup, host: &str) -> Notice {
    let mut left = Vec::new();
    if c.remote == Remote::NotConfirmed {
        left.push(format!(
            "{host} could not be reached to end this computer's session there: remove this computer from the devices in your account settings on {host}."
        ));
    }
    if !c.keychain {
        left.push(
            "The saved sign-in could not be removed from the system keychain. Brainitive never uses it again and retries at the next start; you can also delete the \"app.brainitive.desktop\" entry yourself (Keychain Access / Credential Manager)."
                .to_string(),
        );
    }
    if !c.offline_files || !c.queue_file {
        left.push(
            "Some offline data of this account could not be deleted from this computer yet. Brainitive retries at the next start."
                .to_string(),
        );
    }
    if !c.browsing_data {
        left.push(format!(
            "The web data of {host} in this app could not be cleared: use \"Forget this server\" in Settings → Servers to delete it."
        ));
    }
    if left.is_empty() {
        Notice::info("You are signed out.")
    } else {
        Notice::error(format!(
            "You are signed out on this computer, but not everything could be cleaned up. {}",
            left.join(" ")
        ))
    }
}

/// A native notification when a sign-out of a server that is not on screen (Settings → Servers,
/// Forget) left something behind.
fn report_background_sign_out(app: &AppHandle, cleanup: &Cleanup, host: &str) {
    let notice = sign_out_notice(cleanup, host);
    if notice.level == "error" {
        crate::bridge::show_notification(app, "Sign-out not complete", &notice.message, None);
    }
}

/// Start: sign-outs whose cleanup was not finished (the keychain refused, a crash). Their
/// servers are closed in the session at once (nothing is restored from a leftover keychain
/// entry), then the deletion is tried again.
pub fn close_unfinished_sign_outs(app: &AppHandle) -> Vec<ServerOrigin> {
    let pending = state(app).lock().settings.pending_sign_outs.clone();
    for origin in &pending {
        state(app).session.close(origin);
    }
    pending
}

pub async fn finish_sign_outs(app: &AppHandle, pending: Vec<ServerOrigin>) {
    for origin in pending {
        let keychain = state(app).session.delete_stored_sign_in(&origin).await;
        let queue_file = crate::capture::drop_queue(app, &origin);
        let offline_files = crate::offline::wipe(app, &origin);
        if keychain && queue_file && offline_files {
            log::info!("an unfinished sign-out is complete now");
            let s = state(app);
            let mut inner = s.lock();
            inner.settings.sign_out_done(&origin);
            inner.save_settings();
        } else {
            log::warn!("an unfinished sign-out still could not delete everything; retried at the next start");
        }
    }
}

/// BRAI-377: nothing of the account that signs out stays in memory or in a hidden window — a
/// running voice recording is cancelled, a transcript waiting for quick capture is dropped, and
/// the quick-capture / switcher / offline windows (their text, last results) are destroyed.
fn clear_account_traces(app: &AppHandle) {
    crate::voice::cancel_for_sign_out(app);
    state(app).lock().capture_prefill = None;
    crate::new_task::clear(app);
    crate::popup::close_account_windows(app);
}

/// "Switch server…" / "Add Server…": back to the choice, without signing out of the current
/// server (its "Back to …" button and the saved servers switch without a new sign-in).
pub fn switch_server(app: &AppHandle) {
    state(app).session.cancel_sign_in();
    shell::show_local(app, Phase::Choose, None);
}

/// The id of a saved server (`servers::server_id`) → its origin. Commands never take an origin
/// from a page for a switch: only servers the user added through the connect flow.
pub fn saved_server(app: &AppHandle, id: &str) -> Result<ServerOrigin, UiError> {
    state(app)
        .lock()
        .settings
        .server_by_id(id)
        .map(|r| r.origin.clone())
        .ok_or_else(|| UiError::new("unknown_server", "That server is not in your list any more."))
}

/// A switch to a saved server (menu ⌃⌘1…9, tray, the panel's switcher, Settings, the Choose
/// screen): the old server's waiting offline changes get one bounded try, its in-memory account
/// data goes (not its keychain entry, store or files), then the new one is restored like at
/// start — its own webview store, its stored sign-in, else its sign-in screen.
pub async fn switch_to(app: &AppHandle, id: &str) -> Result<(), UiError> {
    let origin = saved_server(app, id)?;
    let (previous, phase) = {
        let s = state(app);
        let inner = s.lock();
        (inner.current_origin(), inner.phase)
    };
    shell::focus_main(app);
    if previous.as_ref() == Some(&origin) {
        // already there (or on its way): nothing to switch; from the Choose / sign-in screens it
        // goes back into this server
        if !matches!(phase, Phase::InApp | Phase::Connecting | Phase::Waiting) {
            restore(app, origin, None).await;
        }
        return Ok(());
    }
    log::info!("switching servers");
    if let Some(previous) = &previous {
        leave_server(app, previous).await;
    }
    state(app).session.cancel_sign_in();
    {
        let s = state(app);
        let mut inner = s.lock();
        // forgotten while the old server's outbox was sent
        if !inner.settings.touch(&origin) {
            return Err(UiError::new(
                "unknown_server",
                "That server is not in your list any more.",
            ));
        }
        inner.save_settings();
    }
    servers_changed(app);
    restore(app, origin, None).await;
    Ok(())
}

/// Leaving `previous` for another server (a switch, or another server on the Choose screen):
/// its waiting offline changes first (bounded, to that server only), then everything of its
/// account that is in memory or in a hidden window. Its keychain entry, webview store, offline
/// files, capture queue and New-task memory stay, so switching back needs no new sign-in.
async fn leave_server(app: &AppHandle, previous: &ServerOrigin) {
    crate::offline::send_before_leaving(app, previous).await;
    crate::voice::cancel_for_sign_out(app);
    crate::popup::close_account_windows(app);
    drop_account_state(&mut state(app).lock());
    crate::bridge::set_badge(app, 0);
}

/// The in-memory account data of the server that was current (pure, tested): the account
/// epoch moves on (a poll, panel load, New-task call or sync still running for it writes
/// nothing), the panel's and New task's answers, the tray data, the ETag cache, a capture
/// prefill and the loaded offline data go. Per-origin data on disk stays.
pub fn drop_account_state(inner: &mut crate::state::Inner) {
    inner.account_epoch += 1;
    inner.capture_prefill = None;
    inner.task = Default::default();
    inner.panel.clear_account();
    inner.tray_data = Default::default();
    inner.tray_data.my_day_stale = true;
    inner.etags.clear();
    inner.offline.unload();
    inner.instance = None;
    inner.last_good = None;
    inner.reauth_at = None;
}

/// The saved servers, a name or the current one changed: title, Servers menu, tray (and the
/// panel's header, which `tray::refresh` tells).
pub fn servers_changed(app: &AppHandle) {
    shell::refresh_title(app);
    crate::menu::refresh_servers(app);
    crate::tray::refresh(app);
}

/// Settings → Servers → Sign out. The current server: the usual sign-out (sign-in screen). Another
/// one: its device is signed out on the server, its keychain entry, capture queue, offline files
/// and webview data go — but it stays in the list (unlike Forget), ready to sign in again.
pub async fn sign_out_server(app: &AppHandle, origin: ServerOrigin) {
    if state(app).lock().current_origin().as_ref() == Some(&origin) {
        sign_out(app).await;
        servers_changed(app);
        return;
    }
    let cleanup = end_sign_in(app, &origin).await;
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.settings.schedule_wipe(&origin);
        inner.save_settings();
    }
    shell::run_pending_wipes(app).await;
    servers_changed(app);
    report_background_sign_out(app, &cleanup, &origin.display_name());
}

/// "Forget this server": sign out there, delete its keychain entry and its webview data,
/// remove it from the recent list.
pub async fn forget_server(app: &AppHandle, origin: ServerOrigin) {
    let label = state(app).lock().settings.label_of(&origin);
    if state(app).lock().current_origin().as_ref() == Some(&origin) {
        state(app).lock().account_epoch += 1;
        clear_account_traces(app);
    }
    let cleanup = end_sign_in(app, &origin).await;
    report_background_sign_out(app, &cleanup, &origin.display_name());
    let was_current = {
        let s = state(app);
        let mut inner = s.lock();
        let was_current = inner.current_origin().as_ref() == Some(&origin);
        if was_current {
            inner.account_epoch += 1;
            inner.instance = None;
            inner.last_good = None;
        }
        inner.settings.forget(&origin);
        inner.save_settings();
        was_current
    };
    servers_changed(app);
    if was_current {
        crate::bridge::set_badge(app, 0);
    }
    let notice = Notice::info(format!("{label} was removed from this app."));
    let uses_store = state(app).lock().window_store == Some(origin.store_id());
    if uses_store {
        // move the window to the neutral store; the old one is deleted once it is released
        {
            let s = state(app);
            let mut inner = s.lock();
            if was_current {
                inner.phase = Phase::Choose;
            }
            inner.notice = Some(notice);
        }
        shell::open_main(
            app,
            crate::server::local_store_id(),
            tauri::WebviewUrl::App("index.html".into()),
        );
    } else if was_current {
        shell::show_local(app, Phase::Choose, Some(notice));
        shell::run_pending_wipes(app).await;
    } else {
        // another server than the current one (Settings → Servers): the app stays where it is
        shell::run_pending_wipes(app).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    fn o(s: &str) -> ServerOrigin {
        ServerOrigin::parse(s).unwrap()
    }

    fn app_state(name: &str) -> (AppState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("brainitive-switch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let state = AppState::new(dir.join("settings.json"), dir.join("queue.json"), dir.join("offline"));
        (state, dir)
    }

    #[test]
    fn a_switch_leaves_nothing_of_the_old_server_in_memory_and_keeps_its_data() {
        use crate::new_task::model::{remember, Memory};
        use crate::queue::{CaptureInput, QueuedCapture};
        let (app_state, dir) = app_state("leave");
        let work = o("work.acme.com");
        let private = ServerOrigin::cloud();
        let mut inner = app_state.lock();
        inner.settings.remember(&private, "Brainitive", None);
        inner.settings.remember(&work, "Acme", None);
        inner.phase = Phase::InApp;

        // the work server's account in memory: panel answers, New task, tray, ETags, offline data
        let kept = crate::panel::kept_for_current(&mut inner).expect("signed in");
        kept.today = Some(crate::panel::Stored {
            body: serde_json::json!({"events": []}),
            at: Instant::now(),
        });
        let epoch = inner.account_epoch;
        inner.task = crate::new_task::TaskState::kept_for_test(&work, epoch);
        inner.tray_data.unread = Some(7);
        inner.capture_prefill = Some("dictated for work".into());
        let key = crate::tray::etag_key(&work, "/api/v1/me/timer");
        inner.etags.insert(key, ("\"e1\"".into(), serde_json::json!({})));
        inner.offline.ensure(&work);
        inner
            .offline
            .outbox
            .push(crate::outbox::Change::CaptureCreate {
                text: "offline work note".into(),
                url: None,
                intended_type: None,
            })
            .unwrap();
        let offline_epoch = inner.offline.epoch;
        // data on disk / per origin that must stay with the work server
        let note = CaptureInput::new("queued for work", None, None).unwrap();
        inner.queue.push(QueuedCapture::new(&work, note)).unwrap();
        let memory = Memory {
            project_id: Some(10),
            board_id: Some(100),
            type_id: None,
            at: 1,
        };
        remember(&mut inner.settings.preferences.new_task, work.as_str(), memory);

        drop_account_state(&mut inner);
        assert!(inner.settings.touch(&private));

        // nothing of the work account is in memory any more
        assert!(inner.account_epoch > epoch, "polls / loads still running write nothing");
        assert!(!inner.task.holds_answers());
        assert!(!inner.panel.holds_answers());
        assert!(crate::panel::kept_for_current(&mut inner).unwrap().today.is_none());
        assert_eq!(inner.tray_data.unread, None);
        assert!(inner.tray_data.my_day_stale);
        assert!(inner.etags.is_empty());
        assert_eq!(inner.capture_prefill, None);
        assert!(inner.instance.is_none() && inner.last_good.is_none());
        assert_eq!(inner.offline.origin, None, "unloaded");
        assert!(inner.offline.outbox.is_empty());
        assert!(inner.offline.epoch > offline_epoch, "a sync of the work server stops");
        // the private server sees none of it
        assert_eq!(inner.queue.pending_for(&private).len(), 0);
        assert!(!inner.settings.preferences.new_task.contains_key(private.as_str()));
        // … and the work server keeps it for when the user comes back
        assert_eq!(inner.queue.pending_for(&work).len(), 1);
        assert_eq!(inner.settings.preferences.new_task[work.as_str()], memory);
        assert!(inner.settings.server_by_id(&crate::servers::server_id(&work)).is_some());
        drop(inner);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_task_or_panel_answer_of_the_old_server_is_not_shown_for_the_new_one() {
        let (app_state, dir) = app_state("tickets");
        let a = o("a.example.com");
        let b = o("b.example.com");
        let mut inner = app_state.lock();
        inner.settings.remember(&a, "A", None);
        inner.settings.remember(&b, "B", None);
        assert!(inner.settings.touch(&a));
        inner.phase = Phase::InApp;
        let ticket = crate::tray::PollTicket::now(&inner).unwrap();
        drop_account_state(&mut inner);
        assert!(inner.settings.touch(&b));
        // an answer for A that arrives now is dropped (`PollTicket::holds`)
        assert!(!ticket.still_current(inner.current_origin().as_ref(), inner.account_epoch, true));
        // switching back to A is a new sign-in epoch too: A's late answer stays dropped
        drop_account_state(&mut inner);
        assert!(inner.settings.touch(&a));
        assert!(!ticket.still_current(inner.current_origin().as_ref(), inner.account_epoch, true));
        drop(inner);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sign_out_keeps_the_server_saved_and_forget_removes_it() {
        use crate::new_task::model::{remember, Memory};
        let mut s = crate::settings::Settings::default();
        let a = o("a.example.com");
        let b = o("b.example.com");
        s.remember(&a, "A", None);
        s.remember(&b, "B", None);
        let id = crate::servers::server_id(&a);
        s.rename(&id, "Work");
        remember(&mut s.preferences.new_task, a.as_str(), Memory::default());
        // sign-out (of a server that is not current): its webview data is scheduled, it stays
        s.schedule_wipe(&a);
        assert!(s.server_by_id(&id).is_some());
        assert_eq!(s.label_of(&a), "Work", "keeps its name and place");
        assert_eq!(s.recent[0].origin, a);
        assert!(s.preferences.new_task.contains_key(a.as_str()));
        assert_eq!(s.current, Some(b.clone()));
        // signing in there again later keeps the fresh data from the old wipe
        assert!(s.touch(&a));
        assert!(s.pending_wipes.is_empty());
        // forget: off the list, its memory gone, its data scheduled
        s.forget(&a);
        assert!(s.server_by_id(&id).is_none());
        assert!(!s.preferences.new_task.contains_key(a.as_str()));
        assert_eq!(s.pending_wipes, vec![crate::server::hex(&a.store_id())]);
        assert_eq!(s.current, None);
    }

    #[test]
    fn a_server_that_now_answers_elsewhere_needs_a_new_confirm() {
        let o = |s: &str| ServerOrigin::parse(s).unwrap();
        assert!(unconfirmed_redirect(&o("https://a.example.com"), &o("https://a.example.com")).is_none());
        let refused = unconfirmed_redirect(&o("https://a.example.com"), &o("https://b.example.com")).unwrap();
        assert_eq!(refused.kind, "redirected");
        assert!(refused.message.contains("https://b.example.com"), "{}", refused.message);
    }

    #[test]
    fn a_sign_out_says_what_is_left_instead_of_claiming_success() {
        let all = Cleanup {
            remote: Remote::Revoked,
            keychain: true,
            offline_files: true,
            queue_file: true,
            browsing_data: true,
        };
        assert_eq!(sign_out_notice(&all, "acme.com").message, "You are signed out.");
        assert_eq!(sign_out_notice(&all, "acme.com").level, "info");
        let ended = Cleanup {
            remote: Remote::AlreadyEnded,
            ..all
        };
        assert_eq!(
            sign_out_notice(&ended, "acme.com").level,
            "info",
            "nothing left on the server"
        );

        let offline = Cleanup {
            remote: Remote::NotConfirmed,
            ..all
        };
        let n = sign_out_notice(&offline, "acme.com");
        assert_eq!(n.level, "error");
        assert!(n.message.contains("acme.com could not be reached"), "{}", n.message);
        assert!(n.message.contains("devices in your account settings"), "{}", n.message);

        let keychain = Cleanup { keychain: false, ..all };
        let n = sign_out_notice(&keychain, "acme.com");
        assert_eq!(n.level, "error");
        assert!(n.message.contains("keychain"), "{}", n.message);
        assert!(!n.message.starts_with("You are signed out."), "{}", n.message);
        assert!(!keychain.local_done());

        let files = Cleanup {
            offline_files: false,
            ..all
        };
        assert!(sign_out_notice(&files, "acme.com").message.contains("offline data"));
        assert!(!files.local_done());
        let queue = Cleanup {
            queue_file: false,
            ..all
        };
        assert!(!queue.local_done());
        let web = Cleanup {
            browsing_data: false,
            ..all
        };
        let n = sign_out_notice(&web, "acme.com");
        assert!(n.message.contains("Forget this server"), "{}", n.message);
        assert!(
            web.local_done(),
            "the webview store is handled by Forget, not the start-up retry"
        );
        // never a token or an error chain in the text
        for c in [offline, keychain, files, web] {
            let m = sign_out_notice(&c, "acme.com").message;
            assert!(!m.contains("brr_") && !m.contains("bra_"), "{m}");
        }
    }

    #[test]
    fn an_unfinished_sign_out_stays_marked_until_its_cleanup_is_done() {
        let mut s = crate::settings::Settings::default();
        let a = o("a.example.com");
        s.sign_out_started(&a);
        s.sign_out_started(&a);
        assert_eq!(s.pending_sign_outs, vec![a.clone()]);
        // survives a restart
        let json = serde_json::to_string(&s).unwrap();
        let back: crate::settings::Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pending_sign_outs, vec![a.clone()]);
        s.sign_out_done(&a);
        assert!(s.pending_sign_outs.is_empty());
        assert!(!serde_json::to_string(&s).unwrap().contains("pending_sign_outs"));
    }
}
