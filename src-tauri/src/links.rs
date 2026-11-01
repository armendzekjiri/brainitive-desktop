//! What the main webview may load, what opens in the system browser, and how
//! `brainitive://` deep links are read. Pure logic, unit tested below; the Tauri wiring is in
//! `shell.rs`.

use url::Url;

use crate::server::ServerOrigin;

/// Longest URL accepted from remote content or a deep link.
const MAX_URL_LEN: usize = 4096;

/// A `brainitive://` link the app understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLink {
    /// `brainitive://auth/callback?code=…&state=…` — the end of the browser sign-in.
    AuthCallback { code: String, state: String },
    /// `brainitive://auth/callback?error=access_denied&state=…` — the user pressed Cancel.
    AuthError { error: String, state: Option<String> },
    /// `brainitive://open?url=/workspaces/…` — open a page of the current server.
    /// `target` is still unvalidated: resolve it with [`resolve_on_instance`].
    Open { target: String },
}

pub fn parse_deep_link(url: &Url) -> Option<DeepLink> {
    if url.scheme() != "brainitive" || url.as_str().len() > MAX_URL_LEN {
        return None;
    }
    let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    // brainitive://auth/callback → host "auth", path "/callback"
    let route = format!("{}{}", url.host_str().unwrap_or(""), url.path());
    match route.trim_end_matches('/') {
        "auth/callback" => {
            let state = param("state").filter(|s| is_token(s, 256));
            if let Some(error) = param("error") {
                let error = if is_token(&error, 64) {
                    error
                } else {
                    "invalid_request".into()
                };
                return Some(DeepLink::AuthError { error, state });
            }
            let code = param("code").filter(|c| is_token(c, 256))?;
            Some(DeepLink::AuthCallback { code, state: state? })
        }
        "open" => param("url").map(|target| DeepLink::Open { target }),
        _ => None,
    }
}

fn is_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}

/// A page on the current server, from a deep link or a notification: either a path
/// (`/workspaces/x/…`) or an absolute URL on exactly this origin. Anything else → `None`.
pub fn resolve_on_instance(instance: &ServerOrigin, target: &str) -> Option<Url> {
    let target = target.trim();
    if target.is_empty() || target.len() > MAX_URL_LEN || target.chars().any(|c| c.is_control() || c == '\\') {
        return None;
    }
    let url = if target.starts_with('/') {
        // "//evil.com" is protocol-relative → another host
        if target.starts_with("//") {
            return None;
        }
        instance.url().join(target).ok()?
    } else {
        Url::parse(target).ok()?
    };
    if !instance.contains(&url) || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url)
}

/// A link the app hands to the system browser / mail client: `http`, `https` or `mailto` only.
pub fn external_url(raw: &str) -> Option<Url> {
    if raw.len() > MAX_URL_LEN {
        return None;
    }
    let url = Url::parse(raw.trim()).ok()?;
    match url.scheme() {
        "http" | "https" if url.host_str().is_some_and(|h| !h.is_empty()) => Some(url),
        "mailto" => Some(url),
        _ => None,
    }
}

/// What the navigation handler of the main webview does with a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Navigation {
    /// The bundled screens, the current server, or frame plumbing (`about:blank`, `data:`, `blob:`).
    Allow,
    /// The current server's login page: its web session is gone. Sign in again from the stored
    /// refresh token (or show the sign-in screen) instead of a password form in the webview.
    Reauth,
    /// The current server's `/` (the logo link, and where the web app sends you after "Log out").
    Home,
    /// Another http(s) site. On macOS / Linux the handler also sees iframes (YouTube, Figma
    /// embeds), so these are allowed there and a top-level load is caught on page load
    /// ([`is_app_page`]); Windows only reports top-level navigations and opens them outside.
    Foreign(Url),
    /// `mailto:` → the mail client.
    External(Url),
    /// `brainitive://` → handled like a deep link.
    DeepLink(Url),
    /// Everything else (`file:`, `javascript:`, unknown schemes).
    Block,
}

/// `local_origins`: where the bundled screens are served (`tauri://localhost`,
/// `http://tauri.localhost`, the Vite dev server).
pub fn decide_navigation(url: &Url, instance: Option<&ServerOrigin>, local_origins: &[String]) -> Navigation {
    match url.scheme() {
        "about" | "data" | "blob" => return Navigation::Allow,
        "brainitive" => return Navigation::DeepLink(url.clone()),
        "mailto" => return Navigation::External(url.clone()),
        "http" | "https" | "tauri" => {}
        _ => return Navigation::Block,
    }
    if is_local(url, local_origins) {
        return Navigation::Allow;
    }
    if instance.is_some_and(|i| i.contains(url)) {
        let path = url.path().trim_end_matches('/');
        return if path.is_empty() {
            Navigation::Home
        } else if path == "/login" || path.starts_with("/login/") {
            Navigation::Reauth
        } else {
            Navigation::Allow
        };
    }
    if url.scheme() == "tauri" {
        return Navigation::Block;
    }
    Navigation::Foreign(url.clone())
}

