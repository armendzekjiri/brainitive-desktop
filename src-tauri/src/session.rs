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

use std::collections::HashMap;
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
}

struct Access {
    token: String,
    expires_at: Instant,
}

pub struct Session {
    api: ApiClient,
    pending: std::sync::Mutex<Option<Pending>>,
    tokens: Mutex<HashMap<ServerOrigin, Access>>,
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
            pending: std::sync::Mutex::new(None),
            tokens: Mutex::new(HashMap::new()),
        }
    }

    pub fn api(&self) -> &ApiClient {
        &self.api
    }

    /// Starts a sign-in for `origin` and returns the browser URL. A previous unfinished
    /// sign-in is replaced (its callback will then fail the state check).
    pub fn begin_sign_in(&self, origin: &ServerOrigin, instance: &InstanceInfo) -> Url {
        let pkce = PkcePair::generate();
        let url = authorize_url(origin, instance, &pkce, &device_name());
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            origin: origin.clone(),
            pkce,
            started: Instant::now(),
        });
        url
    }

    pub fn cancel_sign_in(&self) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    pub fn sign_in_pending_for(&self) -> Option<ServerOrigin> {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|p| p.started.elapsed() < PENDING_TTL)
            .map(|p| p.origin.clone())
    }

    /// `brainitive://auth/callback?error=…&state=…`: ends the pending sign-in if the state is ours.
    pub fn fail_sign_in(&self, state: Option<&str>) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let ours = matches!((pending.as_ref(), state), (Some(p), Some(s)) if state_matches(&p.pkce.state, s));
        if ours {
            pending.take();
        }
        ours
    }

    /// `brainitive://auth/callback?code=…&state=…`: checks the state (single use: the pending
    /// sign-in is consumed either way), exchanges the code and stores the tokens.
    pub async fn complete_sign_in(&self, code: &str, state: &str) -> Result<ServerOrigin, SessionError> {
        let pending = {
            let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                None => return Err(SessionError::NoPendingSignIn),
                // a forged or stale link must not cancel the real sign-in in progress
                Some(p) if !state_matches(&p.pkce.state, state) => return Err(SessionError::StateMismatch),
                Some(_) => guard.take().expect("checked above"),
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
        self.store_pair(&pending.origin, pair, &mut tokens).await?;
        Ok(pending.origin)
    }

    /// Whether a refresh token is stored for `origin` (one keychain read).
    pub async fn has_stored_sign_in(&self, origin: &ServerOrigin) -> bool {
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
        if let Some(access) = tokens.get(origin) {
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

    async fn refresh_locked(
        &self,
        origin: &ServerOrigin,
        tokens: &mut HashMap<ServerOrigin, Access>,
    ) -> Result<String, SessionError> {
        tokens.remove(origin);
        let o = origin.clone();
        let stored = tauri::async_runtime::spawn_blocking(move || secrets::load_refresh_token(&o))
            .await
            .map_err(|e| SessionError::Keychain(e.to_string()))??;
        let Some(refresh_token) = stored else {
            return Err(SessionError::SignedOut("Please sign in.".into()));
        };
        match self.api.refresh(origin, &refresh_token).await {
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
        tokens: &mut HashMap<ServerOrigin, Access>,
    ) -> Result<String, SessionError> {
        let o = origin.clone();
        let refresh = pair.refresh_token.clone();
        tauri::async_runtime::spawn_blocking(move || secrets::store_refresh_token(&o, &refresh))
            .await
            .map_err(|e| SessionError::Keychain(e.to_string()))??;
        let ttl = Duration::from_secs(pair.expires_in.clamp(30, 24 * 3600));
        tokens.insert(
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
        self.tokens.lock().await.contains_key(origin)
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

    /// Signs this device out on the server (best effort) and forgets its tokens locally.
    pub async fn sign_out(&self, origin: &ServerOrigin) {
        let token = {
            let mut tokens = self.tokens.lock().await;
            match tokens.remove(origin) {
                Some(a) if a.expires_at > Instant::now() => Some(a.token),
                // still revoke the device on the server: one refresh just to log out
                _ => self.refresh_locked(origin, &mut tokens).await.ok().inspect(|_| {
                    tokens.remove(origin);
                }),
            }
        };
        if let Some(token) = token {
            let _ = tokio::time::timeout(Duration::from_secs(8), self.api.logout(origin, &token)).await;
        }
        let o = origin.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || secrets::delete_refresh_token(&o)).await;
        self.cancel_sign_in_for(origin);
    }

    fn cancel_sign_in_for(&self, origin: &ServerOrigin) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.as_ref().is_some_and(|p| &p.origin == origin) {
            pending.take();
        }
    }
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
}
