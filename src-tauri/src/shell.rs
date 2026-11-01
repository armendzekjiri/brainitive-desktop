//! The main window: one webview that shows the bundled screens or the current server, with
//! its own webview data store per server, the navigation guard, the remote IPC capability,
//! downloads and the user agent (BRAI-350 / BRAI-351).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tauri::ipc::CapabilityBuilder;
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Wry};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::links::{self, Navigation};
use crate::server::{self, ServerOrigin};
use crate::state::{state, Phase, Rebuild};
use crate::version::APP_VERSION;

pub const MAIN: &str = "main";

/// The commands remote content (the current server's origin only) may call.
pub const BRIDGE_PERMISSIONS: [&str; 3] = [
    "allow-brainitive-notify",
    "allow-brainitive-set-badge",
    "allow-brainitive-open-external",
];

/// The platform's default user agent + ` BrainitiveDesktop/<version>`. Wry can only replace the
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
    format!("{base} BrainitiveDesktop/{APP_VERSION}")
}

/// Where the bundled screens are served from (`tauri://localhost`, `http://tauri.localhost`
/// on Windows, the Vite dev server in `tauri dev`).
pub fn local_origins(app: &AppHandle) -> Vec<String> {
    let mut origins = vec![
        "tauri://localhost".to_string(),
        "http://tauri.localhost".to_string(),
        "https://tauri.localhost".to_string(),
    ];
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
    let base = if cfg!(windows) {
        "http://tauri.localhost/"
    } else {
        "tauri://localhost/"
    };
    Url::parse(base).expect("static url")
}

pub fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(MAIN)
}

pub fn focus_main(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// "Brainitive — Cloud" / "Brainitive — brainitive.example.com", also in the Server menu.
pub fn refresh_title(app: &AppHandle) {
    let (title, label) = {
        let s = state(app);
        let inner = s.lock();
        match inner.current_origin() {
            Some(o) => (
                format!("Brainitive — {}", o.display_name()),
                format!("Connected to {}", o.display_name()),
            ),
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
}

/// Gives the current server's origin IPC access to the three bridge commands — nothing else.
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
    let mut capability = CapabilityBuilder::new(id)
        .remote(origin.capability_pattern())
        .window(MAIN)
        .webview(MAIN);
    for permission in BRIDGE_PERMISSIONS {
        capability = capability.permission(permission);
    }
    app.add_capability(capability)
}

/// Shows a bundled screen (the main window keeps whatever data store it has).
pub fn show_local(app: &AppHandle, phase: Phase, notice: Option<crate::state::Notice>) {
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.phase = phase;
        inner.notice = notice;
    }
    let local = local_url(app);
    match main_window(app) {
        Some(w) => {
            let on_local = w
                .url()
                .map(|u| links::origin_key(&u) == links::origin_key(&local))
                .unwrap_or(false);
            if on_local {
                let _ = tauri::Emitter::emit_to(&w, MAIN, "desktop://changed", ());
            } else {
                let _ = w.navigate(local);
            }
        }
        None => open_main(app, server::local_store_id(), WebviewUrl::App("index.html".into())),
    }
}

/// Opens `url` of `origin` in the main window, rebuilding it when it uses another server's store.
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
    }
    open_main(app, origin.store_id(), WebviewUrl::External(url));
    refresh_title(app);
    focus_main(app);
}

