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

/// Paths a deep link, a notification or a server-provided menu / tray / switcher link may never
/// open in the app window (BRAI-377): sign-in, sign-out and session plumbing. A
/// `brainitive://open?url=/app/session/<attacker's code>` would otherwise sign the webview into
/// someone else's account (login CSRF), and `/login/google` would start OAuth inside the webview.
/// `(prefix, whole segment)`: `true` matches `/app` and `/app/…` but not `/apps`; `false` is a
/// plain prefix (`/login*` also covers `/login-as`).
const DEEP_LINK_DENIED: [(&str, bool); 15] = [
    ("/app", true),
    ("/login", false),
    ("/logout", false),
    ("/register", false),
    ("/password", true),
    ("/auth", true),
    ("/oauth", true),
    ("/api", true),
    ("/forgot-password", true),
    ("/reset-password", true),
    ("/confirm-password", true),
    ("/verify-email", true),
    ("/email/verification-notification", true),
    ("/github-app", true),
    ("/github/app", true),
];

/// A page of the current server that a deep link (`brainitive://open?url=…`), a web
/// notification's `url`, a notification click or a link the server hands the tray / menus /
/// switcher may open: [`resolve_on_instance`], and the path is none of the auth endpoints in
/// [`DEEP_LINK_DENIED`] — compared after percent-decoding (also double), `\` → `/`, `//` and
/// dot segments collapsed, and lower-cased, so `/%61pp/session`, `/x/..%2Fapp/session`,
/// `//App//session` or `/LOGIN` are caught too. The app's own navigation to the one-time
/// `/app/session/<code>` (from `POST /api/v1/auth/web-session`) uses [`resolve_on_instance`].
pub fn resolve_deep_link_target(instance: &ServerOrigin, target: &str) -> Option<Url> {
    let url = resolve_on_instance(instance, target)?;
    is_deep_link_safe(&url).then_some(url)
}

/// Whether a page URL is outside the auth endpoints (see [`resolve_deep_link_target`]). A path
/// that does not decode cleanly is refused.
pub fn is_deep_link_safe(url: &Url) -> bool {
    let Some(path) = normalised_path(url.path()) else {
        return false;
    };
    !DEEP_LINK_DENIED.iter().any(|(prefix, segment)| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| !*segment || rest.is_empty() || rest.starts_with('/'))
    })
}

/// The path the server's router will most likely see: percent-decoded until stable (≤ 3
/// rounds), backslashes as slashes, `;params` cut, empty and `.` segments dropped, `..` applied,
/// lower-cased.
/// `None` for invalid UTF-8, control characters or more than three layers of encoding.
fn normalised_path(raw: &str) -> Option<String> {
    let mut path = raw.to_string();
    let mut stable = false;
    for _ in 0..=3 {
        let decoded = percent_decode(&path)?;
        if decoded == path {
            stable = true;
            break;
        }
        path = decoded;
    }
    if !stable || path.chars().any(char::is_control) {
        return None;
    }
    let path = path.replace('\\', "/").to_lowercase();
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        // `;params` are cut like servers that know path parameters do (`/app;x/session`)
        let segment = segment.split(';').next().unwrap_or("");
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    Some(format!("/{}", segments.join("/")))
}

/// `%XX` → byte; a `%` without two hex digits stays as it is. `None` for invalid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
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

/// Where Tauri serves the bundled screens on this platform (tauri 2.12.5,
/// `tauri::protocol::origin`, `useHttpsScheme` off): `http://tauri.localhost` on Windows (and
/// Android), `tauri://localhost` on macOS / Linux. Only this one origin counts as bundled: on
/// macOS / Linux `http://tauri.localhost` is just a loopback host name that any local web server
/// could answer (BRAI-377).
pub fn bundled_origin(windows: bool) -> &'static str {
    if windows {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    }
}

/// What the page-load handler of an app window does (pure part of `shell::on_page_load`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageLoad {
    /// An app page: nothing to do.
    Keep,
    /// A page of the current server finished loading: the main window remembers it as the page
    /// to go back to.
    Remember,
    /// The main frame committed to a page that is no app page (another site, a server redirect
    /// elsewhere): go back right away, and open `outside` (http / https) in the system browser.
    Revert { outside: Option<Url> },
}