/// The bundled windows (quick capture, quick switcher, settings) only ever show the bundled
/// pages: http(s) / mailto links open outside (`External`), everything else is blocked.
pub fn decide_local_navigation(url: &Url, local_origins: &[String]) -> Navigation {
    if url.as_str() == "about:blank" || is_local(url, local_origins) {
        return Navigation::Allow;
    }
    match url.scheme() {
        "http" | "https" | "mailto" if external_url(url.as_str()).is_some() => Navigation::External(url.clone()),
        _ => Navigation::Block,
    }
}

/// Whether a command comes from the bundled page of the window it belongs to: the window label
/// and a bundled origin (defence in depth next to the capability files).
pub fn is_local_caller(label: &str, url: &Url, expected_label: &str, local_origins: &[String]) -> bool {
    label == expected_label && is_local(url, local_origins)
}

/// Whether the main frame may show this page: the bundled screens or the current server.
pub fn is_app_page(url: &Url, instance: Option<&ServerOrigin>, local_origins: &[String]) -> bool {
    url.as_str() == "about:blank" || is_local(url, local_origins) || instance.is_some_and(|i| i.contains(url))
}

fn is_local(url: &Url, local_origins: &[String]) -> bool {
    let origin = origin_key(url);
    local_origins.contains(&origin)
}

