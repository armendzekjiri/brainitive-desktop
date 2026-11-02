//! The menu-bar panel: a glass window under the tray / menu-bar icon (left click) with Today, My
//! Day, assigned work, notifications and the timer. The native tray menu stays on right click
//! (and ⌃-click on macOS).
//!
//! The window (`panel.html`, label `panel`, `capabilities/panel.json`) is a bundled page: it never
//! sees a token or a URL. Every call is a command here that checks the caller
//! (`popup::ensure_caller`) and calls the API from Rust with the stored app token
//! (`Session::request`). The answers are kept per server and sign-in; commands only act on ids
//! those answers contain. Pure parts: `geometry` (where it opens) and `view` (what it shows).

pub mod geometry;
pub mod view;

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Webview, WebviewWindow};

use crate::api::ApiRequest;
use crate::flows::UiError;
use crate::popup::{self, Popup};
use crate::server::ServerOrigin;
use crate::session::CallError;
use crate::state::AccountTicket;
use crate::state::{state, Inner, Phase};
use crate::tray::PollTicket;
use geometry::{Edge, Rect, Screen};
use view::Tab;

/// Answers younger than this are shown without asking the server again.
const FRESH: Duration = Duration::from_secs(15);
/// A click on the icon this soon after the panel hid on blur is the click that blurred it.
const BLUR_GUARD: Duration = Duration::from_millis(400);
const NOTIFICATIONS: &str = "25";
const WORK_PAGE: &str = "100";
pub const EVENT: &str = "panel://changed";

/// An API answer the panel keeps.
#[derive(Debug, Clone)]
pub struct Stored {
    pub body: Value,
    pub at: Instant,
}

impl Stored {
    fn new(body: Value) -> Self {
        Self {
            body,
            at: Instant::now(),
        }
    }
    fn fresh(&self) -> bool {
        self.at.elapsed() < FRESH
    }
}

/// The last answers per tab (for the server + sign-in in `owner`).
#[derive(Debug, Clone, Default)]
pub struct Kept {
    pub today: Option<Stored>,
    pub my_day: Option<Stored>,
    pub work: Option<Stored>,
    pub notes: Option<Stored>,
    pub timer: Option<Stored>,
    /// Today's work logs and the date they are for.
    pub entries: Option<(Stored, String)>,
}

/// Who the kept answers belong to: the server and the sign-in (`Inner::account_epoch`). Unlike
/// a poll ticket it holds while the server can't be reached (the panel shows the last data).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner {
    origin: ServerOrigin,
    account: u64,
}

/// Where the panel was opened: the inputs of `geometry::place`, kept for resizes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub screen: Screen,
    pub icon: Option<Rect>,
    pub fallback: Edge,
    /// Physical pixels per point of the target screen (1 on macOS, which places in points).
    pub scale: f64,
    /// macOS: positions in points (`LogicalPosition`); else physical pixels.
    pub logical: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PanelState {
    owner: Option<Owner>,
    pub kept: Kept,
    placement: Option<Placement>,
    height: Option<f64>,
    /// When the panel last hid because it lost the focus.
    hidden_at: Option<Instant>,
}

impl PanelState {
    /// A server switch / sign-out: the kept answers go (where it opened stays).
    pub fn clear_account(&mut self) {
        self.owner = None;
        self.kept = Kept::default();
    }

    /// Whether answers of some server + sign-in are kept.
    pub fn holds_answers(&self) -> bool {
        self.owner.is_some()
    }
}

fn signed_in(phase: Phase) -> bool {
    matches!(phase, Phase::InApp | Phase::Unreachable | Phase::Connecting)
}

/// The kept answers of the current server + sign-in (another one's are dropped first).
pub(crate) fn kept_for_current(inner: &mut Inner) -> Option<&mut Kept> {
    let owner = inner
        .current_origin()
        .filter(|_| signed_in(inner.phase))
        .map(|origin| Owner {
            origin,
            account: inner.account_epoch,
        });
    if inner.panel.owner != owner {
        inner.panel.kept = Kept::default();
        inner.panel.owner = owner.clone();
    }
    owner.map(|_| &mut inner.panel.kept)
}

fn is_offline(inner: &Inner) -> bool {
    crate::offline::is_offline(inner) || inner.phase != Phase::InApp
}

// ── the page's view ────────────────────────────────────────────────────────────

/// The header and tab bar.
#[derive(Debug, Clone, Serialize)]
pub struct Head {
    /// "ready" | "signed_out" | "no_server"
    state: &'static str,
    server: Option<String>,
    /// "online" | "offline" | "syncing" | "pending"
    status: &'static str,
    /// Offline changes waiting to be sent.
    pending: usize,
    unread: u64,
    tab: Tab,
    platform: &'static str,
    /// Timer / mark-read changes are off while offline on an older server (why).
    offline_hint: Option<&'static str>,
    /// The saved servers for the header's switcher (ids and names only).
    servers: Vec<crate::servers::Choice>,
}

fn head(app: &AppHandle) -> Head {
    let s = state(app);
    let inner = s.lock();
    let origin = inner.current_origin();
    let offline = is_offline(&inner);
    let loaded = origin.is_some() && inner.offline.origin == origin;
    let pending = if loaded { inner.offline.outbox.len() } else { 0 };
    let queues = crate::offline::extended_sync(&inner);
    let reads = if loaded {
        view::PendingReads::of(&inner.offline.outbox)
    } else {
        view::PendingReads::default()
    };
    let notes = inner
        .panel
        .kept
        .notes
        .as_ref()
        .filter(|_| inner.panel.owner.as_ref().map(|o| &o.origin) == origin.as_ref())
        .map(|n| &n.body);
    let unread = view::unread_after(inner.tray_data.unread.unwrap_or(0), notes, &reads);
    Head {
        state: match (&origin, signed_in(inner.phase)) {
            (None, _) => "no_server",
            (Some(_), false) => "signed_out",
            (Some(_), true) => "ready",
        },
        server: origin.map(|o| inner.settings.label_of(&o)),
        status: view::sync_status(offline, inner.offline.syncing, pending),
        pending,
        unread,
        tab: Tab::remembered(&inner.settings.preferences.panel_tab),
        platform: crate::api::platform(),
        offline_hint: view::needs_server(offline, queues),
        servers: crate::servers::choices(&inner.settings, cfg!(target_os = "macos")),
    }
}

/// One tab's content. Exactly one of the tab fields is set when there is something to show.
#[derive(Debug, Clone, Serialize)]
pub struct TabAnswer {
    tab: Tab,
    offline: bool,
    /// Why the last refresh failed (the kept data is still shown).
    error: Option<String>,
    /// Seconds since the shown data came from the server (`None`: just now / not known).
    age: Option<u64>,
    /// From the offline copy (no live answer in this session).
    from_offline_copy: bool,
    /// Timer / mark-read changes go to the outbox while offline (the server has their sync types).
    queue_offline: bool,
    today: Option<view::TodayView>,
    my_day: Option<view::MyDayView>,
    assigned: Option<view::AssignedView>,
    notifications: Option<view::NotificationsView>,
    timer: Option<view::TimerView>,
}

fn now_unix() -> i64 {
    crate::outbox::now() as i64
}

