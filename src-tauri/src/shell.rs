//! The main window: one frameless window with the bundled top bar (`tabs::TOPBAR`) and the tabs —
//! the `main` webview (the bundled screens or the current server) and `app-<n>` webviews with
//! pages of the current server (`tabs.rs`). Every tab uses the server's own webview data store,
//! the same navigation guard, remote IPC capability, downloads and user agent (BRAI-350 / BRAI-351).

#[cfg(not(target_os = "macos"))]
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tauri::ipc::CapabilityBuilder;
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Webview, WebviewUrl, Window, Wry};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::links::{self, Navigation};
use crate::server::{self, ServerOrigin};
use crate::state::{state, Phase, Rebuild};
use crate::tabs::{self, BAR_HEIGHT, TOPBAR};
use crate::version::APP_VERSION;

pub const MAIN: &str = "main";
/// Further tabs with pages of the current server: `app-1`, `app-2`, …
pub const APP_WINDOW_PREFIX: &str = "app-";

/// The `main` tab or an `app-<n>` tab (the webviews that may show the server).
pub fn is_app_window(label: &str) -> bool {
    label == MAIN
        || label
            .strip_prefix(APP_WINDOW_PREFIX)
            .is_some_and(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The macOS title bar of every decorated window. Tauri's default (`Visible`) also sets
/// `NSWindowStyleMask::FullSizeContentView`, so the webview spans the title bar; since macOS 26
/// (Tahoe) that title bar is see-through and scrolled content shows behind the window title.
/// `Transparent` = no full-size content view: the webview starts below the title bar, which shows
/// the window's background colour (the system's, or the page's top for bundled windows,
/// `popup::title_bar_color`).
#[cfg(target_os = "macos")]
pub const TITLE_BAR: tauri::TitleBarStyle = tauri::TitleBarStyle::Transparent;

/// The commands remote content (the current server's origin only) may call.
pub const BRIDGE_PERMISSIONS: [&str; 4] = [
    "allow-brainitive-notify",
    "allow-brainitive-set-badge",
    "allow-brainitive-open-external",
    "allow-brainitive-sign-out",
];

/// The platform's default user agent + ` BrainitiveDesktop/<version>` (+ ` (MSStore)` in the
/// Microsoft Store build, BRAI-442). Wry can only replace the
/// user agent, so the default is spelled out (WKWebView's is frozen; WebView2's Chrome version
/// is a pinned approximation — see docs/SPIKE.md).
pub fn user_agent() -> String {
    let base = if cfg!(target_os = "macos") {
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"
    } else if cfg!(target_os = "windows") {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0"
    } else {
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)"
    };
    format!(
        "{base}{}",
        crate::msstore::webview_ua_marker(APP_VERSION, crate::msstore::STORE_BUILD)
    )
}

/// Where the bundled screens are served from: this platform's one Tauri origin
/// (`tauri://localhost` on macOS / Linux, `http://tauri.localhost` on Windows, see
/// [`links::bundled_origin`]) and the Vite dev server in `tauri dev`.
pub fn local_origins(app: &AppHandle) -> Vec<String> {
    let mut origins = vec![links::bundled_origin(cfg!(windows)).to_string()];
    if tauri::is_dev() {
        if let Some(dev) = &app.config().build.dev_url {
            origins.push(links::origin_key(dev));
        }
    }
    origins
}

pub fn local_url(app: &AppHandle) -> Url {
    if tauri::is_dev() {
        if let Some(dev) = &app.config().build.dev_url {
            return dev.clone();
        }
    }
    Url::parse(&format!("{}/", links::bundled_origin(cfg!(windows)))).expect("static url")
}

/// The main window (the OS window holding the top bar and the tabs).
pub fn main_window(app: &AppHandle) -> Option<Window> {
    app.get_window(MAIN)
}

/// The `main` tab's webview (the bundled screens while not signed in).
pub fn main_content(app: &AppHandle) -> Option<Webview> {
    app.get_webview(MAIN)
}

/// The tab the user sees: where menu actions, deep links and the tray's pages go.
pub fn active_tab(app: &AppHandle) -> Option<Webview> {
    let active = state(app).lock().tabs.active.clone();
    app.get_webview(&active)
        .filter(|w| is_app_window(w.label()))
        .or_else(|| main_content(app))
}

/// Every tab's webview (`main` + `app-<n>`), never the top bar.
pub fn content_webviews(app: &AppHandle) -> Vec<Webview> {
    app.webviews()
        .into_values()
        .filter(|w| is_app_window(w.label()))
        .collect()
}

/// Shows the main window in front. A user action (tray, dock, menu, deep link, a second
/// launch): it also ends a hidden start at login.
pub fn focus_main(app: &AppHandle) {
    state(app).lock().start_hidden = false;
    if let Some(w) = main_window(app) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(tab) = active_tab(app) {
            let _ = tab.set_focus();
        }
    }
}

