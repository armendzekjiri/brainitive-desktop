//! Native sign-in and tokens (BRAI-355 / BRAI-357 / BRAI-428).
//!
//! - Sign-in: PKCE verifier + state in memory → system browser at `/app/authorize` →
//!   `brainitive://auth/callback` → state check → `POST /api/v1/auth/exchange`.
//! - The access token (15 min) is kept in memory only; the refresh token (single use, rotates)
//!   in the keychain only.
//! - **Refreshes are serialized**: one async mutex guards the token map *and* the refresh call,
//!   so two callers never present the same refresh token (the server would treat the second as
//!   reuse and revoke the device). The new refresh token is written to the keychain before the
//!   new access token is used.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::Mutex;
use url::Url;

use crate::api::{ApiClient, ApiError, ApiRequest, ApiResponse, DeviceDescription, InstanceInfo, TokenPair};
use crate::pkce::{state_matches, PkcePair};
use crate::secrets;
use crate::server::ServerOrigin;
use crate::version::APP_VERSION;

/// A sign-in started in the browser expires after this long (the server code itself: 60 s).
const PENDING_TTL: Duration = Duration::from_secs(15 * 60);
/// Refresh this long before the access token expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum SessionError {
    /// No (valid) stored sign-in for this server: show the sign-in screen.
    #[error("{0}")]
    SignedOut(String),
    #[error("No sign-in is in progress. Start it again from the app.")]
    NoPendingSignIn,
    #[error("This sign-in link does not belong to the sign-in started in this app.")]
    StateMismatch,
    #[error("The sign-in took too long. Please start it again.")]
    Expired,
    #[error("Sign-in was cancelled in the browser.")]
    Cancelled,
    #[error("{0}")]
    Keychain(String),
    #[error("{0}")]
    Api(String),
    #[error("{0}")]
    Offline(String),
}

impl From<ApiError> for SessionError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Unauthorized { message, .. } => Self::SignedOut(message),
            ApiError::Api {
                ref code, ref message, ..
            } if code == "invalid_grant" => Self::SignedOut(message.clone()),
            e if e.is_connectivity() => Self::Offline(e.to_string()),
            e => Self::Api(e.to_string()),
        }
    }
}

impl From<secrets::KeychainError> for SessionError {
    fn from(e: secrets::KeychainError) -> Self {
        Self::Keychain(e.to_string())
    }
}

/// The outcome of an App API call of the native windows, kept detailed enough to decide
/// whether a queued capture is retried later or dropped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    /// No valid stored sign-in (or it was revoked): the user has to sign in again.
    #[error("{0}")]
    SignedOut(String),
    /// Network down, DNS, timeout: try again later.
    #[error("{0}")]
    Offline(String),
    /// The server answered with an error status.
    #[error("{message}")]
    Server { status: u16, code: String, message: String },
    /// Keychain, TLS, an unexpected answer.
    #[error("{0}")]
    Failed(String),
}

impl CallError {
    /// Worth retrying later with the same `request_id` (offline, 5xx, 408, 429, odd answers).
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Offline(_) | Self::Failed(_) => true,
            Self::Server { status, .. } => *status >= 500 || *status == 408 || *status == 429,
            Self::SignedOut(_) => false,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Server { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl From<ApiError> for CallError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Unauthorized { message, .. } => Self::SignedOut(message),
            ApiError::Api { status, code, message } => Self::Server { status, code, message },
            e if e.is_connectivity() => Self::Offline(e.to_string()),
            e => Self::Failed(e.to_string()),
        }
    }
}

impl From<SessionError> for CallError {
    fn from(e: SessionError) -> Self {
        match e {
            SessionError::SignedOut(m) => Self::SignedOut(m),
            SessionError::Offline(m) => Self::Offline(m),
            e => Self::Failed(e.to_string()),
        }
    }
}

struct Pending {
    origin: ServerOrigin,
    pkce: PkcePair,
    started: Instant,
    /// The origin's session generation when the sign-in began.
    generation: u64,
}

struct Access {
    token: String,
    expires_at: Instant,
}

