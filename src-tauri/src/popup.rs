//! The bundled windows (BRAI-359 quick capture, BRAI-401 quick switcher, the Settings window,
//! BRAI-366 the offline view, BRAI-439 the voice-capture indicator, BRAI-440 the Brainitive key
//! overlay).
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
    /// The Brainitive key overlay (BRAI-440): lists the letters, reads the next key.
    Leader,
    /// The menu-bar panel (left click on the tray icon): Today, My Day, assigned work,
    /// notifications, the timer.
    Panel,
}

impl Popup {
    pub const ALL: [Popup; 7] = [
        Popup::Capture,
        Popup::Switcher,
        Popup::Settings,
        Popup::Offline,
        Popup::Voice,
        Popup::Leader,
        Popup::Panel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Switcher => "switcher",
            Self::Settings => "settings",
            Self::Offline => "offline",
            Self::Voice => "voice",
            Self::Leader => "leader",
            Self::Panel => "panel",
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
            Self::Leader => "leader.html",
            Self::Panel => "panel.html",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Capture => "Quick Capture",
            Self::Switcher => "Quick Switcher",
            Self::Settings => "Settings",
            Self::Offline => "Brainitive Offline",
            Self::Voice => "Voice Capture",
            Self::Leader => "Brainitive Key",
            Self::Panel => "Brainitive",
        }
    }

    fn size(self) -> (f64, f64) {
        match self {
            Self::Capture => (560.0, 260.0),
            Self::Switcher => (660.0, 440.0),
            Self::Settings => (560.0, 640.0),
            Self::Offline => (920.0, 680.0),
            Self::Voice => (420.0, 200.0),
            Self::Leader => (340.0, 252.0),
            Self::Panel => (crate::panel::geometry::WIDTH, 440.0),
        }
    }

    /// Spotlight-like: no frame, above other windows, on every desktop.
    pub fn floating(self) -> bool {
        matches!(
            self,
            Self::Capture | Self::Switcher | Self::Voice | Self::Leader | Self::Panel
        )
    }

    /// Windows that hold the signed-in account's content (typed or transcribed capture text,
    /// search results, the offline copy): destroyed on sign-out, rebuilt empty on next use
    /// (BRAI-377). Settings and the voice indicator hold no account data. The panel shows
    /// the account's work and notifications.
    pub fn holds_account_data(self) -> bool {
        matches!(self, Self::Capture | Self::Switcher | Self::Offline | Self::Panel)
    }

    /// Gone when it loses focus. Not the voice indicator: a recording goes on while the user
    /// clicks elsewhere (its own Stop / Cancel and the shortcut end it).
    pub fn hides_on_blur(self) -> bool {
        matches!(self, Self::Capture | Self::Switcher | Self::Leader | Self::Panel)
    }
}

