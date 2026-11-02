//! Offline mode (BRAI-366): the offline window's commands, the sync with the server and the files
//! per server. The cache (`offline_cache.rs`) and the outbox (`outbox.rs`) are pure; this module
//! loads / stores them and calls the API from Rust with the stored app token (the token never
//! reaches the window).
//!
//! Files: `<app local data>/offline/<store id>/cache.json` and `outbox.json` (0600, atomic).
//! A sync = check whose data it is (`GET /me` once per session: another account → both are
//! deleted) → send the outbox (`POST /api/v1/sync`, in order) → pull the changes
//! (`GET /api/v1/sync/changes`). It runs from the poller while the app is open, from "Sync now"
//! (tray, window) and after every change made in the window. Sign-out and forget delete the files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Webview};

use crate::api::ApiRequest;
use crate::flows::UiError;
use crate::offline_cache::{self, Cache, FeedPage};
use crate::outbox::{self, Change, Entry, Outbox, BATCH};
use crate::popup::{self, Popup};
use crate::queue::CaptureInput;
use crate::server::{self, ServerOrigin};
use crate::session::CallError;
use crate::state::{state, Inner, Phase};

/// The offline data of the current server, kept in `AppState`.
pub struct OfflineData {
    /// `<app local data>/offline`.
    pub base_dir: PathBuf,
    /// The server whose files are loaded.
    pub origin: Option<ServerOrigin>,
    pub cache: Option<Cache>,
    pub outbox: Outbox,
    /// `GET /me` matched the cache's account in this session.
    pub verified: bool,
    /// `Some(false)`: the server can't be reached (shown as "Offline").
    pub online: Option<bool>,
    pub syncing: bool,
    /// Something changed while a sync ran: one more.
    pub again: bool,
    /// The window opened by itself once in this outage.
    pub auto_opened: bool,
    /// Why the last sync failed (shown in the window).
    pub last_error: Option<String>,
    /// Bumped by a wipe and whenever another server's data is loaded (or the data is unloaded on
    /// a switch): a sync that started before it stops and writes nothing back — never another
    /// server's outbox or cache.
    pub epoch: u64,
    /// Bumped whenever the loaded server changes (load / unload): a sync of the previous load no
    /// longer touches `syncing` / `last_error` / `online` of this one.
    pub load: u64,
    /// The generation of the last snapshot / clear (see [`next_generation`]).
    generation: u64,
}

impl OfflineData {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            origin: None,
            cache: None,
            outbox: Outbox::default(),
            verified: false,
            online: None,
            syncing: false,
            again: false,
            auto_opened: false,
            last_error: None,
            epoch: 0,
            load: 0,
            generation: 0,
        }
    }

    pub fn dir_for(&self, origin: &ServerOrigin) -> PathBuf {
        self.base_dir.join(server::hex(&origin.store_id()))
    }

    /// Loads `origin`'s files when another server's (or nothing) is loaded.
    pub fn ensure(&mut self, origin: &ServerOrigin) {
        if self.origin.as_ref() == Some(origin) && self.cache.is_some() {
            return;
        }
        if self.origin.as_ref() != Some(origin) {
            // a sync of the other server still running must not go on with this one's data
            self.epoch += 1;
            self.load += 1;
            self.syncing = false;
            self.again = false;
        }
        let dir = self.dir_for(origin);
        self.cache = Some(Cache::load(&dir.join("cache.json"), origin));
        self.outbox = Outbox::load(&dir.join("outbox.json"));
        self.origin = Some(origin.clone());
        self.verified = false;
        self.online = None;
        self.auto_opened = false;
        self.last_error = None;
    }

    /// A server switch: the previous server's data leaves memory (its files stay for the next
    /// time it is current) and a sync of it still running stops at its next step.
    pub fn unload(&mut self) {
        self.epoch += 1;
        self.load += 1;
        self.origin = None;
        self.cache = None;
        self.outbox = Outbox::default();
        self.verified = false;
        self.online = None;
        self.syncing = false;
        self.again = false;
        self.auto_opened = false;
        self.last_error = None;
    }

    fn cache_mut(&mut self) -> &mut Cache {
        self.cache.get_or_insert_with(|| Cache::new(&ServerOrigin::cloud()))
    }

    /// Deletes `origin`'s data in memory (when loaded) and stops a running sync from writing it
    /// back. Returns the folder to delete.
    pub fn clear(&mut self, origin: &ServerOrigin) -> PathBuf {
        self.epoch += 1;
        self.generation = next_generation();
        if self.origin.as_ref() == Some(origin) {
            self.cache = Some(Cache::new(origin));
            self.outbox = Outbox::default();
            self.verified = false;
            self.last_error = None;
        }
        self.dir_for(origin)
    }

    /// `GET /me` answered `id`: takes the data over for that account. `true` when the data is
    /// another account's (it must be deleted first).
    pub fn adopt_user(&mut self, id: u64, name: Option<String>) -> bool {
        let cache_user = self.cache.as_ref().and_then(|c| c.user_id);
        if matches!(cache_user, Some(u) if u != id) || matches!(self.outbox.user_id, Some(u) if u != id) {
            return true;
        }
        let cache = self.cache_mut();
        cache.user_id = Some(id);
        cache.user_name = name;
        self.outbox.user_id = Some(id);
        self.verified = true;
        false
    }

    /// Whether this server has anything to show offline.
    pub fn has_data(&self) -> bool {
        self.cache.as_ref().is_some_and(|c| !c.is_empty()) || !self.outbox.is_empty()
    }

    /// What has to be written, with its generation (serialised under the state lock).
    fn snapshot(&mut self) -> Option<Snapshot> {
        let origin = self.origin.clone()?;
        self.generation = next_generation();
        let cache = self.cache.as_ref().filter(|c| !c.is_empty() || c.cursor.is_some());
        Some(Snapshot {
            generation: self.generation,
            dir: self.dir_for(&origin),
            cache: cache.map(|c| serde_json::to_vec(c).expect("serializable")),
            outbox: self.outbox.clone(),
        })
    }
}

