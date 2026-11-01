//! Which servers the app may talk to, and how a typed address becomes a server origin.
//!
//! Pure logic (no Tauri), unit tested below. A server is identified by its **origin**
//! (`scheme://host[:port]`): the keychain entry, the webview data store, the remote IPC
//! capability and the navigation guard are all keyed by it.

use std::fmt;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

/// Brainitive Cloud (the hosted app). The marketing site `brainitive.app` redirects here.
pub const CLOUD_ORIGIN: &str = "https://www.brainitive.app";
/// The pre-release environment (`pre-release` branch).
pub const DEV_ORIGIN: &str = "https://dev.brainitive.app";
/// `composer run dev` / `php artisan serve` on this machine.
pub const LOCAL_ORIGIN: &str = "http://localhost:8000";

/// A normalized server origin, e.g. `https://www.brainitive.app` or `http://localhost:8000`.
/// Only constructed through [`ServerOrigin::parse`], so it always passes the TLS rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ServerOrigin(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServerUrlError {
    #[error("Enter the address of your Brainitive server.")]
    Empty,
    #[error("That doesn't look like a web address.")]
    Invalid,
    #[error("Only https:// addresses are supported (not {0}://).")]
    UnsupportedScheme(String),
    #[error("Unencrypted http:// is only allowed for localhost. Use https://.")]
    InsecureHttp,
    #[error("Leave the user name and password out of the address.")]
    Credentials,
}

impl ServerOrigin {
    /// Normalizes what a user typed into the "Self-hosted server" field:
    /// - surrounding whitespace is trimmed; no scheme → `https://` (`http://` for localhost);
    /// - path, query, fragment and trailing slashes are dropped; host lowercased (IDN → punycode);
    ///   the default port is dropped;
    /// - only `https`, or `http` for `localhost` / `127.0.0.1` / `[::1]` (local development);
    /// - no `user:password@`.
    pub fn parse(input: &str) -> Result<Self, ServerUrlError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ServerUrlError::Empty);
        }

        let candidate = if trimmed.contains("://") {
            trimmed.to_string()
        } else {
            // No scheme: https, except for a local dev server where TLS is unusual.
            let host_part = trimmed.split(['/', '?', '#']).next().unwrap_or("");
            if is_loopback_host_str(host_part) {
                format!("http://{trimmed}")
            } else {
                format!("https://{trimmed}")
            }
        };

        let url = Url::parse(&candidate).map_err(|_| ServerUrlError::Invalid)?;
        Self::from_url(&url)
    }

    /// The origin of an already parsed URL, with the same rules as [`Self::parse`].
    pub fn from_url(url: &Url) -> Result<Self, ServerUrlError> {
        match url.scheme() {
            "https" => {}
            "http" => {
                if !is_loopback(url) {
                    return Err(ServerUrlError::InsecureHttp);
                }
            }
            other => return Err(ServerUrlError::UnsupportedScheme(other.to_string())),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ServerUrlError::Credentials);
        }
        let host = url.host_str().unwrap_or("");
        if host.is_empty() {
            return Err(ServerUrlError::Invalid);
        }
        // a bare word like "brainitive" is almost always a typo, not an intranet host
        if matches!(url.host(), Some(Host::Domain(d)) if !d.contains('.') && d != "localhost") {
            return Err(ServerUrlError::Invalid);
        }
        let origin = url.origin();
        if !origin.is_tuple() {
            return Err(ServerUrlError::Invalid);
        }
        Ok(Self(origin.ascii_serialization()))
    }

    pub fn cloud() -> Self {
        Self(CLOUD_ORIGIN.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn url(&self) -> Url {
        // safe: constructed from a valid origin serialization
        Url::parse(&self.0).expect("valid origin")
    }

    pub fn is_cloud(&self) -> bool {
        self.0 == CLOUD_ORIGIN
    }

    /// Short name for the window title / menu: "Cloud", or the host (with a non-default port).
    pub fn display_name(&self) -> String {
        if self.is_cloud() {
            return "Cloud".to_string();
        }
        let url = self.url();
        let host = url.host_str().unwrap_or_default().to_string();
        match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        }
    }

    /// Whether `url` is on this server (same scheme, host and port).
    pub fn contains(&self, url: &Url) -> bool {
        url.origin().ascii_serialization() == self.0
    }

    /// An absolute URL on this server for a path like `/dashboard` or `/api/v1/instance`.
    pub fn join(&self, path: &str) -> Url {
        let mut url = self.url();
        url.set_path(path);
        url
    }

    /// The URLPattern a Tauri `remote` capability uses for this origin.
    pub fn capability_pattern(&self) -> String {
        format!("{}/*", self.0)
    }

    /// Stable 16 bytes derived from the origin: the per-server webview data store id
    /// (macOS 14+) and data directory name (Windows / Linux).
    pub fn store_id(&self) -> [u8; 16] {
        store_id_for(&format!("server:{}", self.0))
    }
}