/// After the app itself changed what the main window shows: in front, unless the app was
/// started hidden at login and the user has not opened it yet.
fn reveal_main(app: &AppHandle) {
    if !state(app).lock().start_hidden {
        focus_main(app);
    }
}

/// "Brainitive — Work" / "Brainitive — Brainitive Cloud" (the server's name, `servers.rs`): the
/// window title (taskbar, window switcher), the Server menu line and the top bar.
pub fn refresh_title(app: &AppHandle) {
    let (title, label) = {
        let s = state(app);
        let inner = s.lock();
        let offline = if crate::offline::is_offline(&inner) {
            " (offline)"
        } else {
            ""
        };
        match inner.current_origin() {
            Some(o) => {
                let name = inner.settings.label_of(&o);
                (format!("Brainitive — {name}{offline}"), format!("Connected to {name}"))
            }
            None => ("Brainitive".to_string(), "Not connected".to_string()),
        }
    };
    if let Some(w) = main_window(app) {
        let _ = w.set_title(&title);
    }
    let item = state(app).lock().server_menu_item.clone();
    if let Some(item) = item {
        let _ = item.set_text(label);
    }
    tabs::changed(app);
}

/// Gives the current server's origin IPC access to the four bridge commands — nothing else.
/// Tauri cannot remove a capability at runtime, so an earlier server keeps its entry; the
/// bridge commands therefore also check that the caller *is* the current server (bridge.rs).
pub fn allow_bridge_for(app: &AppHandle, origin: &ServerOrigin) -> tauri::Result<()> {
    let id = format!("remote-{}", server::hex(&origin.store_id()[..6]));
    {
        let s = state(app);
        let mut inner = s.lock();
        if !inner.capabilities.insert(origin.as_str().to_string()) {
            return Ok(());
        }
    }
    // never for bundled content (`local(false)`); the tabs only — by webview label, not by
    // window: the main window also holds the top bar
    let mut capability = CapabilityBuilder::new(id)
        .remote(origin.capability_pattern())
        .local(false)
        .webviews([MAIN.to_string(), format!("{APP_WINDOW_PREFIX}*")]);
    for permission in BRIDGE_PERMISSIONS {
        capability = capability.permission(permission);
    }
    app.add_capability(capability)
}

/// Shows a bundled screen in the `main` tab (the window keeps whatever data store it has). The
/// other tabs close: they belong to a session that just ended or is being switched.
pub fn show_local(app: &AppHandle, phase: Phase, notice: Option<crate::state::Notice>) {
    close_app_windows(app);
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.phase = phase;
        inner.notice = notice;
        if phase != Phase::Unreachable {
            inner.problem = None;
        }
        if phase != Phase::UpdateRequired {
            inner.update = None;
        }
        if !matches!(phase, Phase::Unreachable | Phase::Connecting) {
            inner.reconnect_attempts = 0;
        }
    }
    crate::tray::reset(app);
    tabs::changed(app);
    let local = local_url(app);
    match main_content(app) {
        Some(w) => {
            let on_local = w
                .url()
                .map(|u| links::origin_key(&u) == links::origin_key(&local))
                .unwrap_or(false);
            if on_local {
                let _ = app.emit_to(tauri::EventTarget::webview(MAIN), "desktop://changed", ());
            } else {
                grant_local(app, MAIN, &local);
                let _ = w.navigate(local);
            }
        }
        None => open_main(app, server::local_store_id(), WebviewUrl::App("index.html".into())),
    }
}