/// The user's date (`/me/today`, `/me/my-day`, the offline copy's My Day).
fn date_hint(inner: &Inner, kept: &Kept) -> Option<String> {
    let from = |s: &Option<Stored>| s.as_ref().and_then(|s| s.body["date"].as_str().map(str::to_string));
    from(&kept.today).or_else(|| from(&kept.my_day)).or_else(|| {
        inner
            .offline
            .cache
            .as_ref()
            .and_then(|c| crate::offline_cache::view(c, &inner.offline.outbox).my_day)
            .map(|d| d.date)
    })
}

/// Builds the answer for `tab` from what is kept (or the offline copy).
fn answer(app: &AppHandle, tab: Tab, error: Option<String>) -> TabAnswer {
    let s = state(app);
    let mut inner = s.lock();
    let offline = is_offline(&inner);
    if let Some(origin) = inner.current_origin().filter(|_| signed_in(inner.phase)) {
        inner.load_offline(&origin);
    }
    let pending = view::pending_done(&inner.offline.outbox);
    let reads = view::PendingReads::of(&inner.offline.outbox);
    let overlay = view::TimerOverlay::of(&inner.offline.outbox);
    let queue_offline = crate::offline::extended_sync(&inner);
    let needed = inner.offline.outbox.activity_needed.clone();
    let offline_view = inner
        .offline
        .cache
        .as_ref()
        .filter(|c| !c.is_empty())
        .map(|c| crate::offline_cache::view(c, &inner.offline.outbox));
    let unread = inner.tray_data.unread.unwrap_or(0);
    let timer_fallback = inner.tray_data.timer.clone();
    let origin = inner.current_origin();
    let kept = kept_for_current(&mut inner).cloned().unwrap_or_default();
    let date = date_hint(&inner, &kept);
    drop(inner);

    let mut out = TabAnswer {
        tab,
        offline,
        error,
        age: None,
        from_offline_copy: false,
        queue_offline,
        today: None,
        my_day: None,
        assigned: None,
        notifications: None,
        timer: None,
    };
    let age = |s: &Stored| s.at.elapsed().as_secs();
    match tab {
        Tab::Today => match (&kept.today, &offline_view) {
            (Some(t), _) => {
                out.age = Some(age(t));
                out.today = Some(view::today(&t.body, now_unix(), &pending));
            }
            (None, Some(v)) => {
                out.from_offline_copy = true;
                out.today = Some(view::offline_today(v, date.as_deref()));
            }
            _ => {}
        },
        Tab::MyDay => match (&kept.my_day, &offline_view) {
            (Some(d), _) => {
                out.age = Some(age(d));
                out.my_day = Some(view::my_day(&d.body, &pending));
            }
            (None, Some(v)) => {
                out.from_offline_copy = true;
                out.my_day = view::offline_my_day(v);
            }
            _ => {}
        },
        Tab::Assigned => match (&kept.work, &offline_view) {
            (Some(w), _) => {
                out.age = Some(age(w));
                out.assigned = Some(view::assigned(&w.body, date.as_deref(), &pending));
            }
            (None, Some(v)) => {
                out.from_offline_copy = true;
                out.assigned = Some(view::offline_assigned(v, date.as_deref()));
            }
            _ => {}
        },
        Tab::Notifications => {
            if let Some(n) = &kept.notes {
                out.age = Some(age(n));
                let has_link = |url: &str| {
                    origin
                        .as_ref()
                        .is_some_and(|o| crate::links::resolve_deep_link_target(o, url).is_some())
                };
                out.notifications = Some(view::notifications(&n.body, unread, &reads, has_link));
            }
        }
        Tab::Timer => {
            let running_id = kept
                .timer
                .as_ref()
                .and_then(|t| view::timer_id(&t.body))
                .or_else(|| timer_fallback.as_ref().and_then(|t| t.id.as_u64()));
            let running = match &kept.timer {
                Some(t) => {
                    out.age = Some(age(t));
                    view::running(&t.body, now_unix(), age(t))
                }
                // the poller's last answer (offline, or before the first refresh)
                None => timer_fallback.map(|t| view::RunningTimer {
                    key: None,
                    title: t.label.clone(),
                    description: None,
                    pending: false,
                    started_offline: false,
                    elapsed_seconds: t.elapsed_now(),
                    label: t.label,
                    started_at: None,
                    activity_required: t.activity_required,
                    default_activity_id: t.default_activity_id.as_ref().and_then(Value::as_u64),
                    activities: Vec::new(),
                    has_link: t.url.is_some(),
                }),
            };
            let (running, pending_note) = overlay.apply(running, running_id, now_unix());
            let today_seconds = kept.timer.as_ref().and_then(|t| view::today_seconds(&t.body));
            let (entries, total) = match (&kept.entries, today_seconds) {
                (Some((e, _)), None) => {
                    let (rows, total) = view::entries(&e.body);
                    (Some(rows), total)
                }
                _ => (None, 0),
            };
            let mut candidates = view::candidates(
                kept.my_day.as_ref().map(|s| &s.body),
                kept.work.as_ref().map(|s| &s.body),
            );
            if kept.my_day.is_none() && kept.work.is_none() {
                // offline without a live answer this session: the offline copy's open issues
                if let Some(v) = &offline_view {
                    candidates = view::offline_candidates(v);
                }
            }
            // the live answer is only compared with while online (offline it may be old)
            let live = kept.timer.as_ref().filter(|_| !offline).map(|t| &t.body);
            let activity_needed = view::activity_needed_view(needed.as_ref(), live);
            if running.is_some()
                || entries.is_some()
                || kept.timer.is_some()
                || pending_note.is_some()
                || activity_needed.is_some()
                || (offline && queue_offline && !candidates.is_empty())
            {
                out.timer = Some(view::TimerView {
                    running,
                    entries,
                    total_minutes: total,
                    today_seconds,
                    candidates,
                    description_supported: queue_offline,
                    pending_note,
                    activity_needed,
                });
            }
        }
    }
    out
}

// ── fetching ───────────────────────────────────────────────────────────────────

fn ticket(app: &AppHandle) -> Option<PollTicket> {
    PollTicket::now(&state(app).lock())
}

/// Stores an answer when it is still for the current server + sign-in.
fn keep(app: &AppHandle, ticket: &PollTicket, f: impl FnOnce(&mut Kept)) {
    let s = state(app);
    let mut inner = s.lock();
    if PollTicket::now(&inner).as_ref() != Some(ticket) {
        return;
    }
    if let Some(kept) = kept_for_current(&mut inner) {
        f(kept);
    }
}

fn is_fresh(app: &AppHandle, pick: impl FnOnce(&Kept) -> Option<&Stored>) -> bool {
    let s = state(app);
    let mut inner = s.lock();
    kept_for_current(&mut inner)
        .and_then(|k| pick(k).map(Stored::fresh))
        .unwrap_or(false)
}

async fn get(app: &AppHandle, ticket: &PollTicket, request: ApiRequest) -> Result<Value, CallError> {
    Ok(state(app).session.request(&ticket.origin, &request).await?.body)
}

async fn fetch_today(app: &AppHandle, t: &PollTicket) -> Result<(), CallError> {
    if let Some(body) = crate::tray::get_cached(app, t, "/api/v1/me/today").await? {
        keep(app, t, |k| k.today = Some(Stored::new(body)));
    }
    Ok(())
}