/// Sign-out / forget (BRAI-377): the bundled windows with the account's content are destroyed
/// (hidden ones keep their page — the capture text, the switcher's last results — otherwise).
pub fn close_account_windows(app: &AppHandle) {
    for popup in Popup::ALL.into_iter().filter(|p| p.holds_account_data()) {
        if let Some(w) = app.get_webview_window(popup.label()) {
            let _ = w.destroy();
        }
        state(app).lock().popup_return.remove(popup.label());
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
    show_with(app, popup, |_| {});
}

/// [`show`], with `prepare` run on the window right before it is shown (the panel's size and
/// position).
pub fn show_with(app: &AppHandle, popup: Popup, prepare: impl FnOnce(&WebviewWindow) + Send + 'static) {
    // opened from another bundled window (voice indicator → quick capture): return the focus to
    // where that one would have
    let app_was_active = shell::app_window_focused(app) || inherited_return(app, popup);
    match app.get_webview_window(popup.label()) {
        Some(w) => {
            prepare(&w);
            present(app, popup, w, app_was_active)
        }
        None => {
            // menu, tray and shortcut handlers run on the main thread; building a webview
            // there can deadlock on Windows, so it is built from the async runtime
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                match build(&app, popup) {
                    Ok(w) => {
                        prepare(&w);
                        present(&app, popup, w, app_was_active)
                    }
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
    // the panel is placed under the tray icon (`panel::show`)
    if popup.floating() && popup != Popup::Panel {
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
        if popup == Popup::Panel {
            crate::panel::hidden_by_blur(app);
        }
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
    let builder = if popup == Popup::Panel {
        crate::panel::window_style(builder)
    } else {
        builder
    };
    // macOS 26: the page must not run under the see-through title bar (see shell::TITLE_BAR)
    #[cfg(target_os = "macos")]
    let builder = if floating {
        builder
    } else {
        builder.title_bar_style(shell::TITLE_BAR)
    };
    let window = builder.build()?;
    if let Ok(theme) = window.theme() {
        apply_title_bar_color(&window, popup, theme);
    }
    Ok(window)
}

/// The colour of the top of a decorated bundled window's page (`--bg` of Settings, `--panel` of
/// the offline view's status bar), light / dark: the title bar shows it, so title bar and page
/// read as one surface. `None`: no title bar (floating windows).
pub fn title_bar_color(popup: Popup, dark: bool) -> Option<(u8, u8, u8)> {
    match (popup, dark) {
        (Popup::Settings, false) => Some((0xff, 0xff, 0xff)),
        (Popup::Settings, true) => Some((0x19, 0x21, 0x2a)),
        (Popup::Offline, false) => Some((0xf4, 0xf6, 0xf8)),
        (Popup::Offline, true) => Some((0x11, 0x16, 0x1c)),
        _ => None,
    }
}

/// Paints the title bar strip (macOS; elsewhere the OS draws its own title bar).
fn apply_title_bar_color(window: &WebviewWindow, popup: Popup, theme: tauri::Theme) {
    if !cfg!(target_os = "macos") {
        return;
    }
    if let Some((r, g, b)) = title_bar_color(popup, theme == tauri::Theme::Dark) {
        let _ = window.set_background_color(Some(tauri::window::Color(r, g, b, 255)));
    }
}

/// `WindowEvent::ThemeChanged`: light ↔ dark, the title bar strip follows the page.
pub fn on_theme_changed(app: &AppHandle, label: &str, theme: tauri::Theme) {
    if let (Some(popup), Some(window)) = (Popup::from_label(label), app.get_webview_window(label)) {
        apply_title_bar_color(&window, popup, theme);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sign_out_closes_the_windows_with_account_data() {
        let closed: Vec<_> = Popup::ALL.into_iter().filter(|p| p.holds_account_data()).collect();
        assert_eq!(
            closed,
            vec![Popup::Capture, Popup::Switcher, Popup::Offline, Popup::Panel]
        );
    }

    use super::*;

    #[test]
    fn title_bar_strip_matches_the_top_of_the_page() {
        // native.css --bg (Settings) and --panel (the offline view's status bar), light and dark
        assert_eq!(title_bar_color(Popup::Settings, false), Some((0xff, 0xff, 0xff)));
        assert_eq!(title_bar_color(Popup::Settings, true), Some((0x19, 0x21, 0x2a)));
        assert_eq!(title_bar_color(Popup::Offline, false), Some((0xf4, 0xf6, 0xf8)));
        assert_eq!(title_bar_color(Popup::Offline, true), Some((0x11, 0x16, 0x1c)));
        let css = include_str!("../../src/native.css");
        assert!(css.contains("--bg: #ffffff") && css.contains("--bg: #19212a"));
        assert!(css.contains("--panel: #f4f6f8") && css.contains("--panel: #11161c"));
        // floating windows have no title bar
        for p in Popup::ALL.into_iter().filter(|p| p.floating()) {
            assert_eq!(title_bar_color(p, false), None, "{}", p.label());
        }
    }

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
        // the Brainitive key overlay: floats, goes when the focus moves on, holds no account data
        assert!(Popup::Leader.floating() && Popup::Leader.hides_on_blur());
        assert!(!Popup::Leader.holds_account_data());
        assert_eq!(Popup::from_label("leader"), Some(Popup::Leader));
        // the menu-bar panel: floats, goes on blur, holds the account's work and notifications
        assert!(Popup::Panel.floating() && Popup::Panel.hides_on_blur() && Popup::Panel.holds_account_data());
        assert_eq!(Popup::from_label("panel"), Some(Popup::Panel));
    }
}