/// `started`: `PageLoadEvent::Started` (WebKit: the main frame *committed* the new document),
/// otherwise `Finished`. Page-load events are main-frame only on every platform, so unlike the
/// navigation handler they never see iframes: a foreign page here is a top-level load and is
/// reverted on `Started` already, not only on `Finished` (BRAI-377).
pub fn decide_page_load(
    url: &Url,
    instance: Option<&ServerOrigin>,
    local_origins: &[String],
    started: bool,
) -> PageLoad {
    if !is_app_page(url, instance, local_origins) {
        let outside = if started { external_url(url.as_str()) } else { None };
        return PageLoad::Revert { outside };
    }
    if !started && instance.is_some_and(|i| i.contains(url)) {
        PageLoad::Remember
    } else {
        PageLoad::Keep
    }
}

/// Where a reverted window goes: the main window to the last good page of the server (or the
/// bundled screens), an extra window to the server's dashboard.
pub fn revert_target(main: bool, last_good: Option<&Url>, instance: Option<&ServerOrigin>, local: &Url) -> Url {
    if main {
        if let Some(page) = last_good.filter(|p| instance.is_some_and(|i| i.contains(p))) {
            return page.clone();
        }
        return local.clone();
    }
    instance.map(|o| o.join("/dashboard")).unwrap_or_else(|| local.clone())
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
        assert_eq!(
            decide_local_navigation(&u("javascript:alert(1)"), &l),
            Navigation::Block
        );
        assert_eq!(
            decide_local_navigation(&u("brainitive://open?url=/x"), &l),
            Navigation::Block
        );
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
    fn deep_links_never_reach_auth_endpoints() {
        let i = instance();
        let ok = |t: &str| resolve_deep_link_target(&i, t).is_some();
        // ordinary pages
        for page in [
            "/workspaces/a/projects/b",
            "/dashboard",
            "/my-work?tab=today",
            "/apps",
            "/apix",
            "/applications/1",
            "/authors",
            "/workspaces/a/pages/login-notes",
            "https://brainitive.example.com/notifications",
        ] {
            assert!(ok(page), "{page} should open");
        }
        // the auth endpoints, plainly and with every disguise
        for bad in [
            "/app/session/abc123",
            "/app",
            "/app/",
            "https://brainitive.example.com/app/session/abc",
            "HTTPS://BRAINITIVE.EXAMPLE.COM/APP/SESSION/abc",
            "/APP/session/abc",
            "/App/Session/abc",
            "/%61pp/session/abc",
            "/%2561pp/session/abc",
            "/%252561pp/session/abc",
            "/app%2Fsession/abc",
            "/app%2fsession/abc",
            "/app%5Csession/abc",
            "/x/..%2Fapp/session/abc",
            "/x/%2e%2e/app/session/abc",
            "/x/%2E%2E%2Fapp/session/abc",
            "/./app/session/abc",
            "/x/../app/session",
            "/x/..%5capp/session",
            "/app/./session/abc",
            "/app;/session/abc",
            "/login",
            "/login/google",
            "/LOGIN/github/callback",
            "/loginx",
            "/logout",
            "/logout?next=/",
            "/register",
            "/register/invite",
            "/password/reset",
            "/password",
            "/auth/callback",
            "/oauth/authorize",
            "/api/v1/me",
            "/API/v1/auth/web-session",
            "/forgot-password",
            "/reset-password/tok",
            "/verify-email/1/abc",
            "/github-app/callback",
        ] {
            assert!(!ok(bad), "{bad} must be refused");
        }
        // still only this server
        assert!(!ok("//evil.com/dashboard"));
        assert!(!ok("https://evil.com/dashboard"));
        // undecodable / more than three layers of encoding → refused
        assert!(!ok("/%ff%fe"));
        assert!(!ok("/%2525252561pp/x"));
        assert!(!ok("/a%00b"));
        // the app's own session link still resolves through the internal path
        assert!(resolve_on_instance(&i, "/app/session/abc123").is_some());
        assert!(!is_deep_link_safe(&u("https://brainitive.example.com/app/session/x")));
        assert!(is_deep_link_safe(&u("https://brainitive.example.com/captures")));
    }

    #[test]
    fn path_normalisation() {
        assert_eq!(normalised_path("/A//b/./c/../D").as_deref(), Some("/a/b/d"));
        assert_eq!(normalised_path("/../../x").as_deref(), Some("/x"));
        assert_eq!(normalised_path("/100%").as_deref(), Some("/100%"));
        assert_eq!(normalised_path("/%zz").as_deref(), Some("/%zz"));
        assert_eq!(normalised_path("/caf%C3%A9").as_deref(), Some("/café"));
    }

    #[test]
    fn the_bundled_origin_is_per_platform() {
        assert_eq!(bundled_origin(false), "tauri://localhost");
        assert_eq!(bundled_origin(true), "http://tauri.localhost");
        // macOS / Linux: http://tauri.localhost is a loopback web server, not the app
        let mac = vec![bundled_origin(false).to_string()];
        assert!(!is_app_page(&u("http://tauri.localhost/"), None, &mac));
        assert!(!is_local_caller(
            "capture",
            &u("http://tauri.localhost/capture.html"),
            "capture",
            &mac
        ));
        assert_eq!(
            decide_local_navigation(&u("http://tauri.localhost/x"), &mac),
            Navigation::External(u("http://tauri.localhost/x"))
        );
        assert!(is_app_page(&u("tauri://localhost/index.html"), None, &mac));
        // Windows: tauri:// is nothing
        let win = vec![bundled_origin(true).to_string()];
        assert!(is_app_page(&u("http://tauri.localhost/index.html"), None, &win));
        assert!(!is_app_page(&u("tauri://localhost/index.html"), None, &win));
        assert!(!is_app_page(&u("https://tauri.localhost/index.html"), None, &win));
        assert_eq!(
            decide_navigation(&u("tauri://localhost/index.html"), None, &win),
            Navigation::Block
        );
    }

    #[test]
    fn foreign_top_level_loads_are_reverted_on_start() {
        let i = instance();
        let l = locals();
        let d = |s: &str, started: bool| decide_page_load(&u(s), Some(&i), &l, started);
        assert_eq!(
            d("https://evil.com/phish", true),
            PageLoad::Revert {
                outside: Some(u("https://evil.com/phish"))
            }
        );
        // on Finished (the start was missed): back without opening it twice
        assert_eq!(d("https://evil.com/phish", false), PageLoad::Revert { outside: None });
        assert_eq!(
            d("file:///etc/passwd", true),
            PageLoad::Revert { outside: None },
            "never handed to the browser"
        );
        assert_eq!(
            d("http://brainitive.example.com/x", true),
            PageLoad::Revert {
                outside: Some(u("http://brainitive.example.com/x"))
            },
            "the server over plain http is another origin"
        );
        assert_eq!(d("https://brainitive.example.com/dashboard", true), PageLoad::Keep);
        assert_eq!(d("https://brainitive.example.com/dashboard", false), PageLoad::Remember);
        assert_eq!(d("tauri://localhost/index.html", false), PageLoad::Keep);
        assert_eq!(d("about:blank", true), PageLoad::Keep);
        // signed out: the server is no app page
        assert_eq!(
            decide_page_load(&u("https://brainitive.example.com/"), None, &l, true),
            PageLoad::Revert {
                outside: Some(u("https://brainitive.example.com/"))
            }
        );
    }

    #[test]
    fn reverts_go_back_to_a_page_of_the_server() {
        let i = instance();
        let local = u("tauri://localhost/");
        let good = u("https://brainitive.example.com/workspaces/a");
        assert_eq!(revert_target(true, Some(&good), Some(&i), &local), good);
        assert_eq!(revert_target(true, None, Some(&i), &local), local);
        // a last page of another server is never the way back
        let other = u("https://other.example.com/x");
        assert_eq!(revert_target(true, Some(&other), Some(&i), &local), local);
        assert_eq!(
            revert_target(false, Some(&good), Some(&i), &local),
            u("https://brainitive.example.com/dashboard")
        );
        assert_eq!(revert_target(false, None, None, &local), local);
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