async fn fetch_my_day(app: &AppHandle, t: &PollTicket) -> Result<(), CallError> {
    if let Some(body) = crate::tray::get_cached(app, t, "/api/v1/me/my-day").await? {
        if let Some(items) = crate::tray::parse_my_day(&body) {
            let s = state(app);
            let mut inner = s.lock();
            if PollTicket::now(&inner).as_ref() == Some(t) {
                inner.tray_data.my_day = Some(items);
                inner.tray_data.my_day_stale = false;
            }
        }
        keep(app, t, |k| k.my_day = Some(Stored::new(body)));
    }
    Ok(())
}

async fn fetch_work(app: &AppHandle, t: &PollTicket) -> Result<(), CallError> {
    let request = ApiRequest::get("/api/v1/me/work")
        .query("status", "open")
        .query("limit", WORK_PAGE);
    if let Some(body) = crate::tray::get_cached_request(app, t, request).await? {
        keep(app, t, |k| k.work = Some(Stored::new(body)));
    }
    Ok(())
}

async fn fetch_notes(app: &AppHandle, t: &PollTicket) -> Result<(), CallError> {
    // the first page (no cursor) answers an ETag on newer servers
    let request = ApiRequest::get("/api/v1/notifications").query("limit", NOTIFICATIONS);
    if let Some(body) = crate::tray::get_cached_request(app, t, request).await? {
        keep(app, t, |k| k.notes = Some(Stored::new(body)));
    }
    if let Some(count) = crate::tray::get_cached(app, t, "/api/v1/notifications/unread-count")
        .await?
        .as_ref()
        .and_then(crate::tray::parse_unread)
    {
        set_unread(app, t, count);
    }
    Ok(())
}

async fn fetch_timer(app: &AppHandle, t: &PollTicket) -> Result<(), CallError> {
    if let Some(body) = crate::tray::get_cached(app, t, "/api/v1/me/timer").await? {
        crate::offline::note_timer_answer(app, &t.origin, &body);
        if let Some(timer) = crate::tray::parse_timer(&body) {
            let s = state(app);
            let mut inner = s.lock();
            if PollTicket::now(&inner).as_ref() == Some(t) {
                inner.tray_data.timer = timer;
                inner.tray_data.timer_known = true;
            }
        }
        // a stop that waited for an activity, for a timer that isn't running any more
        let running = view::timer_id(&body);
        let stale = crate::offline::read_outbox(app, |o, _| {
            o.activity_needed.as_ref().is_some_and(|n| Some(n.timer_id) != running)
        })
        .unwrap_or(false);
        if stale {
            crate::offline::with_outbox(app, &t.account(), |o| o.clear_activity_needed(None));
        }
        keep(app, t, |k| k.timer = Some(Stored::new(body)));
    }
    Ok(())
}

async fn fetch_entries(app: &AppHandle, t: &PollTicket) -> Result<(), CallError> {
    let date = {
        let s = state(app);
        let mut inner = s.lock();
        let kept = kept_for_current(&mut inner).cloned().unwrap_or_default();
        date_hint(&inner, &kept)
    };
    let date = match date {
        Some(d) => d,
        None => {
            fetch_today(app, t).await?;
            let s = state(app);
            let mut inner = s.lock();
            let kept = kept_for_current(&mut inner).cloned().unwrap_or_default();
            match date_hint(&inner, &kept) {
                Some(d) => d,
                None => return Ok(()),
            }
        }
    };
    let request = ApiRequest::get("/api/v1/work-logs")
        .query("from", date.clone())
        .query("to", date.clone())
        .query("limit", WORK_PAGE);
    let body = get(app, t, request).await?;
    keep(app, t, |k| k.entries = Some((Stored::new(body), date)));
    Ok(())
}

fn set_unread(app: &AppHandle, t: &PollTicket, count: u64) {
    let current = {
        let s = state(app);
        let mut inner = s.lock();
        let current = PollTicket::now(&inner).as_ref() == Some(t);
        if current {
            inner.tray_data.unread = Some(count);
        }
        current
    };
    if current {
        crate::bridge::set_badge(app, count as i64);
    }
}