/// The session epoch (codex review, finding 3): per origin, a generation that every sign-out
/// and every completed sign-in moves on, and whether a sign-out began. A refresh or a sign-in
/// that started under an older generation, or for an origin that is signing out, never stores
/// tokens. One mutex, never held across an `.await`.
#[derive(Default)]
struct Control {
    pending: Option<Pending>,
    generation: HashMap<ServerOrigin, u64>,
    /// A sign-out began: no token is handed out or stored until a sign-in completes.
    closed: HashSet<ServerOrigin>,
}

impl Control {
    fn generation(&self, origin: &ServerOrigin) -> u64 {
        self.generation.get(origin).copied().unwrap_or(0)
    }
}

#[derive(Default)]
struct Tokens {
    access: HashMap<ServerOrigin, Access>,
    /// An access token a refresh got after the sign-out began (it was not stored): the sign-out
    /// uses it to revoke the device.
    orphans: HashMap<ServerOrigin, String>,
}

/// What the server said to "sign this device out".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    /// `POST /auth/logout` succeeded: the device is revoked.
    Revoked,
    /// The stored sign-in was already dead on the server (revoked, expired): nothing to revoke.
    AlreadyEnded,
    /// The server could not be reached or refused: the device may still be signed in there.
    NotConfirmed,
}

/// The result of [`Session::sign_out`]: what is gone for sure, what may be left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignOutReport {
    pub remote: Remote,
    /// The keychain entry is gone (or a newer sign-in owns it now).
    pub keychain_deleted: bool,
}

/// How often a refused keychain deletion is tried again during one sign-out.
const DELETE_TRIES: u32 = 3;

pub struct Session {
    api: ApiClient,
    control: std::sync::Mutex<Control>,
    tokens: Mutex<Tokens>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(ApiClient::new())
    }
}

impl Session {
    pub fn new(api: ApiClient) -> Self {
        Self {
            api,
            control: std::sync::Mutex::new(Control::default()),
            tokens: Mutex::new(Tokens::default()),
        }
    }

    pub fn api(&self) -> &ApiClient {
        &self.api
    }

    fn control(&self) -> std::sync::MutexGuard<'_, Control> {
        self.control.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts a sign-in for `origin` and returns the browser URL. A previous unfinished
    /// sign-in is replaced (its callback will then fail the state check).
    pub fn begin_sign_in(&self, origin: &ServerOrigin, instance: &InstanceInfo) -> Url {
        let pkce = PkcePair::generate();
        let url = authorize_url(origin, instance, &pkce, &device_name());
        let mut control = self.control();
        let generation = control.generation(origin);
        control.pending = Some(Pending {
            origin: origin.clone(),
            pkce,
            started: Instant::now(),
            generation,
        });
        url
    }

    pub fn cancel_sign_in(&self) {
        self.control().pending.take();
    }

    pub fn sign_in_pending_for(&self) -> Option<ServerOrigin> {
        self.control()
            .pending
            .as_ref()
            .filter(|p| p.started.elapsed() < PENDING_TTL)
            .map(|p| p.origin.clone())
    }

    /// `brainitive://auth/callback?error=…&state=…`: ends the pending sign-in if the state is ours.
    pub fn fail_sign_in(&self, state: Option<&str>) -> bool {
        let mut control = self.control();
        let ours = matches!((control.pending.as_ref(), state), (Some(p), Some(s)) if state_matches(&p.pkce.state, s));
        if ours {
            control.pending.take();
        }
        ours
    }

    /// Whether credentials that `generation` produced may still be stored for `origin`. A
    /// completed sign-in (`sign_in`) reopens a closed origin and starts a new generation.
    fn claim(&self, origin: &ServerOrigin, generation: u64, sign_in: bool) -> bool {
        let mut control = self.control();
        if control.generation(origin) != generation {
            return false;
        }
        if sign_in {
            control.closed.remove(origin);
            *control.generation.entry(origin.clone()).or_insert(0) += 1;
            return true;
        }
        !control.closed.contains(origin)
    }

