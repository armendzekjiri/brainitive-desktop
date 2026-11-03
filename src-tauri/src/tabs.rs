//! Tabs of the main window and its top bar.
//!
//! The main window is one OS window with several webviews (Tauri's multi-webview):
//! - `topbar`: the bundled top bar (`topbar.html`) — app menus, tabs, the server, window controls;
//! - `main`: the first tab, and the bundled screens (connect, sign-in) while not signed in;
//! - `app-<n>`: further tabs, pages of the current server.
//!
//! Every tab is an app webview with the server's data store and all guards (`shell.rs`); only the
//! active one is shown. The top bar is a bundled page under the same CSP as the other bundled
//! windows; it gets its own commands (`capabilities/topbar.json`), each checking the caller first
//! ([`topbar`]). It never sees a token or a URL: tabs are labels, titles and a kind for the icon.
//!
//! `main` always exists while the window does (the bridge takes notifications and the badge from
//! it, the bundled screens run in it): closing the `main` tab while others are open moves the
//! neighbour's page into it instead ([`plan_close`]).

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Webview};
use url::Url;

use crate::links;
use crate::shell::{self, APP_WINDOW_PREFIX, MAIN};
use crate::state::{state, Phase};

/// The label of the top bar's webview (never an app window: `shell::is_app_window` is false).
pub const TOPBAR: &str = "topbar";
/// Height of the top bar in logical pixels: the title bar row with the tabs (40) and the menu
/// row (36), as in Windows 11 Notepad.
pub const BAR_HEIGHT: f64 = 76.0;
/// At most this many tabs at once.
pub const MAX_TABS: usize = 30;
/// At most this many tabs are restored per server.
pub const MAX_RESTORED: usize = 12;
/// At most this many closed tabs can be reopened (Ctrl+Shift+T).
pub const MAX_CLOSED: usize = 10;
/// The top bar's event (sent to the top bar only).
pub const CHANGED: &str = "topbar://changed";

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub label: String,
    /// The page's `<title>` (the server's text: shown with `textContent` only).
    pub title: String,
    /// The page the tab shows (or will load when first selected).
    pub url: Option<Url>,
    /// Whether its webview exists (restored tabs are built when first selected).
    pub built: bool,
}

impl Tab {
    pub fn new(label: &str, url: Option<Url>, built: bool) -> Self {
        Self {
            label: label.to_string(),
            title: String::new(),
            url,
            built,
        }
    }
}

/// The tabs of the main window, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Tabs {
    pub list: Vec<Tab>,
    pub active: String,
    /// The saved tabs of this server were put back (once per window build).
    pub restored: bool,
    /// The restored tab to show once the `main` tab has its web session (see [`restore`]).
    pub pending_active: Option<String>,
    /// Pages of tabs closed in this session, last closed last (Ctrl+Shift+T reopens them).
    pub closed: Vec<Url>,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            list: vec![Tab::new(MAIN, None, true)],
            active: MAIN.to_string(),
            restored: false,
            pending_active: None,
            closed: Vec::new(),
        }
    }
}