/// Asks the server for what `tab` shows (answers younger than [`FRESH`] are kept as they are).
async fn refresh(app: &AppHandle, tab: Tab) -> Result<(), CallError> {
    let Some(t) = ticket(app) else { return Ok(()) };
    match tab {
        Tab::Today if !is_fresh(app, |k| k.today.as_ref()) => fetch_today(app, &t).await?,
        Tab::MyDay if !is_fresh(app, |k| k.my_day.as_ref()) => fetch_my_day(app, &t).await?,
        Tab::Assigned => {
            if !is_fresh(app, |k| k.work.as_ref()) {
                fetch_work(app, &t).await?;
            }
            // the overdue marks need the user's date
            if !is_fresh(app, |k| k.today.as_ref()) && !is_fresh(app, |k| k.my_day.as_ref()) {
                fetch_my_day(app, &t).await?;
            }
        }
        Tab::Notifications if !is_fresh(app, |k| k.notes.as_ref()) => fetch_notes(app, &t).await?,
        Tab::Timer => {
            if !is_fresh(app, |k| k.timer.as_ref()) {
                fetch_timer(app, &t).await?;
            }
            // today's total comes with the timer on newer servers; older ones: `/work-logs`
            let total_in_timer = {
                let s = state(app);
                let mut inner = s.lock();
                kept_for_current(&mut inner)
                    .and_then(|k| k.timer.as_ref())
                    .is_some_and(|t| view::today_seconds(&t.body).is_some())
            };
            if !total_in_timer && !is_fresh(app, |k| k.entries.as_ref().map(|(e, _)| e)) {
                fetch_entries(app, &t).await?;
            }
            // what a timer can be started on
            if !is_fresh(app, |k| k.my_day.as_ref()) {
                fetch_my_day(app, &t).await?;
            }
            if !is_fresh(app, |k| k.work.as_ref()) {
                fetch_work(app, &t).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Text for a failed call (shown above the kept data).
fn call_message(e: &CallError) -> String {
    match e {
        CallError::Offline(_) => "Brainitive can't reach the server. Showing what was loaded last.".into(),
        CallError::SignedOut(_) => "Your sign-in ended. Open Brainitive to sign in again.".into(),
        CallError::Server { status: 403, .. } => "You don't have access to this.".into(),
        CallError::Server { status: 404 | 410, .. } => "This is gone or you can no longer open it.".into(),
        CallError::Server { status: 429, .. } => "Too many requests. Try again in a minute.".into(),
        CallError::Server { status, message, .. } if *status == 422 && !message.is_empty() => message.clone(),
        CallError::Server { status, .. } => format!("The server answered with an error ({status})."),
        CallError::Failed(_) => "Something went wrong. Try again.".into(),
    }
}

fn ui(kind: &'static str, message: impl Into<String>) -> UiError {
    UiError {
        kind,
        message: message.into(),
    }
}

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), UiError> {
    popup::ensure_caller(app, webview, Popup::Panel).map_err(|m| ui("not_allowed", m))
}

/// The sign-in a panel command acts for, taken when it starts (before any network wait): what it
/// queues offline afterwards goes to that sign-in's outbox or nowhere (codex review 1).
fn owner(app: &AppHandle) -> Result<AccountTicket, UiError> {
    AccountTicket::now(&state(app).lock()).ok_or_else(|| ui("signed_out", "Sign in to Brainitive first."))
}

/// Whether the server can be asked now (signed in, in the app, not marked offline).
fn online_ticket(app: &AppHandle) -> Result<PollTicket, UiError> {
    let s = state(app);
    let inner = s.lock();
    if is_offline(&inner) {
        return Err(ui("offline", view::needs_server(true, false).unwrap_or_default()));
    }
    PollTicket::now(&inner).ok_or_else(|| ui("signed_out", "Sign in to Brainitive first."))
}

/// Where a timer / mark-read change goes.
enum Route {
    /// The server now.
    Api(PollTicket),
    /// The outbox: offline, and the server has the sync type for it.
    Outbox,
}

fn route(app: &AppHandle) -> Result<Route, UiError> {
    let s = state(app);
    let inner = s.lock();
    if is_offline(&inner) {
        if signed_in(inner.phase) && crate::offline::extended_sync(&inner) {
            return Ok(Route::Outbox);
        }
        return Err(ui("offline", view::needs_server(true, false).unwrap_or_default()));
    }
    PollTicket::now(&inner)
        .map(Route::Api)
        .ok_or_else(|| ui("signed_out", "Sign in to Brainitive first."))
}

/// A call that failed because the server can't be reached goes to the outbox instead, when the
/// server has the sync type for it.
fn queue_instead(app: &AppHandle, e: &CallError) -> bool {
    if !view::queue_after(e) {
        return false;
    }
    crate::offline::note_poll(app, &Err(e.clone()));
    crate::offline::extended_sync(&state(app).lock())
}

/// The panel's page reads everything again (data, unread count, sync status).
pub fn notify(app: &AppHandle) {
    let _ = app.emit_to(Popup::Panel.label(), EVENT, ());
}

/// After a change: the tray menu, the badge and the page follow; the poller asks soon.
fn changed(app: &AppHandle) {
    // `tray::refresh` also tells the panel's page (`notify`)
    crate::tray::refresh(app);
}

// ── commands ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn panel_view(app: AppHandle, webview: Webview) -> Result<Head, UiError> {
    caller(&app, &webview)?;
    Ok(head(&app))
}

/// One tab: what is kept at once (`refresh: false`), or after asking the server (`true`).
#[tauri::command]
pub async fn panel_load(app: AppHandle, webview: Webview, tab: String, refresh: bool) -> Result<TabAnswer, UiError> {
    caller(&app, &webview)?;
    let tab = Tab::parse(&tab).ok_or_else(|| ui("invalid", "Unknown tab."))?;
    let mut error = None;
    if refresh {
        let result = self::refresh(&app, tab).await;
        if let Err(e) = &result {
            log::info!("panel refresh ({}): {e}", tab.as_str());
            error = Some(call_message(e));
        }
        // the connection state follows what the panel saw, like the poller's
        match &result {
            Err(CallError::Offline(_)) => crate::offline::note_poll(&app, &result),
            Ok(()) if ticket(&app).is_some() => crate::offline::note_poll(&app, &result),
            _ => {}
        }
    }
    Ok(answer(&app, tab, error))
}

#[tauri::command]
pub fn panel_set_tab(app: AppHandle, webview: Webview, tab: String) -> Result<(), UiError> {
    caller(&app, &webview)?;
    let tab = Tab::parse(&tab).ok_or_else(|| ui("invalid", "Unknown tab."))?;
    let s = state(&app);
    let mut inner = s.lock();
    if inner.settings.preferences.panel_tab != tab.as_str() {
        inner.settings.preferences.panel_tab = tab.as_str().to_string();
        inner.save_settings();
    }
    Ok(())
}

/// The checkbox of My Day / assigned / due work: `PATCH /me/work/{id}` now, or the offline outbox
/// (`work.done`) when the server can't be reached.
#[tauri::command]
pub async fn panel_set_done(app: AppHandle, webview: Webview, work_id: String, done: bool) -> Result<(), UiError> {
    caller(&app, &webview)?;
    let owner = owner(&app)?;
    let (work, offline) = {
        let s = state(&app);
        let mut inner = s.lock();
        let offline = is_offline(&inner);
        let kept = kept_for_current(&mut inner).ok_or_else(|| ui("signed_out", "Sign in to Brainitive first."))?;
        let bodies: Vec<&Value> = [&kept.today, &kept.my_day, &kept.work]
            .into_iter()
            .flatten()
            .map(|s| &s.body)
            .collect();
        let found = view::find_work(&bodies, &work_id);
        // offline without a live answer this session: the offline copy's items count too
        let found = found.or_else(|| {
            inner
                .offline
                .cache
                .as_ref()
                .and_then(|c| {
                    c.work(&work_id)
                        .or_else(|| offline_day_item(c, &inner.offline.outbox, &work_id))
                })
                .map(|w| view::WorkRef {
                    label: w.label(),
                    done: w.done,
                    url: None,
                })
        });
        (
            found.ok_or_else(|| ui("not_found", "This item is not in the panel any more."))?,
            offline,
        )
    };
    if view::done_route(offline) == view::DoneRoute::Api {
        let t = online_ticket(&app)?;
        let request = ApiRequest::patch(format!("/api/v1/me/work/{work_id}"), json!({ "done": done }));
        match state(&app).session.request(&t.origin, &request).await {
            Ok(_) => {
                keep(&app, &t, |k| {
                    for s in [&mut k.today, &mut k.my_day, &mut k.work].into_iter().flatten() {
                        view::set_done(&mut s.body, &work_id, done);
                    }
                });
                {
                    let s = state(&app);
                    let mut inner = s.lock();
                    if let Some(items) = inner.tray_data.my_day.as_mut() {
                        for i in items.iter_mut().filter(|i| i.id == work_id) {
                            i.done = done;
                        }
                    }
                    inner.tray_data.my_day_stale = true;
                }
                state(&app).poll_now.notify_one();
                changed(&app);
                return Ok(());
            }
            Err(e) if view::queue_after(&e) => {
                crate::offline::note_poll(&app, &Err(e));
            }
            Err(e) => return Err(ui("server", call_message(&e))),
        }
    }
    crate::offline::queue_work_done(&app, &owner, &work_id, done, work.done, &work.label)?;
    changed(&app);
    Ok(())
}

/// A My Day item of the offline copy that is not in its work list (planned, not assigned).
fn offline_day_item(
    cache: &crate::offline_cache::Cache,
    outbox: &crate::outbox::Outbox,
    work_id: &str,
) -> Option<crate::offline_cache::ViewWork> {
    crate::offline_cache::view(cache, outbox)
        .my_day?
        .items
        .into_iter()
        .find(|w| w.id == work_id)
}

/// What a click in the panel opens (by reference: the URL comes from the kept answer).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum OpenTarget {
    Work(String),
    Event(String),
    Notification(String),
    Timer,
    MyWork,
    Notifications,
    Calendar,
}

/// Opens an item / event / notification (marked read) or a fixed page in the main window.
#[tauri::command]
pub async fn panel_open(app: AppHandle, webview: Webview, target: OpenTarget) -> Result<(), UiError> {
    caller(&app, &webview)?;
    let owner = owner(&app)?;
    let mut mark_read: Option<(String, String)> = None;
    let path = {
        let s = state(&app);
        let mut inner = s.lock();
        let timer_url = inner.tray_data.timer.as_ref().and_then(|t| t.url.clone());
        let kept = kept_for_current(&mut inner).ok_or_else(|| ui("signed_out", "Sign in to Brainitive first."))?;
        let bodies: Vec<&Value> = [&kept.today, &kept.my_day, &kept.work]
            .into_iter()
            .flatten()
            .map(|s| &s.body)
            .collect();
        match &target {
            OpenTarget::Work(id) => {
                let work = view::find_work(&bodies, id).ok_or_else(|| ui("not_found", "Unknown item."))?;
                work.url.unwrap_or_else(|| "/my-work".into())
            }
            OpenTarget::Event(id) => kept
                .today
                .as_ref()
                .and_then(|t| view::find_event(&t.body, id))
                .ok_or_else(|| ui("not_found", "Unknown event."))?
                .unwrap_or_else(|| "/calendar".into()),
            OpenTarget::Notification(id) => {
                let (read, url) = kept
                    .notes
                    .as_ref()
                    .and_then(|n| view::find_notification(&n.body, id))
                    .ok_or_else(|| ui("not_found", "Unknown notification."))?;
                if !read {
                    let title = kept
                        .notes
                        .as_ref()
                        .map(|n| view::note_title(&n.body, id))
                        .unwrap_or_default();
                    mark_read = Some((id.clone(), title));
                }
                url.unwrap_or_else(|| "/notifications".into())
            }
            OpenTarget::Timer => kept
                .timer
                .as_ref()
                .and_then(|t| t.body["timer"]["issue"]["url"].as_str().map(str::to_string))
                .or(timer_url)
                .unwrap_or_else(|| "/my-work".into()),
            OpenTarget::MyWork => "/my-work".into(),
            OpenTarget::Notifications => "/notifications".into(),
            OpenTarget::Calendar => "/calendar".into(),
        }
    };
    popup::hide(&app, Popup::Panel);
    // a server page (relative or on this origin, never an auth page): else just the app
    let allowed = state(&app)
        .lock()
        .current_origin()
        .is_some_and(|o| crate::links::resolve_deep_link_target(&o, &path).is_some());
    if allowed {
        crate::shell::open_page(&app, &path);
    } else {
        crate::shell::focus_main(&app);
    }
    if let Some((id, title)) = mark_read {
        match route(&app) {
            Ok(Route::Api(t)) if t.account() == owner => read_one(&app, &t, &id, &title).await,
            Ok(Route::Outbox) => queue_read(&app, &owner, &id, &title),
            Ok(Route::Api(_)) => {}
            Err(_) => {}
        }
    }
    Ok(())
}

/// A notification read offline: `notification.read` in the outbox.
fn queue_read(app: &AppHandle, owner: &AccountTicket, id: &str, title: &str) {
    if !view::is_notification_id(id) {
        return;
    }
    let change = crate::outbox::Change::NotificationRead {
        notification_id: id.to_string(),
        title: title.to_string(),
    };
    if let Err(e) = crate::offline::queue(app, owner, change) {
        log::info!("notification read not queued: {}", e.message);
    }
    changed(app);
}

async fn read_one(app: &AppHandle, t: &PollTicket, id: &str, title: &str) {
    if !view::is_notification_id(id) {
        return;
    }
    let request = ApiRequest::post(format!("/api/v1/notifications/{id}/read"), json!({}));
    match state(app).session.request(&t.origin, &request).await {
        Ok(response) => {
            keep(app, t, |k| {
                if let Some(n) = k.notes.as_mut() {
                    view::mark_read(&mut n.body, Some(id));
                }
            });
            if let Some(count) = crate::tray::parse_unread(&response.body) {
                set_unread(app, t, count);
            }
            changed(app);
        }
        Err(e) if queue_instead(app, &e) => queue_read(app, &t.account(), id, title),
        Err(e) => log::info!("notification not marked read: {e}"),
    }
}

#[tauri::command]
pub async fn panel_mark_all_read(app: AppHandle, webview: Webview) -> Result<(), UiError> {
    caller(&app, &webview)?;
    let owner = owner(&app)?;
    // offline: only what the user saw (newest shown), so a later one stays unread
    let before = {
        let s = state(&app);
        let mut inner = s.lock();
        let kept = kept_for_current(&mut inner);
        view::read_all_before(kept.and_then(|k| k.notes.as_ref()).map(|n| &n.body), now_unix())
    };
    let queue = |app: &AppHandle| -> Result<(), UiError> {
        crate::offline::queue(
            app,
            &owner,
            crate::outbox::Change::NotificationsReadAll { before: before.clone() },
        )?;
        changed(app);
        Ok(())
    };
    let t = match route(&app)? {
        Route::Api(t) => t,
        Route::Outbox => return queue(&app),
    };
    let request = ApiRequest::post("/api/v1/notifications/read-all", json!({}));
    match state(&app).session.request(&t.origin, &request).await {
        Ok(_) => {}
        Err(e) if queue_instead(&app, &e) => return queue(&app),
        Err(e) => return Err(ui("server", call_message(&e))),
    }
    keep(&app, &t, |k| {
        if let Some(n) = k.notes.as_mut() {
            view::mark_read(&mut n.body, None);
        }
    });
    set_unread(&app, &t, 0);
    changed(&app);
    Ok(())
}

/// The live state a sync result brings (`timer`, `notification`, `unread_count`): the tray, the
/// badge and the panel show what the server has now (also for a refused offline start: the timer
/// that runs).
pub fn apply_sync_states(app: &AppHandle, origin: &ServerOrigin, states: &[crate::outbox::SyncResult]) {
    if states.is_empty() {
        return;
    }
    let Some(t) = ticket(app).filter(|t| &t.origin == origin) else {
        return;
    };
    for r in states {
        let Some((kind, entity)) = r.live_state() else { continue };
        match kind {
            "timer" => {
                crate::offline::note_timer_answer(app, origin, entity);
                let running = view::timer_id(entity);
                let stale = crate::offline::read_outbox(app, |o, _| {
                    o.activity_needed.as_ref().is_some_and(|n| Some(n.timer_id) != running)
                })
                .unwrap_or(false);
                if stale {
                    crate::offline::with_outbox(app, &t.account(), |o| o.clear_activity_needed(None));
                }
                timer_changed(app, &t, entity.clone());
            }
            "notification" => keep(app, &t, |k| {
                if let Some(n) = k.notes.as_mut() {
                    view::replace_note(&mut n.body, entity);
                }
            }),
            "unread_count" => {
                if let Some(count) = crate::tray::parse_unread(entity) {
                    set_unread(app, &t, count);
                }
            }
            _ => {}
        }
    }
    state(app).poll_now.notify_one();
    changed(app);
}

/// After a timer change: the tray shows it, today's entries are asked again.
fn timer_changed(app: &AppHandle, t: &PollTicket, body: Value) {
    let timer = crate::tray::parse_timer(&body).flatten();
    {
        let s = state(app);
        let mut inner = s.lock();
        if PollTicket::now(&inner).as_ref() == Some(t) {
            inner.tray_data.timer = timer;
            inner.tray_data.timer_known = true;
        }
    }
    keep(app, t, |k| {
        k.timer = Some(Stored::new(body));
        k.entries = None;
    });
    state(app).poll_now.notify_one();
    changed(app);
}

/// The label of `work_id` when the panel offered it to start a timer on (My Day / assigned, or
/// the offline copy's open issues when nothing live was loaded).
fn offered(app: &AppHandle, work_id: &str) -> Option<String> {
    let s = state(app);
    let mut inner = s.lock();
    let offline_view = inner
        .offline
        .cache
        .as_ref()
        .filter(|c| !c.is_empty())
        .map(|c| crate::offline_cache::view(c, &inner.offline.outbox));
    let kept = kept_for_current(&mut inner).cloned().unwrap_or_default();
    let mut list = view::candidates(
        kept.my_day.as_ref().map(|s| &s.body),
        kept.work.as_ref().map(|s| &s.body),
    );
    if kept.my_day.is_none() && kept.work.is_none() {
        list = offline_view.as_ref().map(view::offline_candidates).unwrap_or_default();
    }
    list.into_iter().find(|c| c.id == work_id).map(|c| c.label)
}

/// The timer's optional description: one line, ≤ 255 characters, empty = none.
fn clean_description(text: Option<String>) -> Option<String> {
    let text: String = text?.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.chars().take(255).collect())
}

