//! The four commands the web app may call (`resources/js/lib/desktop.ts` in the app repo,
//! `window.__TAURI_INTERNALS__.invoke(cmd, args)`):
//! `brainitive_notify {title, body, url?}`, `brainitive_set_badge {count}`,
//! `brainitive_open_external {url}`, `brainitive_sign_out {}` (after the web "Sign out").
//!
//! Two locks: the runtime capability lets only the current server's origin reach them, and
//! every command checks again that the calling webview is an app window (main / `app-<n>`) on
//! that origin (an earlier server's capability cannot be removed at runtime). Notifications and
//! the badge are taken from the main window only: extra windows run the same web app and would
//! show every notification twice.

use std::time::{Duration, Instant};

use tauri::{AppHandle, Webview};
use tauri_plugin_notification::NotificationExt;
use url::Url;

use crate::links;
use crate::shell::{self, MAIN};
use crate::state::{state, NotificationTarget};

const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 500;
/// At most this many notifications per minute from the web app.
const NOTIFY_PER_MINUTE: usize = 20;
/// At most this many links per minute the web app may open in the browser / mail client.
const OPEN_EXTERNAL_PER_MINUTE: usize = 10;

/// A sliding one-minute window: records `now` and returns `true` while fewer than `per_minute`
/// calls happened in the last 60 s; `false` (nothing recorded) once the limit is reached.
pub fn allow_rate(times: &mut std::collections::VecDeque<Instant>, now: Instant, per_minute: usize) -> bool {
    while times
        .front()
        .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
    {
        times.pop_front();
    }
    if times.len() >= per_minute {
        return false;
    }
    times.push_back(now);
    true
}

fn ensure_current_server(app: &AppHandle, webview: &Webview) -> Result<crate::server::ServerOrigin, String> {
    let origin = state(app).lock().current_origin().ok_or("not connected")?;
    let url = webview.url().map_err(|_| "unknown caller")?;
    if !is_bridge_caller(webview.label(), &url, &origin) {
        return Err("not allowed".into());
    }
    Ok(origin)
}

/// The bridge's own check (next to the origin-scoped runtime capability): an app window whose
/// page is on the current server's exact origin. A foreign page that committed in the main
/// window for the few milliseconds before the page-load guard reverts it (macOS / Linux, codex
/// review 7) is refused here, and the capability (`<origin>/*`) never matches it either.
pub fn is_bridge_caller(label: &str, url: &url::Url, current: &crate::server::ServerOrigin) -> bool {
    shell::is_app_window(label) && current.contains(url)
}

fn clip(text: &str, max: usize) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control() || *c == '\n').collect();
    let mut out: String = clean.trim().chars().take(max).collect();
    if clean.trim().chars().count() > max {
        out.push('…');
    }
    out
}

#[tauri::command]
pub fn brainitive_notify(
    app: AppHandle,
    webview: Webview,
    title: String,
    body: String,
    url: Option<String>,
) -> Result<(), String> {
    let origin = ensure_current_server(&app, &webview)?;
    if webview.label() != MAIN {
        return Ok(());
    }
    if !allow_rate(&mut state(&app).lock().notify_times, Instant::now(), NOTIFY_PER_MINUTE) {
        return Err("rate limited".into());
    }
    // a link that is not a page of this server (or is one of its auth endpoints) is dropped,
    // the notification still shows
    let target = url.as_deref().and_then(|u| links::resolve_deep_link_target(&origin, u));
    show_notification(&app, &clip(&title, MAX_TITLE), &clip(&body, MAX_BODY), target);
    Ok(())
}

#[tauri::command]
pub fn brainitive_set_badge(app: AppHandle, webview: Webview, count: i64) -> Result<(), String> {
    ensure_current_server(&app, &webview)?;
    if webview.label() != MAIN {
        return Ok(());
    }
    set_badge(&app, count.clamp(0, 99_999));
    Ok(())
}

#[tauri::command]
pub fn brainitive_open_external(app: AppHandle, webview: Webview, url: String) -> Result<(), String> {
    ensure_current_server(&app, &webview)?;
    let url = links::external_url(&url).ok_or("only http, https and mailto links")?;
    // a page must not be able to flood the browser with tabs (BRAI-377)
    if !allow_rate(
        &mut state(&app).lock().external_times,
        Instant::now(),
        OPEN_EXTERNAL_PER_MINUTE,
    ) {
        return Err("rate limited".into());
    }
    shell::open_external(&app, &url);
    Ok(())
}

/// The web app's "Sign out" succeeded: sign the app out too (revoke this device, delete the
/// keychain entry, clear this server's data, show the sign-in screen). Without it the stored
/// sign-in would open a fresh web session right away.
#[tauri::command]
pub async fn brainitive_sign_out(app: AppHandle, webview: Webview) -> Result<(), String> {
    ensure_current_server(&app, &webview)?;
    crate::flows::sign_out(&app).await;
    Ok(())
}

/// A native notification; a click focuses the app and opens `target` (a page of the server).
pub fn show_notification(app: &AppHandle, title: &str, body: &str, target: Option<Url>) {
    notify(app, title, body, target.map(NotificationTarget::Page));
}

/// A notification about offline changes; a click opens the offline window.
pub fn show_offline_notification(app: &AppHandle, title: &str, body: &str) {
    notify(app, title, body, Some(NotificationTarget::OfflineView));
}

/// A notification about a downloaded file; a click shows it in Finder / Explorer.
pub fn show_file_notification(app: &AppHandle, title: &str, body: &str, file: std::path::PathBuf) {
    notify(app, title, body, Some(NotificationTarget::File(file)));
}