/// Opens `url` of `origin` in the `main` tab, rebuilding the window when it uses another
/// server's store; a freshly built window gets the server's saved tabs back.
pub fn enter_app(app: &AppHandle, origin: &ServerOrigin, url: Url) {
    if let Err(e) = allow_bridge_for(app, origin) {
        log::error!("could not add the remote capability: {e}");
        show_local(
            app,
            Phase::Unreachable,
            Some(crate::state::Notice::error(
                "The app could not set up access for this server.",
            )),
        );
        return;
    }
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.phase = Phase::InApp;
        inner.notice = None;
        inner.problem = None;
        inner.update = None;
        inner.reconnect_attempts = 0;
    }
    open_main(app, origin.store_id(), WebviewUrl::External(url.clone()));
    if let Some(tab) = state(app).lock().tabs.get_mut(MAIN) {
        tab.url = Some(url);
    }
    refresh_title(app);
    reveal_main(app);
    tabs::restore(app);
    // signed in: the tray polls now (and syncs the offline data); the quick captures that
    // waited are sent
    crate::offline::on_enter_app(app);
    crate::tray::reset(app);
    let handle = app.clone();
    tauri::async_runtime::spawn(async move { crate::capture::flush(&handle).await });
}

/// Navigates the `main` tab, or (re)builds the window when the data store differs.
pub fn open_main(app: &AppHandle, store: [u8; 16], url: WebviewUrl) {
    let existing = main_window(app);
    let current_store = state(app).lock().window_store;
    match existing {
        Some(_) if current_store == Some(store) => {
            let target = match &url {
                WebviewUrl::External(u) => u.clone(),
                _ => local_url(app),
            };
            grant_local(app, MAIN, &target);
            if let Some(main) = main_content(app) {
                let _ = main.navigate(target);
            }
            let _ = tabs::select(app, MAIN);
        }
        Some(w) => {
            close_app_windows(app);
            {
                let s = state(app);
                let mut inner = s.lock();
                inner.rebuild = Some(Rebuild { store, url });
                inner.rebuilding = true;
            }
            // continued in `on_main_destroyed` once the old webviews are gone
            let _ = w.destroy();
        }
        None => {
            if let Err(e) = build_main(app, store, url) {
                log::error!("could not create the main window: {e}");
            }
        }
    }
}

/// `RunEvent::WindowEvent { Destroyed }` of the main window.
pub fn on_main_destroyed(app: &AppHandle) {
    let rebuild = state(app).lock().rebuild.take();
    if let Some(Rebuild { store, url }) = rebuild {
        if let Err(e) = build_main(app, store, url) {
            log::error!("could not rebuild the main window: {e}");
        }
        state(app).lock().rebuilding = false;
        refresh_title(app);
        reveal_main(app);
        let handle = app.clone();
        tauri::async_runtime::spawn(async move { run_pending_wipes(&handle).await });
    } else {
        state(app).lock().window_store = None;
        // closed for good (not "keep running in the tray"): quit, the popups are only hidden
        app.exit(0);
    }
}

/// The main window: frameless on Windows / Linux (the top bar draws the controls), the title
/// bar's traffic lights over the top bar on macOS; the top bar, then the `main` tab.
fn build_main(app: &AppHandle, store: [u8; 16], url: WebviewUrl) -> tauri::Result<Window> {
    // launched at login with "start hidden": the window exists but stays in the tray
    let visible = !state(app).lock().start_hidden;
    let builder = tauri::window::WindowBuilder::new(app, MAIN)
        .title(window_title(app))
        .inner_size(1280.0, 840.0)
        .min_inner_size(720.0, 500.0)
        .visible(visible);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false).shadow(true);
    let window = builder.build()?;
    let _ = window.set_background_color(Some(bar_color(dark_theme(&window))));
    // Windows / Linux: the app menu opens from the top bar's buttons (its shortcuts still work)
    #[cfg(not(target_os = "macos"))]
    let _ = window.hide_menu();

    {
        let s = state(app);
        let mut inner = s.lock();
        inner.tabs = tabs::Tabs::default();
        inner.window_store = Some(store);
    }
    let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
    let dark = dark_theme(&window);
    let bar = window.add_child(
        topbar_builder(app).background_color(bar_color(dark)),
        LogicalPosition::new(0.0, 0.0),
        LogicalSize::new(size.width, BAR_HEIGHT),
    )?;
    crate::keys::attach(app, &bar);
    let main = window.add_child(
        tab_builder(app, MAIN, store, url)?.background_color(page_color(dark)),
        LogicalPosition::new(0.0, BAR_HEIGHT),
        LogicalSize::new(size.width, (size.height - BAR_HEIGHT).max(0.0)),
    )?;
    crate::keys::attach(app, &main);
    apply_zoom(app);
    Ok(window)
}