struct Snapshot {
    generation: u64,
    dir: PathBuf,
    cache: Option<Vec<u8>>,
    outbox: Outbox,
}

/// Every snapshot / clear gets a higher number, so an older write never lands after a newer
/// one (writes happen outside the state lock).
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

/// The generation written last per folder; one writer at a time.
static WRITTEN: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());

fn write(snapshot: Snapshot) {
    let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    let last = written.entry(snapshot.dir.clone()).or_insert(0);
    if snapshot.generation <= *last {
        return;
    }
    *last = snapshot.generation;
    let cache_path = snapshot.dir.join("cache.json");
    let result = match &snapshot.cache {
        Some(bytes) => outbox::write_atomic(&cache_path, bytes),
        None => outbox::remove_file(&cache_path),
    }
    .and_then(|_| snapshot.outbox.save(&snapshot.dir.join("outbox.json")));
    if let Err(e) = result {
        log::warn!("could not save the offline data: {e}");
    }
    // nothing left: no empty folder either
    let _ = std::fs::remove_dir(&snapshot.dir);
}

/// Writes the current server's cache and outbox (outside the state lock).
fn persist(app: &AppHandle) {
    let snapshot = state(app).lock().offline.snapshot();
    if let Some(s) = snapshot {
        write(s);
    }
}

/// Tells the offline window to read its view again.
fn changed(app: &AppHandle) {
    let _ = app.emit_to(Popup::Offline.label(), "offline://changed", ());
    crate::tray::refresh(app);
}

/// Whether the user is signed in to `phase`'s server (or was, and it can't be reached).
fn signed_in(phase: Phase) -> bool {
    matches!(phase, Phase::InApp | Phase::Unreachable | Phase::Connecting)
}

fn ui(kind: &'static str, message: impl Into<String>) -> UiError {
    UiError {
        kind,
        message: message.into(),
    }
}

// ── status: window title, tray ─────────────────────────────────────────────────

/// What the tray shows about offline mode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayStatus {
    /// Offer "Open Offline View" / "Sync Now".
    pub available: bool,
    pub offline: bool,
    pub pending: usize,
}

pub fn tray_status(inner: &Inner) -> TrayStatus {
    let current = inner.current_origin();
    let loaded = current.is_some() && inner.offline.origin == current;
    TrayStatus {
        available: current.is_some() && signed_in(inner.phase),
        offline: is_offline(inner),
        pending: if loaded { inner.offline.outbox.len() } else { 0 },
    }
}

/// The current server can't be reached (unreachable screen, or failed calls while in the app).
pub fn is_offline(inner: &Inner) -> bool {
    let current = inner.current_origin();
    current.is_some()
        && (inner.phase == Phase::Unreachable
            || (inner.offline.origin == current && inner.offline.online == Some(false)))
}