impl Tabs {
    /// Remembers a closed tab's page for Ctrl+Shift+T (bounded).
    pub fn remember_closed(&mut self, url: Option<Url>) {
        if let Some(url) = url {
            self.closed.push(url);
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
    }

    pub fn index(&self, label: &str) -> Option<usize> {
        self.list.iter().position(|t| t.label == label)
    }

    pub fn get_mut(&mut self, label: &str) -> Option<&mut Tab> {
        self.list.iter_mut().find(|t| t.label == label)
    }

    /// The tab after (`step` 1) or before (`step` -1) the active one, wrapping around.
    pub fn neighbour(&self, step: i32) -> Option<String> {
        let n = self.list.len() as i32;
        if n < 2 {
            return None;
        }
        let i = self.index(&self.active)? as i32;
        Some(self.list[((i + step).rem_euclid(n)) as usize].label.clone())
    }

    /// Moves `label` to position `to` (clamped).
    pub fn move_to(&mut self, label: &str, to: usize) -> bool {
        let Some(from) = self.index(label) else { return false };
        let tab = self.list.remove(from);
        let to = to.min(self.list.len());
        self.list.insert(to, tab);
        true
    }
}

/// What closing a tab does.
#[derive(Debug, Clone, PartialEq)]
pub enum ClosePlan {
    /// The only tab: nothing.
    Keep,
    /// Destroy `remove`; `activate` becomes the active tab (`None`: the active one stays).
    Remove { remove: String, activate: Option<String> },
    /// The `main` tab with others open: `main` loads `from`'s page (`url`) and takes its place and
    /// title; `from` is destroyed.
    MoveIntoMain { from: String },
}

/// Closing `label` (pure; [`close`] carries it out).
pub fn plan_close(tabs: &Tabs, label: &str) -> ClosePlan {
    let Some(i) = tabs.index(label) else {
        return ClosePlan::Keep;
    };
    if tabs.list.len() < 2 {
        return ClosePlan::Keep;
    }
    // the tab the user gets next: the right neighbour, else the left one
    let next = if i + 1 < tabs.list.len() { i + 1 } else { i - 1 };
    let next_label = tabs.list[next].label.clone();
    if label == MAIN {
        if tabs.active == MAIN {
            return ClosePlan::MoveIntoMain { from: next_label };
        }
        // the main tab is in the background: the active tab's page moves into it
        return ClosePlan::MoveIntoMain {
            from: tabs.active.clone(),
        };
    }
    ClosePlan::Remove {
        remove: label.to_string(),
        activate: (tabs.active == label).then_some(next_label),
    }
}

/// Applies a [`ClosePlan::MoveIntoMain`] to the list: `main` takes `from`'s place, url and title.
pub fn move_into_main(tabs: &mut Tabs, from: &str) {
    let Some(f) = tabs.index(from) else { return };
    let donor = tabs.list.remove(f);
    let Some(m) = tabs.index(MAIN) else { return };
    let mut main = tabs.list.remove(m);
    main.url = donor.url;
    main.title = donor.title;
    let at = if m < f { f - 1 } else { f };
    tabs.list.insert(at.min(tabs.list.len()), main);
    tabs.active = MAIN.to_string();
}

/// What the tab's icon shows, from its path (the web app's routes).
pub fn kind_of(url: Option<&Url>) -> &'static str {
    let Some(url) = url else { return "page" };
    let path = url.path().to_ascii_lowercase();
    let has = |s: &str| path.split('/').any(|seg| seg == s);
    if has("boards") || has("board") || has("backlog") || has("sprints") {
        "board"
    } else if has("docs") || has("pages") || has("doc") || has("page") {
        "doc"
    } else if has("whiteboards") || has("whiteboard") {
        "whiteboard"
    } else if has("plans") || has("plan") || has("gantt") {
        "plan"
    } else if has("calendar") {
        "calendar"
    } else if has("notifications") {
        "bell"
    } else if has("my-work") || has("tasks") || has("task-lists") {
        "check"
    } else if has("captures") || has("inbox") {
        "inbox"
    } else if has("snippets") || has("bookmarks") {
        "bookmark"
    } else if path == "/" || has("dashboard") {
        "home"
    } else {
        "page"
    }
}

/// A tab's title: the page's, else a readable name from its path.
pub fn title_of(tab: &Tab) -> String {
    let t: String = tab
        .title
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(120)
        .collect();
    if !t.is_empty() {
        // "Sprint 12 · Brainitive" → "Sprint 12"
        for sep in [" · Brainitive", " - Brainitive", " | Brainitive", " — Brainitive"] {
            if let Some(s) = t.strip_suffix(sep) {
                if !s.trim().is_empty() {
                    return s.trim().to_string();
                }
            }
        }
        return t;
    }
    match tab
        .url
        .as_ref()
        .and_then(|u| u.path_segments()?.rfind(|s| !s.is_empty()).map(str::to_string))
    {
        Some(seg) => {
            let seg = seg.replace(['-', '_'], " ");
            let mut c = seg.chars();
            match c.next() {
                Some(f) if !f.is_ascii_digit() => f.to_uppercase().chain(c).collect(),
                _ => "Brainitive".to_string(),
            }
        }
        None => "Brainitive".to_string(),
    }
}

/// The saved tabs of one server: pages as `path?query` of that server.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SavedTabs {
    pub pages: Vec<String>,
    /// Index of the active tab.
    #[serde(default)]
    pub active: usize,
    /// Index of the `main` tab.
    #[serde(default)]
    pub main: usize,
}