fn dark_theme(window: &Window) -> bool {
    window.theme().map(|t| t == tauri::Theme::Dark).unwrap_or(false)
}

/// What a tab paints before its page does: the web app's background, so a new tab never
/// flashes white in dark mode.
fn page_color(dark: bool) -> tauri::window::Color {
    if dark {
        tauri::window::Color(0x18, 0x21, 0x2a, 0xff)
    } else {
        tauri::window::Color(0xff, 0xff, 0xff, 0xff)
    }
}

/// The top bar's backdrop (`topbar.css` `--backdrop`).
fn bar_color(dark: bool) -> tauri::window::Color {
    if dark {
        tauri::window::Color(0x1c, 0x1c, 0x1c, 0xff)
    } else {
        tauri::window::Color(0xee, 0xee, 0xee, 0xff)
    }
}

/// The top bar: a bundled page with its own capability (`capabilities/topbar.json`) and the
/// bundled windows' navigation guard.
fn topbar_builder(app: &AppHandle) -> WebviewBuilder<Wry> {
    let nav = app.clone();
    let popup = app.clone();
    WebviewBuilder::new(TOPBAR, WebviewUrl::App("topbar.html".into()))
        .disable_drag_drop_handler()
        .on_navigation(
            move |url| match links::decide_local_navigation(url, &local_origins(&nav)) {
                Navigation::Allow => true,
                Navigation::External(url) => {
                    open_external(&nav, &url);
                    false
                }
                _ => false,
            },
        )
        .on_new_window(move |url, _| {
            if links::external_url(url.as_str()).is_some() {
                open_external(&popup, &url);
            }
            NewWindowResponse::Deny
        })
}

/// A tab (`main` or `app-<n>`) with the server's data store and every guard.
fn tab_builder(app: &AppHandle, label: &str, store: [u8; 16], url: WebviewUrl) -> tauri::Result<WebviewBuilder<Wry>> {
    let nav = app.clone();
    let nav_label = label.to_string();
    let load = app.clone();
    let popup = app.clone();
    let download = app.clone();
    let titles = app.clone();

    // a new webview: its first load is the app's own
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.committed.remove(label);
        inner.native_navigation.remove(label);
    }
    let builder = WebviewBuilder::new(label, url)
        .user_agent(&user_agent())
        // HTML5 drag & drop (boards, Gantt, file drops from Finder / Explorer) must reach the
        // page, not Tauri (BRAI-363); a file dropped elsewhere can't navigate (`file:` blocked)
        .disable_drag_drop_handler()
        .enable_clipboard_access()
        .on_navigation(move |url| on_navigation(&nav, &nav_label, url))
        .on_page_load(move |webview, payload| on_page_load(&load, &webview, payload.url(), payload.event()))
        .on_new_window(move |url, _features| {
            on_new_window(&popup, url);
            NewWindowResponse::Deny
        })
        .on_document_title_changed(move |webview, title| tabs::on_title(&titles, webview.label(), title))
        .on_download(move |_webview, event| crate::downloads::on_download(&download, event));

    #[cfg(target_os = "macos")]
    let builder = builder.data_store_identifier(store);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.data_directory(webview_data_dir(app, &store)?);
    Ok(builder)
}

/// Builds the webview of tab `label` (in the tab list already) with `url`, hidden, below the bar.
pub fn build_tab(app: &AppHandle, label: &str, url: Url) -> Result<(), String> {
    let window = main_window(app).ok_or("no main window")?;
    let store = state(app).lock().window_store.ok_or("no data store")?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let size = window.inner_size().map_err(|e| e.to_string())?.to_logical::<f64>(scale);
    let builder = tab_builder(app, label, store, WebviewUrl::External(url))
        .map_err(|e| e.to_string())?
        .background_color(page_color(dark_theme(&window)));
    let webview = window
        .add_child(
            builder,
            LogicalPosition::new(0.0, BAR_HEIGHT),
            LogicalSize::new(size.width, (size.height - BAR_HEIGHT).max(0.0)),
        )
        .map_err(|e| e.to_string())?;
    crate::keys::attach(app, &webview);
    let _ = webview.hide();
    let zoom = state(app).lock().settings.preferences.zoom;
    if (zoom - 1.0).abs() > f64::EPSILON {
        let _ = webview.set_zoom(zoom);
    }
    Ok(())
}

