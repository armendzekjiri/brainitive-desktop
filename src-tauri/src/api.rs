//! The App API v1 client (`/api/v1`, see `../docs/ai/API.md` in the app repo).
//!
//! Every call is made here, from Rust: the API has no CORS on purpose, and tokens must never be
//! visible to web content. TLS uses the OS trust store (rustls-platform-verifier), so a server
//! signed by a company CA installed on the machine works, and nothing is ever auto-trusted.

use std::time::Duration;

use reqwest::{redirect, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::server::{ServerOrigin, ServerUrlError};
use crate::version::APP_VERSION;

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum ApiError {
    /// Certificate problems: expired, self-signed / unknown issuer, wrong host name.
    #[error("The server's security certificate is not trusted ({0}). The app never skips this check.")]
    Tls(TlsReason),
    #[error("The server could not be reached. Check the address and your connection.")]
    Network(String),
    #[error("The server took too long to answer.")]
    Timeout,
    #[error("This address does not belong to a Brainitive server.")]
    NotBrainitive,
    #[error("This Brainitive server is too old for the desktop app (it needs version 1.49 or later). Ask your administrator to update it.")]
    ServerTooOld,
    #[error("The server redirects to an address the app does not follow (only https, never via http): {0}")]
    BadRedirect(String),
    /// 401 `invalid_grant` / `unauthenticated`: the stored sign-in is no longer valid.
    #[error("{message}")]
    Unauthorized { code: String, message: String },
    /// Any other error response in the `{message, code, errors?}` shape.
    #[error("{message}")]
    Api { status: u16, code: String, message: String },
    #[error("The server sent an unexpected answer.")]
    InvalidResponse,
}

/// Why a certificate was refused (the verifier's wording is mapped, never shown raw).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsReason {
    Expired,
    WrongHost,
    /// Self-signed, or issued by an authority this computer does not trust.
    Untrusted,
    Revoked,
    Other,
}

impl std::fmt::Display for TlsReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Expired => "the certificate has expired",
            Self::WrongHost => "the certificate is for a different address",
            Self::Untrusted => "it is self-signed or issued by an unknown authority",
            Self::Revoked => "the certificate was revoked",
            Self::Other => "the secure connection failed",
        })
    }
}

impl ApiError {
    /// Transient: keep the stored refresh token and offer "Retry" / offline.
    pub fn is_connectivity(&self) -> bool {
        matches!(self, Self::Network(_) | Self::Timeout)
    }
}

/// `GET /api/v1/instance`. Fields beyond `name` are optional so an older or newer server
/// still reads; the app only needs `name`, `version`, `min_versions` and `authorize_path`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub api_version: Option<String>,
    #[serde(default)]
    pub sign_in: Option<Value>,
    #[serde(default)]
    pub sign_up_open: Option<bool>,
    #[serde(default)]
    pub min_versions: Option<MinVersions>,
    #[serde(default)]
    pub authorize_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MinVersions {
    #[serde(default)]
    pub desktop: Option<String>,
    #[serde(default)]
    pub mobile: Option<String>,
}

impl InstanceInfo {
    /// `authorize_path` if it is a plain path, else the contract default.
    pub fn authorize_path(&self) -> &str {
        match self.authorize_path.as_deref() {
            Some(p) if p.starts_with('/') && !p.starts_with("//") && !p.contains(['\\', '?', '#']) => p,
            _ => "/app/authorize",
        }
    }
}

/// Exchange / refresh response (`TokenResource`).
#[derive(Clone, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub expires_in: u64,
    pub refresh_token: String,
    #[serde(default)]
    pub refresh_expires_in: Option<u64>,
    #[serde(default)]
    pub device: Option<DeviceInfo>,
}

impl std::fmt::Debug for TokenPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenPair")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceInfo {
    pub id: Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
}

/// `device` of `POST /api/v1/auth/exchange`.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceDescription {
    pub name: String,
    pub platform: &'static str,
    pub app_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
}

