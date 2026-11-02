//! The tray / menu-bar icon (BRAI-360): a native menu (no webview) with quick capture, the quick
//! switcher, today's My Day (tick done), the running timer (stop and log / discard), the unread
//! notification count, Open Brainitive, Settings and Quit.
//!
//! The data comes from a poller (every 60 s while signed in, backing off on errors, see
//! `poll.rs`) that also keeps the dock / taskbar badge and sends queued quick captures.
//! All calls go through `Session::request` with the stored app token.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Wry};

use crate::api::ApiRequest;
use crate::poll::{self, Backoff};
use crate::popup::{self, Popup};
use crate::server::ServerOrigin;
use crate::session::CallError;
use crate::state::{state, Phase};

pub const TRAY_ID: &str = "brainitive";
const MAX_DAY_ITEMS: usize = 15;
const MAX_TITLE: usize = 48;

/// A work item of `GET /me/my-day` (only what the menu shows).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DayItem {
    pub id: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub url: Option<String>,
}

/// The running timer of `GET /me/timer`.
#[derive(Debug, Clone, PartialEq)]
pub struct Timer {
    pub id: Value,
    pub label: String,
    pub url: Option<String>,
    pub elapsed_seconds: u64,
    pub fetched_at: Instant,
    pub activity_required: bool,
    pub default_activity_id: Option<Value>,
}

impl Timer {
    pub fn elapsed_now(&self) -> u64 {
        self.elapsed_seconds + self.fetched_at.elapsed().as_secs()
    }
}

/// What the tray shows. `None` = not known (signed out, not fetched yet).
#[derive(Debug, Clone, Default)]
pub struct TrayData {
    pub unread: Option<u64>,
    pub timer: Option<Timer>,
    pub timer_known: bool,
    pub my_day: Option<Vec<DayItem>>,
    /// Ask My Day again on the next poll (after a change, a wake, a sign-in).
    pub my_day_stale: bool,
}

/// What a click on a tray item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    /// The Brainitive key overlay (BRAI-440).
    Leader,
    Capture,
    Switcher,
    /// Voice capture (BRAI-439).
    Voice,
    Open,
    MyDay,
    ToggleDay(String),
    TimerOpen,
    TimerStop,
    TimerDiscard,
    Notifications,
    Settings,
    Quit,
    /// The offline window (BRAI-366).
    Offline,
    SyncNow,
    /// Auto-updates (BRAI-373): check by hand / install the waiting update and restart.
    CheckForUpdates,
    RestartToUpdate,
    /// The menu-bar panel, from the menu (Linux: AppIndicator reports no clicks on the icon).
    Panel,
    /// The New task window.
    Task,
    /// Servers ▸ a saved server (`servers::server_id`).
    Server(String),
    /// Servers ▸ Add Server… / Manage Servers….
    AddServer,
    ManageServers,
}

impl TrayAction {
    pub fn id(&self) -> String {
        match self {
            Self::Leader => "tray-leader".into(),
            Self::Capture => "tray-capture".into(),
            Self::Switcher => "tray-switcher".into(),
            Self::Voice => "tray-voice".into(),
            Self::Open => "tray-open".into(),
            Self::MyDay => "tray-my-day".into(),
            Self::ToggleDay(id) => format!("tray-day:{id}"),
            Self::TimerOpen => "tray-timer-open".into(),
            Self::TimerStop => "tray-timer-stop".into(),
            Self::TimerDiscard => "tray-timer-discard".into(),
            Self::Notifications => "tray-notifications".into(),
            Self::Settings => "tray-settings".into(),
            Self::Quit => "tray-quit".into(),
            Self::Offline => "tray-offline".into(),
            Self::SyncNow => "tray-sync-now".into(),
            Self::CheckForUpdates => "tray-check-updates".into(),
            Self::RestartToUpdate => "tray-restart-update".into(),
            Self::Panel => "tray-panel".into(),
            Self::Task => "tray-task".into(),
            Self::Server(id) => format!("tray-server:{id}"),
            Self::AddServer => "tray-servers-add".into(),
            Self::ManageServers => "tray-servers-manage".into(),
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        if let Some(work) = id.strip_prefix("tray-day:") {
            return is_work_id(work).then(|| Self::ToggleDay(work.to_string()));
        }
        if let Some(server) = id.strip_prefix("tray-server:") {
            let valid = server.len() == 16 && server.bytes().all(|b| b.is_ascii_hexdigit());
            return valid.then(|| Self::Server(server.to_string()));
        }
        Some(match id {
            "tray-leader" => Self::Leader,
            "tray-capture" => Self::Capture,
            "tray-switcher" => Self::Switcher,
            "tray-voice" => Self::Voice,
            "tray-open" => Self::Open,
            "tray-my-day" => Self::MyDay,
            "tray-timer-open" => Self::TimerOpen,
            "tray-timer-stop" => Self::TimerStop,
            "tray-timer-discard" => Self::TimerDiscard,
            "tray-notifications" => Self::Notifications,
            "tray-settings" => Self::Settings,
            "tray-quit" => Self::Quit,
            "tray-offline" => Self::Offline,
            "tray-sync-now" => Self::SyncNow,
            "tray-check-updates" => Self::CheckForUpdates,
            "tray-restart-update" => Self::RestartToUpdate,
            "tray-panel" => Self::Panel,
            "tray-task" => Self::Task,
            "tray-servers-add" => Self::AddServer,
            "tray-servers-manage" => Self::ManageServers,
            _ => return None,
        })
    }
}

/// `issue:12` / `task:7`: the only work ids that go into an API path.
pub fn is_work_id(id: &str) -> bool {
    match id.split_once(':') {
        Some(("issue" | "task", n)) => !n.is_empty() && n.len() <= 12 && n.bytes().all(|b| b.is_ascii_digit()),
        _ => false,
    }
}

/// One line of the tray menu (built into native items by [`build_menu`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Item {
        action: Option<TrayAction>,
        text: String,
        accelerator: Option<String>,
    },
    Check {
        action: TrayAction,
        text: String,
        checked: bool,
    },
    Submenu {
        text: String,
        entries: Vec<Entry>,
    },
    Separator,
}