fn set_online(app: &AppHandle, online: bool) {
    let was = {
        let s = state(app);
        let mut inner = s.lock();
        inner.offline.online.replace(online)
    };
    if was != Some(online) {
        log::info!("server {}", if online { "reachable again" } else { "unreachable" });
        crate::shell::refresh_title(app);
        changed(app);
    }
}

/// The tray poller's view of the connection (while the app shows the server).
pub fn note_poll(app: &AppHandle, result: &Result<(), CallError>) {
    match result {
        Ok(()) => set_online(app, true),
        Err(CallError::Offline(_)) => set_online(app, false),
        Err(_) => {}
    }
}

/// The unreachable screen is up: "Offline" everywhere, and the offline window opens by itself
/// once per outage when there is something to show.
pub fn on_unreachable(app: &AppHandle) {
    let open = {
        let s = state(app);
        let mut inner = s.lock();
        let Some(origin) = inner.current_origin() else { return };
        let start_hidden = inner.start_hidden;
        let data = &mut inner.offline;
        data.ensure(&origin);
        data.online = Some(false);
        let open = data.has_data() && !data.auto_opened && !start_hidden;
        if open {
            data.auto_opened = true;
        }
        open
    };
    crate::shell::refresh_title(app);
    changed(app);
    if open {
        popup::show(app, Popup::Offline);
    }
}

/// The app (re)entered the server: check the account again on the next sync; the next outage
/// may open the window again.
pub fn on_enter_app(app: &AppHandle) {
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.offline.verified = false;
        inner.offline.auto_opened = false;
        // not known until the first poll (the web app may also have been opened offline)
        inner.offline.online = None;
    }
    crate::shell::refresh_title(app);
    changed(app);
}

/// Sign out / forget / another account: this server's offline data is deleted.
pub fn wipe(app: &AppHandle, origin: &ServerOrigin) {
    let (dir, generation) = {
        let s = state(app);
        let mut inner = s.lock();
        let dir = inner.offline.clear(origin);
        (dir, inner.offline.generation)
    };
    if let Err(e) = remove_files(&dir, generation) {
        log::warn!("could not delete the offline data: {e}");
    }
    changed(app);
}

/// Deletes the folder after any write that is still running; older writes never land later.
fn remove_files(dir: &Path, generation: u64) -> std::io::Result<()> {
    let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    let last = written.entry(dir.to_path_buf()).or_insert(0);
    *last = (*last).max(generation);
    remove_dir(dir)
}

fn remove_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

// ── sync ───────────────────────────────────────────────────────────────────────

struct Stale;

impl From<Stale> for CallError {
    fn from(_: Stale) -> Self {
        CallError::Failed("signed out meanwhile".into())
    }
}

/// Runs `f` on the offline data unless a wipe happened since `epoch`.
fn with_data<T>(app: &AppHandle, epoch: u64, f: impl FnOnce(&mut OfflineData) -> T) -> Result<T, Stale> {
    let s = state(app);
    let mut inner = s.lock();
    if inner.offline.epoch != epoch {
        return Err(Stale);
    }
    Ok(f(&mut inner.offline))
}

/// Sends the outbox and pulls the changes of `origin` (one sync at a time; a call during a sync
/// makes it run once more).
pub async fn sync(app: &AppHandle, origin: &ServerOrigin) -> Result<(), CallError> {
    sync_with(app, origin, true).await
}

/// How long a server switch waits for the old server's outbox (`send_before_leaving`).
pub const LEAVE_FLUSH: std::time::Duration = std::time::Duration::from_secs(3);

/// A switch to another server: the current one's waiting offline changes get one try (at most
/// [`LEAVE_FLUSH`], only while it is signed in and reachable), sent to **this** server. What is
/// left stays in its `outbox.json` and goes out the next time it is current.
pub async fn send_before_leaving(app: &AppHandle, origin: &ServerOrigin) {
    let waiting = {
        let s = state(app);
        let inner = s.lock();
        inner.phase == Phase::InApp
            && inner.offline.origin.as_ref() == Some(origin)
            && inner.offline.online != Some(false)
            && !inner.offline.outbox.is_empty()
    };
    if !waiting {
        return;
    }
    match tokio::time::timeout(LEAVE_FLUSH, sync_with(app, origin, false)).await {
        Ok(Ok(())) => log::info!("offline changes sent before the switch"),
        Ok(Err(e)) => log::info!("offline changes kept for later ({e})"),
        Err(_) => log::info!("offline changes kept for later (took too long)"),
    }
}