#[derive(Clone)]
pub struct ApiClient {
    /// Token endpoints: no redirects at all (a redirected POST could leak a code or token).
    http: reqwest::Client,
    /// The instance check: follows up to 5 redirects, each to an allowed origin, so
    /// `brainitive.app` → `www.brainitive.app` works and the final origin is shown to the user.
    discovery: reqwest::Client,
}

impl ApiClient {
    pub fn new() -> Self {
        let user_agent = format!("BrainitiveDesktop/{APP_VERSION} ({})", platform());
        let base = || {
            reqwest::Client::builder()
                .user_agent(user_agent.clone())
                .timeout(TIMEOUT)
                .connect_timeout(Duration::from_secs(10))
                .https_only(false) // http is allowed for loopback only, enforced by ServerOrigin
        };
        let http = base().redirect(redirect::Policy::none()).build().expect("http client");
        let discovery = base()
            .redirect(redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if ServerOrigin::from_url(attempt.url()).is_err() {
                    attempt.error("redirect to a disallowed address")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .expect("discovery client");
        Self { http, discovery }
    }

    /// `GET {origin}/api/v1/instance`. Returns the info and the origin that answered (after
    /// redirects). A 404 is told apart: an older Brainitive (it serves `/site.webmanifest`
    /// with our theme colour) vs. not Brainitive at all.
    pub async fn instance(&self, origin: &ServerOrigin) -> Result<(InstanceInfo, ServerOrigin), ApiError> {
        let response = self
            .discovery
            .get(origin.join("/api/v1/instance"))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(map_transport)?;

        let final_origin =
            ServerOrigin::from_url(response.url()).map_err(|e: ServerUrlError| ApiError::BadRedirect(e.to_string()))?;

        match response.status() {
            StatusCode::OK => {}
            StatusCode::NOT_FOUND => {
                return Err(if self.looks_like_brainitive(&final_origin).await {
                    ApiError::ServerTooOld
                } else {
                    ApiError::NotBrainitive
                });
            }
            status if status.is_server_error() => return Err(read_error(response).await),
            _ => return Err(ApiError::NotBrainitive),
        }

        let body: Value = response.json().await.map_err(|_| ApiError::NotBrainitive)?;
        let info = parse_instance(body).ok_or(ApiError::NotBrainitive)?;
        Ok((info, final_origin))
    }

    async fn looks_like_brainitive(&self, origin: &ServerOrigin) -> bool {
        let Ok(response) = self.discovery.get(origin.join("/site.webmanifest")).send().await else {
            return false;
        };
        if !response.status().is_success() {
            return false;
        }
        match response.json::<Value>().await {
            Ok(manifest) => manifest.get("theme_color").and_then(Value::as_str) == Some("#0E83DC"),
            Err(_) => false,
        }
    }

    pub async fn exchange(
        &self,
        origin: &ServerOrigin,
        code: &str,
        verifier: &str,
        device: &DeviceDescription,
    ) -> Result<TokenPair, ApiError> {
        let body = serde_json::json!({ "code": code, "code_verifier": verifier, "device": device });
        self.post_json(origin, "/api/v1/auth/exchange", None, &body)
            .await
            .and_then(parse_tokens)
    }

    pub async fn refresh(&self, origin: &ServerOrigin, refresh_token: &str) -> Result<TokenPair, ApiError> {
        let body = serde_json::json!({ "refresh_token": refresh_token });
        self.post_json(origin, "/api/v1/auth/refresh", None, &body)
            .await
            .and_then(parse_tokens)
    }

    pub async fn logout(&self, origin: &ServerOrigin, access_token: &str) -> Result<(), ApiError> {
        self.post_json(
            origin,
            "/api/v1/auth/logout",
            Some(access_token),
            &serde_json::json!({}),
        )
        .await
        .map(|_| ())
    }

    /// `POST /api/v1/auth/web-session` → the one-time URL that signs the webview in. Only a URL
    /// on the same origin is accepted.
    pub async fn web_session(&self, origin: &ServerOrigin, access_token: &str) -> Result<Url, ApiError> {
        let body = self
            .post_json(
                origin,
                "/api/v1/auth/web-session",
                Some(access_token),
                &serde_json::json!({}),
            )
            .await?;
        let raw = unwrap_data(body)
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(ApiError::InvalidResponse)?;
        crate::links::resolve_on_instance(origin, &raw).ok_or(ApiError::InvalidResponse)
    }

    async fn post_json(
        &self,
        origin: &ServerOrigin,
        path: &str,
        bearer: Option<&str>,
        body: &Value,
    ) -> Result<Value, ApiError> {
        let mut request = self
            .http
            .post(origin.join(path))
            .header("Accept", "application/json")
            .json(body);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(map_transport)?;
        let status = response.status();
        if status == StatusCode::NO_CONTENT {
            return Ok(Value::Null);
        }
        if !status.is_success() {
            return Err(read_error(response).await);
        }
        response.json::<Value>().await.map_err(|_| ApiError::InvalidResponse)
    }
}

impl ApiClient {
    /// An authenticated App API call (`Bearer bra_…`) of the native windows: tray, quick
    /// capture, quick switcher. Redirects are never followed (a redirect would carry the token
    /// elsewhere). `304 Not Modified` (after `If-None-Match`) comes back as `status: 304`.
    pub async fn request(
        &self,
        origin: &ServerOrigin,
        request: &ApiRequest,
        access_token: &str,
    ) -> Result<ApiResponse, ApiError> {
        let url = request.url(origin).ok_or(ApiError::InvalidResponse)?;
        let mut builder = self
            .http
            .request(request.method.clone(), url)
            .header("Accept", "application/json")
            .bearer_auth(access_token);
        if let Some(etag) = &request.etag {
            builder = builder.header("If-None-Match", etag);
        }
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        let response = builder.send().await.map_err(map_transport)?;
        let status = response.status();
        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() <= 200)
            .map(str::to_string);
        if status == StatusCode::NOT_MODIFIED {
            return Ok(ApiResponse {
                status: 304,
                etag: etag.or_else(|| request.etag.clone()),
                body: Value::Null,
            });
        }
        if status == StatusCode::NO_CONTENT {
            return Ok(ApiResponse {
                status: 204,
                etag,
                body: Value::Null,
            });
        }
        if status.is_redirection() {
            return Err(ApiError::BadRedirect(status.to_string()));
        }
        if !status.is_success() {
            return Err(read_error(response).await);
        }
        let body = response.json::<Value>().await.map_err(|_| ApiError::InvalidResponse)?;
        Ok(ApiResponse {
            status: status.as_u16(),
            etag,
            body: unwrap_data(body),
        })
    }
}

/// One App API call: method, a path under `/api/v1/`, query, JSON body, a known ETag.
#[derive(Debug, Clone)]
pub struct ApiRequest {
    pub method: Method,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
    pub etag: Option<String>,
}

impl ApiRequest {
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Vec::new(),
            body: None,
            etag: None,
        }
    }
    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }
    pub fn post(path: impl Into<String>, body: Value) -> Self {
        Self::new(Method::POST, path).body(body)
    }
    pub fn patch(path: impl Into<String>, body: Value) -> Self {
        Self::new(Method::PATCH, path).body(body)
    }
    pub fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
    pub fn query(mut self, key: &str, value: impl Into<String>) -> Self {
        self.query.push((key.to_string(), value.into()));
        self
    }
    pub fn etag(mut self, etag: Option<String>) -> Self {
        self.etag = etag;
        self
    }

    /// The absolute URL, or `None` when the path is not a plain `/api/v1/…` path (so a value
    /// from a server answer can never point the token somewhere else).
    pub fn url(&self, origin: &ServerOrigin) -> Option<Url> {
        let path = &self.path;
        if !path.starts_with("/api/v1/")
            || path.contains(['?', '#', '\\'])
            || path.split('/').any(|segment| segment == ".." || segment == ".")
            || path.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return None;
        }
        let mut url = origin.join(path);
        if !self.query.is_empty() {
            url.query_pairs_mut().extend_pairs(self.query.iter());
        }
        origin.contains(&url).then_some(url)
    }
}

