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

fn ensure_current_server(app: &AppHandle, webview: &Webview) -> Result<crate::server::ServerOrigin, String> {
    let origin = state(app).lock().current_origin().ok_or("not connected")?;
    let url = webview.url().map_err(|_| "unknown caller")?;
    if !shell::is_app_window(webview.label()) || !origin.contains(&url) {
        return Err("not allowed".into());
    }
    Ok(origin)
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
    {
        let s = state(&app);
        let mut inner = s.lock();
        let now = Instant::now();
        while inner
            .notify_times
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
        {
            inner.notify_times.pop_front();
        }
        if inner.notify_times.len() >= NOTIFY_PER_MINUTE {
            return Err("rate limited".into());
        }
        inner.notify_times.push_back(now);
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
    fn clips_and_strips_control_characters() {
        assert_eq!(clip("  Hello\u{0007}  ", 10), "Hello");
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("line1\nline2", 20), "line1\nline2");
    }
}