/// `pull`: also ask for what changed (not before a switch: only the outbox matters then).
async fn sync_with(app: &AppHandle, origin: &ServerOrigin, pull_changes: bool) -> Result<(), CallError> {
    let load = {
        let s = state(app);
        let mut inner = s.lock();
        if inner.current_origin().as_ref() != Some(origin) || crate::flows::app_too_old(&inner, origin) {
            return Ok(());
        }
        inner.offline.ensure(origin);
        if inner.offline.syncing {
            inner.offline.again = true;
            return Ok(());
        }
        inner.offline.syncing = true;
        inner.offline.again = false;
        inner.offline.load
    };
    changed(app);
    let mut result;
    loop {
        result = sync_once(app, origin, pull_changes).await;
        let again = {
            let s = state(app);
            let mut inner = s.lock();
            std::mem::take(&mut inner.offline.again)
        };
        if !again || result.is_err() {
            break;
        }
    }
    {
        let s = state(app);
        let mut inner = s.lock();
        if inner.offline.load != load {
            // the server was switched meanwhile: nothing of this run belongs to what is loaded now
            return result;
        }
        inner.offline.syncing = false;
        inner.offline.last_error = match &result {
            Ok(()) => None,
            Err(CallError::Offline(_)) => {
                Some("The server can't be reached. Your changes are kept on this computer.".into())
            }
            Err(CallError::SignedOut(_)) => Some("Sign in again to send your changes.".into()),
            Err(CallError::Server { status, .. }) => Some(format!(
                "The server could not sync right now (HTTP {status}). Your changes are kept and sent later."
            )),
            Err(CallError::Failed(_)) => Some("Syncing failed. Your changes are kept and sent later.".into()),
        };
    }
    match &result {
        Ok(()) => set_online(app, true),
        Err(CallError::Offline(_)) => set_online(app, false),
        Err(e) => log::info!("offline sync failed: {e}"),
    }
    persist(app);
    changed(app);
    result
}

async fn sync_once(app: &AppHandle, origin: &ServerOrigin, pull_changes: bool) -> Result<(), CallError> {
    let epoch = state(app).lock().offline.epoch;
    let epoch = verify_user(app, origin, epoch).await?;
    let (sent, problems, error) = flush(app, origin, epoch).await;
    if sent > 0 {
        let body = if sent == 1 {
            "1 change you made offline is on the server now.".to_string()
        } else {
            format!("{sent} changes you made offline are on the server now.")
        };
        crate::bridge::show_offline_notification(app, "Offline changes synced", &body);
    }
    if problems > 0 {
        let body = if problems == 1 {
            "1 change could not be applied as it was. Open the offline view to see why; your text is kept there."
                .to_string()
        } else {
            format!("{problems} changes could not be applied as they were. Open the offline view to see why; your text is kept there.")
        };
        crate::bridge::show_offline_notification(app, "Offline changes need a look", &body);
    }
    if let Some(e) = error {
        return Err(e);
    }
    if !pull_changes {
        return Ok(());
    }
    pull(app, origin, epoch).await
}

/// `GET /me` once per session: the cache and outbox must belong to the signed-in account.
/// Returns the epoch to go on with (a new one when another account's data was deleted).
async fn verify_user(app: &AppHandle, origin: &ServerOrigin, epoch: u64) -> Result<u64, CallError> {
    if state(app).lock().offline.verified {
        return Ok(epoch);
    }
    let me = state(app)
        .session
        .request(origin, &ApiRequest::get("/api/v1/me"))
        .await?;
    let Some(id) = me.body.get("id").and_then(Value::as_u64) else {
        return Err(CallError::Failed("unexpected answer to /me".into()));
    };
    let name = me
        .body
        .get("name")
        .and_then(Value::as_str)
        .map(|n| n.chars().take(120).collect::<String>());
    let other = with_data(app, epoch, |data| data.adopt_user(id, name.clone()))?;
    if !other {
        return Ok(epoch);
    }
    // another account signed in on this server: the previous one's data must not be shown or
    // sent as this account
    log::info!("offline data of another account deleted");
    wipe(app, origin);
    let epoch = state(app).lock().offline.epoch;
    with_data(app, epoch, |data| data.adopt_user(id, name))?;
    Ok(epoch)
}