fn apply_zoom(app: &AppHandle) {
    let zoom = state(app).lock().settings.preferences.zoom;
    if (zoom - 1.0).abs() > f64::EPSILON {
        for w in content_webviews(app) {
            let _ = w.set_zoom(zoom);
        }
    }
}

fn window_title(app: &AppHandle) -> String {
    let s = state(app);
    let inner = s.lock();
    match inner.current_origin() {
        Some(o) => format!("Brainitive — {}", inner.settings.label_of(&o)),
        None => "Brainitive".to_string(),
    }
}

/// Whether the tabs show the server (not a bundled screen).
pub fn in_app(app: &AppHandle) -> bool {
    state(app).lock().phase == Phase::InApp
}

/// Whether the main window has the keyboard focus (the app is in front).
pub fn app_window_focused(app: &AppHandle) -> bool {
    main_window(app).is_some_and(|w| w.is_focused().unwrap_or(false))
}

/// Gives the keyboard back to the page when the main window is on screen (a popup closed).
pub fn refocus_active_tab(app: &AppHandle) -> bool {
    let Some(w) = main_window(app).filter(|w| w.is_visible().unwrap_or(false)) else {
        return false;
    };
    let _ = w.set_focus();
    if let Some(tab) = active_tab(app) {
        let _ = tab.set_focus();
    }
    true
}

/// Closes every tab but `main` (server switched, signed out, forgotten). Not saved: the server's
/// saved tabs come back with its next sign-in.
pub fn close_app_windows(app: &AppHandle) {
    for w in content_webviews(app) {
        if w.label() != MAIN {
            let _ = w.close();
        }
    }
    {
        let s = state(app);
        let mut inner = s.lock();
        // another session: its closed tabs don't come back either
        inner.tabs = tabs::Tabs::default();
        inner.committed.retain(|label, _| label == MAIN);
        inner.native_navigation.retain(|label, _| label == MAIN);
        inner.focused_app_window = None;
    }
    if let Some(main) = main_content(app) {
        let _ = main.show();
    }
    tabs::changed(app);
}

/// Opens a page of the current server in the active tab (menus, tray).
pub fn open_page(app: &AppHandle, path: &str) {
    let Some(origin) = state(app).lock().current_origin() else {
        focus_main(app);
        return;
    };
    // menus use fixed paths, the tray and timer links come from the server: never an auth page
    let Some(url) = links::resolve_deep_link_target(&origin, path) else {
        return;
    };
    navigate_to(app, &origin, url);
}

/// Shows `url` (a page of `origin`): in the active tab when signed in, else after the stored
/// sign-in is restored.
pub fn navigate_to(app: &AppHandle, origin: &ServerOrigin, url: Url) {
    if in_app(app) {
        if let Some(tab) = active_tab(app) {
            focus_main(app);
            let _ = tab.navigate(url);
            return;
        }
    }
    focus_main(app);
    let handle = app.clone();
    let origin = origin.clone();
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    tauri::async_runtime::spawn(async move {
        if state(&handle).session.has_stored_sign_in(&origin).await {
            crate::flows::restore(&handle, origin, Some(path)).await;
        }
    });
}

/// View → Actual Size / Zoom In / Zoom Out: one level for every tab, remembered.
pub fn zoom(app: &AppHandle, direction: i8) {
    let zoom = {
        let s = state(app);
        let mut inner = s.lock();
        let next = crate::settings::next_zoom(inner.settings.preferences.zoom, direction);
        inner.settings.preferences.zoom = next;
        inner.save_settings();
        next
    };
    for w in content_webviews(app) {
        let _ = w.set_zoom(zoom);
    }
}

#[cfg(not(target_os = "macos"))]
fn webview_data_dir(app: &AppHandle, store: &[u8; 16]) -> tauri::Result<PathBuf> {
    Ok(app
        .path()
        .app_local_data_dir()?
        .join("webview")
        .join(server::hex(store)))
}

/// A native transition to a bundled page (show a screen, revert a foreign page) announces itself:
/// the next navigation of `label` to a bundled page within a few seconds is the app's own.
fn grant_local(app: &AppHandle, label: &str, url: &Url) {
    if links::is_app_page(url, None, &local_origins(app)) && url.as_str() != "about:blank" {
        state(app)
            .lock()
            .native_navigation
            .insert(label.to_string(), Instant::now());
    }
}