    /// `brainitive://auth/callback?code=…&state=…`: checks the state (single use: the pending
    /// sign-in is consumed either way), exchanges the code and stores the tokens — unless the
    /// user signed out of that server while the exchange ran (the new device is revoked then).
    pub async fn complete_sign_in(&self, code: &str, state: &str) -> Result<ServerOrigin, SessionError> {
        let pending = {
            let mut control = self.control();
            match control.pending.as_ref() {
                None => return Err(SessionError::NoPendingSignIn),
                // a forged or stale link must not cancel the real sign-in in progress
                Some(p) if !state_matches(&p.pkce.state, state) => return Err(SessionError::StateMismatch),
                Some(_) => control.pending.take().expect("checked above"),
            }
        };
        if pending.started.elapsed() > PENDING_TTL {
            return Err(SessionError::Expired);
        }

        let device = DeviceDescription {
            name: device_name(),
            platform: crate::api::platform(),
            app_version: APP_VERSION,
            os_version: os_version(),
        };
        let pair = self
            .api
            .exchange(&pending.origin, code, &pending.pkce.verifier, &device)
            .await?;
        let mut tokens = self.tokens.lock().await;
        if !self.claim(&pending.origin, pending.generation, true) {
            drop(tokens);
            log::info!("sign-in finished after a sign-out: its tokens are not kept");
            let _ = tokio::time::timeout(LOGOUT_TIMEOUT, self.api.logout(&pending.origin, &pair.access_token)).await;
            return Err(SessionError::Cancelled);
        }
        self.store_pair(&pending.origin, pair, &mut tokens).await?;
        Ok(pending.origin)
    }

    /// Whether a refresh token is stored for `origin` (one keychain read). `false` while a
    /// sign-out of it runs or ran (until the next sign-in).
    pub async fn has_stored_sign_in(&self, origin: &ServerOrigin) -> bool {
        if self.control().closed.contains(origin) {
            return false;
        }
        let origin = origin.clone();
        tauri::async_runtime::spawn_blocking(move || secrets::load_refresh_token(&origin))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .is_some()
    }

    /// A valid access token for `origin`, refreshing (serialized) when needed.
    pub async fn access_token(&self, origin: &ServerOrigin) -> Result<String, SessionError> {
        let mut tokens = self.tokens.lock().await;
        if self.control().closed.contains(origin) {
            return Err(signed_out());
        }
        if let Some(access) = tokens.access.get(origin) {
            if access.expires_at > Instant::now() + REFRESH_MARGIN {
                return Ok(access.token.clone());
            }
        }
        self.refresh_locked(origin, &mut tokens).await
    }

    /// Forces a refresh (e.g. the server answered 401 to a still-unexpired access token).
    pub async fn refresh(&self, origin: &ServerOrigin) -> Result<String, SessionError> {
        let mut tokens = self.tokens.lock().await;
        self.refresh_locked(origin, &mut tokens).await
    }