/// The tabs worth saving: pages of `origin` (no fragment; auth pages are filtered on restore).
pub fn to_saved(tabs: &Tabs, origin: &crate::server::ServerOrigin) -> SavedTabs {
    let mut saved = SavedTabs::default();
    for tab in &tabs.list {
        let Some(url) = tab.url.as_ref().filter(|u| origin.contains(u)) else {
            continue;
        };
        let mut page = url.path().to_string();
        if let Some(q) = url.query() {
            page.push('?');
            page.push_str(q);
        }
        if tab.label == tabs.active {
            saved.active = saved.pages.len();
        }
        if tab.label == MAIN {
            saved.main = saved.pages.len();
        }
        saved.pages.push(page);
    }
    saved
}

// ---------------------------------------------------------------------------------------------
// the top bar's view

#[derive(Debug, Clone, Serialize)]
pub struct TabView {
    pub id: String,
    pub title: String,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopbarView {
    /// "windows" | "macos" | "linux": window controls and menus differ.
    pub platform: &'static str,
    /// Signed in: tabs, navigation and the server are shown.
    pub in_app: bool,
    pub server: Option<String>,
    pub offline: bool,
    pub tabs: Vec<TabView>,
    pub active: String,
    pub maximized: bool,
    /// The app menus shown as buttons (none on macOS: it has the menu bar).
    pub menus: Vec<(&'static str, &'static str)>,
}

pub const PLATFORM: &str = if cfg!(target_os = "macos") {
    "macos"
} else if cfg!(windows) {
    "windows"
} else {
    "linux"
};

/// The menus of the top bar: id (`menu::TOPBAR_MENUS`) and text.
pub fn menus() -> Vec<(&'static str, &'static str)> {
    if cfg!(target_os = "macos") {
        Vec::new()
    } else {
        crate::menu::TOPBAR_MENUS.to_vec()
    }
}

pub fn view(app: &AppHandle) -> TopbarView {
    let maximized = shell::main_window(app)
        .and_then(|w| w.is_maximized().ok())
        .unwrap_or(false);
    let s = state(app);
    let inner = s.lock();
    let in_app = inner.phase == Phase::InApp;
    TopbarView {
        platform: PLATFORM,
        in_app,
        server: inner.current_origin().map(|o| inner.settings.label_of(&o)),
        offline: crate::offline::is_offline(&inner),
        tabs: if in_app {
            inner
                .tabs
                .list
                .iter()
                .map(|t| TabView {
                    id: t.label.clone(),
                    title: title_of(t),
                    kind: kind_of(t.url.as_ref()),
                })
                .collect()
        } else {
            Vec::new()
        },
        active: inner.tabs.active.clone(),
        maximized,
        menus: menus(),
    }
}

/// Tells the top bar to ask for its view again.
pub fn changed(app: &AppHandle) {
    let _ = app.emit_to(TOPBAR, CHANGED, ());
}

// ---------------------------------------------------------------------------------------------
// tab operations (Rust side; menus, the top bar and the guards call them)

/// The webview of `label` when it is a tab.
fn tab_webview(app: &AppHandle, label: &str) -> Option<Webview> {
    shell::is_app_window(label).then(|| app.get_webview(label)).flatten()
}

/// Lays out the top bar and the tabs after a resize: the bar on top, every tab below it.
pub fn layout(app: &AppHandle) {
    let Some(window) = shell::main_window(app) else { return };
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    if let Some(bar) = app.get_webview(TOPBAR) {
        let _ = bar.set_bounds(tauri::Rect {
            position: tauri::LogicalPosition::new(0.0, 0.0).into(),
            size: tauri::LogicalSize::new(size.width, BAR_HEIGHT).into(),
        });
    }
    let content = tauri::Rect {
        position: tauri::LogicalPosition::new(0.0, BAR_HEIGHT).into(),
        size: tauri::LogicalSize::new(size.width, (size.height - BAR_HEIGHT).max(0.0)).into(),
    };
    for w in shell::content_webviews(app) {
        let _ = w.set_bounds(content);
    }
}

/// Shows `label` and hides the other tabs (building a restored tab on first use).
pub fn select(app: &AppHandle, label: &str) -> Result<(), String> {
    let pending = {
        let s = state(app);
        let mut inner = s.lock();
        let tab = inner.tabs.get_mut(label).ok_or("no such tab")?;
        let pending = (!tab.built).then(|| tab.url.clone()).flatten();
        tab.built = true;
        inner.tabs.active = label.to_string();
        inner.focused_app_window = Some(label.to_string());
        pending
    };
    if let Some(url) = pending {
        if let Err(e) = shell::build_tab(app, label, url) {
            log::warn!("could not open the tab: {e}");
            remove_tab_entry(app, label);
            return Err(e);
        }
    }
    // the new tab first, then the others away: never a frame with no page (an empty flash)
    let webviews = shell::content_webviews(app);
    if let Some(w) = webviews.iter().find(|w| w.label() == label) {
        let _ = w.show();
        let _ = w.set_focus();
    }
    for w in webviews.iter().filter(|w| w.label() != label) {
        let _ = w.hide();
    }
    persist(app);
    changed(app);
    Ok(())
}

fn remove_tab_entry(app: &AppHandle, label: &str) {
    let s = state(app);
    let mut inner = s.lock();
    if let Some(i) = inner.tabs.index(label) {
        inner.tabs.list.remove(i);
    }
    if inner.tabs.active == label {
        inner.tabs.active = MAIN.to_string();
    }
}

/// A page of the current server in a new tab next to the active one.
pub fn open(app: &AppHandle, url: Url, activate: bool) -> Result<String, String> {
    let label = {
        let s = state(app);
        let mut inner = s.lock();
        let origin = inner.current_origin().ok_or("not connected")?;
        if inner.phase != Phase::InApp {
            return Err("not signed in".into());
        }
        if !origin.contains(&url) {
            return Err("not a page of this server".into());
        }
        if inner.window_store != Some(origin.store_id()) {
            return Err("the main window is not on this server".into());
        }
        if inner.tabs.list.len() >= MAX_TABS {
            return Err("too many tabs".into());
        }
        let label = next_label(&mut inner);
        let at = inner
            .tabs
            .index(&inner.tabs.active.clone())
            .map(|i| i + 1)
            .unwrap_or(inner.tabs.list.len());
        inner.tabs.list.insert(at, Tab::new(&label, Some(url.clone()), true));
        label
    };
    if let Err(e) = shell::build_tab(app, &label, url) {
        remove_tab_entry(app, &label);
        return Err(e);
    }
    layout(app);
    if activate {
        select(app, &label)?;
    } else {
        if let Some(w) = tab_webview(app, &label) {
            let _ = w.hide();
        }
        persist(app);
        changed(app);
    }
    Ok(label)
}

fn next_label(inner: &mut crate::state::Inner) -> String {
    loop {
        inner.app_window_seq = (inner.app_window_seq % 999_999) + 1;
        let label = format!("{APP_WINDOW_PREFIX}{}", inner.app_window_seq);
        if inner.tabs.index(&label).is_none() {
            return label;
        }
    }
}

/// A new tab on the dashboard (⌘T / Ctrl+T, the + button).
pub fn open_new(app: &AppHandle) {
    let origin = state(app).lock().current_origin();
    if let Some(origin) = origin {
        if let Err(e) = open(app, origin.join("/dashboard"), true) {
            log::info!("no new tab: {e}");
        }
    }
}

/// Closes a tab (see [`plan_close`]).
pub fn close(app: &AppHandle, label: &str) {
    let plan = {
        let s = state(app);
        let mut inner = s.lock();
        let plan = plan_close(&inner.tabs, label);
        if plan != ClosePlan::Keep {
            // the page it shows now, else the one it was opened with
            let page = inner.committed.get(label).cloned().or_else(|| {
                inner
                    .tabs
                    .list
                    .iter()
                    .find(|t| t.label == label)
                    .and_then(|t| t.url.clone())
            });
            let page = page.filter(|u| inner.current_origin().is_some_and(|o| o.contains(u)));
            inner.tabs.remember_closed(page);
        }
        plan
    };
    match plan {
        ClosePlan::Keep => {}
        ClosePlan::Remove { remove, activate } => {
            remove_tab_entry(app, &remove);
            if let Some(w) = tab_webview(app, &remove) {
                let _ = w.close();
            }
            let next = activate.unwrap_or_else(|| state(app).lock().tabs.active.clone());
            let _ = select(app, &next);
        }
        ClosePlan::MoveIntoMain { from } => {
            let url = {
                let s = state(app);
                let mut inner = s.lock();
                // the page the donor shows now (its committed page), else the one it was opened with
                let live = inner.committed.get(&from).cloned();
                if let (Some(live), Some(t)) = (live, inner.tabs.get_mut(&from)) {
                    t.url = Some(live);
                }
                let url = inner
                    .tabs
                    .list
                    .iter()
                    .find(|t| t.label == from)
                    .and_then(|t| t.url.clone());
                move_into_main(&mut inner.tabs, &from);
                inner.committed.remove(&from);
                url
            };
            if let Some(w) = tab_webview(app, &from) {
                let _ = w.close();
            }
            if let (Some(url), Some(main)) = (url, shell::main_content(app)) {
                let _ = main.navigate(url);
            }
            let _ = select(app, MAIN);
        }
    }
}

/// Closes every tab but `keep`.
pub fn close_others(app: &AppHandle, keep: &str) {
    let others: Vec<String> = state(app)
        .lock()
        .tabs
        .list
        .iter()
        .filter(|t| t.label != keep)
        .map(|t| t.label.clone())
        .collect();
    // `main` last: closing it moves `keep` into it
    for label in others.iter().filter(|l| *l != MAIN) {
        close(app, label);
    }
    if others.iter().any(|l| l == MAIN) {
        let _ = select(app, keep);
        close(app, MAIN);
    }
}

/// The page of a tab in a new tab next to it.
pub fn duplicate(app: &AppHandle, label: &str) {
    let url = {
        let s = state(app);
        let inner = s.lock();
        inner.committed.get(label).cloned().or_else(|| {
            inner
                .tabs
                .list
                .iter()
                .find(|t| t.label == label)
                .and_then(|t| t.url.clone())
        })
    };
    if let Some(url) = url {
        if let Err(e) = open(app, url, true) {
            log::info!("no duplicate: {e}");
        }
    }
}

/// Ctrl+Shift+T: the last closed tab again (a page of the current server; never an auth page).
pub fn reopen_closed(app: &AppHandle) {
    let page = {
        let s = state(app);
        let mut inner = s.lock();
        let Some(origin) = inner.current_origin() else { return };
        loop {
            match inner.tabs.closed.pop() {
                Some(url) => {
                    let mut path = url.path().to_string();
                    if let Some(q) = url.query() {
                        path.push('?');
                        path.push_str(q);
                    }
                    if let Some(page) = links::resolve_deep_link_target(&origin, &path) {
                        break Some(page);
                    }
                }
                None => break None,
            }
        }
    };
    if let Some(page) = page {
        if let Err(e) = open(app, page, true) {
            log::info!("closed tab not reopened: {e}");
        }
    }
}

/// ⌃Tab / ⌃⇧Tab.
pub fn cycle(app: &AppHandle, step: i32) {
    let next = state(app).lock().tabs.neighbour(step);
    if let Some(next) = next {
        let _ = select(app, &next);
    }
}

/// The tab's page committed (`shell::on_page_load`): remember it for the bar and the saved tabs.
pub fn on_tab_page(app: &AppHandle, label: &str, url: &Url) {
    let pending = {
        let s = state(app);
        let mut inner = s.lock();
        let Some(origin) = inner.current_origin() else { return };
        if !origin.contains(url) {
            return;
        }
        match inner.tabs.get_mut(label) {
            Some(tab) => tab.url = Some(url.clone()),
            None => return,
        }
        // the session link is not a page of its own: wait for where it leads
        let on_session_link = url.path().starts_with("/app/session");
        if label == MAIN && !on_session_link {
            inner.tabs.pending_active.take()
        } else {
            None
        }
    };
    if let Some(pending) = pending {
        // a webview may be built: not on this page-load handler's thread
        let h = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = select(&h, &pending);
        });
    }
    persist(app);
    changed(app);
}