fn item(action: TrayAction, text: impl Into<String>) -> Entry {
    Entry::Item {
        action: Some(action),
        text: text.into(),
        accelerator: None,
    }
}

fn label(text: impl Into<String>) -> Entry {
    Entry::Item {
        action: None,
        text: text.into(),
        accelerator: None,
    }
}

pub fn short(text: &str) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.chars().count() > MAX_TITLE {
        format!("{}…", clean.chars().take(MAX_TITLE - 1).collect::<String>().trim_end())
    } else {
        clean.to_string()
    }
}

fn work_label(key: Option<&str>, title: &str) -> String {
    match key.filter(|k| !k.is_empty()) {
        Some(k) => short(&format!("{k} {title}")),
        None => short(title),
    }
}

/// The tray menu for the current state (pure, so it is tested).
pub fn model(
    server: Option<&str>,
    signed_in: bool,
    data: &TrayData,
    capture_shortcut: Option<String>,
    switcher_shortcut: Option<String>,
    voice_shortcut: Option<String>,
    offline: &crate::offline::TrayStatus,
) -> Vec<Entry> {
    let mut out = Vec::new();
    out.push(label(match (server, signed_in) {
        (Some(s), true) => format!("Brainitive — {s}"),
        (Some(s), false) if offline.available => format!("Brainitive — {s}"),
        (Some(s), false) => format!("Brainitive — {s} (signed out)"),
        (None, _) => "Brainitive — not connected".to_string(),
    }));
    if offline.available {
        // offline mode (BRAI-366): the status, the offline window and "Sync now"
        let changes = |n: usize| {
            if n == 1 {
                "1 change".to_string()
            } else {
                format!("{n} changes")
            }
        };
        match (offline.offline, offline.pending) {
            (true, 0) => out.push(label("Offline — changes will sync")),
            (true, n) => out.push(label(format!("Offline — {} will sync", changes(n)))),
            (false, 0) => {}
            (false, n) => out.push(label(format!("{} waiting to sync", changes(n)))),
        }
        out.push(item(TrayAction::Offline, "Open Offline View…"));
        out.push(item(TrayAction::SyncNow, "Sync Now"));
    }
    out.push(Entry::Separator);
    out.push(Entry::Item {
        action: Some(TrayAction::Capture),
        text: "Quick Capture…".into(),
        accelerator: capture_shortcut,
    });
    out.push(Entry::Item {
        action: Some(TrayAction::Switcher),
        text: "Quick Switcher…".into(),
        accelerator: switcher_shortcut,
    });
    out.push(Entry::Item {
        action: Some(TrayAction::Voice),
        text: "Voice Capture…".into(),
        accelerator: voice_shortcut,
    });

    if signed_in {
        out.push(Entry::Separator);
        // My Day
        let mut day = Vec::new();
        match &data.my_day {
            None => day.push(label("Loading…")),
            Some(items) if items.is_empty() => day.push(label("Nothing planned for today")),
            Some(items) => {
                for i in items.iter().take(MAX_DAY_ITEMS) {
                    if is_work_id(&i.id) {
                        day.push(Entry::Check {
                            action: TrayAction::ToggleDay(i.id.clone()),
                            text: work_label(i.key.as_deref(), &i.title),
                            checked: i.done,
                        });
                    }
                }
                if items.len() > MAX_DAY_ITEMS {
                    day.push(label(format!("and {} more…", items.len() - MAX_DAY_ITEMS)));
                }
            }
        }
        day.push(Entry::Separator);
        day.push(item(TrayAction::MyDay, "Open My Day"));
        let title = match &data.my_day {
            Some(items) if !items.is_empty() => {
                let done = items.iter().filter(|i| i.done).count();
                format!("My Day ({done}/{})", items.len())
            }
            _ => "My Day".to_string(),
        };
        out.push(Entry::Submenu {
            text: title,
            entries: day,
        });

        // the timer
        match &data.timer {
            Some(t) => {
                out.push(item(
                    TrayAction::TimerOpen,
                    format!("⏱ {} — {}", poll::clock(t.elapsed_now()), t.label),
                ));
                out.push(item(TrayAction::TimerStop, "Stop and Log Time"));
                out.push(item(TrayAction::TimerDiscard, "Discard Timer"));
            }
            None if data.timer_known => out.push(label("No timer running")),
            None => {}
        }

        // notifications
        out.push(item(
            TrayAction::Notifications,
            match data.unread {
                Some(0) => "No unread notifications".to_string(),
                Some(1) => "1 unread notification".to_string(),
                Some(n) => format!("{n} unread notifications"),
                None => "Notifications".to_string(),
            },
        ));
    }

    out.push(Entry::Separator);
    out.push(item(TrayAction::Open, "Open Brainitive"));
    out.push(item(TrayAction::Settings, "Settings…"));
    out.push(Entry::Separator);
    out.push(item(TrayAction::Quit, "Quit Brainitive"));
    out
}

/// Adds "Brainitive Key…" with the key (BRAI-440) above Quick Capture, while the key is on.
pub fn add_leader_entry(entries: &mut Vec<Entry>, leader: Option<String>) {
    let Some(leader) = leader else { return };
    let at = entries
        .iter()
        .position(|e| {
            matches!(
                e,
                Entry::Item {
                    action: Some(TrayAction::Capture),
                    ..
                }
            )
        })
        .unwrap_or(entries.len());
    entries.insert(
        at,
        Entry::Item {
            action: Some(TrayAction::Leader),
            text: "Brainitive Key…".into(),
            accelerator: Some(leader),
        },
    );
}

/// Adds "New Task…" right under Quick Capture, its global shortcut (if any) as the accelerator.
pub fn add_task_entry(entries: &mut Vec<Entry>, shortcut: Option<String>) {
    let at = entries
        .iter()
        .position(|e| {
            matches!(
                e,
                Entry::Item {
                    action: Some(TrayAction::Capture),
                    ..
                }
            )
        })
        .map(|i| i + 1)
        .unwrap_or(entries.len());
    entries.insert(
        at,
        Entry::Item {
            action: Some(TrayAction::Task),
            text: "New Task…".into(),
            accelerator: shortcut,
        },
    );
}