    async fn refresh_locked(&self, origin: &ServerOrigin, tokens: &mut Tokens) -> Result<String, SessionError> {
        let generation = {
            let control = self.control();
            if control.closed.contains(origin) {
                return Err(signed_out());
            }
            control.generation(origin)
        };
        tokens.access.remove(origin);
        let o = origin.clone();
        let stored = tauri::async_runtime::spawn_blocking(move || secrets::load_refresh_token(&o))
            .await
            .map_err(|e| SessionError::Keychain(e.to_string()))??;
        let Some(refresh_token) = stored else {
            return Err(SessionError::SignedOut("Please sign in.".into()));
        };
        match self.api.refresh(origin, &refresh_token).await {
            // a sign-out began while the refresh ran: nothing is stored, the sign-out revokes
            // the device with this token
            Ok(pair) if !self.claim(origin, generation, false) => {
                tokens.orphans.insert(origin.clone(), pair.access_token);
                Err(signed_out())
            }
            Ok(pair) => self.store_pair(origin, pair, tokens).await,
            Err(e) => {
                let e = SessionError::from(e);
                if matches!(e, SessionError::SignedOut(_)) {
                    // revoked, expired or reused: the stored token is dead
                    let o = origin.clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || secrets::delete_refresh_token(&o)).await;
                }
                Err(e)
            }
        }
    }

    async fn store_pair(
        &self,
        origin: &ServerOrigin,
        pair: TokenPair,
        tokens: &mut Tokens,
    ) -> Result<String, SessionError> {
        let o = origin.clone();
        let refresh = pair.refresh_token.clone();
        tauri::async_runtime::spawn_blocking(move || secrets::store_refresh_token(&o, &refresh))
            .await
            .map_err(|e| SessionError::Keychain(e.to_string()))??;
        let ttl = Duration::from_secs(pair.expires_in.clamp(30, 24 * 3600));
        tokens.access.insert(
            origin.clone(),
            Access {
                token: pair.access_token.clone(),
                expires_at: Instant::now() + ttl,
            },
        );
        Ok(pair.access_token)
    }

    /// An App API call with this server's access token: refreshed (serialized) when it is about
    /// to expire, and once more plus one retry when the server answers 401 anyway.
    pub async fn request(&self, origin: &ServerOrigin, request: &ApiRequest) -> Result<ApiResponse, CallError> {
        let token = self.access_token(origin).await?;
        match self.api.request(origin, request, &token).await {
            Err(ApiError::Unauthorized { .. }) => {
                let token = self.refresh(origin).await?;
                Ok(self.api.request(origin, request, &token).await?)
            }
            other => Ok(other?),
        }
    }

    /// Whether an access token for `origin` is in memory (no keychain read, no network).
    pub async fn has_access_token(&self, origin: &ServerOrigin) -> bool {
        self.tokens.lock().await.access.contains_key(origin) && !self.control().closed.contains(origin)
    }

    /// The one-time URL that signs the webview in, optionally landing on `to` (a path).
    pub async fn web_session_url(&self, origin: &ServerOrigin, to: Option<&str>) -> Result<Url, SessionError> {
        let token = self.access_token(origin).await?;
        let mut url = match self.api.web_session(origin, &token).await {
            Ok(url) => url,
            // the access token died early (device revoked elsewhere?) → one refresh, one retry
            Err(ApiError::Unauthorized { .. }) => {
                let token = self.refresh(origin).await?;
                self.api.web_session(origin, &token).await?
            }
            Err(e) => return Err(e.into()),
        };
        if let Some(to) = to.filter(|t| t.starts_with('/') && !t.starts_with("//")) {
            url.query_pairs_mut().append_pair("to", to);
        }
        Ok(url)
    }

    /// Signs this device out of `origin`: first the session epoch moves on (no refresh or
    /// sign-in still running can store a token from now on, and none is handed out), then the
    /// server revokes the device (bounded), then the keychain entry goes (tried again when the
    /// keychain refuses). Reports what could not be done instead of claiming success.
    pub async fn sign_out(&self, origin: &ServerOrigin) -> SignOutReport {
        let generation = {
            let mut control = self.control();
            let generation = control.generation.entry(origin.clone()).or_insert(0);
            *generation += 1;
            let generation = *generation;
            control.closed.insert(origin.clone());
            if control.pending.as_ref().is_some_and(|p| &p.origin == origin) {
                control.pending.take();
            }
            generation
        };
        // waits for a refresh / sign-in that is storing right now; afterwards none can
        let token = {
            let mut tokens = self.tokens.lock().await;
            let orphan = tokens.orphans.remove(origin);
            match tokens.access.remove(origin) {
                Some(a) if a.expires_at > Instant::now() => Ok(a.token),
                _ => match orphan {
                    Some(token) => Ok(token),
                    // still revoke the device on the server: one refresh just to log out (the
                    // rotated token is never stored; the old one is dead after the rotation)
                    None => self.refresh_for_logout(origin).await,
                },
            }
        };
        let remote = match token {
            Ok(token) => match tokio::time::timeout(LOGOUT_TIMEOUT, self.api.logout(origin, &token)).await {
                Ok(Ok(())) => Remote::Revoked,
                Ok(Err(ApiError::Unauthorized { .. })) => Remote::AlreadyEnded,
                _ => Remote::NotConfirmed,
            },
            Err(remote) => remote,
        };
        let keychain_deleted = {
            let mut tokens = self.tokens.lock().await;
            if self.control().generation(origin) != generation {
                // a sign-in completed after this sign-out began: its credentials stay
                true
            } else {
                tokens.access.remove(origin);
                tokens.orphans.remove(origin);
                delete_with_retries(origin).await
            }
        };
        SignOutReport {
            remote,
            keychain_deleted,
        }
    }

    /// A fresh access token only to call `/auth/logout` (not stored anywhere). `Err`: what that
    /// means for the device on the server.
    async fn refresh_for_logout(&self, origin: &ServerOrigin) -> Result<String, Remote> {
        let o = origin.clone();
        let stored = tauri::async_runtime::spawn_blocking(move || secrets::load_refresh_token(&o))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
        let Some(refresh_token) = stored else {
            return Err(Remote::NotConfirmed);
        };
        match self.api.refresh(origin, &refresh_token).await {
            Ok(pair) => Ok(pair.access_token),
            Err(e) => match SessionError::from(e) {
                SessionError::SignedOut(_) => Err(Remote::AlreadyEnded),
                _ => Err(Remote::NotConfirmed),
            },
        }
    }

    /// The keychain entry of `origin` only (another try after a failed sign-out cleanup).
    pub async fn delete_stored_sign_in(&self, origin: &ServerOrigin) -> bool {
        let _tokens = self.tokens.lock().await;
        if !self.control().closed.contains(origin) {
            // signed in again meanwhile: that sign-in's entry stays
            return true;
        }
        delete_with_retries(origin).await
    }

    /// Marks `origin` as signed out without a network call (a cleanup left from an earlier run):
    /// nothing is restored from a keychain entry that could not be deleted yet.
    pub fn close(&self, origin: &ServerOrigin) {
        self.control().closed.insert(origin.clone());
    }
}