/// The page's `<title>` changed.
pub fn on_title(app: &AppHandle, label: &str, title: String) {
    {
        let s = state(app);
        let mut inner = s.lock();
        match inner.tabs.get_mut(label) {
            Some(tab) if tab.title != title => tab.title = title,
            _ => return,
        }
    }
    changed(app);
}

/// Saves the tabs of the current server (signed in only).
pub fn persist(app: &AppHandle) {
    let s = state(app);
    let mut inner = s.lock();
    // not before the saved tabs were put back: the sign-in selects the `main` tab first, which
    // would save a single tab over them
    if inner.phase != Phase::InApp || !inner.tabs.restored {
        return;
    }
    let Some(origin) = inner.current_origin() else { return };
    if inner.window_store != Some(origin.store_id()) {
        return;
    }
    let saved = to_saved(&inner.tabs, &origin);
    let key = crate::server::hex(&origin.store_id());
    if inner.settings.tabs.get(&key) != Some(&saved) {
        inner.settings.tabs.insert(key, saved);
        inner.save_settings();
    }
}

/// After signing in to a freshly built window: the server's saved tabs come back (built when
/// selected). `main` keeps the page the sign-in opened.
pub fn restore(app: &AppHandle) {
    let active = {
        let s = state(app);
        let mut inner = s.lock();
        if inner.tabs.restored || inner.phase != Phase::InApp {
            return;
        }
        inner.tabs.restored = true;
        let Some(origin) = inner.current_origin() else { return };
        let key = crate::server::hex(&origin.store_id());
        let Some(saved) = inner.settings.tabs.get(&key).cloned() else {
            return;
        };
        if inner.tabs.list.len() != 1 {
            return;
        }
        let mut list = Vec::new();
        let mut active = None;
        for (i, page) in saved.pages.iter().enumerate().take(MAX_RESTORED) {
            if i == saved.main {
                let mut main = inner.tabs.list.remove(0);
                main.title.clear();
                list.push(main);
                if i == saved.active {
                    active = Some(MAIN.to_string());
                }
                continue;
            }
            // never an auth page (`/app/session/…`, `/login`, …)
            let Some(url) = links::resolve_deep_link_target(&origin, page) else {
                continue;
            };
            let label = next_label(&mut inner);
            if i == saved.active {
                active = Some(label.clone());
            }
            list.push(Tab::new(&label, Some(url), false));
        }
        if !list.iter().any(|t| t.label == MAIN) {
            let main = inner.tabs.list.remove(0);
            list.insert(0, main);
        }
        inner.tabs.list = list;
        active
    };
    log::info!("tabs: {} restored", state(app).lock().tabs.list.len());
    // Not yet: the `main` tab is still on its one-time session link. A tab loaded now would reach
    // the server before the session cookie exists, land on /login and start the sign-in again
    // (`flows::on_web_session_lost`). It is shown once `main` has a page (`on_tab_page`).
    state(app).lock().tabs.pending_active = active.filter(|a| a != MAIN);
    changed(app);
}