/// Adds a "Servers" submenu under the server line: the saved servers (the current one checked;
/// no shortcuts here, they belong to the app menu), Add Server…, Manage Servers….
pub fn add_servers_entry(entries: &mut Vec<Entry>, servers: &[crate::servers::Choice]) {
    let mut sub: Vec<Entry> = servers
        .iter()
        .map(|c| Entry::Check {
            action: TrayAction::Server(c.id.clone()),
            text: short(&c.name),
            checked: c.current,
        })
        .collect();
    if !sub.is_empty() {
        sub.push(Entry::Separator);
    }
    sub.push(item(TrayAction::AddServer, "Add Server…"));
    sub.push(item(TrayAction::ManageServers, "Manage Servers…"));
    entries.insert(
        1.min(entries.len()),
        Entry::Submenu {
            text: "Servers".into(),
            entries: sub,
        },
    );
}

/// Adds "Open Panel…" at the top (under the server line) where the icon itself can't open it
/// (Linux: AppIndicator sends no click events).
pub fn add_panel_entry(entries: &mut Vec<Entry>, needed: bool) {
    if needed {
        entries.insert(1.min(entries.len()), item(TrayAction::Panel, "Open Panel…"));
    }
}

/// Adds the update line above "Open Brainitive" (BRAI-373): "Restart to Update (x.y.z)" when an
/// update waits, else "Check for Updates…" (greyed while a check or download runs). Nothing when
/// updates are off for this build (placeholder key, development build).
pub fn add_update_entry(
    entries: &mut Vec<Entry>,
    availability: crate::updater::Availability,
    status: &crate::updater::Status,
) {
    if !availability.is_available() {
        return;
    }
    let (text, enabled) = crate::updater::menu_item(availability, status);
    let action = match (enabled, status.is_ready()) {
        (false, _) => None,
        (true, true) => Some(TrayAction::RestartToUpdate),
        (true, false) => Some(TrayAction::CheckForUpdates),
    };
    let entry = Entry::Item {
        action,
        text,
        accelerator: None,
    };
    let open = entries
        .iter()
        .position(|e| {
            matches!(
                e,
                Entry::Item {
                    action: Some(TrayAction::Open),
                    ..
                }
            )
        })
        .unwrap_or(entries.len());
    entries.insert(open, entry);
}

fn build_items(app: &AppHandle, entries: &[Entry]) -> tauri::Result<Vec<Box<dyn IsMenuItem<Wry>>>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    for e in entries {
        match e {
            Entry::Separator => items.push(Box::new(PredefinedMenuItem::separator(app)?)),
            Entry::Item {
                action,
                text,
                accelerator,
            } => {
                let id = action
                    .as_ref()
                    .map(TrayAction::id)
                    .unwrap_or_else(|| "tray-label".into());
                items.push(Box::new(MenuItem::with_id(
                    app,
                    id,
                    text,
                    action.is_some(),
                    accelerator.as_deref(),
                )?));
            }
            Entry::Check { action, text, checked } => items.push(Box::new(CheckMenuItem::with_id(
                app,
                action.id(),
                text,
                true,
                *checked,
                None::<&str>,
            )?)),
            Entry::Submenu { text, entries } => {
                let sub = Submenu::new(app, text, true)?;
                for child in build_items(app, entries)? {
                    sub.append(child.as_ref())?;
                }
                items.push(Box::new(sub));
            }
        }
    }
    Ok(items)
}

fn build_menu(app: &AppHandle, entries: &[Entry]) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;
    for item in build_items(app, entries)? {
        menu.append(item.as_ref())?;
    }
    Ok(menu)
}

/// Creates the tray icon (once, at start).
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    // left click: the menu-bar panel; right click (and ⌃-click on macOS): this native menu
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Brainitive")
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| crate::panel::on_tray_event(tray.app_handle(), event));
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    let tray = builder.build(app)?;
    state(app).lock().tray = Some(tray);
    refresh(app);
    Ok(())
}

fn tray(app: &AppHandle) -> Option<TrayIcon<Wry>> {
    state(app).lock().tray.clone()
}

/// Rebuilds the menu, tooltip and (macOS) the count next to the icon from the current data.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = tray(app) else { return };
    let (
        server,
        signed_in,
        data,
        leader,
        capture,
        switcher,
        voice,
        task,
        offline,
        update_availability,
        update_status,
        servers,
    ) = {
        let s = state(app);
        let inner = s.lock();
        let accel = |a: Option<&str>| {
            a.and_then(|a| crate::shortcuts::parse(a).ok())
                .map(|s| crate::shortcuts::canonical(&s))
        };
        (
            inner.current_origin().map(|o| inner.settings.label_of(&o)),
            inner.phase == Phase::InApp,
            inner.tray_data.clone(),
            accel(inner.settings.preferences.leader_shortcut.as_deref()),
            accel(inner.settings.preferences.capture_shortcut.as_deref()),
            accel(inner.settings.preferences.switcher_shortcut.as_deref()),
            accel(inner.settings.preferences.active_voice_shortcut().as_deref()),
            accel(inner.settings.preferences.task_shortcut.as_deref()),
            crate::offline::tray_status(&inner),
            inner.updater.availability,
            inner.updater.status.clone(),
            crate::servers::choices(&inner.settings, cfg!(target_os = "macos")),
        )
    };
    let mut entries = model(server.as_deref(), signed_in, &data, capture, switcher, voice, &offline);
    add_leader_entry(&mut entries, leader);
    add_task_entry(&mut entries, task);
    add_update_entry(&mut entries, update_availability, &update_status);
    add_servers_entry(&mut entries, &servers);
    add_panel_entry(&mut entries, cfg!(target_os = "linux"));
    // the panel reads its data, the unread count and the sync status again
    crate::panel::notify(app);
    {
        // unchanged: keep the native menu (it may be open right now)
        let s = state(app);
        let mut inner = s.lock();
        if inner.tray_model.as_ref() == Some(&entries) {
            return;
        }
        inner.tray_model = Some(entries.clone());
    }
    match build_menu(app, &entries) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => log::warn!("tray menu: {e}"),
    }
    let unread = data.unread.filter(|n| *n > 0 && signed_in);
    let tooltip = match (unread, offline.offline) {
        (_, true) => "Brainitive — offline, changes will sync".to_string(),
        (Some(n), false) => format!("Brainitive — {n} unread"),
        (None, false) => "Brainitive".to_string(),
    };
    let _ = tray.set_tooltip(Some(tooltip));
    #[cfg(target_os = "macos")]
    let _ = tray.set_title(unread.map(|n| if n > 99 { "99+".to_string() } else { n.to_string() }));
}