/// What came back: the status, the ETag (if any) and the body without its `data` wrapper.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub body: Value,
}

impl ApiResponse {
    pub fn not_modified(&self) -> bool {
        self.status == 304
    }
}

impl Default for ApiClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Resources may come wrapped in `{"data": …}` (Laravel's default) or bare (`$wrap = null`).
fn unwrap_data(body: Value) -> Value {
    match body {
        Value::Object(mut map) if map.len() == 1 && map.contains_key("data") => {
            map.remove("data").unwrap_or(Value::Null)
        }
        other => other,
    }
}

pub(crate) fn parse_instance(body: Value) -> Option<InstanceInfo> {
    let body = unwrap_data(body);
    // our marker: an instance answer always carries min_versions or authorize_path
    if body.get("min_versions").is_none() && body.get("authorize_path").is_none() {
        return None;
    }
    let info: InstanceInfo = serde_json::from_value(body).ok()?;
    if info.name.trim().is_empty() {
        return None;
    }
    Some(info)
}

fn parse_tokens(body: Value) -> Result<TokenPair, ApiError> {
    let pair: TokenPair = serde_json::from_value(unwrap_data(body)).map_err(|_| ApiError::InvalidResponse)?;
    if !pair.access_token.starts_with("bra_") || !pair.refresh_token.starts_with("brr_") {
        return Err(ApiError::InvalidResponse);
    }
    Ok(pair)
}

async fn read_error(response: reqwest::Response) -> ApiError {
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    error_from_body(status.as_u16(), &body)
}

pub(crate) fn error_from_body(status: u16, body: &Value) -> ApiError {
    let code = body.get("code").and_then(Value::as_str).unwrap_or("").to_string();
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .map(|m| m.chars().take(300).collect::<String>())
        .unwrap_or_else(|| format!("The server answered with an error ({status})."));
    if status == 401 {
        ApiError::Unauthorized { code, message }
    } else {
        ApiError::Api { status, code, message }
    }
}

fn map_transport(error: reqwest::Error) -> ApiError {
    if error.is_timeout() {
        return ApiError::Timeout;
    }
    if let Some(tls) = tls_problem(&error) {
        return ApiError::Tls(tls);
    }
    if error.is_redirect() {
        return ApiError::BadRedirect(
            error
                .url()
                .map(|u| u.origin().ascii_serialization())
                .unwrap_or_default(),
        );
    }
    ApiError::Network(error_chain(&error))
}

fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

/// A reason when the transport error is a certificate problem.
fn tls_problem(error: &reqwest::Error) -> Option<TlsReason> {
    classify_tls(&error_chain(error))
}

pub(crate) fn classify_tls(chain: &str) -> Option<TlsReason> {
    let lower = chain.to_ascii_lowercase();
    if !(lower.contains("certificate") || lower.contains("tls") || lower.contains("handshake")) {
        return None;
    }
    Some(if lower.contains("expired") {
        TlsReason::Expired
    } else if lower.contains("notvalidforname") || lower.contains("not valid for") || lower.contains("hostname") {
        TlsReason::WrongHost
    } else if lower.contains("unknownissuer")
        || lower.contains("unknown issuer")
        || lower.contains("self-signed")
        || lower.contains("self signed")
        || lower.contains("untrusted")
    {
        TlsReason::Untrusted
    } else if lower.contains("revoked") {
        TlsReason::Revoked
    } else {
        TlsReason::Other
    })
}

/// `device.platform` of the exchange request.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_wrapped_and_bare_instance_answers() {
        let bare = json!({"name": "Acme", "version": "1.49.0", "min_versions": {"desktop": "0.1.0"}, "authorize_path": "/app/authorize"});
        let info = parse_instance(bare.clone()).unwrap();
        assert_eq!(info.name, "Acme");
        assert_eq!(info.min_versions.unwrap().desktop.as_deref(), Some("0.1.0"));
        assert_eq!(parse_instance(json!({"data": bare})).unwrap().name, "Acme");
    }

    #[test]
    fn rejects_answers_that_are_not_an_instance() {
        assert!(parse_instance(json!({"name": "Some other app"})).is_none());
        assert!(parse_instance(json!({"min_versions": {}, "name": "  "})).is_none());
        assert!(parse_instance(json!([1, 2])).is_none());
    }

    #[test]
    fn authorize_path_falls_back_to_the_contract_default() {
        let mut info = InstanceInfo {
            name: "x".into(),
            ..Default::default()
        };
        assert_eq!(info.authorize_path(), "/app/authorize");
        info.authorize_path = Some("/custom/authorize".into());
        assert_eq!(info.authorize_path(), "/custom/authorize");
        for bad in ["//evil.com/x", "https://evil.com", "/a?b", "/a\\b", ""] {
            info.authorize_path = Some(bad.into());
            assert_eq!(info.authorize_path(), "/app/authorize", "{bad}");
        }
    }

    #[test]
    fn token_pairs_need_the_app_prefixes() {
        let ok = json!({"token_type": "Bearer", "access_token": "bra_x", "expires_in": 900, "refresh_token": "brr_y", "refresh_expires_in": 5184000, "device": {"id": 12, "name": "Mac", "platform": "macos"}});
        let pair = parse_tokens(ok).unwrap();
        assert_eq!(pair.expires_in, 900);
        assert!(!format!("{pair:?}").contains("brr_y"));
        assert!(parse_tokens(json!({"access_token": "eyJ...", "expires_in": 900, "refresh_token": "brr_y"})).is_err());
    }

    #[test]
    fn error_shape() {
        let e = error_from_body(
            401,
            &json!({"message": "The refresh token is invalid.", "code": "invalid_grant"}),
        );
        assert_eq!(
            e,
            ApiError::Unauthorized {
                code: "invalid_grant".into(),
                message: "The refresh token is invalid.".into()
            }
        );
        let e = error_from_body(
            422,
            &json!({"message": "Bad", "code": "validation_failed", "errors": {"code": ["x"]}}),
        );
        assert!(matches!(e, ApiError::Api { status: 422, ref code, .. } if code == "validation_failed"));
        let e = error_from_body(500, &Value::Null);
        assert!(matches!(e, ApiError::Api { status: 500, .. }));
    }

    #[test]
    fn requests_stay_on_the_api_of_the_origin() {
        let origin = ServerOrigin::parse("brainitive.example.com").unwrap();
        let url = ApiRequest::get("/api/v1/search")
            .query("q", "a b&c")
            .query("limit", "12")
            .url(&origin)
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://brainitive.example.com/api/v1/search?q=a+b%26c&limit=12"
        );
        assert_eq!(
            ApiRequest::get("/api/v1/me/my-day/items/issue:12")
                .url(&origin)
                .unwrap()
                .path(),
            "/api/v1/me/my-day/items/issue:12"
        );
        for bad in [
            "/api/v2/x",
            "/web/x",
            "//evil.com/api/v1/x",
            "https://evil.com/api/v1/x",
            "/api/v1/../../x",
            "/api/v1/a?b=c",
            "/api/v1/a b",
            "/api/v1/a\\b",
        ] {
            assert!(ApiRequest::get(bad).url(&origin).is_none(), "{bad}");
        }
    }

    #[test]
    fn tls_errors_are_recognised() {
        assert_eq!(
            classify_tls("error sending request: invalid peer certificate: UnknownIssuer"),
            Some(TlsReason::Untrusted)
        );
        assert_eq!(
            classify_tls("invalid peer certificate: Expired"),
            Some(TlsReason::Expired)
        );
        assert_eq!(
            classify_tls("invalid peer certificate: NotValidForName"),
            Some(TlsReason::WrongHost)
        );
        assert_eq!(
            classify_tls("invalid peer certificate: Revoked"),
            Some(TlsReason::Revoked)
        );
        assert_eq!(classify_tls("tls handshake eof"), Some(TlsReason::Other));
        assert_eq!(classify_tls("tcp connect error: Connection refused"), None);
    }
}