/// A timer started offline: `timer.start` with the device's clock. Not while another one shows as
/// running (the server would refuse it as a conflict anyway).
fn queue_start(
    app: &AppHandle,
    owner: &AccountTicket,
    work_id: &str,
    label: &str,
    description: Option<String>,
) -> Result<(), UiError> {
    let issue_id = crate::outbox::work_ref(work_id)
        .filter(|(kind, _)| *kind == "board")
        .map(|(_, id)| id)
        .ok_or_else(|| ui("invalid", "Timers run on issues only."))?;
    let busy = {
        let s = state(app);
        let mut inner = s.lock();
        let overlay = view::TimerOverlay::of(&inner.offline.outbox);
        let tray = inner.tray_data.timer.as_ref().and_then(|t| t.id.as_u64());
        let kept = kept_for_current(&mut inner).and_then(|k| k.timer.as_ref().map(|t| view::timer_id(&t.body)));
        let running = kept.unwrap_or(tray);
        overlay.start.is_some() || running.is_some_and(|id| !overlay.ended.iter().any(|(t, _)| *t == id))
    };
    if busy {
        return Err(ui("conflict", "Another timer is running. Stop it first."));
    }
    crate::offline::queue(
        app,
        owner,
        crate::outbox::Change::TimerStart {
            issue_id,
            description,
            started_at: crate::outbox::iso_utc(now_unix()),
            title: label.chars().take(crate::outbox::MAX_TITLE).collect(),
        },
    )?;
    changed(app);
    Ok(())
}