/// Sends the outbox in order. Returns (sent, new problems, the error that stopped it).
async fn flush(app: &AppHandle, origin: &ServerOrigin, epoch: u64) -> (usize, usize, Option<CallError>) {
    let (mut sent, mut problems) = (0, 0);
    let mut size = BATCH;
    loop {
        let Ok(batch) = with_data(app, epoch, |d| d.outbox.next_batch(size)) else {
            return (sent, problems, Some(Stale.into()));
        };
        if batch.is_empty() {
            break;
        }
        let ids: Vec<String> = batch.iter().map(|e| e.request_id.clone()).collect();
        let body = json!({ "changes": batch.iter().map(Entry::to_json).collect::<Vec<_>>() });
        let result = state(app)
            .session
            .request(origin, &ApiRequest::post("/api/v1/sync", body))
            .await;
        let applied = with_data(app, epoch, |d| {
            let before = d.outbox.len();
            let step = match &result {
                Ok(response) => d.outbox.apply_results(&ids, &outbox::parse_results(&response.body)),
                Err(e) => d.outbox.apply_batch_error(&ids, e),
            };
            for (entry, result) in &step.done {
                d.cache_mut()
                    .apply_result(&entry.change, result.record_type.as_deref(), result.entity.as_ref());
            }
            (step, before - d.outbox.len())
        });
        let Ok((step, removed)) = applied else {
            return (sent, problems, Some(Stale.into()));
        };
        persist(app);
        changed(app);
        sent += step.done.len();
        problems += step.problems;
        if step.split {
            size = 1;
            continue;
        }
        if let Err(e) = result {
            if step.stop {
                return (sent, problems, Some(e));
            }
        }
        // stop for now (rate limit, server error) or nothing moved (waiting changes): next sync
        if step.stop || removed == 0 {
            break;
        }
    }
    (sent, problems, None)
}

/// Pulls what changed since the cursor (a full run when there is none).
async fn pull(app: &AppHandle, origin: &ServerOrigin, epoch: u64) -> Result<(), CallError> {
    let mut run = with_data(app, epoch, |d| d.cache_mut().start_run())?;
    loop {
        let mut request = ApiRequest::get("/api/v1/sync/changes");
        request.query = run.query();
        let response = state(app).session.request(origin, &request).await?;
        let page: FeedPage = serde_json::from_value(response.body)
            .map_err(|_| CallError::Failed("unexpected answer to /sync/changes".into()))?;
        let more = with_data(app, epoch, |d| d.cache_mut().apply_page(&mut run, page, outbox::now()))?;
        if !more {
            break;
        }
    }
    Ok(())
}

/// After a change in the window: send it right away when possible.
fn sync_soon(app: &AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let origin = {
            let s = state(&handle);
            let inner = s.lock();
            inner.current_origin().filter(|_| signed_in(inner.phase))
        };
        if let Some(origin) = origin {
            let _ = sync(&handle, &origin).await;
        }
    });
}

/// "Sync now" (tray, window). When the server answers while the unreachable screen is up, the
/// app is opened again too.
pub async fn sync_now(app: &AppHandle) {
    let (origin, phase) = {
        let s = state(app);
        let inner = s.lock();
        (inner.current_origin(), inner.phase)
    };
    let Some(origin) = origin.filter(|_| signed_in(phase)) else {
        return;
    };
    let ok = sync(app, &origin).await.is_ok();
    if ok && state(app).lock().phase == Phase::Unreachable {
        if let Err(e) = crate::flows::retry_now(app).await {
            log::info!("reconnect after sync: {}", e.message);
        }
    }
}

// ── the offline window ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct WindowView {
    server: Option<String>,
    signed_in: bool,
    /// `false`: the server can't be reached; `null`: not known yet.
    online: Option<bool>,
    syncing: bool,
    last_error: Option<String>,
    platform: &'static str,
    #[serde(flatten)]
    data: Option<offline_cache::View>,
}

fn window_view(app: &AppHandle) -> WindowView {
    let s = state(app);
    let mut inner = s.lock();
    let origin = inner.current_origin();
    let signed_in = signed_in(inner.phase);
    let offline = is_offline(&inner);
    let data = match (&origin, signed_in) {
        (Some(o), true) => {
            inner.offline.ensure(o);
            let d = &inner.offline;
            d.cache.as_ref().map(|c| offline_cache::view(c, &d.outbox))
        }
        _ => None,
    };
    WindowView {
        server: origin.as_ref().map(|o| inner.settings.label_of(o)),
        signed_in,
        online: if offline { Some(false) } else { inner.offline.online },
        syncing: inner.offline.syncing,
        last_error: inner.offline.last_error.clone(),
        platform: crate::api::platform(),
        data,
    }
}