/// Signed out / server switched: forget what the tray showed and poll soon.
pub fn reset(app: &AppHandle) {
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.tray_data = TrayData {
            my_day_stale: true,
            ..TrayData::default()
        };
        inner.etags.clear();
    }
    refresh(app);
    state(app).poll_now.notify_one();
}

/// Every tray menu event (ids `tray-…`).
pub fn on_menu_event(app: &AppHandle, id: &str) {
    let Some(action) = TrayAction::from_id(id) else { return };
    match action {
        TrayAction::Leader => crate::leader::show(app),
        TrayAction::Capture => popup::show(app, Popup::Capture),
        TrayAction::Switcher => popup::show(app, Popup::Switcher),
        TrayAction::Voice => crate::voice::start_from_menu(app),
        TrayAction::Settings => popup::show(app, Popup::Settings),
        TrayAction::Open => crate::shell::focus_main(app),
        TrayAction::MyDay => crate::shell::open_page(app, "/my-work"),
        TrayAction::Notifications => crate::shell::open_page(app, "/notifications"),
        TrayAction::TimerOpen => {
            let url = state(app).lock().tray_data.timer.as_ref().and_then(|t| t.url.clone());
            crate::shell::open_page(app, url.as_deref().unwrap_or("/my-work"));
        }
        TrayAction::Quit => app.exit(0),
        TrayAction::Offline => popup::show(app, Popup::Offline),
        TrayAction::Panel => crate::panel::toggle(app, None),
        TrayAction::Task => crate::new_task::show(app),
        TrayAction::Server(id) => crate::menu::pick_server(app, &id),
        TrayAction::AddServer => crate::menu::add_server(app),
        TrayAction::ManageServers => crate::menu::manage_servers(app),
        TrayAction::CheckForUpdates | TrayAction::RestartToUpdate => crate::updater::on_menu_click(app),
        TrayAction::SyncNow => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move { crate::offline::sync_now(&h).await });
        }
        TrayAction::ToggleDay(work) => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move { toggle_day(&h, &work).await });
        }
        TrayAction::TimerStop | TrayAction::TimerDiscard => {
            let h = app.clone();
            let stop = action == TrayAction::TimerStop;
            tauri::async_runtime::spawn(async move { end_timer(&h, stop).await });
        }
    }
}

fn current(app: &AppHandle) -> Option<ServerOrigin> {
    let s = state(app);
    let inner = s.lock();
    (inner.phase == Phase::InApp).then(|| inner.current_origin()).flatten()
}

async fn toggle_day(app: &AppHandle, work: &str) {
    let Some(origin) = current(app) else { return };
    let done = state(app)
        .lock()
        .tray_data
        .my_day
        .as_ref()
        .and_then(|items| items.iter().find(|i| i.id == work))
        .map(|i| !i.done)
        .unwrap_or(true);
    let request = ApiRequest::patch(format!("/api/v1/me/my-day/items/{work}"), json!({ "done": done }));
    match state(app).session.request(&origin, &request).await {
        Ok(response) => {
            if let Some(items) = parse_my_day(&response.body) {
                state(app).lock().tray_data.my_day = Some(items);
            }
        }
        Err(e) => {
            crate::bridge::show_notification(app, "My Day was not updated", &e.to_string(), None);
            state(app).lock().tray_data.my_day_stale = true;
        }
    }
    refresh(app);
}

async fn end_timer(app: &AppHandle, stop: bool) {
    let Some(origin) = current(app) else { return };
    let Some(timer) = state(app).lock().tray_data.timer.clone() else {
        return;
    };
    let request = if stop {
        let mut body = json!({ "timer_id": timer.id });
        if timer.activity_required {
            if let Some(activity) = &timer.default_activity_id {
                body["activity_id"] = activity.clone();
            }
        }
        ApiRequest::post("/api/v1/me/timer/stop", body)
    } else {
        ApiRequest::post("/api/v1/me/timer/discard", json!({ "timer_id": timer.id }))
    };
    match state(app).session.request(&origin, &request).await {
        Ok(response) => {
            if stop {
                let result = response.body.get("result").and_then(Value::as_str).unwrap_or("");
                let body = match result {
                    "logged" => format!("{} logged on {}.", poll::clock(timer.elapsed_now()), timer.label),
                    "already_stopped" => "The timer was already stopped.".to_string(),
                    _ => "The time could not be logged on this issue any more.".to_string(),
                };
                crate::bridge::show_notification(app, "Timer stopped", &body, None);
            }
            state(app).lock().tray_data.timer = None;
        }
        Err(CallError::Server {
            status: 422, message, ..
        }) => {
            // e.g. an activity must be chosen: the web's timer can do that
            crate::bridge::show_notification(app, "Choose how to log the time", &message, None);
            crate::shell::open_page(app, timer.url.as_deref().unwrap_or("/my-work"));
        }
        Err(e) => crate::bridge::show_notification(app, "The timer was not stopped", &e.to_string(), None),
    }
    state(app).poll_now.notify_one();
    refresh(app);
}

pub fn parse_my_day(body: &Value) -> Option<Vec<DayItem>> {
    let items = body.get("items")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|i| serde_json::from_value::<DayItem>(i.clone()).ok())
            .filter(|i| is_work_id(&i.id))
            .collect(),
    )
}

