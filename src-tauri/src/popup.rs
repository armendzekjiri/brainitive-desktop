//! The bundled windows (BRAI-359 quick capture, BRAI-401 quick switcher, the Settings window,
//! BRAI-366 the offline view, BRAI-439 the voice-capture indicator).
//!
//! They load bundled pages only (`capture.html`, `switcher.html`, `settings.html`, `offline.html`), never the
//! server. They talk to the server exclusively through their own Rust commands, which use the
//! stored app token; the token never reaches their JavaScript. Each window has its own
//! capability file (`capabilities/<label>.json`) and every command checks the caller again
//! ([`ensure_caller`]). Built on first use, then hidden instead of closed so they open instantly.

use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::links::{self, Navigation};
use crate::shell;
use crate::state::state;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Popup {
    Capture,
    Switcher,
    Settings,
    /// The offline view (BRAI-366): the cached My Day, work, task lists and captures.
    Offline,
    /// The voice-capture indicator (BRAI-439): level meter, timer, set-up steps.
    Voice,
}

impl Popup {
    pub const ALL: [Popup; 5] = [
        Popup::Capture,
        Popup::Switcher,
        Popup::Settings,
        Popup::Offline,
        Popup::Voice,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Switcher => "switcher",
            Self::Settings => "settings",
            Self::Offline => "offline",
            Self::Voice => "voice",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.label() == label)
    }

    fn page(self) -> &'static str {
        match self {
            Self::Capture => "capture.html",
            Self::Switcher => "switcher.html",
            Self::Settings => "settings.html",
            Self::Offline => "offline.html",
            Self::Voice => "voice.html",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Capture => "Quick Capture",
            Self::Switcher => "Quick Switcher",
            Self::Settings => "Settings",
            Self::Offline => "Brainitive Offline",
            Self::Voice => "Voice Capture",
        }
    }

    fn size(self) -> (f64, f64) {
        match self {
            Self::Capture => (560.0, 260.0),
            Self::Switcher => (660.0, 440.0),
            Self::Settings => (560.0, 640.0),
            Self::Offline => (920.0, 680.0),
            Self::Voice => (420.0, 200.0),
        }
    }

    /// Spotlight-like: no frame, above other windows, on every desktop.
    pub fn floating(self) -> bool {
        matches!(self, Self::Capture | Self::Switcher | Self::Voice)
    }

    /// Gone when it loses focus. Not the voice indicator: a recording goes on while the user
    /// clicks elsewhere (its own Stop / Cancel and the shortcut end it).
    pub fn hides_on_blur(self) -> bool {
        matches!(self, Self::Capture | Self::Switcher)
    }
}

/// Labels of the bundled windows, for the window-state plugin's denylist.
pub fn labels() -> Vec<&'static str> {
    Popup::ALL.iter().map(|p| p.label()).collect()
}

/// Commands of a bundled window accept calls from that window's bundled page only.
pub fn ensure_caller(app: &AppHandle, webview: &Webview, popup: Popup) -> Result<(), String> {
    let url = webview.url().map_err(|_| "unknown caller".to_string())?;
    if links::is_local_caller(webview.label(), &url, popup.label(), &shell::local_origins(app)) {
        Ok(())
    } else {
        Err("not allowed".into())
    }
}

/// Shows (building it on first use) and focuses the window.
pub fn show(app: &AppHandle, popup: Popup) {
    // opened from another bundled window (voice indicator → quick capture): return the focus to
    // where that one would have
    let app_was_active = shell::app_window_focused(app) || inherited_return(app, popup);
    match app.get_webview_window(popup.label()) {
        Some(w) => present(app, popup, w, app_was_active),
        None => {
            // menu, tray and shortcut handlers run on the main thread; building a webview
            // there can deadlock on Windows, so it is built from the async runtime
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                match build(&app, popup) {
                    Ok(w) => present(&app, popup, w, app_was_active),
                    Err(e) => log::error!("could not open {}: {e}", popup.label()),
                }
            });
        }
    }
}

/// A bundled window that has the focus now hands its "return the focus to the app" over.
fn inherited_return(app: &AppHandle, popup: Popup) -> bool {
    Popup::ALL.into_iter().filter(|p| *p != popup).any(|p| {
        app.get_webview_window(p.label())
            .is_some_and(|w| w.is_focused().unwrap_or(false))
            && state(app).lock().popup_return.get(p.label()).copied().unwrap_or(false)
    })
}