/// Adds a change to the outbox (checked against the cache by `make`), then tries to send it.
fn record(
    app: &AppHandle,
    make: impl FnOnce(&mut OfflineData) -> Result<Option<Change>, UiError>,
) -> Result<WindowView, UiError> {
    {
        let s = state(app);
        let mut inner = s.lock();
        let origin = inner
            .current_origin()
            .filter(|_| signed_in(inner.phase))
            .ok_or_else(|| ui("signed_out", "Sign in to a server first."))?;
        let data = &mut inner.offline;
        data.ensure(&origin);
        if let Some(change) = make(data)? {
            if data.outbox.user_id.is_none() {
                data.outbox.user_id = data.cache.as_ref().and_then(|c| c.user_id);
            }
            data.outbox.push(change).map_err(|e| ui("full", e.to_string()))?;
        }
    }
    persist(app);
    changed(app);
    sync_soon(app);
    Ok(window_view(app))
}

fn clean_title(title: &str) -> Result<String, UiError> {
    let title: String = title.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let title = title.trim();
    if title.is_empty() {
        return Err(ui("invalid", "Write a title first."));
    }
    if title.chars().count() > outbox::MAX_TITLE {
        return Err(ui("invalid", "That title is too long (at most 500 characters)."));
    }
    Ok(title.to_string())
}

fn cached_work(data: &OfflineData, work_id: &str) -> Result<offline_cache::ViewWork, UiError> {
    if !crate::tray::is_work_id(work_id) {
        return Err(ui("invalid", "Unknown item."));
    }
    data.cache
        .as_ref()
        .and_then(|c| c.work(work_id))
        .ok_or_else(|| ui("not_found", "This item is not on this computer."))
}

#[tauri::command]
pub fn offline_view(app: AppHandle, webview: Webview) -> Result<WindowView, String> {
    popup::ensure_caller(&app, &webview, Popup::Offline)?;
    Ok(window_view(&app))
}

/// The circle: done / not done (`work.done`).
#[tauri::command]
pub fn offline_set_done(app: AppHandle, webview: Webview, work_id: String, done: bool) -> Result<WindowView, UiError> {
    popup::ensure_caller(&app, &webview, Popup::Offline).map_err(|m| ui("not_allowed", m))?;
    record(&app, |data| {
        let work = cached_work(data, &work_id)?;
        // the value the server had when the user ticked it: an unsent change is folded in
        let base_done = data.outbox.base_done(&work_id, work.done);
        Ok(Some(Change::WorkDone {
            title: work.label(),
            work_id,
            done,
            base_done,
        }))
    })
}

#[tauri::command]
pub fn offline_my_day_add(app: AppHandle, webview: Webview, work_id: String) -> Result<WindowView, UiError> {
    popup::ensure_caller(&app, &webview, Popup::Offline).map_err(|m| ui("not_allowed", m))?;
    record(&app, |data| {
        let work = cached_work(data, &work_id)?;
        Ok(Some(Change::MyDayAdd {
            title: work.label(),
            work_id,
        }))
    })
}

#[tauri::command]
pub fn offline_add_task(
    app: AppHandle,
    webview: Webview,
    task_list_id: u64,
    title: String,
) -> Result<WindowView, UiError> {
    popup::ensure_caller(&app, &webview, Popup::Offline).map_err(|m| ui("not_allowed", m))?;
    let title = clean_title(&title)?;
    record(&app, |data| {
        let cache = data.cache.as_ref().filter(|c| c.has_list(task_list_id));
        let Some(cache) = cache else {
            return Err(ui("not_found", "This list is not on this computer."));
        };
        if !cache.can_manage_items(task_list_id) {
            return Err(ui("forbidden", "You can't add tasks to this list."));
        }
        Ok(Some(Change::TaskCreate {
            task_list_id,
            title,
            done: false,
        }))
    })
}