/// Who navigates `label` to `url` (codex review 6): the window's committed page, or a fresh
/// native grant for a bundled target (consumed).
fn initiator_of(app: &AppHandle, label: &str, url: &Url) -> links::Initiator {
    let locals = local_origins(app);
    let s = state(app);
    let mut inner = s.lock();
    let target_local = links::is_app_page(url, None, &locals) && url.as_str() != "about:blank";
    let granted = target_local
        && inner
            .native_navigation
            .remove(label)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(10));
    links::initiator(inner.committed.get(label), granted, &locals)
}

fn on_navigation(app: &AppHandle, label: &str, url: &Url) -> bool {
    let instance = state(app).lock().current_origin();
    let initiator = initiator_of(app, label, url);
    let decision = links::decide_navigation(url, instance.as_ref(), &local_origins(app), initiator);
    if decision == Navigation::Block && initiator == links::Initiator::Content {
        log::info!("navigation by server content to a bundled page / deep link blocked");
    }
    match decision {
        Navigation::Allow => true,
        Navigation::Home => {
            // the logo link (→ `/`): stay in the app; "Sign out" calls brainitive_sign_out
            if let Some(origin) = instance {
                let target = origin.join("/dashboard");
                defer_navigate(app, label, target);
            }
            false
        }
        Navigation::Reauth => {
            if let Some(origin) = instance {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move { crate::flows::on_web_session_lost(&handle, origin).await });
            }
            false
        }
        Navigation::Foreign(url) => {
            // Windows reports top-level navigations only → open outside right away. macOS and
            // Linux also report iframes (YouTube, Figma embeds), and wry 0.57.1 hands this handler
            // the URL only (no WKNavigationAction.targetFrame / WebKitGTK frame name; re-checked
            // for the codex review, finding 7: still so in the pinned tauri 2.12.5 / wry 0.57.1),
            // so a top-level load can't be told apart here: allow, the page-load guard reverts a
            // top-level one as soon as it commits (`links::decide_page_load`, BRAI-377). Until
            // then the foreign page has no IPC: the bridge capability is `<origin>/*` and every
            // bridge command checks the current origin again (`bridge::is_bridge_caller`).
            if cfg!(windows) {
                open_external(app, &url);
                false
            } else {
                true
            }
        }
        Navigation::External(url) => {
            open_external(app, &url);
            false
        }
        Navigation::DeepLink(url) => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move { crate::flows::handle_deep_link(&handle, url).await });
            false
        }
        Navigation::Block => false,
    }
}

fn on_page_load(app: &AppHandle, window: &Webview, url: &Url, event: PageLoadEvent) {
    let instance = {
        let s = state(app);
        let mut inner = s.lock();
        // page-load events are main-frame only and come after the commit on every platform
        inner.committed.insert(window.label().to_string(), url.clone());
        inner.current_origin()
    };
    let allowed = links::is_app_page(url, instance.as_ref(), &local_origins(app));
    if event == PageLoadEvent::Finished && window.label() == MAIN && allowed {
        // the bundled screens are the first paint (cached shell); the server's page is "loaded"
        let remote = instance.as_ref().is_some_and(|o| o.contains(url));
        crate::startup::mark(if remote {
            "remote page loaded"
        } else {
            "bundled shell loaded"
        });
    }
    let started = event == PageLoadEvent::Started;
    match links::decide_page_load(url, instance.as_ref(), &local_origins(app), started) {
        links::PageLoad::Revert { outside } => {
            // The main frame committed to another site (a link the navigation handler had to let
            // through because macOS / Linux report iframes there too, or a server redirect). It
            // never gets IPC (see capabilities); stop it and go back at once — on `Started`, not
            // only after it finished loading — and open it in the browser instead.
            let _ = window.eval("window.stop()");
            let back = {
                let last_good = state(app).lock().last_good.clone();
                links::revert_target(
                    window.label() == MAIN,
                    last_good.as_ref(),
                    instance.as_ref(),
                    &local_url(app),
                )
            };
            defer_navigate(app, window.label(), back);
            if let Some(outside) = outside {
                open_external(app, &outside);
            }
        }
        links::PageLoad::Remember => {
            if window.label() == MAIN {
                state(app).lock().last_good = Some(url.clone());
            }
            tabs::on_tab_page(app, window.label(), url);
        }
        links::PageLoad::Keep => {}
    }
}