const LOGOUT_TIMEOUT: Duration = Duration::from_secs(8);

fn signed_out() -> SessionError {
    SessionError::SignedOut("You signed out of this server.".into())
}

async fn delete_with_retries(origin: &ServerOrigin) -> bool {
    for attempt in 0..DELETE_TRIES {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let o = origin.clone();
        match tauri::async_runtime::spawn_blocking(move || secrets::delete_refresh_token(&o)).await {
            Ok(Ok(())) => return true,
            Ok(Err(e)) => log::warn!("keychain entry not deleted (try {}): {e}", attempt + 1),
            Err(e) => log::warn!("keychain entry not deleted (try {}): {e}", attempt + 1),
        }
    }
    false
}

pub fn authorize_url(origin: &ServerOrigin, instance: &InstanceInfo, pkce: &PkcePair, device: &str) -> Url {
    let mut url = origin.join(instance.authorize_path());
    url.query_pairs_mut()
        .append_pair("client", "desktop")
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &pkce.state)
        .append_pair("device_name", device);
    url
}

/// "Armend's MacBook Pro" etc. — shown in the consent page and the server's device list.
pub fn device_name() -> String {
    let raw = computer_name().unwrap_or_default();
    let name: String = raw.trim().chars().filter(|c| !c.is_control()).take(80).collect();
    if name.is_empty() {
        format!("Brainitive Desktop ({})", crate::api::platform())
    } else {
        name
    }
}

fn computer_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        command_output("scutil", &["--get", "ComputerName"])
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("COMPUTERNAME").ok()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .or_else(|| std::env::var("HOSTNAME").ok())
    }
}

/// `os_version` of the exchange request, e.g. "macOS 15.4".
pub fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        command_output("sw_vers", &["-productVersion"]).map(|v| format!("macOS {}", v.trim()))
    }
    #[cfg(target_os = "windows")]
    {
        None
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let release = std::fs::read_to_string("/etc/os-release").ok()?;
        release
            .lines()
            .find_map(|l| l.strip_prefix("PRETTY_NAME="))
            .map(|v| v.trim_matches('"').chars().take(60).collect())
    }
}