/// Rename and / or tick a task. `task_id`: the task's id, or `ref:<request id>` for one added
/// offline that wasn't sent yet (its create is changed instead).
#[tauri::command]
pub fn offline_update_task(
    app: AppHandle,
    webview: Webview,
    task_list_id: u64,
    task_id: String,
    title: Option<String>,
    done: Option<bool>,
) -> Result<WindowView, UiError> {
    popup::ensure_caller(&app, &webview, Popup::Offline).map_err(|m| ui("not_allowed", m))?;
    let title = title.as_deref().map(clean_title).transpose()?;
    if title.is_none() && done.is_none() {
        return Ok(window_view(&app));
    }
    record(&app, |data| {
        if let Some(request_id) = task_id.strip_prefix("ref:") {
            data.outbox
                .edit_pending_create(request_id, title, done)
                .map_err(|e| ui("busy", e.to_string()))?;
            return Ok(None);
        }
        let id: u64 = task_id.parse().map_err(|_| ui("invalid", "Unknown task."))?;
        let cache = data.cache.as_ref().filter(|c| c.has_list(task_list_id));
        let task = cache
            .and_then(|c| c.task(id))
            .ok_or_else(|| ui("not_found", "This task is not on this computer."))?;
        // an unsent change of the same task keeps its base (the server's value)
        let (base_title, base_done) = data.outbox.task_base(id, &task.title, task.done);
        Ok(Some(Change::TaskUpdate {
            task_list_id,
            task_id: id,
            title,
            done,
            base_title,
            base_done,
        }))
    })
}

#[tauri::command]
pub fn offline_add_capture(
    app: AppHandle,
    webview: Webview,
    text: String,
    url: Option<String>,
) -> Result<WindowView, UiError> {
    popup::ensure_caller(&app, &webview, Popup::Offline).map_err(|m| ui("not_allowed", m))?;
    let input = CaptureInput::new(&text, url.as_deref(), None).map_err(|e| ui("invalid", e.to_string()))?;
    record(&app, |_| {
        Ok(Some(Change::CaptureCreate {
            text: input.text,
            url: input.url,
            intended_type: input.intended_type,
        }))
    })
}

#[tauri::command]
pub async fn offline_sync_now(app: AppHandle, webview: Webview) -> Result<WindowView, String> {
    popup::ensure_caller(&app, &webview, Popup::Offline)?;
    sync_now(&app).await;
    Ok(window_view(&app))
}

/// Dismiss one problem (`request_id`) or all of them.
#[tauri::command]
pub fn offline_dismiss_problem(
    app: AppHandle,
    webview: Webview,
    request_id: Option<String>,
) -> Result<WindowView, String> {
    popup::ensure_caller(&app, &webview, Popup::Offline)?;
    {
        let s = state(&app);
        let mut inner = s.lock();
        inner.offline.outbox.dismiss(request_id.as_deref());
    }
    persist(&app);
    changed(&app);
    Ok(window_view(&app))
}

/// A done tick from the menu-bar panel while the server can't be reached: the same `work.done`
/// change as this window's circle (`server_done` = what the server last showed; an unsent change
/// of the same item keeps its base).
pub fn queue_work_done(
    app: &AppHandle,
    work_id: &str,
    done: bool,
    server_done: bool,
    title: &str,
) -> Result<(), UiError> {
    if !crate::tray::is_work_id(work_id) {
        return Err(ui("invalid", "Unknown item."));
    }
    record(app, |data| {
        let base_done = data.outbox.base_done(work_id, server_done);
        Ok(Some(Change::WorkDone {
            title: title.chars().take(outbox::MAX_TITLE).collect(),
            work_id: work_id.to_string(),
            done,
            base_done,
        }))
    })
    .map(|_| ())
}