/// `scheme://host[:port]`, also for `tauri://localhost` (the `url` crate gives non-special
/// schemes an opaque origin that never equals anything).
pub fn origin_key(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
        (Some(host), None) => format!("{}://{host}", url.scheme()),
        _ => format!("{}:", url.scheme()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn instance() -> ServerOrigin {
        ServerOrigin::parse("https://brainitive.example.com").unwrap()
    }

    fn locals() -> Vec<String> {
        vec!["tauri://localhost".into(), "http://tauri.localhost".into()]
    }

    #[test]
    fn bundled_windows_stay_on_the_bundled_pages() {
        let l = locals();
        assert_eq!(
            decide_local_navigation(&u("tauri://localhost/capture.html"), &l),
            Navigation::Allow
        );
        assert_eq!(
            decide_local_navigation(&u("https://brainitive.example.com/x"), &l),
            Navigation::External(u("https://brainitive.example.com/x"))
        );
        assert_eq!(decide_local_navigation(&u("file:///etc/passwd"), &l), Navigation::Block);
        assert_eq!(decide_local_navigation(&u("javascript:alert(1)"), &l), Navigation::Block);
        assert_eq!(decide_local_navigation(&u("brainitive://open?url=/x"), &l), Navigation::Block);
    }

    #[test]
    fn local_callers_need_their_own_window_and_a_bundled_origin() {
        let l = locals();
        let page = u("tauri://localhost/capture.html");
        assert!(is_local_caller("capture", &page, "capture", &l));
        assert!(!is_local_caller("main", &page, "capture", &l));
        assert!(!is_local_caller("switcher", &page, "capture", &l));
        assert!(!is_local_caller(
            "capture",
            &u("https://brainitive.example.com/capture.html"),
            "capture",
            &l
        ));
    }

    #[test]
    fn a_file_dropped_on_the_page_never_replaces_it() {
        // dropping a file outside a drop zone makes the webview navigate to file://…
        assert_eq!(
            decide_navigation(&u("file:///Users/me/report.pdf"), Some(&instance()), &locals()),
            Navigation::Block
        );
    }

    #[test]
    fn auth_callback() {
        assert_eq!(
            parse_deep_link(&u("brainitive://auth/callback?code=abc123&state=xyz-_9")),
            Some(DeepLink::AuthCallback {
                code: "abc123".into(),
                state: "xyz-_9".into()
            })
        );
        assert_eq!(
            parse_deep_link(&u("brainitive://auth/callback/?error=access_denied&state=s1")),
            Some(DeepLink::AuthError {
                error: "access_denied".into(),
                state: Some("s1".into())
            })
        );
        // a code without state is never accepted
        assert_eq!(parse_deep_link(&u("brainitive://auth/callback?code=abc")), None);
        // garbage in the code
        assert_eq!(
            parse_deep_link(&u("brainitive://auth/callback?code=a%22b&state=s")),
            None
        );
        // error text is not reflected verbatim
        assert_eq!(
            parse_deep_link(&u("brainitive://auth/callback?error=%3Cscript%3E&state=s")),
            Some(DeepLink::AuthError {
                error: "invalid_request".into(),
                state: Some("s".into())
            })
        );
    }

    #[test]
    fn open_links_and_unknown_routes() {
        assert_eq!(
            parse_deep_link(&u("brainitive://open?url=%2Fworkspaces%2Fa%2Fprojects%2Fb")),
            Some(DeepLink::Open {
                target: "/workspaces/a/projects/b".into()
            })
        );
        assert_eq!(parse_deep_link(&u("brainitive://open")), None);
        assert_eq!(parse_deep_link(&u("brainitive://settings")), None);
        assert_eq!(parse_deep_link(&u("https://www.brainitive.app/open?url=/x")), None);
    }

    #[test]
    fn resolves_only_pages_of_the_instance() {
        let i = instance();
        assert_eq!(
            resolve_on_instance(&i, "/workspaces/a?x=1#c").unwrap().as_str(),
            "https://brainitive.example.com/workspaces/a?x=1#c"
        );
        assert!(resolve_on_instance(&i, "https://brainitive.example.com/dashboard").is_some());
        assert!(resolve_on_instance(&i, "//evil.com/x").is_none());
        assert!(resolve_on_instance(&i, "/\\evil.com").is_none());
        assert!(resolve_on_instance(&i, "https://evil.com/dashboard").is_none());
        assert!(resolve_on_instance(&i, "http://brainitive.example.com/dashboard").is_none());
        assert!(resolve_on_instance(&i, "https://brainitive.example.com.evil.com/").is_none());
        assert!(resolve_on_instance(&i, "https://u:p@brainitive.example.com/").is_none());
        assert!(resolve_on_instance(&i, "javascript:alert(1)").is_none());
        assert!(resolve_on_instance(&i, "dashboard").is_none());
        assert!(resolve_on_instance(&i, "/a\nb").is_none());
        assert!(resolve_on_instance(&i, "").is_none());
        // dot segments cannot climb out of the origin
        assert_eq!(
            resolve_on_instance(&i, "/../../etc/passwd").unwrap().as_str(),
            "https://brainitive.example.com/etc/passwd"
        );
    }

    #[test]
    fn external_links_are_http_https_or_mailto() {
        assert!(external_url("https://github.com/x").is_some());
        assert!(external_url("http://example.com").is_some());
        assert!(external_url("mailto:hello@brainitive.app").is_some());
        assert!(external_url("file:///etc/passwd").is_none());
        assert!(external_url("javascript:alert(1)").is_none());
        assert!(external_url("brainitive://open?url=/").is_none());
        assert!(external_url("smb://server/share").is_none());
        assert!(external_url("tel:+491234").is_none());
        assert!(external_url("/relative").is_none());
        assert!(external_url(&format!("https://x.com/{}", "a".repeat(5000))).is_none());
    }

    #[test]
    fn origin_keys() {
        assert_eq!(origin_key(&u("tauri://localhost/index.html")), "tauri://localhost");
        assert_eq!(origin_key(&u("http://localhost:1420/")), "http://localhost:1420");
        assert_eq!(
            origin_key(&u("https://www.brainitive.app/x")),
            "https://www.brainitive.app"
        );
        assert_eq!(origin_key(&u("about:blank")), "about:");
    }

    #[test]
    fn navigation_decisions() {
        let i = instance();
        let l = locals();
        let d = |s: &str| decide_navigation(&u(s), Some(&i), &l);

        assert_eq!(d("tauri://localhost/index.html"), Navigation::Allow);
        assert_eq!(d("http://tauri.localhost/"), Navigation::Allow);
        assert_eq!(d("https://brainitive.example.com/dashboard"), Navigation::Allow);
        assert_eq!(d("https://brainitive.example.com/app/session/abc"), Navigation::Allow);
        assert_eq!(d("https://brainitive.example.com/"), Navigation::Home);
        assert_eq!(d("https://brainitive.example.com"), Navigation::Home);
        assert_eq!(d("https://brainitive.example.com/login"), Navigation::Reauth);
        assert_eq!(d("https://brainitive.example.com/login/google"), Navigation::Reauth);
        assert_eq!(d("https://brainitive.example.com/loginx"), Navigation::Allow);
        assert_eq!(d("about:blank"), Navigation::Allow);
        assert_eq!(
            d("https://www.youtube.com/embed/x"),
            Navigation::Foreign(u("https://www.youtube.com/embed/x"))
        );
        assert_eq!(
            d("http://brainitive.example.com/dashboard"),
            Navigation::Foreign(u("http://brainitive.example.com/dashboard"))
        );
        assert_eq!(d("mailto:a@b.c"), Navigation::External(u("mailto:a@b.c")));
        assert_eq!(
            d("brainitive://open?url=/x"),
            Navigation::DeepLink(u("brainitive://open?url=/x"))
        );
        assert_eq!(d("file:///etc/passwd"), Navigation::Block);
        assert_eq!(d("javascript:alert(1)"), Navigation::Block);
        assert_eq!(d("tauri://evil/x"), Navigation::Block);
    }

    #[test]
    fn without_a_server_only_the_bundled_screens_are_app_pages() {
        let l = locals();
        assert_eq!(
            decide_navigation(&u("https://brainitive.example.com/"), None, &l),
            Navigation::Foreign(u("https://brainitive.example.com/"))
        );
        assert!(is_app_page(&u("tauri://localhost/"), None, &l));
        assert!(!is_app_page(&u("https://brainitive.example.com/"), None, &l));
        assert!(is_app_page(
            &u("https://brainitive.example.com/x"),
            Some(&instance()),
            &l
        ));
        assert!(!is_app_page(&u("https://evil.com/"), Some(&instance()), &l));
    }
}