/// Start a timer on an issue the panel offered (My Day / assigned), with an optional description
/// (newer servers). Offline on a server that has `timer.start`: queued in the outbox.
#[tauri::command]
pub async fn panel_timer_start(
    app: AppHandle,
    webview: Webview,
    work_id: String,
    description: Option<String>,
) -> Result<(), UiError> {
    caller(&app, &webview)?;
    let owner = owner(&app)?;
    let label = offered(&app, &work_id).ok_or_else(|| ui("not_found", "Pick an issue from the list."))?;
    let supported = crate::offline::extended_sync(&state(&app).lock());
    let description = clean_description(description).filter(|_| supported);
    let t = match route(&app)? {
        Route::Api(t) => t,
        Route::Outbox => return queue_start(&app, &owner, &work_id, &label, description),
    };
    let mut body = json!({ "id": work_id });
    if let Some(d) = &description {
        body["description"] = json!(d);
    }
    let request = ApiRequest::post("/api/v1/me/timer/start", body);
    match state(&app).session.request(&t.origin, &request).await {
        Ok(response) => {
            timer_changed(&app, &t, response.body);
            Ok(())
        }
        Err(CallError::Server { status: 409, .. }) => Err(ui("conflict", "Another timer is running. Stop it first.")),
        Err(CallError::Server { status: 403, .. }) => Err(ui("forbidden", "You can't log time on this issue.")),
        Err(e) if queue_instead(&app, &e) => queue_start(&app, &owner, &work_id, &label, description),
        Err(e) => Err(ui("server", call_message(&e))),
    }
}

/// The running timer as the panel knows it: the kept answer, else the poller's (offline), as a
/// `GET /me/timer`-shaped body. `Err`: only a start made offline runs, which has no id yet.
fn current_timer(app: &AppHandle) -> Result<Value, UiError> {
    let s = state(app);
    let mut inner = s.lock();
    let start = inner.offline.outbox.pending_start().map(|(_, sent)| sent);
    let ended: Vec<u64> = view::TimerOverlay::of(&inner.offline.outbox)
        .ended
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let tray = inner.tray_data.timer.clone();
    let kept = kept_for_current(&mut inner).and_then(|k| k.timer.as_ref().map(|t| t.body.clone()));
    let body = match (kept, tray) {
        (Some(body), _) => body,
        (None, Some(t)) => json!({
            "running": true,
            "timer": {"id": t.id, "issue": {"title": t.label}},
            "activity_required": t.activity_required,
            "default_activity_id": t.default_activity_id,
        }),
        (None, None) => Value::Null,
    };
    match view::timer_id(&body) {
        Some(id) if !ended.contains(&id) => Ok(body),
        _ if start == Some(false) => Err(ui(
            "pending",
            "This timer was started offline. Discard it, or stop it once it has synced.",
        )),
        _ if start == Some(true) => Err(ui("pending", "This timer is being synced. Try again in a moment.")),
        _ => Err(ui("not_found", "No timer is running.")),
    }
}

/// A stop made offline: `timer.stop` with the device's clock.
fn queue_stop(
    app: &AppHandle,
    owner: &AccountTicket,
    timer_id: u64,
    activity_id: Option<u64>,
    title: String,
) -> Result<StopOutcome, UiError> {
    crate::offline::queue(
        app,
        owner,
        crate::outbox::Change::TimerStop {
            timer_id,
            stopped_at: crate::outbox::iso_utc(now_unix()),
            activity_id,
            title,
        },
    )?;
    changed(app);
    Ok(StopOutcome {
        stopped: true,
        needs_activity: false,
        message: "Timer stopped. The time is logged once Brainitive is back online.".into(),
    })
}

/// The result of Stop.
#[derive(Debug, Clone, Serialize)]
pub struct StopOutcome {
    stopped: bool,
    /// Pick an activity (the timer's `activities`, shown by the page) and stop again.
    needs_activity: bool,
    message: String,
}