pub fn parse_timer(body: &Value) -> Option<Option<Timer>> {
    let running = body.get("running")?.as_bool()?;
    let timer = body.get("timer").filter(|t| t.is_object());
    let (true, Some(t)) = (running, timer) else {
        return Some(None);
    };
    let issue = t.get("issue").filter(|i| i.is_object());
    let title = issue
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("Timer");
    let key = issue.and_then(|i| i.get("key")).and_then(Value::as_str);
    Some(Some(Timer {
        id: t.get("id")?.clone(),
        label: work_label(key, title),
        url: issue
            .and_then(|i| i.get("url"))
            .and_then(Value::as_str)
            .map(str::to_string),
        elapsed_seconds: elapsed_seconds(t, crate::outbox::now() as i64),
        fetched_at: Instant::now(),
        activity_required: body.get("activity_required").and_then(Value::as_bool).unwrap_or(false),
        default_activity_id: body.get("default_activity_id").filter(|v| !v.is_null()).cloned(),
    }))
}

/// Seconds the timer runs: from `started_at` (a 304 gives back the older body, whose
/// `elapsed_seconds` is stale, BRAI-440), else the answer's `elapsed_seconds`.
fn elapsed_seconds(timer: &Value, now: i64) -> u64 {
    match timer["started_at"].as_str().and_then(crate::panel::view::parse_instant) {
        Some(start) if now >= start => (now - start) as u64,
        _ => timer.get("elapsed_seconds").and_then(Value::as_u64).unwrap_or(0),
    }
}

pub fn parse_unread(body: &Value) -> Option<u64> {
    body.get("unread_count").and_then(Value::as_u64)
}

/// Who a poll was made for: the server and the sign-in (`Inner::account_epoch`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollTicket {
    pub origin: ServerOrigin,
    pub account: u64,
}

impl PollTicket {
    pub fn now(inner: &crate::state::Inner) -> Option<Self> {
        let origin = inner.current_origin().filter(|_| inner.phase == Phase::InApp)?;
        Some(Self {
            origin,
            account: inner.account_epoch,
        })
    }

    /// Whether an answer to this poll may still be shown (BRAI-377): the same server, the same
    /// sign-in, and still in the app. A sign-out, a server switch or another account while the
    /// request ran → the answer is dropped (no badge, timer or My Day of the old account).
    pub fn still_current(&self, origin: Option<&ServerOrigin>, account: u64, in_app: bool) -> bool {
        in_app && origin == Some(&self.origin) && account == self.account
    }

    fn holds(&self, inner: &crate::state::Inner) -> bool {
        self.still_current(
            inner.current_origin().as_ref(),
            inner.account_epoch,
            inner.phase == Phase::InApp,
        )
    }
}

/// GET with the ETag of the last answer: a 304 gives the cached body back. `None`: the answer
/// came after the ticket expired.
pub(crate) async fn get_cached(app: &AppHandle, ticket: &PollTicket, path: &str) -> Result<Option<Value>, CallError> {
    let origin = &ticket.origin;
    // keyed by server too: a 304 of one server never gets another server's body
    let key = etag_key(origin, path);
    let cached = state(app).lock().etags.get(&key).cloned();
    let request = ApiRequest::get(path).etag(cached.as_ref().map(|(etag, _)| etag.clone()));
    let response = state(app).session.request(origin, &request).await?;
    if response.not_modified() {
        if let Some((_, body)) = cached {
            return Ok(Some(body));
        }
        // a 304 without a cached body (should not happen): ask again without the ETag
        let response = state(app).session.request(origin, &ApiRequest::get(path)).await?;
        return Ok(Some(response.body));
    }
    let s = state(app);
    let mut inner = s.lock();
    if !ticket.holds(&inner) {
        return Ok(None);
    }
    if let Some(etag) = response.etag.clone() {
        inner.etags.insert(key, (etag, response.body.clone()));
    }
    Ok(Some(response.body))
}

/// One poll: unread count + timer, My Day when due. Returns whether the server answered.
/// Every answer is written only while `ticket` still holds (checked under the same lock as the
/// write): a sign-out or switch during the request drops it.
async fn poll_once(app: &AppHandle, ticket: &PollTicket, plan: poll::Plan) -> Result<(), CallError> {
    if plan.unread {
        let body = get_cached(app, ticket, "/api/v1/notifications/unread-count").await?;
        if let Some(n) = body.as_ref().and_then(parse_unread) {
            let current = {
                let s = state(app);
                let mut inner = s.lock();
                let current = ticket.holds(&inner);
                if current {
                    inner.tray_data.unread = Some(n);
                }
                current
            };
            if current {
                crate::bridge::set_badge(app, n as i64);
            }
        }
    }
    if plan.timer {
        let body = get_cached(app, ticket, "/api/v1/me/timer").await?;
        if let Some(timer) = body.as_ref().and_then(parse_timer) {
            let s = state(app);
            let mut inner = s.lock();
            if ticket.holds(&inner) {
                inner.tray_data.timer = timer;
                inner.tray_data.timer_known = true;
            }
        }
    }
    if plan.my_day {
        let body = get_cached(app, ticket, "/api/v1/me/my-day").await?;
        if let Some(items) = body.as_ref().and_then(parse_my_day) {
            let s = state(app);
            let mut inner = s.lock();
            if ticket.holds(&inner) {
                inner.tray_data.my_day = Some(items);
                inner.tray_data.my_day_stale = false;
            }
        }
    }
    Ok(())
}