// ---------------------------------------------------------------------------------------------
// the top bar's commands (capabilities/topbar.json)

/// Every top-bar command accepts calls from the top bar's bundled page only.
fn topbar(app: &AppHandle, webview: &Webview) -> Result<(), String> {
    let url = webview.url().map_err(|_| "unknown caller".to_string())?;
    if links::is_local_caller(webview.label(), &url, TOPBAR, &shell::local_origins(app)) {
        Ok(())
    } else {
        Err("not allowed".into())
    }
}

fn known_tab(app: &AppHandle, id: &str) -> Result<(), String> {
    if state(app).lock().tabs.index(id).is_some() {
        Ok(())
    } else {
        Err("no such tab".into())
    }
}

fn signed_in(app: &AppHandle) -> Result<(), String> {
    let phase = state(app).lock().phase;
    if phase == Phase::InApp {
        Ok(())
    } else {
        log::info!("tabs: refused while {phase:?}");
        Err("not signed in".into())
    }
}

#[tauri::command]
pub fn topbar_view(app: AppHandle, webview: Webview) -> Result<TopbarView, String> {
    topbar(&app, &webview)?;
    Ok(view(&app))
}

#[tauri::command]
pub async fn topbar_select(app: AppHandle, webview: Webview, id: String) -> Result<(), String> {
    topbar(&app, &webview)?;
    signed_in(&app)?;
    known_tab(&app, &id)?;
    select(&app, &id)
}