fn on_new_window(app: &AppHandle, url: Url) {
    let instance = state(app).lock().current_origin();
    match instance {
        // "open in new tab" of a page of the server (an attachment, a link with target=_blank):
        // a new tab; the active tab when that isn't possible (not on this handler's thread)
        Some(origin) if origin.contains(&url) => {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = tabs::open(&handle, url.clone(), true) {
                    log::info!("new tab refused ({e}), navigating instead");
                    if let Some(w) = active_tab(&handle) {
                        let _ = w.navigate(url);
                    }
                }
            });
        }
        _ => {
            if links::external_url(url.as_str()).is_some() {
                open_external(app, &url);
            }
        }
    }
}

/// Opens an http(s) / mailto link in the system browser / mail client (once per 2 s per URL:
/// the navigation and page-load handlers can both see the same link).
pub fn open_external(app: &AppHandle, url: &Url) {
    let Some(url) = links::external_url(url.as_str()) else {
        return;
    };
    {
        let s = state(app);
        let mut inner = s.lock();
        if matches!(&inner.last_external, Some((u, at)) if *u == url && at.elapsed() < Duration::from_secs(2)) {
            return;
        }
        inner.last_external = Some((url.clone(), Instant::now()));
    }
    if let Err(e) = app.opener().open_url(url.as_str(), None::<&str>) {
        log::warn!("could not open {url}: {e}");
    }
}

/// Navigation from inside a webview handler must not re-enter the webview synchronously.
fn defer_navigate(app: &AppHandle, label: &str, url: Url) {
    grant_local(app, label, &url);
    let handle = app.clone();
    let label = label.to_string();
    tauri::async_runtime::spawn(async move {
        if let Some(w) = handle.get_webview(&label) {
            let _ = w.navigate(url);
        }
    });
}

/// Deletes the webview data of forgotten servers that no window uses any more.
pub async fn run_pending_wipes(app: &AppHandle) {
    let (pending, in_use) = {
        let s = state(app);
        let inner = s.lock();
        (
            inner.settings.pending_wipes.clone(),
            inner.window_store.map(|s| server::hex(&s)),
        )
    };
    let mut done = Vec::new();
    for id in pending {
        if Some(&id) == in_use.as_ref() {
            continue;
        }
        if wipe_store(app, &id).await {
            done.push(id);
        }
    }
    if !done.is_empty() {
        let s = state(app);
        let mut inner = s.lock();
        inner.settings.pending_wipes.retain(|w| !done.contains(w));
        inner.save_settings();
    }
}

#[cfg(target_os = "macos")]
async fn wipe_store(app: &AppHandle<Wry>, id: &str) -> bool {
    let Some(bytes) = unhex16(id) else { return true };
    let known = app.fetch_data_store_identifiers().await.unwrap_or_default();
    if !known.contains(&bytes) {
        return true;
    }
    match app.remove_data_store(bytes).await {
        Ok(()) => true,
        Err(e) => {
            log::warn!("webview data store {id} not removed yet: {e}");
            false
        }
    }
}

#[cfg(not(target_os = "macos"))]
async fn wipe_store(app: &AppHandle<Wry>, id: &str) -> bool {
    let Some(bytes) = unhex16(id) else { return true };
    let Ok(dir) = webview_data_dir(app, &bytes) else {
        return false;
    };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        // WebView2 can hold files for a moment after the window closed: next launch
        Err(_) => false,
    }
}

fn unhex16(id: &str) -> Option<[u8; 16]> {
    if id.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, chunk) in id.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_carries_the_app_marker() {
        let marker = if cfg!(feature = "msstore") {
            format!(" BrainitiveDesktop/{APP_VERSION} (MSStore)")
        } else {
            format!(" BrainitiveDesktop/{APP_VERSION}")
        };
        assert!(user_agent().ends_with(&marker), "{}", user_agent());
        assert!(user_agent().starts_with("Mozilla/5.0 ("));
    }

    #[test]
    fn app_window_labels() {
        assert!(is_app_window("main"));
        assert!(is_app_window("app-1"));
        assert!(is_app_window("app-123456"));
        for bad in [
            "app-",
            "app-x",
            "app-1-2",
            "app-1234567",
            "capture",
            "switcher",
            "settings",
            "mainx",
            "",
        ] {
            assert!(!is_app_window(bad), "{bad}");
        }
    }

    #[test]
    fn hex_round_trip() {
        let id = ServerOrigin::cloud().store_id();
        assert_eq!(unhex16(&server::hex(&id)), Some(id));
        assert_eq!(unhex16("zz"), None);
    }
}