/// The poller: runs for the app's lifetime, polls only while signed in.
pub fn start_poller(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut backoff = Backoff::default();
        let mut tick: u32 = 0;
        let mut polled: Option<PollTicket> = None;
        let mut delay = Duration::from_secs(3);
        let mut recheck = crate::version::Recheck::default();
        loop {
            let wall_before = SystemTime::now();
            let notified = {
                let st = state(&app);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => false,
                    _ = st.poll_now.notified() => true,
                }
            };
            let wall = SystemTime::now().duration_since(wall_before).unwrap_or_default();
            if !notified && poll::woke_from_sleep(delay, wall) {
                state(&app).lock().tray_data.my_day_stale = true;
                recheck.woke(std::time::Instant::now());
                tokio::time::sleep(poll::WAKE_GRACE).await;
            }

            let ticket = PollTicket::now(&state(&app).lock());
            let Some(ticket) = ticket else {
                // signed out / no server / a bundled screen: nothing to poll
                if polled.take().is_some() {
                    state(&app).lock().tray_data = TrayData::default();
                    refresh(&app);
                }
                backoff.succeeded();
                recheck.reset();
                delay = poll::INTERVAL;
                continue;
            };
            let origin = ticket.origin.clone();
            // another server or another sign-in: nothing of the previous one stays
            if polled.as_ref() != Some(&ticket) {
                polled = Some(ticket.clone());
                tick = 0;
                recheck.reset();
                let s = state(&app);
                let mut inner = s.lock();
                inner.etags.clear();
                inner.tray_data = TrayData::default();
            }
            let stale = state(&app).lock().tray_data.my_day_stale;
            let result = poll_once(&app, &ticket, poll::plan(tick, stale)).await;
            tick = tick.wrapping_add(1);
            crate::offline::note_poll(&app, &result);
            match result {
                Ok(()) => {
                    backoff.succeeded();
                    // the minimum desktop version can rise while the app runs
                    if recheck.due(std::time::Instant::now()) {
                        if crate::flows::recheck_minimum(&app, &origin).await {
                            recheck.checked(std::time::Instant::now());
                        } else {
                            recheck.failed(std::time::Instant::now());
                        }
                        if state(&app).lock().phase == crate::state::Phase::UpdateRequired {
                            continue;
                        }
                    }
                    crate::capture::flush(&app).await;
                    // offline mode: send what waited, pull what changed (BRAI-366)
                    if let Err(e) = crate::offline::sync(&app, &origin).await {
                        log::info!("offline sync: {e}");
                    }
                }
                Err(e) => {
                    backoff.failed();
                    log::info!("tray poll failed ({} in a row): {e}", backoff.failures());
                }
            }
            refresh(&app);
            delay = backoff.delay();
        }
    });
}

/// The last ETag + body per server and polled path ([`etag_key`]).
pub type EtagCache = HashMap<String, (String, Value)>;

/// `https://brain.example.com /api/v1/me/timer`: one entry per server and path.
pub fn etag_key(origin: &ServerOrigin, path: &str) -> String {
    format!("{} {path}", origin.as_str())
}

#[cfg(test)]
mod tests {
    #[test]
    fn poll_answers_for_another_server_or_account_are_dropped() {
        let a = ServerOrigin::parse("https://a.example.com").unwrap();
        let b = ServerOrigin::parse("https://b.example.com").unwrap();
        let ticket = PollTicket {
            origin: a.clone(),
            account: 3,
        };
        assert!(ticket.still_current(Some(&a), 3, true));
        assert!(!ticket.still_current(Some(&a), 4, true), "signed out / another account");
        assert!(!ticket.still_current(Some(&b), 3, true), "server switched");
        assert!(!ticket.still_current(None, 3, true), "server forgotten");
        assert!(
            !ticket.still_current(Some(&a), 3, false),
            "left the app (sign-in screen)"
        );
    }

    use super::*;