#[tauri::command]
pub async fn topbar_close(app: AppHandle, webview: Webview, id: String) -> Result<(), String> {
    topbar(&app, &webview)?;
    signed_in(&app)?;
    known_tab(&app, &id)?;
    close(&app, &id);
    Ok(())
}

#[tauri::command]
pub async fn topbar_new_tab(app: AppHandle, webview: Webview) -> Result<(), String> {
    topbar(&app, &webview)?;
    signed_in(&app)?;
    open_new(&app);
    Ok(())
}

#[tauri::command]
pub fn topbar_move(app: AppHandle, webview: Webview, id: String, to: usize) -> Result<(), String> {
    topbar(&app, &webview)?;
    known_tab(&app, &id)?;
    if state(&app).lock().tabs.move_to(&id, to) {
        persist(&app);
    }
    changed(&app);
    Ok(())
}

/// ← → ⟳ of the active tab.
#[tauri::command]
pub fn topbar_nav(app: AppHandle, webview: Webview, action: String) -> Result<(), String> {
    topbar(&app, &webview)?;
    let Some(tab) = shell::active_tab(&app) else {
        return Ok(());
    };
    match action.as_str() {
        "back" if shell::in_app(&app) => tab.eval("history.back()").map_err(|e| e.to_string()),
        "forward" if shell::in_app(&app) => tab.eval("history.forward()").map_err(|e| e.to_string()),
        "reload" => tab.reload().map_err(|e| e.to_string()),
        "home" if shell::in_app(&app) => {
            shell::open_page(&app, "/dashboard");
            Ok(())
        }
        _ => Err("unknown action".into()),
    }
}