/// Navigates the main window, or (re)builds it when the data store differs.
pub fn open_main(app: &AppHandle, store: [u8; 16], url: WebviewUrl) {
    let existing = main_window(app);
    let current_store = state(app).lock().window_store;
    match existing {
        Some(w) if current_store == Some(store) => {
            let target = match &url {
                WebviewUrl::External(u) => u.clone(),
                _ => local_url(app),
            };
            let _ = w.navigate(target);
        }
        Some(w) => {
            {
                let s = state(app);
                let mut inner = s.lock();
                inner.rebuild = Some(Rebuild { store, url });
                inner.rebuilding = true;
            }
            // continued in `on_main_destroyed` once the old webview is gone
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
        focus_main(app);
        let handle = app.clone();
        tauri::async_runtime::spawn(async move { run_pending_wipes(&handle).await });
    } else {
        state(app).lock().window_store = None;
    }
}

fn build_main(app: &AppHandle, store: [u8; 16], url: WebviewUrl) -> tauri::Result<WebviewWindow> {
    let nav = app.clone();
    let load = app.clone();
    let popup = app.clone();
    let download = app.clone();

    let builder = WebviewWindowBuilder::new(app, MAIN, url)
        .title("Brainitive")
        .inner_size(1280.0, 840.0)
        .min_inner_size(900.0, 600.0)
        .user_agent(&user_agent())
        // HTML5 drag & drop (boards, Gantt, file drops) must reach the page, not Tauri
        .disable_drag_drop_handler()
        .enable_clipboard_access()
        .on_navigation(move |url| on_navigation(&nav, url))
        .on_page_load(move |window, payload| on_page_load(&load, &window, payload.url(), payload.event()))
        .on_new_window(move |url, _features| {
            on_new_window(&popup, url);
            NewWindowResponse::Deny
        })
        .on_download(move |_webview, event| on_download(&download, event));

    #[cfg(target_os = "macos")]
    let builder = builder.data_store_identifier(store);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.data_directory(webview_data_dir(app, &store)?);

    let window = builder.build()?;
    state(app).lock().window_store = Some(store);
    Ok(window)
}

#[cfg(not(target_os = "macos"))]
fn webview_data_dir(app: &AppHandle, store: &[u8; 16]) -> tauri::Result<PathBuf> {
    Ok(app
        .path()
        .app_local_data_dir()?
        .join("webview")
        .join(server::hex(store)))
}

fn on_navigation(app: &AppHandle, url: &Url) -> bool {
    let instance = state(app).lock().current_origin();
    match links::decide_navigation(url, instance.as_ref(), &local_origins(app)) {
        Navigation::Allow => true,
        Navigation::Home => {
            // the logo link or the web app's "Log out" (→ `/`): stay in the app
            state(app).lock().home_at = Some(Instant::now());
            if let Some(origin) = instance {
                let target = origin.join("/dashboard");
                defer_navigate(app, target);
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
            // Linux also report iframes (YouTube, Figma embeds): allow, the page-load guard
            // catches a top-level load.
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

fn on_page_load(app: &AppHandle, window: &WebviewWindow, url: &Url, event: PageLoadEvent) {
    let instance = state(app).lock().current_origin();
    let allowed = links::is_app_page(url, instance.as_ref(), &local_origins(app));
    match (event, allowed) {
        (PageLoadEvent::Started, false) => {
            // a top-level load of another site: cancel it in the still-current document and
            // open it in the browser instead (it never gets IPC either way: see capabilities)
            let _ = window.eval("window.stop()");
            if links::external_url(url.as_str()).is_some() {
                open_external(app, url);
            }
        }
        (PageLoadEvent::Finished, false) => {
            // window.stop() came too late (e.g. a server redirect): go back
            let back = state(app).lock().last_good.clone().unwrap_or_else(|| local_url(app));
            let _ = window.navigate(back);
        }
        (PageLoadEvent::Finished, true) => {
            if instance.is_some_and(|o| o.contains(url)) {
                state(app).lock().last_good = Some(url.clone());
            }
        }
        (PageLoadEvent::Started, true) => {}
    }
}

fn on_new_window(app: &AppHandle, url: Url) {
    let instance = state(app).lock().current_origin();
    match instance {
        // "open in new tab" of a page of the server: one window for now → navigate it
        Some(origin) if origin.contains(&url) => defer_navigate(app, url),
        _ => {
            if links::external_url(url.as_str()).is_some() {
                open_external(app, &url);
            }
        }
    }
}

fn on_download(app: &AppHandle, event: DownloadEvent<'_>) -> bool {
    match event {
        DownloadEvent::Requested { destination, .. } => {
            let name = destination
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "download".to_string());
            if let Ok(dir) = app.path().download_dir() {
                *destination = unique_path(dir, &name);
            }
            true
        }
        DownloadEvent::Finished { success, .. } => {
            let (title, body) = if success {
                ("Download finished", "The file is in your Downloads folder.")
            } else {
                ("Download failed", "The file could not be downloaded.")
            };
            crate::bridge::show_notification(app, title, body, None);
            true
        }
        _ => true,
    }
}

/// `name.ext`, `name (1).ext`, … — never overwrites an existing file.
pub fn unique_path(dir: PathBuf, name: &str) -> PathBuf {
    // keep only the last path component: a server-suggested "../x" must stay in the folder
    let name = std::path::Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".into());
    let candidate = dir.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    (1..10_000)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(candidate)
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
fn defer_navigate(app: &AppHandle, url: Url) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(w) = main_window(&handle) {
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
        assert!(user_agent().ends_with(&format!(" BrainitiveDesktop/{APP_VERSION}")));
        assert!(user_agent().starts_with("Mozilla/5.0 ("));
    }

    #[test]
    fn hex_round_trip() {
        let id = ServerOrigin::cloud().store_id();
        assert_eq!(unhex16(&server::hex(&id)), Some(id));
        assert_eq!(unhex16("zz"), None);
    }

    #[test]
    fn downloads_never_overwrite_or_escape_the_folder() {
        let dir = std::env::temp_dir().join(format!("brainitive-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(dir.clone(), "a.pdf"), dir.join("a.pdf"));
        std::fs::write(dir.join("a.pdf"), b"x").unwrap();
        assert_eq!(unique_path(dir.clone(), "a.pdf"), dir.join("a (1).pdf"));
        assert_eq!(unique_path(dir.clone(), "../../etc/a.pdf"), dir.join("a (1).pdf"));
        std::fs::write(dir.join("README"), b"x").unwrap();
        assert_eq!(unique_path(dir.clone(), "README"), dir.join("README (1)"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