/// Stop and log. An activity is needed where the workspace requires one: the default one, the
/// one picked in the panel (one of the timer's `activities`), or the page shows the choice.
#[tauri::command]
pub async fn panel_timer_stop(
    app: AppHandle,
    webview: Webview,
    activity_id: Option<u64>,
) -> Result<StopOutcome, UiError> {
    caller(&app, &webview)?;
    let owner = owner(&app)?;
    // a stop refused offline for want of an activity: the pick sends it again (same stop time)
    let waiting = crate::offline::read_outbox(&app, |o, _| o.activity_needed.clone()).flatten();
    if let (Some(_), Some(id)) = (&waiting, activity_id) {
        crate::offline::resend_stop(&app, &owner, id)?;
        changed(&app);
        return Ok(StopOutcome {
            stopped: true,
            needs_activity: false,
            message: "Logging the time…".into(),
        });
    }
    let body = current_timer(&app)?;
    let timer_id = view::timer_id(&body).ok_or_else(|| ui("not_found", "No timer is running."))?;
    let title = view::running(&body, now_unix(), 0).map(|r| r.label).unwrap_or_default();
    let choices = view::activities(&body);
    let activity = match activity_id {
        Some(id) if choices.iter().any(|a| a.id == id) => Some(id),
        Some(_) => return Err(ui("invalid", "Pick one of the activities shown.")),
        None if body["activity_required"] == true => match body["default_activity_id"].as_u64() {
            Some(id) => Some(id),
            None => {
                return Ok(StopOutcome {
                    stopped: false,
                    needs_activity: true,
                    message: "Choose what to log this time as.".into(),
                })
            }
        },
        None => None,
    };
    let t = match route(&app)? {
        Route::Api(t) => t,
        Route::Outbox => return queue_stop(&app, &owner, timer_id, activity, title),
    };
    let mut request = json!({ "timer_id": timer_id });
    if let Some(id) = activity {
        request["activity_id"] = json!(id);
    }
    match state(&app)
        .session
        .request(&t.origin, &ApiRequest::post("/api/v1/me/timer/stop", request))
        .await
    {
        Ok(response) => {
            crate::offline::with_outbox(&app, &owner, |o| o.clear_activity_needed(Some(timer_id)));
            let message = match response.body["result"].as_str() {
                Some("logged") => "Time logged.",
                Some("already_stopped") => "The timer was already stopped.",
                _ => "The time could not be logged on this issue any more.",
            };
            timer_changed(&app, &t, json!({ "running": false, "timer": null }));
            Ok(StopOutcome {
                stopped: true,
                needs_activity: false,
                message: message.into(),
            })
        }
        Err(CallError::Server {
            status: 422, message, ..
        }) => {
            // the timer keeps running: show the activities (asked again, they may have changed)
            let _ = fetch_timer(&app, &t).await;
            notify(&app);
            Ok(StopOutcome {
                stopped: false,
                needs_activity: true,
                message: if message.is_empty() {
                    "Choose what to log this time as.".into()
                } else {
                    message
                },
            })
        }
        Err(e) if queue_instead(&app, &e) => queue_stop(&app, &owner, timer_id, activity, title),
        Err(e) => Err(ui("server", call_message(&e))),
    }
}

/// Stop without logging.
#[tauri::command]
pub async fn panel_timer_discard(app: AppHandle, webview: Webview) -> Result<(), UiError> {
    caller(&app, &webview)?;
    let owner = owner(&app)?;
    // a timer started offline and not sent yet: the start is taken back
    let unsent = crate::offline::read_outbox(&app, |o, _| o.pending_start().is_some_and(|(_, sent)| !sent));
    if unsent == Some(true) && crate::offline::with_outbox(&app, &owner, |o| o.cancel_unsent_start()) == Some(true) {
        changed(&app);
        return Ok(());
    }
    let body = current_timer(&app)?;
    let timer_id = view::timer_id(&body).ok_or_else(|| ui("not_found", "No timer is running."))?;
    let title = view::running(&body, now_unix(), 0).map(|r| r.label).unwrap_or_default();
    let queue = |app: &AppHandle| -> Result<(), UiError> {
        crate::offline::queue(
            app,
            &owner,
            crate::outbox::Change::TimerDiscard {
                timer_id,
                title: title.clone(),
            },
        )?;
        changed(app);
        Ok(())
    };
    let t = match route(&app)? {
        Route::Api(t) => t,
        Route::Outbox => return queue(&app),
    };
    let request = ApiRequest::post("/api/v1/me/timer/discard", json!({ "timer_id": timer_id }));
    let response = match state(&app).session.request(&t.origin, &request).await {
        Ok(r) => r,
        Err(e) if queue_instead(&app, &e) => return queue(&app),
        Err(e) => return Err(ui("server", call_message(&e))),
    };
    crate::offline::with_outbox(&app, &owner, |o| o.clear_activity_needed(Some(timer_id)));
    let body = if response.body.get("running").is_some() {
        response.body
    } else {
        json!({ "running": false, "timer": null })
    };
    timer_changed(&app, &t, body);
    Ok(())
}

/// The footer and the signed-out button.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PanelAction {
    Capture,
    /// The New task window.
    Task,
    Open,
    Settings,
    SignIn,
    /// The header's switcher: Settings → Servers.
    ManageServers,
}

#[tauri::command]
pub fn panel_run(app: AppHandle, webview: Webview, action: PanelAction) -> Result<(), UiError> {
    caller(&app, &webview)?;
    popup::hide(&app, Popup::Panel);
    match action {
        PanelAction::Capture => popup::show(&app, Popup::Capture),
        PanelAction::Task => crate::new_task::show(&app),
        PanelAction::Settings => popup::show(&app, Popup::Settings),
        PanelAction::ManageServers => crate::menu::manage_servers(&app),
        PanelAction::Open | PanelAction::SignIn => crate::shell::focus_main(&app),
    }
    Ok(())
}

/// The header's switcher: a saved server by its id (never an origin). The panel hides; the
/// switch destroys it with the old server's answers, so it opens fresh for the new one.
#[tauri::command]
pub fn panel_switch_server(app: AppHandle, webview: Webview, id: String) -> Result<(), UiError> {
    caller(&app, &webview)?;
    crate::flows::saved_server(&app, &id)?;
    popup::hide(&app, Popup::Panel);
    crate::menu::pick_server(&app, &id);
    Ok(())
}

/// The page's content height (points): the window follows, within 220…560.
#[tauri::command]
pub fn panel_resize(app: AppHandle, webview: Webview, height: f64) -> Result<(), UiError> {
    caller(&app, &webview)?;
    let height = geometry::clamp_height(height);
    let placement = {
        let s = state(&app);
        let mut inner = s.lock();
        if inner.panel.height == Some(height) {
            return Ok(());
        }
        inner.panel.height = Some(height);
        inner.panel.placement
    };
    if let (Some(placement), Some(window)) = (placement, app.get_webview_window(Popup::Panel.label())) {
        place_window(&window, &placement, height);
    }
    Ok(())
}

/// Esc.
#[tauri::command]
pub fn panel_close(app: AppHandle, webview: Webview) -> Result<(), UiError> {
    caller(&app, &webview)?;
    popup::dismiss(&app, Popup::Panel);
    Ok(())
}

// ── the tray icon and the window ───────────────────────────────────────────────

/// What a click on the tray icon does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickAction {
    TogglePanel,
    /// The native menu (⌃-click on macOS; right click opens it by itself).
    ShowMenu,
    /// Right click: the native menu opens, the panel goes.
    HidePanel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Other,
}

/// macOS: on mouse-down like the system's menu-bar extras, ⌃-click = right click. Windows: on
/// mouse-up. Linux: AppIndicator reports no clicks (the menu has "Open Panel…").
pub fn click_action(os: &str, button: Button, down: bool, control: bool) -> Option<ClickAction> {
    match (os, button, down) {
        ("macos", Button::Left, true) if control => Some(ClickAction::ShowMenu),
        ("macos", Button::Left, true) => Some(ClickAction::TogglePanel),
        ("windows", Button::Left, false) => Some(ClickAction::TogglePanel),
        ("macos" | "windows", Button::Right, true) => Some(ClickAction::HidePanel),
        _ => None,
    }
}

/// Open, close, or nothing (the click that just blurred the panel away).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Show,
    Hide,
    Ignore,
}

pub fn toggle_decision(visible: bool, hidden_at: Option<Instant>, now: Instant) -> Toggle {
    if visible {
        return Toggle::Hide;
    }
    match hidden_at {
        Some(at) if now.saturating_duration_since(at) < BLUR_GUARD => Toggle::Ignore,
        _ => Toggle::Show,
    }
}