#[cfg(target_os = "macos")]
fn command_output(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_carries_the_pkce_parameters() {
        let origin = ServerOrigin::parse("brainitive.example.com").unwrap();
        let pkce = PkcePair::generate();
        let info = InstanceInfo {
            name: "Acme".into(),
            ..Default::default()
        };
        let url = authorize_url(&origin, &info, &pkce, "Armend's Mac & Co");
        assert_eq!(url.origin().ascii_serialization(), "https://brainitive.example.com");
        assert_eq!(url.path(), "/app/authorize");
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client"], "desktop");
        assert_eq!(q["code_challenge"], pkce.challenge);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], pkce.state);
        assert_eq!(q["device_name"], "Armend's Mac & Co");
        assert!(
            !url.as_str().contains(&pkce.verifier),
            "the verifier never leaves the app"
        );
    }

    #[tokio::test]
    async fn callbacks_need_the_pending_state() {
        let session = Session::default();
        assert!(matches!(
            session.complete_sign_in("code", "state").await,
            Err(SessionError::NoPendingSignIn)
        ));

        let origin = ServerOrigin::parse("brainitive.example.com").unwrap();
        let info = InstanceInfo {
            name: "Acme".into(),
            ..Default::default()
        };
        let url = session.begin_sign_in(&origin, &info);
        let state: String = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.into_owned();

        // a forged link fails and does not cancel the real sign-in
        assert!(matches!(
            session.complete_sign_in("code", "forged").await,
            Err(SessionError::StateMismatch)
        ));
        assert_eq!(session.sign_in_pending_for(), Some(origin.clone()));

        // a cancel with someone else's state is ignored, ours ends it
        assert!(!session.fail_sign_in(Some("forged")));
        assert!(!session.fail_sign_in(None));
        assert!(session.fail_sign_in(Some(&state)));
        assert_eq!(session.sign_in_pending_for(), None);
    }

    #[test]
    fn invalid_grant_means_signed_out() {
        let e: SessionError = ApiError::Api {
            status: 400,
            code: "invalid_grant".into(),
            message: "Expired".into(),
        }
        .into();
        assert!(matches!(e, SessionError::SignedOut(_)));
        let e: SessionError = ApiError::Unauthorized {
            code: "invalid_grant".into(),
            message: "x".into(),
        }
        .into();
        assert!(matches!(e, SessionError::SignedOut(_)));
        let e: SessionError = ApiError::Timeout.into();
        assert!(matches!(e, SessionError::Offline(_)));
        let e: SessionError = ApiError::Api {
            status: 500,
            code: "server_error".into(),
            message: "x".into(),
        }
        .into();
        assert!(matches!(e, SessionError::Api(_)));
    }

    #[test]
    fn call_errors_tell_retry_from_rejected() {
        let e: CallError = ApiError::Timeout.into();
        assert!(e.is_transient());
        let e: CallError = ApiError::Api {
            status: 503,
            code: "server_error".into(),
            message: "x".into(),
        }
        .into();
        assert!(e.is_transient());
        let e: CallError = ApiError::Api {
            status: 429,
            code: "rate_limited".into(),
            message: "x".into(),
        }
        .into();
        assert!(e.is_transient());
        let e: CallError = ApiError::Api {
            status: 422,
            code: "validation_failed".into(),
            message: "Too long".into(),
        }
        .into();
        assert!(!e.is_transient());
        assert_eq!(e.status(), Some(422));
        assert_eq!(e.to_string(), "Too long");
        let e: CallError = ApiError::Unauthorized {
            code: "unauthenticated".into(),
            message: "x".into(),
        }
        .into();
        assert!(matches!(e, CallError::SignedOut(_)) && !e.is_transient());
        let e: CallError = SessionError::Offline("down".into()).into();
        assert_eq!(e, CallError::Offline("down".into()));
    }

    #[test]
    fn device_name_is_never_empty() {
        assert!(!device_name().is_empty());
    }

    // ── the session epoch (codex review, finding 3) ─────────────────────────────

    use crate::testing::{tokens, FakeServer};

    fn session_for(_server: &FakeServer) -> std::sync::Arc<Session> {
        std::sync::Arc::new(Session::new(ApiClient::loopback()))
    }

    const REFRESH: &str = "/api/v1/auth/refresh";
    const EXCHANGE: &str = "/api/v1/auth/exchange";
    const LOGOUT: &str = "/api/v1/auth/logout";

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refresh_that_finishes_after_the_sign_out_began_stores_nothing() {
        let server = FakeServer::start().await;
        let origin = server.origin.clone();
        crate::secrets::fake::store(&origin, "brr_0").unwrap();
        server.reply(REFRESH, 200, tokens(1));
        server.reply(LOGOUT, 204, serde_json::json!({}));
        server.hold(REFRESH);
        let session = session_for(&server);

        // a native window asks for a token: the refresh is on the wire
        let s = session.clone();
        let o = origin.clone();
        let refresh = tokio::spawn(async move { s.access_token(&o).await });
        server.arrived(REFRESH).await;
        // the user signs out meanwhile
        let s = session.clone();
        let o = origin.clone();
        let sign_out = tokio::spawn(async move { s.sign_out(&o).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        server.release(REFRESH);

        assert!(matches!(refresh.await.unwrap(), Err(SessionError::SignedOut(_))));
        let report = sign_out.await.unwrap();
        assert_eq!(report.remote, Remote::Revoked, "the refreshed token revoked the device");
        assert!(report.keychain_deleted);
        assert_eq!(
            crate::secrets::fake::peek(&origin),
            None,
            "nothing restored in the keychain"
        );
        assert!(!session.has_access_token(&origin).await, "nothing restored in memory");
        assert!(matches!(
            session.access_token(&origin).await,
            Err(SessionError::SignedOut(_))
        ));
        assert_eq!(server.hits(REFRESH).len(), 1, "no new refresh after the sign-out");
        assert_eq!(server.hits(LOGOUT)[0].bearer.as_deref(), Some("bra_1"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sign_in_that_completes_after_the_sign_out_stores_nothing() {
        let server = FakeServer::start().await;
        let origin = server.origin.clone();
        server.reply(EXCHANGE, 200, tokens(7));
        server.reply(LOGOUT, 204, serde_json::json!({}));
        server.hold(EXCHANGE);
        let session = session_for(&server);
        let info = InstanceInfo {
            name: "Fake".into(),
            ..Default::default()
        };
        let url = session.begin_sign_in(&origin, &info);
        let state: String = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.into_owned();

        // the browser came back; the code is being exchanged …
        let s = session.clone();
        let complete = tokio::spawn(async move { s.complete_sign_in("code", &state).await });
        server.arrived(EXCHANGE).await;
        // … and the user signs out before it answered
        let report = session.sign_out(&origin).await;
        assert_eq!(report.remote, Remote::NotConfirmed, "nothing to revoke yet");
        server.release(EXCHANGE);

        assert!(matches!(complete.await.unwrap(), Err(SessionError::Cancelled)));
        assert_eq!(
            crate::secrets::fake::peek(&origin),
            None,
            "the new pair never reached the keychain"
        );
        assert!(!session.has_access_token(&origin).await);
        assert!(!session.has_stored_sign_in(&origin).await);
        // the device the late exchange created is revoked with its own token
        assert_eq!(server.hits(LOGOUT)[0].bearer.as_deref(), Some("bra_7"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_token_is_handed_out_while_the_sign_out_runs_and_a_new_sign_in_works() {
        let server = FakeServer::start().await;
        let origin = server.origin.clone();
        crate::secrets::fake::store(&origin, "brr_0").unwrap();
        server.reply(REFRESH, 200, tokens(1));
        server.reply(LOGOUT, 204, serde_json::json!({}));
        server.hold(LOGOUT);
        let session = session_for(&server);

        let s = session.clone();
        let o = origin.clone();
        let sign_out = tokio::spawn(async move { s.sign_out(&o).await });
        server.arrived(LOGOUT).await;
        // the logout is on the wire: a request now gets no token (and starts no refresh)
        assert!(matches!(
            session.access_token(&origin).await,
            Err(SessionError::SignedOut(_))
        ));
        assert!(matches!(
            session.refresh(&origin).await,
            Err(SessionError::SignedOut(_))
        ));
        assert!(!session.has_stored_sign_in(&origin).await);
        server.release(LOGOUT);
        let report = sign_out.await.unwrap();
        assert_eq!(report.remote, Remote::Revoked);
        assert_eq!(server.hits(REFRESH).len(), 1, "only the sign-out's own refresh");

        // signing in again afterwards stores the new pair
        server.reply(EXCHANGE, 200, tokens(2));
        let info = InstanceInfo {
            name: "Fake".into(),
            ..Default::default()
        };
        let url = session.begin_sign_in(&origin, &info);
        let state: String = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.into_owned();
        assert_eq!(session.complete_sign_in("code", &state).await.unwrap(), origin);
        assert_eq!(crate::secrets::fake::peek(&origin).as_deref(), Some("brr_2"));
        assert_eq!(session.access_token(&origin).await.unwrap(), "bra_2");
    }

    // ── an honest sign-out (codex review, finding 4) ────────────────────────────

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_keychain_deletion_is_retried_reported_and_never_restored() {
        let server = FakeServer::start().await;
        let origin = server.origin.clone();
        server.reply(REFRESH, 200, tokens(1));
        server.reply(LOGOUT, 204, serde_json::json!({}));
        let session = session_for(&server);

        // refused twice, then it works: the sign-out's own retries delete it
        crate::secrets::fake::store(&origin, "brr_0").unwrap();
        crate::secrets::fake::fail_deletes(&origin, 2);
        let report = session.sign_out(&origin).await;
        assert!(report.keychain_deleted);
        assert_eq!(crate::secrets::fake::peek(&origin), None);

        // refused every time: reported, and the leftover entry is never used
        let other = FakeServer::start().await;
        let origin = other.origin.clone();
        other.reply(REFRESH, 200, tokens(1));
        other.reply(LOGOUT, 204, serde_json::json!({}));
        crate::secrets::fake::store(&origin, "brr_0").unwrap();
        crate::secrets::fake::fail_deletes(&origin, 100);
        let report = session.sign_out(&origin).await;
        assert!(!report.keychain_deleted);
        assert_eq!(report.remote, Remote::Revoked);
        assert!(crate::secrets::fake::peek(&origin).is_some(), "still in the keychain");
        assert!(!session.has_stored_sign_in(&origin).await, "but never restored from");
        assert!(matches!(
            session.access_token(&origin).await,
            Err(SessionError::SignedOut(_))
        ));
        // the next start closes it before anything restores, and tries again
        let restarted = Session::new(ApiClient::loopback());
        restarted.close(&origin);
        assert!(!restarted.has_stored_sign_in(&origin).await);
        crate::secrets::fake::fail_deletes(&origin, 0);
        assert!(restarted.delete_stored_sign_in(&origin).await);
        assert_eq!(crate::secrets::fake::peek(&origin), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_that_cannot_be_reached_is_reported_as_not_revoked() {
        let server = FakeServer::start().await;
        let origin = server.origin.clone();
        crate::secrets::fake::store(&origin, "brr_0").unwrap();
        server.reply(REFRESH, 503, serde_json::json!({ "code": "server_error" }));
        let session = session_for(&server);
        let report = session.sign_out(&origin).await;
        assert_eq!(report.remote, Remote::NotConfirmed);
        assert!(report.keychain_deleted, "local deletion does not wait for the server");
        // a dead refresh token: the server already ended the session
        let dead = FakeServer::start().await;
        crate::secrets::fake::store(&dead.origin, "brr_0").unwrap();
        dead.reply(REFRESH, 401, serde_json::json!({ "code": "invalid_grant" }));
        assert_eq!(session.sign_out(&dead.origin).await.remote, Remote::AlreadyEnded);
    }
}