fn present(app: &AppHandle, popup: Popup, window: WebviewWindow, app_was_active: bool) {
    state(app)
        .lock()
        .popup_return
        .insert(popup.label().to_string(), app_was_active);
    if popup.floating() {
        let _ = window.center();
    }
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// The global shortcut: open, or close again when it is already in front.
pub fn toggle(app: &AppHandle, popup: Popup) {
    match app.get_webview_window(popup.label()) {
        Some(w) if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) => dismiss(app, popup),
        _ => show(app, popup),
    }
}

/// Esc / after saving: hide and give the focus back to where the user was. When another app
/// was in front (the shortcut was pressed there), macOS hides this app so that app is active
/// again; Windows / Linux return the focus by themselves.
pub fn dismiss(app: &AppHandle, popup: Popup) {
    let Some(window) = app.get_webview_window(popup.label()) else {
        return;
    };
    let _ = window.hide();
    if popup == Popup::Settings {
        // a shortcut recording that was not finished must not leave the shortcuts paused
        if let Err(e) = crate::hotkeys::apply(app) {
            log::warn!("{e}");
        }
    }
    let app_was_active = state(app).lock().popup_return.remove(popup.label()).unwrap_or(true);
    if app_was_active {
        if let Some(w) = shell::target_window(app).filter(|w| w.is_visible().unwrap_or(false)) {
            let _ = w.set_focus();
        }
    } else {
        #[cfg(target_os = "macos")]
        {
            let _ = app.hide();
        }
    }
}

/// Hides without handing the focus anywhere (the caller is about to show an app window).
pub fn hide(app: &AppHandle, popup: Popup) {
    if let Some(w) = app.get_webview_window(popup.label()) {
        let _ = w.hide();
    }
    state(app).lock().popup_return.remove(popup.label());
}

/// `WindowEvent::Focused(false)`: a floating window closes when the user clicks elsewhere
/// (its text stays for next time).
pub fn on_focus_lost(app: &AppHandle, label: &str) {
    if let Some(popup) = Popup::from_label(label).filter(|p| p.hides_on_blur()) {
        if let Some(w) = app.get_webview_window(popup.label()) {
            let _ = w.hide();
        }
        state(app).lock().popup_return.remove(popup.label());
    }
}

fn build(app: &AppHandle, popup: Popup) -> tauri::Result<WebviewWindow> {
    let nav = app.clone();
    let popup_app = app.clone();
    let (width, height) = popup.size();
    let floating = popup.floating();
    let builder = WebviewWindowBuilder::new(app, popup.label(), WebviewUrl::App(popup.page().into()))
        .title(popup.title())
        .inner_size(width, height)
        .resizable(!floating)
        .minimizable(!floating)
        .maximizable(false)
        .decorations(!floating)
        .always_on_top(floating)
        .skip_taskbar(floating)
        .visible_on_all_workspaces(floating)
        .visible(false)
        .center()
        // a link / text dropped into a field is handled by the page; files never navigate
        .disable_drag_drop_handler()
        .on_navigation(
            move |url| match links::decide_local_navigation(url, &shell::local_origins(&nav)) {
                Navigation::Allow => true,
                Navigation::External(url) => {
                    shell::open_external(&nav, &url);
                    false
                }
                _ => false,
            },
        )
        .on_new_window(move |url, _| {
            if links::external_url(url.as_str()).is_some() {
                shell::open_external(&popup_app, &url);
            }
            NewWindowResponse::Deny
        });
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_round_trip_and_never_clash_with_app_windows() {
        for p in Popup::ALL {
            assert_eq!(Popup::from_label(p.label()), Some(p));
            assert!(
                !shell::is_app_window(p.label()),
                "{} must not get the remote bridge",
                p.label()
            );
        }
        assert_eq!(Popup::from_label("main"), None);
        assert!(Popup::Capture.floating() && Popup::Switcher.floating() && !Popup::Settings.floating());
        assert!(!Popup::Offline.floating());
        // the voice indicator floats but stays while the user clicks elsewhere
        assert!(Popup::Voice.floating() && !Popup::Voice.hides_on_blur());
        assert!(Popup::Capture.hides_on_blur() && Popup::Switcher.hides_on_blur());
        assert_eq!(Popup::from_label("voice"), Some(Popup::Voice));
    }
}