fn notify(app: &AppHandle, title: &str, body: &str, target: Option<NotificationTarget>) {
    // Windows: a page link opens from the banner and from the notification centre (toast.rs)
    if let Some(NotificationTarget::Page(page)) = &target {
        if crate::toast::show_page(app, title, body, page) {
            return;
        }
    }
    let id = {
        let s = state(app);
        let mut inner = s.lock();
        let id = inner.next_notification;
        inner.next_notification = inner.next_notification.wrapping_add(1).max(1);
        if let Some(target) = target {
            if inner.notifications.len() >= 200 {
                inner.notifications.clear();
            }
            inner.notifications.insert(id, target);
        }
        id
    };
    // the Microsoft Store build, packaged: a toast from the package's identity (BRAI-442)
    if crate::msstore::show_toast(app, id, title, body) {
        return;
    }
    let result = app.notification().builder().id(id).title(title).body(body).show();
    if let Err(e) = result {
        log::warn!("notification failed: {e}");
    }
}

/// Registered once in setup: a click (`tap`) on one of our notifications.
pub fn on_notification_action(app: &AppHandle, action_id: &str, id: Option<i32>) {
    if action_id != "tap" {
        return;
    }
    let target = id.and_then(|id| state(app).lock().notifications.remove(&id));
    match target {
        Some(NotificationTarget::File(path)) => {
            use tauri_plugin_opener::OpenerExt;
            if let Err(e) = app.opener().reveal_item_in_dir(&path) {
                log::warn!("could not show the file: {e}");
            }
        }
        Some(NotificationTarget::Page(page)) => {
            // the server may have been switched since the notification was shown
            let current = state(app).lock().current_origin();
            match current {
                Some(origin) if origin.contains(&page) && links::is_deep_link_safe(&page) => {
                    shell::navigate_to(app, &origin, page)
                }
                _ => shell::focus_main(app),
            }
        }
        Some(NotificationTarget::OfflineView) => crate::popup::show(app, crate::popup::Popup::Offline),
        None => shell::focus_main(app),
    }
}

/// The unread badge, from the web app (`brainitive_set_badge`) or the tray poller
/// (`GET /notifications/unread-count`): dock badge on macOS, launcher badge on Linux (Unity
/// API), an overlay icon with the count on the Windows taskbar button (`badge.rs`).
pub fn set_badge(app: &AppHandle, count: i64) {
    #[cfg(not(target_os = "windows"))]
    if let Some(w) = shell::main_window(app) {
        let _ = w.set_badge_count(if count > 0 { Some(count) } else { None });
    }
    #[cfg(target_os = "windows")]
    if let Some(w) = shell::main_window(app) {
        let icon = crate::badge::render(count)
            .map(|rgba| tauri::image::Image::new_owned(rgba, crate::badge::SIZE, crate::badge::SIZE));
        let _ = w.set_overlay_icon(icon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limits_slide_over_one_minute() {
        let mut times = std::collections::VecDeque::new();
        let t0 = Instant::now();
        for i in 0..OPEN_EXTERNAL_PER_MINUTE {
            assert!(allow_rate(
                &mut times,
                t0 + Duration::from_secs(i as u64),
                OPEN_EXTERNAL_PER_MINUTE
            ));
        }
        assert!(!allow_rate(
            &mut times,
            t0 + Duration::from_secs(30),
            OPEN_EXTERNAL_PER_MINUTE
        ));
        assert_eq!(times.len(), OPEN_EXTERNAL_PER_MINUTE, "a refused call is not counted");
        // the first call leaves the window after 60 s: one more is allowed
        assert!(allow_rate(
            &mut times,
            t0 + Duration::from_secs(61),
            OPEN_EXTERNAL_PER_MINUTE
        ));
        assert!(!allow_rate(
            &mut times,
            t0 + Duration::from_secs(61),
            OPEN_EXTERNAL_PER_MINUTE
        ));
        assert!(allow_rate(
            &mut times,
            t0 + Duration::from_secs(200),
            OPEN_EXTERNAL_PER_MINUTE
        ));
    }

    #[test]
    fn clips_and_strips_control_characters() {
        assert_eq!(clip("  Hello\u{0007}  ", 10), "Hello");
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("line1\nline2", 20), "line1\nline2");
    }

    #[test]
    fn a_foreign_page_in_an_app_window_never_reaches_the_bridge() {
        let current = crate::server::ServerOrigin::parse("brainitive.example.com").unwrap();
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert!(is_bridge_caller(
            "main",
            &u("https://brainitive.example.com/dashboard"),
            &current
        ));
        assert!(is_bridge_caller(
            "app-3",
            &u("https://brainitive.example.com/x"),
            &current
        ));
        // the foreign page committed before the revert (macOS / Linux), a look-alike, http
        for foreign in [
            "https://evil.com/phish",
            "https://brainitive.example.com.evil.com/",
            "https://evil.brainitive.example.com/",
            "http://brainitive.example.com/dashboard",
            "https://brainitive.example.com:8443/",
            "tauri://localhost/index.html",
            "about:blank",
        ] {
            assert!(!is_bridge_caller("main", &u(foreign), &current), "{foreign}");
        }
        // bundled windows never, whatever they show
        assert!(!is_bridge_caller(
            "capture",
            &u("https://brainitive.example.com/"),
            &current
        ));
        // the runtime capability is scoped to the exact origin as well
        assert_eq!(current.capability_pattern(), "https://brainitive.example.com/*");
    }
}