    #[test]
    fn action_ids_round_trip() {
        for a in [
            TrayAction::Leader,
            TrayAction::Capture,
            TrayAction::Switcher,
            TrayAction::Voice,
            TrayAction::Open,
            TrayAction::MyDay,
            TrayAction::ToggleDay("issue:12".into()),
            TrayAction::ToggleDay("task:7".into()),
            TrayAction::TimerOpen,
            TrayAction::TimerStop,
            TrayAction::TimerDiscard,
            TrayAction::Notifications,
            TrayAction::Settings,
            TrayAction::Quit,
            TrayAction::Offline,
            TrayAction::SyncNow,
            TrayAction::CheckForUpdates,
            TrayAction::RestartToUpdate,
            TrayAction::Task,
            TrayAction::Server("0123456789abcdef".into()),
            TrayAction::AddServer,
            TrayAction::ManageServers,
        ] {
            assert_eq!(TrayAction::from_id(&a.id()), Some(a));
        }
        // a server is picked by its id only (never an origin)
        for bad in [
            "tray-server:",
            "tray-server:https://evil.example.com",
            "tray-server:0123456789abcdeg",
            "tray-server:0123456789abcdef0",
        ] {
            assert_eq!(TrayAction::from_id(bad), None, "{bad}");
        }
        assert_eq!(TrayAction::from_id("tray-label"), None);
        assert_eq!(TrayAction::from_id("settings"), None);
        // only plain work ids reach an API path
        for bad in [
            "tray-day:issue:",
            "tray-day:issue:1/../x",
            "tray-day:page:3",
            "tray-day:issue:12?x",
        ] {
            assert_eq!(TrayAction::from_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_servers_submenu_sits_under_the_server_line_without_shortcuts() {
        let mut s = crate::settings::Settings::default();
        let work = ServerOrigin::parse("brain.acme.com").unwrap();
        s.remember(&work, "Acme", None);
        s.remember(&ServerOrigin::cloud(), "Brainitive", None);
        let choices = crate::servers::choices(&s, true);
        let data = TrayData::default();
        let mut entries = model(
            Some("Brainitive Cloud"),
            true,
            &data,
            None,
            None,
            None,
            &Default::default(),
        );
        add_servers_entry(&mut entries, &choices);
        add_panel_entry(&mut entries, true);
        assert!(matches!(
            &entries[1],
            Entry::Item {
                action: Some(TrayAction::Panel),
                ..
            }
        ));
        let Entry::Submenu { text, entries: sub } = &entries[2] else {
            panic!("{:?}", entries[2]);
        };
        assert_eq!(text, "Servers");
        assert_eq!(
            sub[0],
            Entry::Check {
                action: TrayAction::Server(crate::servers::server_id(&work)),
                text: "Acme".into(),
                checked: false,
            }
        );
        assert!(matches!(&sub[1], Entry::Check { checked: true, .. }));
        assert_eq!(sub[2], Entry::Separator);
        assert!(matches!(
            &sub[3],
            Entry::Item {
                action: Some(TrayAction::AddServer),
                accelerator: None,
                ..
            }
        ));
        assert!(matches!(
            &sub[4],
            Entry::Item {
                action: Some(TrayAction::ManageServers),
                accelerator: None,
                ..
            }
        ));
        // no saved server yet: Add / Manage only
        let mut entries = model(None, false, &data, None, None, None, &Default::default());
        add_servers_entry(&mut entries, &[]);
        let Entry::Submenu { entries: sub, .. } = &entries[1] else {
            panic!()
        };
        assert_eq!(sub.len(), 2);
    }

    #[test]
    fn etags_are_kept_per_server() {
        let a = ServerOrigin::parse("a.example.com").unwrap();
        let b = ServerOrigin::parse("b.example.com").unwrap();
        assert_ne!(etag_key(&a, "/api/v1/me/timer"), etag_key(&b, "/api/v1/me/timer"));
        assert_ne!(etag_key(&a, "/api/v1/me/timer"), etag_key(&a, "/api/v1/me/my-day"));
    }

    #[test]
    fn parses_the_api_answers() {
        let day = json!({"date": "2026-11-02", "items": [
            {"id": "issue:12", "type": "issue", "key": "BRAI-12", "title": "Fix login", "done": false, "url": "https://x/1"},
            {"id": "task:3", "type": "task", "key": null, "title": "Call Anna", "done": true},
            {"id": "evil/../x", "title": "?"}
        ], "carry_over_count": 0});
        let items = parse_my_day(&day).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].key.as_deref(), Some("BRAI-12"));
        assert!(items[1].done);

        let timer = json!({"running": true, "timer": {"id": 5, "started_at": "x", "elapsed_seconds": 3900,
            "issue": {"id": 12, "work_id": "issue:12", "key": "BRAI-12", "title": "Fix login", "url": "https://x/1"}},
            "activities": [], "activity_required": true, "default_activity_id": 2});
        let t = parse_timer(&timer).unwrap().unwrap();
        assert_eq!(t.id, json!(5));
        assert_eq!(t.label, "BRAI-12 Fix login");
        assert!(t.activity_required);
        assert_eq!(t.default_activity_id, Some(json!(2)));
        assert_eq!(parse_timer(&json!({"running": false, "timer": null})), Some(None));
        assert_eq!(parse_timer(&json!({})), None);

        assert_eq!(parse_unread(&json!({"unread_count": 4})), Some(4));
        assert_eq!(parse_unread(&json!({})), None);
    }

    #[test]
    fn the_timer_counts_from_started_at_so_a_304_body_is_not_stale() {
        let started = crate::panel::view::parse_instant("2026-11-02T09:00:00Z").unwrap();
        let t = json!({"started_at": "2026-11-02T09:00:00Z", "elapsed_seconds": 60});
        assert_eq!(elapsed_seconds(&t, started + 3600), 3600);
        // no / broken started_at, or a clock behind the server: the answer's own count
        assert_eq!(
            elapsed_seconds(&json!({"started_at": "x", "elapsed_seconds": 60}), started),
            60
        );
        assert_eq!(elapsed_seconds(&t, started - 10), 60);
    }

    #[test]
    fn new_task_sits_right_under_quick_capture_with_its_shortcut() {
        let base = model(
            Some("Cloud"),
            true,
            &TrayData::default(),
            None,
            None,
            None,
            &Default::default(),
        );
        let capture = base
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Entry::Item {
                        action: Some(TrayAction::Capture),
                        ..
                    }
                )
            })
            .unwrap();
        let mut with = base.clone();
        add_task_entry(&mut with, Some("ctrl+super+KeyT".into()));
        assert_eq!(with.len(), base.len() + 1);
        assert_eq!(
            with[capture + 1],
            Entry::Item {
                action: Some(TrayAction::Task),
                text: "New Task…".into(),
                accelerator: Some("ctrl+super+KeyT".into()),
            }
        );
        // off: still offered, without a key
        let mut off = base.clone();
        add_task_entry(&mut off, None);
        assert!(matches!(
            &off[capture + 1],
            Entry::Item {
                action: Some(TrayAction::Task),
                accelerator: None,
                ..
            }
        ));
        assert_eq!(TrayAction::from_id("tray-task"), Some(TrayAction::Task));
    }

    #[test]
    fn linux_gets_an_open_panel_line_under_the_server_line() {
        let mut entries = vec![label("Brainitive — Cloud"), Entry::Separator];
        add_panel_entry(&mut entries, false);
        assert_eq!(entries.len(), 2);
        add_panel_entry(&mut entries, true);
        assert_eq!(entries[1], item(TrayAction::Panel, "Open Panel…"));
        assert_eq!(TrayAction::from_id("tray-panel"), Some(TrayAction::Panel));
    }

    #[test]
    fn menu_model_signed_out_only_offers_the_basics() {
        let m = model(
            Some("Cloud"),
            false,
            &TrayData::default(),
            None,
            None,
            None,
            &Default::default(),
        );
        assert_eq!(
            m[0],
            Entry::Item {
                action: None,
                text: "Brainitive — Cloud (signed out)".into(),
                accelerator: None
            }
        );
        assert!(!m.iter().any(|e| matches!(e, Entry::Submenu { .. })));
        assert!(m.contains(&item(TrayAction::Quit, "Quit Brainitive")));
        // voice capture is offered even signed out (the text waits in quick capture)
        assert!(m.iter().any(
            |e| matches!(e, Entry::Item { action: Some(TrayAction::Voice), text, .. } if text == "Voice Capture…")
        ));
    }

    #[test]
    fn the_brainitive_key_sits_above_quick_capture_while_it_is_on() {
        let base = model(
            Some("Cloud"),
            true,
            &TrayData::default(),
            None,
            None,
            None,
            &Default::default(),
        );
        let mut off = base.clone();
        add_leader_entry(&mut off, None);
        assert_eq!(off, base);
        let mut on = base.clone();
        add_leader_entry(&mut on, Some("control+super+KeyK".into()));
        assert_eq!(on.len(), base.len() + 1);
        let at = on
            .iter()
            .position(|e| {
                matches!(
                    e,
                    Entry::Item {
                        action: Some(TrayAction::Leader),
                        ..
                    }
                )
            })
            .unwrap();
        assert_eq!(
            on[at],
            Entry::Item {
                action: Some(TrayAction::Leader),
                text: "Brainitive Key…".into(),
                accelerator: Some("control+super+KeyK".into())
            }
        );
        assert!(matches!(
            &on[at + 1],
            Entry::Item {
                action: Some(TrayAction::Capture),
                ..
            }
        ));
    }

    #[test]
    fn menu_model_signed_in() {
        let data = TrayData {
            unread: Some(3),
            timer: Some(Timer {
                id: json!(1),
                label: "BRAI-1 Thing".into(),
                url: None,
                elapsed_seconds: 65 * 60,
                fetched_at: Instant::now(),
                activity_required: false,
                default_activity_id: None,
            }),
            timer_known: true,
            my_day: Some(vec![
                DayItem {
                    id: "issue:1".into(),
                    key: Some("BRAI-1".into()),
                    title: "Thing".into(),
                    done: true,
                    url: None,
                },
                DayItem {
                    id: "task:2".into(),
                    key: None,
                    title: "x".repeat(80),
                    done: false,
                    url: None,
                },
            ]),
            my_day_stale: false,
        };
        let m = model(
            Some("Cloud"),
            true,
            &data,
            Some("shift+super+Space".into()),
            None,
            None,
            &Default::default(),
        );
        let Some(Entry::Submenu { text, entries }) = m.iter().find(|e| matches!(e, Entry::Submenu { .. })) else {
            panic!("no My Day");
        };
        assert_eq!(text, "My Day (1/2)");
        assert_eq!(
            entries[0],
            Entry::Check {
                action: TrayAction::ToggleDay("issue:1".into()),
                text: "BRAI-1 Thing".into(),
                checked: true
            }
        );
        let Entry::Check { text: long, .. } = &entries[1] else {
            panic!()
        };
        assert_eq!(long.chars().count(), MAX_TITLE);
        assert!(m.contains(&item(TrayAction::TimerOpen, "⏱ 1:05 — BRAI-1 Thing")));
        assert!(m.contains(&item(TrayAction::TimerStop, "Stop and Log Time")));
        assert!(m.contains(&item(TrayAction::Notifications, "3 unread notifications")));

        let none = TrayData {
            timer_known: true,
            my_day: Some(vec![]),
            unread: Some(0),
            ..TrayData::default()
        };
        let m = model(Some("Cloud"), true, &none, None, None, None, &Default::default());
        assert!(m.contains(&label("No timer running")));
        assert!(m.contains(&item(TrayAction::Notifications, "No unread notifications")));
    }

    #[test]
    fn menu_model_offline() {
        use crate::offline::TrayStatus;
        let offline = TrayStatus {
            available: true,
            offline: true,
            pending: 3,
        };
        // the unreachable screen: not "signed out", the offline view and Sync Now
        let m = model(Some("Cloud"), false, &TrayData::default(), None, None, None, &offline);
        assert_eq!(m[0], label("Brainitive — Cloud"));
        assert_eq!(m[1], label("Offline — 3 changes will sync"));
        assert!(m.contains(&item(TrayAction::Offline, "Open Offline View…")));
        assert!(m.contains(&item(TrayAction::SyncNow, "Sync Now")));
        let one = TrayStatus {
            pending: 1,
            ..offline.clone()
        };
        assert_eq!(
            model(Some("Cloud"), false, &TrayData::default(), None, None, None, &one)[1],
            label("Offline — 1 change will sync")
        );
        let none = TrayStatus {
            pending: 0,
            ..offline.clone()
        };
        assert_eq!(
            model(Some("Cloud"), true, &TrayData::default(), None, None, None, &none)[1],
            label("Offline — changes will sync")
        );
        // online with changes still waiting
        let online = TrayStatus {
            offline: false,
            ..offline
        };
        assert_eq!(
            model(Some("Cloud"), true, &TrayData::default(), None, None, None, &online)[1],
            label("3 changes waiting to sync")
        );
        // signed out: no offline entries
        let m = model(
            Some("Cloud"),
            false,
            &TrayData::default(),
            None,
            None,
            None,
            &TrayStatus::default(),
        );
        assert!(!m.contains(&item(TrayAction::SyncNow, "Sync Now")));
    }

    #[test]
    fn the_update_line_sits_above_open_and_only_when_updates_can_run() {
        use crate::updater::{Availability, Status};
        let base = model(
            Some("Cloud"),
            false,
            &TrayData::default(),
            None,
            None,
            None,
            &Default::default(),
        );

        let mut m = base.clone();
        add_update_entry(&mut m, Availability::PlaceholderKey, &Status::Idle);
        assert_eq!(m, base, "no update line while the build has no real key");
        let mut m = base.clone();
        add_update_entry(&mut m, Availability::DevBuild, &Status::Idle);
        assert_eq!(m, base);

        let mut m = base.clone();
        add_update_entry(&mut m, Availability::Available, &Status::Idle);
        let open = m
            .iter()
            .position(|e| *e == item(TrayAction::Open, "Open Brainitive"))
            .unwrap();
        assert_eq!(m[open - 1], item(TrayAction::CheckForUpdates, "Check for Updates…"));

        let mut m = base.clone();
        add_update_entry(&mut m, Availability::Available, &Status::Checking);
        assert!(m.contains(&label("Checking for Updates…")));

        let mut m = base.clone();
        add_update_entry(
            &mut m,
            Availability::Available,
            &Status::Ready {
                version: "1.2.0".into(),
            },
        );
        assert!(m.contains(&item(TrayAction::RestartToUpdate, "Restart to Update (1.2.0)")));
        assert!(!m.iter().any(|e| matches!(
            e,
            Entry::Item {
                action: Some(TrayAction::CheckForUpdates),
                ..
            }
        )));
    }

    #[test]
    fn work_ids() {
        assert!(is_work_id("issue:12"));
        assert!(is_work_id("task:1"));
        assert!(!is_work_id("issue:"));
        assert!(!is_work_id("item:1"));
        assert!(!is_work_id("issue:1x"));
        assert!(!is_work_id("issue:1234567890123"));
    }
}