/// An app menu (File, Edit, …), the servers or a tab's menu, as a native menu under the button.
/// `x` / `y`: where it opens, in the bar's CSS pixels.
#[tauri::command]
pub fn topbar_menu(
    app: AppHandle,
    webview: Webview,
    menu: String,
    x: f64,
    y: f64,
    tab: Option<String>,
) -> Result<(), String> {
    topbar(&app, &webview)?;
    if !(0.0..=10_000.0).contains(&x) || !(0.0..=10_000.0).contains(&y) {
        return Err("bad position".into());
    }
    if let Some(tab) = &tab {
        known_tab(&app, tab)?;
    }
    let window = shell::main_window(&app).ok_or("no window")?;
    // Edit ▸ Copy / Paste act on the focused webview: the page, not the bar
    if let Some(active) = shell::active_tab(&app) {
        let _ = active.set_focus();
    }
    crate::menu::popup_topbar_menu(&app, &window, &menu, tab.as_deref(), tauri::LogicalPosition::new(x, y))
}

/// The window controls (Windows / Linux draw them in the bar) and dragging the window by the bar.
#[tauri::command]
pub fn topbar_window(app: AppHandle, webview: Webview, action: String) -> Result<(), String> {
    topbar(&app, &webview)?;
    let window = shell::main_window(&app).ok_or("no window")?;
    let result = match action.as_str() {
        "minimize" => window.minimize(),
        "maximize" => {
            if window.is_maximized().unwrap_or(false) {
                window.unmaximize()
            } else {
                window.maximize()
            }
        }
        "close" => window.close(),
        "drag" => window.start_dragging(),
        _ => return Err("unknown action".into()),
    };
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(labels: &[&str], active: &str) -> Tabs {
        Tabs {
            list: labels.iter().map(|l| Tab::new(l, None, true)).collect(),
            active: active.to_string(),
            restored: true,
            pending_active: None,
            closed: Vec::new(),
        }
    }

    fn labels(t: &Tabs) -> Vec<&str> {
        t.list.iter().map(|t| t.label.as_str()).collect()
    }

    #[test]
    fn the_only_tab_stays() {
        assert_eq!(plan_close(&tabs(&["main"], "main"), "main"), ClosePlan::Keep);
        assert_eq!(plan_close(&tabs(&["main"], "main"), "app-9"), ClosePlan::Keep);
    }

    #[test]
    fn closing_the_active_tab_activates_the_right_then_the_left_neighbour() {
        let t = tabs(&["main", "app-1", "app-2"], "app-1");
        assert_eq!(
            plan_close(&t, "app-1"),
            ClosePlan::Remove {
                remove: "app-1".into(),
                activate: Some("app-2".into())
            }
        );
        let t = tabs(&["main", "app-1", "app-2"], "app-2");
        assert_eq!(
            plan_close(&t, "app-2"),
            ClosePlan::Remove {
                remove: "app-2".into(),
                activate: Some("app-1".into())
            }
        );
        // a background tab: the active one stays
        assert_eq!(
            plan_close(&t, "app-1"),
            ClosePlan::Remove {
                remove: "app-1".into(),
                activate: None
            }
        );
    }

    #[test]
    fn closing_main_moves_a_neighbour_into_it() {
        let t = tabs(&["main", "app-1", "app-2"], "main");
        assert_eq!(plan_close(&t, "main"), ClosePlan::MoveIntoMain { from: "app-1".into() });
        let t = tabs(&["app-1", "main"], "main");
        assert_eq!(plan_close(&t, "main"), ClosePlan::MoveIntoMain { from: "app-1".into() });
        // main in the background: the active page moves into main
        let t = tabs(&["main", "app-1", "app-2"], "app-2");
        assert_eq!(plan_close(&t, "main"), ClosePlan::MoveIntoMain { from: "app-2".into() });
    }

    #[test]
    fn move_into_main_takes_the_donors_place() {
        let mut t = tabs(&["main", "app-1", "app-2"], "main");
        t.list[1].title = "Roadmap".into();
        move_into_main(&mut t, "app-1");
        assert_eq!(labels(&t), ["main", "app-2"]);
        assert_eq!(t.list[0].title, "Roadmap");
        assert_eq!(t.active, "main");

        let mut t = tabs(&["main", "app-1", "app-2"], "app-2");
        move_into_main(&mut t, "app-2");
        assert_eq!(labels(&t), ["app-1", "main"]);

        let mut t = tabs(&["app-1", "app-2", "main"], "main");
        move_into_main(&mut t, "app-2");
        assert_eq!(labels(&t), ["app-1", "main"]);
    }

    #[test]
    fn closed_tabs_are_bounded_last_first() {
        let mut t = tabs(&["main"], "main");
        for i in 0..(MAX_CLOSED + 3) {
            t.remember_closed(Url::parse(&format!("https://x.test/p/{i}")).ok());
        }
        t.remember_closed(None);
        assert_eq!(t.closed.len(), MAX_CLOSED);
        assert_eq!(t.closed.last().map(|u| u.path()), Some("/p/12"));
        assert_eq!(t.closed.first().map(|u| u.path()), Some("/p/3"));
    }

    #[test]
    fn neighbours_wrap_and_moves_clamp() {
        let mut t = tabs(&["main", "app-1", "app-2"], "app-2");
        assert_eq!(t.neighbour(1).as_deref(), Some("main"));
        assert_eq!(t.neighbour(-1).as_deref(), Some("app-1"));
        assert!(t.move_to("main", 99));
        assert_eq!(labels(&t), ["app-1", "app-2", "main"]);
        assert!(!t.move_to("app-7", 0));
        assert_eq!(tabs(&["main"], "main").neighbour(1), None);
    }

    #[test]
    fn titles_and_kinds() {
        let mut tab = Tab::new(
            "app-1",
            Url::parse("https://x.test/workspaces/a/projects/b/boards/sprint-board").ok(),
            true,
        );
        assert_eq!(kind_of(tab.url.as_ref()), "board");
        assert_eq!(title_of(&tab), "Sprint board");
        tab.title = "Sprint 12 · Brainitive".into();
        assert_eq!(title_of(&tab), "Sprint 12");
        tab.title = "  \u{7}  ".into();
        assert_eq!(title_of(&tab), "Sprint board");
        assert_eq!(kind_of(Url::parse("https://x.test/dashboard").ok().as_ref()), "home");
        assert_eq!(kind_of(Url::parse("https://x.test/docs/abc").ok().as_ref()), "doc");
        assert_eq!(kind_of(None), "page");
        let tab = Tab::new("app-1", Url::parse("https://x.test/").ok(), true);
        assert_eq!(title_of(&tab), "Brainitive");
    }

    #[test]
    fn saved_tabs_keep_order_active_and_main_and_drop_other_origins() {
        let origin = crate::server::ServerOrigin::cloud();
        let mut t = tabs(&["app-1", "main", "app-2", "app-3"], "app-2");
        t.list[0].url = Url::parse(&format!("{}/boards/1?view=list", origin.as_str())).ok();
        t.list[1].url = Some(origin.join("/dashboard"));
        t.list[2].url = Some(origin.join("/docs/x"));
        t.list[3].url = Url::parse("https://elsewhere.test/a").ok();
        let saved = to_saved(&t, &origin);
        assert_eq!(saved.pages, ["/boards/1?view=list", "/dashboard", "/docs/x"]);
        assert_eq!(saved.active, 2);
        assert_eq!(saved.main, 1);
    }
}