/// The store id of the neutral store the connect screen uses before a server is chosen.
pub fn local_store_id() -> [u8; 16] {
    store_id_for("local")
}

fn store_id_for(key: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("brainitive-desktop:{key}").as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    // make it a valid RFC 4122 v4-style UUID (WKWebsiteDataStore takes an NSUUID)
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    id
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl fmt::Display for ServerOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ServerOrigin {
    type Error = ServerUrlError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ServerOrigin> for String {
    fn from(value: ServerOrigin) -> Self {
        value.0
    }
}

/// `localhost`, `127.0.0.1` or `[::1]` — the only hosts allowed over plain http.
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
        Some(Host::Ipv6(ip)) => ip == std::net::Ipv6Addr::LOCALHOST,
        None => false,
    }
}

fn is_loopback_host_str(host_and_port: &str) -> bool {
    let lower = host_and_port.to_ascii_lowercase();
    let host = if lower.starts_with('[') {
        lower.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        lower.split(':').next().unwrap_or("").to_string()
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> String {
        ServerOrigin::parse(input)
            .unwrap_or_else(|e| panic!("{input}: {e}"))
            .to_string()
    }

    #[test]
    fn adds_https_when_the_scheme_is_missing() {
        assert_eq!(ok("brainitive.example.com"), "https://brainitive.example.com");
        assert_eq!(ok("  brainitive.example.com  "), "https://brainitive.example.com");
    }

    #[test]
    fn drops_paths_queries_fragments_and_trailing_slashes() {
        assert_eq!(ok("https://brainitive.example.com/"), "https://brainitive.example.com");
        assert_eq!(ok("brainitive.example.com///"), "https://brainitive.example.com");
        assert_eq!(
            ok("https://brainitive.example.com/workspaces/x/projects?tab=1#top"),
            "https://brainitive.example.com"
        );
    }

    #[test]
    fn lowercases_the_host_and_drops_the_default_port() {
        assert_eq!(
            ok("HTTPS://Brainitive.Example.COM:443/"),
            "https://brainitive.example.com"
        );
        assert_eq!(ok("https://example.com:8443"), "https://example.com:8443");
    }

    #[test]
    fn converts_international_domains_to_punycode() {
        assert_eq!(ok("bücher.example"), "https://xn--bcher-kva.example");
    }

    #[test]
    fn rejects_http_except_for_localhost() {
        assert_eq!(
            ServerOrigin::parse("http://brainitive.example.com"),
            Err(ServerUrlError::InsecureHttp)
        );
        assert_eq!(
            ServerOrigin::parse("http://localhost.evil.com"),
            Err(ServerUrlError::InsecureHttp)
        );
        assert_eq!(
            ServerOrigin::parse("http://192.168.1.10:8000"),
            Err(ServerUrlError::InsecureHttp)
        );
        assert_eq!(ok("http://localhost:8000/"), "http://localhost:8000");
        assert_eq!(ok("http://127.0.0.1:8000"), "http://127.0.0.1:8000");
        assert_eq!(ok("http://[::1]:8000"), "http://[::1]:8000");
    }

    #[test]
    fn uses_http_for_a_localhost_address_without_scheme() {
        assert_eq!(ok("localhost:8000"), "http://localhost:8000");
        assert_eq!(ok("127.0.0.1:8000/dashboard"), "http://127.0.0.1:8000");
        assert_eq!(ok("LOCALHOST"), "http://localhost");
    }

    #[test]
    fn rejects_other_schemes_credentials_and_garbage() {
        assert_eq!(ServerOrigin::parse(""), Err(ServerUrlError::Empty));
        assert_eq!(ServerOrigin::parse("   "), Err(ServerUrlError::Empty));
        assert_eq!(
            ServerOrigin::parse("ftp://example.com"),
            Err(ServerUrlError::UnsupportedScheme("ftp".into()))
        );
        assert_eq!(
            ServerOrigin::parse("file:///etc/passwd"),
            Err(ServerUrlError::UnsupportedScheme("file".into()))
        );
        assert_eq!(
            ServerOrigin::parse("https://user:pw@example.com"),
            Err(ServerUrlError::Credentials)
        );
        assert_eq!(ServerOrigin::parse("https://"), Err(ServerUrlError::Invalid));
        assert_eq!(ServerOrigin::parse("javascript:alert(1)"), Err(ServerUrlError::Invalid));
        assert_eq!(ServerOrigin::parse("brainitive"), Err(ServerUrlError::Invalid));
        assert_eq!(ServerOrigin::parse("exa mple.com"), Err(ServerUrlError::Invalid));
    }

    #[test]
    fn the_known_environments_are_valid_origins() {
        assert_eq!(ok(CLOUD_ORIGIN), CLOUD_ORIGIN);
        assert_eq!(ok(DEV_ORIGIN), DEV_ORIGIN);
        assert_eq!(ok(LOCAL_ORIGIN), LOCAL_ORIGIN);
        assert!(ServerOrigin::cloud().is_cloud());
    }

    #[test]
    fn display_names() {
        assert_eq!(ServerOrigin::cloud().display_name(), "Cloud");
        assert_eq!(
            ServerOrigin::parse("brainitive.example.com").unwrap().display_name(),
            "brainitive.example.com"
        );
        assert_eq!(
            ServerOrigin::parse("localhost:8000").unwrap().display_name(),
            "localhost:8000"
        );
    }

    #[test]
    fn contains_only_matches_the_exact_origin() {
        let origin = ServerOrigin::parse("brainitive.example.com").unwrap();
        assert!(origin.contains(&Url::parse("https://brainitive.example.com/dashboard").unwrap()));
        assert!(!origin.contains(&Url::parse("http://brainitive.example.com/dashboard").unwrap()));
        assert!(!origin.contains(&Url::parse("https://brainitive.example.com:8443/").unwrap()));
        assert!(!origin.contains(&Url::parse("https://evil.brainitive.example.com/").unwrap()));
        assert!(!origin.contains(&Url::parse("https://brainitive.example.com.evil.com/").unwrap()));
    }

    #[test]
    fn store_ids_are_stable_and_distinct_per_origin() {
        let a = ServerOrigin::cloud().store_id();
        let b = ServerOrigin::parse(DEV_ORIGIN).unwrap().store_id();
        assert_eq!(a, ServerOrigin::cloud().store_id());
        assert_ne!(a, b);
        assert_ne!(a, local_store_id());
        assert_eq!(a[6] >> 4, 4, "uuid v4 version nibble");
        assert_eq!(hex(&a).len(), 32);
    }

    #[test]
    fn deserializing_validates() {
        let parsed: Result<ServerOrigin, _> = serde_json::from_str("\"http://example.com\"");
        assert!(parsed.is_err());
        let parsed: ServerOrigin = serde_json::from_str("\"https://example.com/\"").unwrap();
        assert_eq!(parsed.as_str(), "https://example.com");
    }

    #[test]
    fn capability_pattern_covers_the_whole_origin() {
        assert_eq!(
            ServerOrigin::cloud().capability_pattern(),
            "https://www.brainitive.app/*"
        );
    }
}