/// Every event of the tray icon (`TrayIconBuilder::on_tray_icon_event`, main thread).
pub fn on_tray_event(app: &AppHandle, event: TrayIconEvent) {
    let TrayIconEvent::Click {
        button,
        button_state,
        rect,
        ..
    } = event
    else {
        return;
    };
    let button = match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        _ => Button::Other,
    };
    let down = button_state == MouseButtonState::Down;
    match click_action(crate::api::platform(), button, down, control_held()) {
        Some(ClickAction::TogglePanel) => toggle(app, Some(rect)),
        Some(ClickAction::ShowMenu) => {
            hide(app);
            show_menu(app);
        }
        Some(ClickAction::HidePanel) => hide(app),
        None => {}
    }
}

/// The panel's corner radius (points), the glass and the page's frame use the same.
pub const RADIUS: f64 = 12.0;

/// The panel window (on top of `popup::build`'s floating style): macOS gets the system's popover
/// material (an NSVisualEffectView behind the transparent page, light / dark by itself) with
/// rounded corners; Windows / Linux an opaque page with a shadow.
pub fn window_style<'a>(
    builder: tauri::WebviewWindowBuilder<'a, tauri::Wry, AppHandle>,
) -> tauri::WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    #[cfg(target_os = "macos")]
    let builder = builder
        .transparent(true)
        .effects(tauri::utils::config::WindowEffectsConfig {
            effects: vec![tauri::window::Effect::Popover],
            state: Some(tauri::window::EffectState::Active),
            radius: Some(RADIUS),
            color: None,
            interactive: false,
        });
    builder.shadow(true)
}

/// ⌃ held right now (macOS `NSEvent.modifierFlags`).
#[cfg(target_os = "macos")]
fn control_held() -> bool {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    const CONTROL: usize = 1 << 18; // NSEventModifierFlagControl
    let Some(cls) = AnyClass::get(c"NSEvent") else {
        return false;
    };
    let flags: usize = unsafe { msg_send![cls, modifierFlags] };
    flags & CONTROL != 0
}

#[cfg(not(target_os = "macos"))]
fn control_held() -> bool {
    false
}

/// Opens the native tray menu (⌃-click). The menu runs its own loop on the main thread, so it
/// is asked for from a blocking task, never from inside the click handler.
fn show_menu(app: &AppHandle) {
    let Some(tray) = state(app).lock().tray.clone() else {
        return;
    };
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = tray.with_inner_tray_icon(|t| t.show_menu()) {
            log::warn!("tray menu: {e}");
        }
    });
}

fn hide(app: &AppHandle) {
    if app
        .get_webview_window(Popup::Panel.label())
        .is_some_and(|w| w.is_visible().unwrap_or(false))
    {
        popup::dismiss(app, Popup::Panel);
    }
}

/// `WindowEvent::Focused(false)` hid the panel (see [`toggle_decision`]).
pub fn hidden_by_blur(app: &AppHandle) {
    state(app).lock().panel.hidden_at = Some(Instant::now());
}

/// Left click on the icon (or "Open Panel…" in the menu on Linux, without a rect).
pub fn toggle(app: &AppHandle, icon: Option<tauri::Rect>) {
    let visible = app
        .get_webview_window(Popup::Panel.label())
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    let hidden_at = state(app).lock().panel.hidden_at;
    match toggle_decision(visible, hidden_at, Instant::now()) {
        Toggle::Hide => popup::dismiss(app, Popup::Panel),
        Toggle::Ignore => {}
        Toggle::Show => show(app, icon),
    }
}

fn show(app: &AppHandle, icon: Option<tauri::Rect>) {
    let placement = placement(app, icon);
    let height = {
        let s = state(app);
        let mut inner = s.lock();
        inner.panel.placement = placement;
        inner.panel.height.unwrap_or(geometry::MIN_HEIGHT * 2.0)
    };
    popup::show_with(app, Popup::Panel, move |window| {
        if let Some(p) = placement {
            place_window(window, &p, height);
        }
        notify(window.app_handle());
    });
}

fn place_window(window: &WebviewWindow, p: &Placement, height: f64) {
    let size = (geometry::WIDTH * p.scale, height * p.scale);
    let (x, y) = geometry::place(&p.screen, p.icon, size, p.fallback);
    if p.logical {
        let _ = window.set_size(tauri::LogicalSize::new(geometry::WIDTH, height));
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    } else {
        let _ = window.set_position(tauri::PhysicalPosition::new(x.round() as i32, y.round() as i32));
        let _ = window.set_size(tauri::PhysicalSize::new(size.0.round() as u32, size.1.round() as u32));
    }
}

fn pair(position: &tauri::Position) -> (f64, f64) {
    match position {
        tauri::Position::Physical(p) => (p.x as f64, p.y as f64),
        tauri::Position::Logical(p) => (p.x, p.y),
    }
}

fn dims(size: &tauri::Size) -> (f64, f64) {
    match size {
        tauri::Size::Physical(s) => (s.width as f64, s.height as f64),
        tauri::Size::Logical(s) => (s.width, s.height),
    }
}

/// The screens and the icon in one coordinate space (see `geometry`).
fn placement(app: &AppHandle, icon: Option<tauri::Rect>) -> Option<Placement> {
    let monitors = app.available_monitors().ok().filter(|m| !m.is_empty())?;
    let logical = cfg!(target_os = "macos");
    let screens: Vec<(Screen, f64)> = monitors
        .iter()
        .map(|m| {
            let scale = m.scale_factor();
            let bounds = Rect::new(
                m.position().x as f64,
                m.position().y as f64,
                m.size().width as f64,
                m.size().height as f64,
            );
            let wa = m.work_area();
            let work = Rect::new(
                wa.position.x as f64,
                wa.position.y as f64,
                wa.size.width as f64,
                wa.size.height as f64,
            );
            if logical {
                (
                    Screen {
                        bounds: bounds.scaled(scale),
                        work: work.scaled(scale),
                    },
                    scale,
                )
            } else {
                (Screen { bounds, work }, scale)
            }
        })
        .collect();
    let list: Vec<Screen> = screens.iter().map(|(s, _)| *s).collect();
    let icon_rect = icon.map(|r| {
        let (x, y) = pair(&r.position);
        let (w, h) = dims(&r.size);
        let physical = Rect::new(x, y, w, h);
        if logical {
            // the icon's own screen scale: the one whose points contain it
            screens
                .iter()
                .map(|(s, scale)| (physical.scaled(*scale), s))
                .find(|(r, s)| s.bounds.contains(r.center()))
                .map(|(r, _)| r)
                .unwrap_or_else(|| physical.scaled(screens[0].1))
        } else {
            physical
        }
    });
    let anchor = match icon_rect {
        Some(r) => r.center(),
        None => match app.cursor_position() {
            Ok(p) if logical => {
                let point = Rect::new(p.x, p.y, 0.0, 0.0);
                screens
                    .iter()
                    .map(|(s, scale)| (point.scaled(*scale), s))
                    .find(|(r, s)| s.bounds.contains((r.x, r.y)))
                    .map(|(r, _)| (r.x, r.y))
                    .unwrap_or((list[0].bounds.x, list[0].bounds.y))
            }
            Ok(p) => (p.x, p.y),
            Err(_) => (list[0].bounds.x, list[0].bounds.y),
        },
    };
    let i = geometry::pick_screen(&list, anchor)?;
    Some(Placement {
        screen: list[i],
        icon: icon_rect,
        fallback: Edge::default_for(crate::api::platform()),
        scale: if logical { 1.0 } else { screens[i].1 },
        logical,
    })
}

#[cfg(test)]
mod tests;