/// "Open Brainitive": back to the main window (it retries the server by itself).
#[tauri::command]
pub fn offline_open_app(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Offline)?;
    crate::shell::focus_main(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::Change;

    fn origin() -> ServerOrigin {
        ServerOrigin::parse("brain.example.com").unwrap()
    }

    #[test]
    fn files_live_per_server_and_are_written_in_order() {
        let base = std::env::temp_dir().join(format!("brainitive-offline-{}", std::process::id()));
        let mut data = OfflineData::new(base.clone());
        data.ensure(&origin());
        let dir = data.dir_for(&origin());
        assert!(dir.starts_with(&base));
        assert_eq!(dir.file_name().unwrap().len(), 32, "the store id, not the host name");
        assert_ne!(dir, data.dir_for(&ServerOrigin::cloud()));

        data.outbox
            .push(Change::CaptureCreate {
                text: "Idea".into(),
                url: None,
                intended_type: None,
            })
            .unwrap();
        let older = data.snapshot().unwrap();
        data.outbox.entries.clear();
        let newer = data.snapshot().unwrap();
        write(newer);
        write(older); // late: ignored
        assert!(!dir.join("outbox.json").exists(), "the newer (empty) state wins");
        assert!(!dir.join("cache.json").exists(), "an empty cache is not written");

        // loading another server replaces the data in memory
        data.outbox
            .push(Change::CaptureCreate {
                text: "x".into(),
                url: None,
                intended_type: None,
            })
            .unwrap();
        write(data.snapshot().unwrap());
        assert!(dir.join("outbox.json").exists());
        data.ensure(&ServerOrigin::cloud());
        assert!(data.outbox.is_empty());
        data.ensure(&origin());
        assert_eq!(data.outbox.len(), 1, "read back from its file");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn another_server_or_a_switch_stops_a_running_sync() {
        let base = std::env::temp_dir().join(format!("brainitive-offline-switch-{}", std::process::id()));
        let mut data = OfflineData::new(base.clone());
        data.ensure(&origin());
        data.syncing = true;
        let (epoch, load) = (data.epoch, data.load);
        // the same server again: nothing changes for a sync that runs
        data.ensure(&origin());
        assert_eq!((data.epoch, data.load), (epoch, load));
        assert!(data.syncing);
        // another server's data: the old run's next `with_data` is stale (it can't take this
        // outbox to the old server), and this server may sync on its own
        data.ensure(&ServerOrigin::cloud());
        assert!(data.epoch > epoch && data.load > load);
        assert!(!data.syncing);
        // a switch unloads everything
        data.outbox
            .push(Change::CaptureCreate {
                text: "cloud".into(),
                url: None,
                intended_type: None,
            })
            .unwrap();
        let epoch = data.epoch;
        data.unload();
        assert!(data.epoch > epoch);
        assert_eq!(data.origin, None);
        assert!(data.cache.is_none() && data.outbox.is_empty() && !data.verified);
        assert!(data.snapshot().is_none(), "nothing to write for no server");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn sign_out_and_another_account_clear_the_data() {
        let base = std::env::temp_dir().join(format!("brainitive-offline-clear-{}", std::process::id()));
        let mut data = OfflineData::new(base.clone());
        data.ensure(&origin());
        assert!(
            !data.adopt_user(4, Some("Armend".into())),
            "a fresh cache takes the account"
        );
        assert!(data.verified);
        data.outbox
            .push(Change::CaptureCreate {
                text: "Idea".into(),
                url: None,
                intended_type: None,
            })
            .unwrap();
        data.cache_mut().records.insert(
            "work:issue:1".into(),
            crate::offline_cache::Record {
                kind: "work".into(),
                id: "issue:1".into(),
                project_id: Some(1),
                updated_at: None,
                data: serde_json::json!({}),
            },
        );
        write(data.snapshot().unwrap());
        let dir = data.dir_for(&origin());
        assert!(dir.join("cache.json").exists() && dir.join("outbox.json").exists());

        // the same account again: kept
        assert!(!data.adopt_user(4, None));
        assert!(data.has_data());
        // another account: its data has to go first
        assert!(data.adopt_user(5, None));
        assert_eq!(data.cache.as_ref().unwrap().user_id, Some(4), "nothing taken over");

        // sign out / forget / account switch: memory and files
        let epoch = data.epoch;
        let removed = data.clear(&origin());
        assert_eq!(removed, dir);
        assert!(data.epoch > epoch, "a running sync stops writing");
        assert!(!data.has_data() && !data.verified);
        assert_eq!(data.cache.as_ref().unwrap().user_id, None);
        remove_files(&removed, data.generation).unwrap();
        assert!(!dir.exists());
        assert!(!data.adopt_user(5, None), "now the new account takes over");
        // clearing another (not loaded) server leaves the loaded one alone
        data.outbox
            .push(Change::CaptureCreate {
                text: "x".into(),
                url: None,
                intended_type: None,
            })
            .unwrap();
        data.clear(&ServerOrigin::cloud());
        assert_eq!(data.outbox.len(), 1);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn clean_titles() {
        assert_eq!(clean_title("  Buy\nmilk ").unwrap(), "Buy milk");
        assert!(clean_title("   ").is_err());
        assert!(clean_title(&"a".repeat(501)).is_err());
    }
}
