//! Auto-updates (BRAI-373): `tauri-plugin-updater`, used from Rust only.
//!
//! - **Signed bundles**: the plugin installs a bundle only when its minisign (Ed25519) signature
//!   matches the public key compiled into the app (`tauri.conf.json` → `plugins.updater.pubkey`)
//!   and, with `requireSignedVersion`, was made for the version the manifest announces.
//! - **Placeholder guard**: while that key is the repo's placeholder (or not a valid key), and in
//!   development builds, the updater refuses to run ([`availability`]): no request, no install,
//!   the menu item is disabled and Settings says why.
//! - **Channels**: stable (default) reads `latest.json` of GitHub's latest release (never a
//!   pre-release); beta also reads `beta.json` of the rolling `updater-beta` pre-release, and the
//!   higher version of the two wins.
//! - **Network**: https only, ≤ 5 redirects, every hop on GitHub / its asset CDN ([`allowed_url`]).
//! - **Flow**: check 30 s after start and every 6 h (or from the menu, the tray, Settings, the
//!   "Please update" screen) → download in the background → "Update ready — restart to install".
//!   Only the user's click on "Restart to Update" installs (signature checked again) and restarts.
//!
//! - **Microsoft Store build** (BRAI-442): the plugin is not registered ([`Availability::Store`]);
//!   every way to ask for an update opens the app's Store page instead.
//!
//! Pure logic first (tested below); the runtime part is at the end.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};
use url::Url;

use crate::state::state;
use crate::version::APP_VERSION;

/// The GitHub repository the releases come from.
pub const REPO: &str = "armendzekjiri/brainitive-desktop";
/// The rolling pre-release whose asset [`BETA_MANIFEST`] CI replaces for every published beta.
pub const BETA_TAG: &str = "updater-beta";
pub const STABLE_MANIFEST: &str = "latest.json";
pub const BETA_MANIFEST: &str = "beta.json";

/// Hosts a manifest, a bundle or one of their redirects may be on: GitHub and its release-asset
/// CDN (old and new name).
pub const ALLOWED_HOSTS: [&str; 3] = [
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
pub const MAX_REDIRECTS: usize = 5;

/// The marker in the repo's placeholder key (`tauri.conf.json`). See docs/RELEASING.md.
pub const PLACEHOLDER_MARK: &str = "PLACEHOLDER";

/// First check after start, then every 6 hours; 1 hour after a failed check.
pub const START_DELAY: Duration = Duration::from_secs(30);
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
pub const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// How often the loop looks at the (wall) clock; a check itself only runs when due.
pub const TICK: Duration = Duration::from_secs(30);
/// Timeout of the manifest request (the bundle download has none: it can be large).
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

// ---- channels and endpoints -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

impl Channel {
    /// The manifests this channel reads, in order. Beta reads both: a stable release newer than
    /// the last beta reaches beta users too, even if `beta.json` is stale.
    pub fn manifests(self) -> Vec<Url> {
        match self {
            Self::Stable => vec![stable_manifest_url(REPO)],
            Self::Beta => vec![beta_manifest_url(REPO), stable_manifest_url(REPO)],
        }
    }
}

/// An unknown channel in `settings.json` falls back to stable (instead of failing the file).
pub fn lenient_channel<'de, D: Deserializer<'de>>(d: D) -> Result<Channel, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// `https://github.com/<repo>/releases/latest/download/latest.json`: GitHub resolves "latest" to
/// the newest published release that is neither a draft nor a pre-release.
pub fn stable_manifest_url(repo: &str) -> Url {
    Url::parse(&format!(
        "https://github.com/{repo}/releases/latest/download/{STABLE_MANIFEST}"
    ))
    .expect("a valid constant URL")
}

/// `https://github.com/<repo>/releases/download/updater-beta/beta.json`.
pub fn beta_manifest_url(repo: &str) -> Url {
    Url::parse(&format!(
        "https://github.com/{repo}/releases/download/{BETA_TAG}/{BETA_MANIFEST}"
    ))
    .expect("a valid constant URL")
}

/// A manifest, bundle or redirect target the updater may fetch: https on the default port, no
/// user info, host exactly one of [`ALLOWED_HOSTS`].
pub fn allowed_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url
            .host_str()
            .is_some_and(|h| ALLOWED_HOSTS.contains(&h.to_ascii_lowercase().as_str()))
}

/// Whether a redirect to `next` is followed; `visited` = URLs requested so far (≥ 1).
pub fn follow_redirect(next: &Url, visited: usize) -> bool {
    visited <= MAX_REDIRECTS && allowed_url(next)
}

/// The HTTP client of every updater request: https only, GitHub-only redirects. No cookies and
/// no token are ever sent (a plain client, not the API client).
pub fn github_only(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    builder
        .https_only(true)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if follow_redirect(attempt.url(), attempt.previous().len()) {
                attempt.follow()
            } else {
                attempt.error("the updater only follows https redirects on GitHub")
            }
        }))
}

// ---- the public key guard -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStatus {
    Valid,
    /// The repo's placeholder (or no key at all).
    Placeholder,
    /// Something else that is not a minisign Ed25519 public key.
    Invalid,
}

/// Checks the configured public key: base64 of a minisign public key file — an `untrusted
/// comment:` line, then base64 of `Ed` + 8-byte key id + 32-byte Ed25519 key.
pub fn key_status(pubkey: &str) -> KeyStatus {
    let pubkey = pubkey.trim();
    if pubkey.is_empty() || pubkey.to_ascii_uppercase().contains(PLACEHOLDER_MARK) {
        return KeyStatus::Placeholder;
    }
    let Some(text) = STANDARD.decode(pubkey).ok().and_then(|b| String::from_utf8(b).ok()) else {
        return KeyStatus::Invalid;
    };
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let (Some(comment), Some(key), None) = (lines.next(), lines.next(), lines.next()) else {
        return KeyStatus::Invalid;
    };
    match STANDARD.decode(key) {
        Ok(raw) if comment.starts_with("untrusted comment:") && raw.len() == 42 && raw.starts_with(b"Ed") => {
            KeyStatus::Valid
        }
        _ => KeyStatus::Invalid,
    }
}

/// Whether this build may check for and install updates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    PlaceholderKey,
    InvalidKey,
    DevBuild,
    /// The Microsoft Store build: the Store updates the app, this build has no updater plugin.
    Store,
    /// Before [`init`] ran.
    #[default]
    NotStarted,
}

impl Availability {
    pub fn is_available(self) -> bool {
        self == Self::Available
    }

    /// What Settings / the log say when it isn't.
    pub fn reason(self) -> Option<&'static str> {
        Some(match self {
            Self::Available => return None,
            Self::PlaceholderKey => {
                "This build has no update signing key, so it never downloads or installs updates. Get new versions from brainitive.app/download."
            }
            Self::InvalidKey => {
                "This build's update signing key is not valid, so it never downloads or installs updates. Get new versions from brainitive.app/download."
            }
            Self::DevBuild => "Development builds don't update themselves.",
            Self::Store => "Brainitive from the Microsoft Store is updated by the Store.",
            Self::NotStarted => "Updates are not ready yet.",
        })
    }
}

/// The guard: a real key and a release build, or nothing runs.
pub fn availability(pubkey: Option<&str>, dev_build: bool) -> Availability {
    match key_status(pubkey.unwrap_or("")) {
        KeyStatus::Placeholder => Availability::PlaceholderKey,
        KeyStatus::Invalid => Availability::InvalidKey,
        KeyStatus::Valid if dev_build => Availability::DevBuild,
        KeyStatus::Valid => Availability::Available,
    }
}

/// The guard of this binary: the Store build never updates itself, whatever the key.
pub fn build_availability(store_build: bool, pubkey: Option<&str>, dev_build: bool) -> Availability {
    if store_build {
        Availability::Store
    } else {
        availability(pubkey, dev_build)
    }
}

/// `plugins.updater.pubkey` of the config compiled into the app (the value the plugin verifies with).
pub fn configured_pubkey(config: &tauri::Config) -> Option<String> {
    config
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .map(str::to_string)
}

// ---- versions -------------------------------------------------------------------------------

/// Only a strictly newer version is an update (semver: `1.0.0-beta.2 < 1.0.0`). An equal or older
/// one — e.g. a manifest pointing back at a previous release — is ignored.
pub fn is_newer(current: &Version, remote: &Version) -> bool {
    remote > current
}

/// The item with the highest version (beta: the beta and the stable manifest's answers).
pub fn newest<T>(items: Vec<T>, version: impl Fn(&T) -> Option<Version>) -> Option<T> {
    items
        .into_iter()
        .filter_map(|i| version(&i).map(|v| (v, i)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, i)| i)
}

// ---- scheduling -----------------------------------------------------------------------------

/// When automatic checks run, on the wall clock (unix seconds), so a computer that slept through
/// the 6 hours checks at the next tick after waking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Schedule {
    started: Option<u64>,
    /// The last attempt (automatic or by hand) and whether it got an answer.
    last: Option<(u64, bool)>,
}

impl Schedule {
    pub fn start(&mut self, now: u64) {
        self.started = Some(now);
    }

    pub fn record(&mut self, now: u64, ok: bool) {
        self.last = Some((now, ok));
    }

    /// Whether the next automatic check is due.
    pub fn due(&self, now: u64) -> bool {
        let Some(started) = self.started else { return false };
        let after = |at: u64, wait: Duration| now >= at.saturating_add(wait.as_secs()) || now < at;
        match self.last {
            None => after(started, START_DELAY),
            Some((at, true)) => after(at, CHECK_EVERY),
            Some((at, false)) => after(at, RETRY_AFTER),
        }
    }
}

/// Whether the loop starts a check now.
pub fn should_auto_check(
    auto_check: bool,
    availability: Availability,
    status: &Status,
    schedule: &Schedule,
    now: u64,
) -> bool {
    auto_check && availability.is_available() && !status.busy() && !status.is_ready() && schedule.due(now)
}

// ---- status and labels ----------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    /// Downloaded and verified; waits for "Restart to Update".
    Ready {
        version: String,
    },
    Installing {
        version: String,
    },
    Failed {
        message: String,
    },
}

impl Status {
    pub fn busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing { .. }
        )
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

/// The app menu's update item: its text and whether it can be clicked. When an update is ready
/// it becomes "Restart to Update (x.y.z)" — the only way an update gets installed.
pub fn menu_item(availability: Availability, status: &Status) -> (String, bool) {
    // the Store build: the item opens the Store page
    if availability == Availability::Store {
        return ("Check for Updates…".into(), true);
    }
    if !availability.is_available() {
        return ("Check for Updates…".into(), false);
    }
    match status {
        Status::Checking => ("Checking for Updates…".into(), false),
        Status::Downloading { .. } => ("Downloading Update…".into(), false),
        Status::Installing { .. } => ("Installing Update…".into(), false),
        Status::Ready { version } => (format!("Restart to Update ({version})"), true),
        _ => ("Check for Updates…".into(), true),
    }
}

/// Percent of a download, `None` without a length.
pub fn percent(done: u64, total: Option<u64>) -> Option<u8> {
    total.filter(|t| *t > 0).map(|t| ((done.min(t) * 100) / t) as u8)
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---- runtime --------------------------------------------------------------------------------

/// The updater's part of the app state.
#[derive(Default)]
pub struct UpdaterState {
    pub availability: Availability,
    pub status: Status,
    pub schedule: Schedule,
    pub pending: Option<Pending>,
    /// Bumped when the channel changes (BRAI-377): a check or download started before belongs
    /// to the old channel, and whatever it brings back is discarded.
    pub generation: u64,
}

impl UpdaterState {
    /// Starts a check unless one runs or an update waits: the generation it belongs to.
    pub fn begin_check(&mut self) -> Option<u64> {
        if self.status.busy() || self.status.is_ready() {
            return None;
        }
        self.status = Status::Checking;
        Some(self.generation)
    }

    /// Whether a check / download of `generation` still belongs to the current channel.
    pub fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// A status from a check or download of `generation`; dropped (`false`) when the channel
    /// changed meanwhile.
    pub fn report(&mut self, generation: u64, status: Status) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        self.status = status;
        true
    }

    /// Settings changed the channel: a waiting update is dropped, a running check or download
    /// is discarded when it comes back (only an install already running goes on), and an
    /// automatic check of the new channel runs soon.
    pub fn switch_channel(&mut self, now: u64) {
        self.generation = self.generation.wrapping_add(1);
        if matches!(self.status, Status::Installing { .. }) {
            return;
        }
        self.pending = None;
        self.status = Status::Idle;
        self.schedule = Schedule::default();
        self.schedule.start(now.saturating_sub(START_DELAY.as_secs()));
    }
}

/// A downloaded, verified update waiting for the user's click.
#[derive(Clone)]
pub struct Pending {
    pub update: Update,
    pub file: PathBuf,
}

/// What Settings and the "Please update" screen show.
#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub available: bool,
    /// The Microsoft Store build: Settings and the "Please update" screen point to the Store.
    pub store: bool,
    pub unavailable_reason: Option<&'static str>,
    pub auto_check: bool,
    pub channel: Channel,
    /// Unix seconds of the last check that got an answer.
    pub last_check: Option<u64>,
    pub current_version: &'static str,
    pub status: Status,
}

pub fn view(app: &AppHandle) -> View {
    let s = state(app);
    let inner = s.lock();
    let availability = inner.updater.availability;
    View {
        available: availability.is_available(),
        store: availability == Availability::Store,
        unavailable_reason: availability.reason(),
        auto_check: inner.settings.preferences.auto_update_check,
        channel: inner.settings.preferences.update_channel,
        last_check: inner.settings.last_update_check,
        current_version: APP_VERSION,
        status: inner.updater.status.clone(),
    }
}

/// Who asked for a check: a menu / tray click gets a notification with the result, a window
/// shows it itself, an automatic check only notifies when an update is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Auto,
    Menu,
    Window,
}

/// An updater for one manifest URL. `client` is [`github_only`] in the app (tests pass a
/// loopback client). The public key and `requireSignedVersion` come from the plugin config.
pub fn build_updater<R: Runtime>(
    app: &AppHandle<R>,
    manifest: Url,
    client: fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> Result<Updater, String> {
    app.updater_builder()
        .endpoints(vec![manifest])
        .map_err(|e| e.to_string())?
        .timeout(CHECK_TIMEOUT)
        .version_comparator(|current, release| is_newer(&current, &release.version))
        .configure_client(client)
        .build()
        .map_err(|e| e.to_string())
}

/// Asks the channel's manifests; the newest update any of them offers. An error only when none
/// of them answered.
pub async fn find_update<R: Runtime>(
    app: &AppHandle<R>,
    manifests: Vec<Url>,
    client: fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> Result<Option<Update>, String> {
    let mut found = Vec::new();
    let mut answered = false;
    let mut last_error = None;
    for manifest in manifests {
        match build_updater(app, manifest, client)?.check().await {
            Ok(Some(update)) => {
                answered = true;
                found.push(update);
            }
            Ok(None) => answered = true,
            Err(e) => last_error = Some(e.to_string()),
        }
    }
    if !answered {
        return Err(last_error.unwrap_or_else(|| "no update manifest answered".into()));
    }
    Ok(newest(found, |u| Version::parse(&u.version).ok()))
}

fn staging_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|d| d.join("update"))
}

/// At start: decides whether updates may run (logs why not), clears a bundle a previous run
/// downloaded but never installed, and starts the check loop.
pub fn init(app: &AppHandle) {
    let pubkey = configured_pubkey(app.config());
    let availability = build_availability(
        crate::msstore::STORE_BUILD,
        pubkey.as_deref(),
        cfg!(debug_assertions) || tauri::is_dev(),
    );
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.updater.availability = availability;
        inner.updater.schedule.start(now_secs());
    }
    if let Some(dir) = staging_dir(app) {
        let _ = std::fs::remove_dir_all(dir);
    }
    crate::menu::refresh_update_item(app);
    crate::tray::refresh(app);
    match availability.reason() {
        None => log::info!("updater: on (checks {} and {})", REPO, BETA_TAG),
        Some(reason) => {
            log::warn!("updater: off ({availability:?}) — {reason}");
            return;
        }
    }
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            let due = {
                let s = state(&h);
                let inner = s.lock();
                should_auto_check(
                    inner.settings.preferences.auto_update_check,
                    inner.updater.availability,
                    &inner.updater.status,
                    &inner.updater.schedule,
                    now_secs(),
                )
            };
            if let Some((channel, generation)) = due.then(|| begin_check(&h)).flatten() {
                run_check(&h, channel, generation, Trigger::Auto).await;
            }
        }
    });
}

/// `status` from the check / download of `generation`; nothing when the channel changed since.
fn set_status(app: &AppHandle, generation: u64, status: Status) -> bool {
    let current = state(app).lock().updater.report(generation, status);
    if current {
        ui_changed(app);
    }
    current
}

/// The menu item, the tray and (while it shows a bundled screen) the main window follow.
fn ui_changed(app: &AppHandle) {
    crate::menu::refresh_update_item(app);
    crate::tray::refresh(app);
    let local_screen = state(app).lock().phase != crate::state::Phase::InApp;
    if local_screen {
        if let Some(w) = crate::shell::main_window(app) {
            let _ = tauri::Emitter::emit_to(&w, crate::shell::MAIN, "desktop://changed", ());
        }
    }
}

/// "Check for Updates…" (menu, tray, Settings, the "Please update" screen). The Store build
/// opens the Store page.
pub fn check_now(app: &AppHandle, trigger: Trigger) -> Result<(), String> {
    {
        let s = state(app);
        let inner = s.lock();
        if inner.updater.availability == Availability::Store {
            drop(inner);
            return open_store_page(app);
        }
        if let Some(reason) = inner.updater.availability.reason() {
            return Err(reason.to_string());
        }
    }
    // marked as checking before the command returns, so the window that asked sees it at once
    let Some((channel, generation)) = begin_check(app) else {
        return Ok(());
    };
    let h = app.clone();
    tauri::async_runtime::spawn(async move { run_check(&h, channel, generation, trigger).await });
    Ok(())
}

/// Marks a check as running, unless one runs or an update already waits. The channel to ask
/// and the generation the check belongs to.
fn begin_check(app: &AppHandle) -> Option<(Channel, u64)> {
    let started = {
        let s = state(app);
        let mut inner = s.lock();
        let channel = inner.settings.preferences.update_channel;
        inner.updater.begin_check().map(|generation| (channel, generation))
    };
    if started.is_some() {
        ui_changed(app);
    }
    started
}

async fn run_check(app: &AppHandle, channel: Channel, generation: u64, trigger: Trigger) {
    let result = find_update(app, channel.manifests(), github_only).await;
    let now = now_secs();
    {
        let s = state(app);
        let mut inner = s.lock();
        if !inner.updater.is_current(generation) {
            log::info!("updater: the channel changed during the check; its answer is dropped");
            return;
        }
        inner.updater.schedule.record(now, result.is_ok());
        if result.is_ok() {
            inner.settings.last_update_check = Some(now);
            inner.save_settings();
        }
    }
    match result {
        Ok(Some(update)) => download(app, update, generation, trigger).await,
        Ok(None) => {
            log::info!("updater: {APP_VERSION} is the newest version ({channel:?})");
            if set_status(app, generation, Status::UpToDate) && trigger == Trigger::Menu {
                crate::bridge::show_notification(
                    app,
                    "Brainitive is up to date",
                    &format!("Version {APP_VERSION} is the newest version."),
                    None,
                );
            }
        }
        Err(e) => {
            log::warn!("updater: check failed: {e}");
            failed(
                app,
                generation,
                trigger,
                "Could not check for updates. Check your connection and try again later.",
            );
        }
    }
}

fn failed(app: &AppHandle, generation: u64, trigger: Trigger, message: &str) {
    let current = set_status(
        app,
        generation,
        Status::Failed {
            message: message.to_string(),
        },
    );
    if current && trigger == Trigger::Menu {
        crate::bridge::show_notification(app, "Update failed", message, None);
    }
}

async fn download(app: &AppHandle, update: Update, generation: u64, trigger: Trigger) {
    let version = update.version.clone();
    if !allowed_url(&update.download_url) {
        log::warn!(
            "updater: {version} refused: its bundle is not on GitHub ({})",
            update.download_url
        );
        failed(app, generation, trigger, "The update could not be downloaded.");
        return;
    }
    log::info!("updater: downloading {version}");
    if !set_status(
        app,
        generation,
        Status::Downloading {
            version: version.clone(),
            percent: None,
        },
    ) {
        return;
    }
    let mut done: u64 = 0;
    let mut shown: Option<u8> = None;
    let progress = app.clone();
    let progress_version = version.clone();
    // `download` verifies the signature (and the signed version) before it returns the bytes
    let bytes = update
        .download(
            |chunk, total| {
                done += chunk as u64;
                let p = percent(done, total);
                if p.map(|p| p / 10) != shown.map(|p| p / 10) {
                    shown = p;
                    let status = Status::Downloading {
                        version: progress_version.clone(),
                        percent: p,
                    };
                    if state(&progress).lock().updater.report(generation, status) {
                        ui_changed(&progress);
                    }
                }
            },
            || {},
        )
        .await;
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(e) => {
            log::warn!("updater: {version} not downloaded or not verified: {e}");
            failed(
                app,
                generation,
                trigger,
                "The update could not be downloaded or its signature did not match. Nothing was installed.",
            );
            return;
        }
    };
    if !state(app).lock().updater.is_current(generation) {
        log::info!("updater: the channel changed during the download; {version} is discarded");
        return;
    }
    // one file per generation: a discarded download never overwrites the current one
    let Some(file) = staging_dir(app).map(|d| d.join(format!("pending-{generation}.update"))) else {
        failed(app, generation, trigger, "The update could not be stored.");
        return;
    };
    if let Err(e) = write_private(&file, &bytes) {
        log::warn!("updater: could not store the update: {e}");
        failed(app, generation, trigger, "The update could not be stored.");
        return;
    }
    let accepted = {
        let s = state(app);
        let mut inner = s.lock();
        let accepted = inner.updater.report(
            generation,
            Status::Ready {
                version: version.clone(),
            },
        );
        if accepted {
            inner.updater.pending = Some(Pending {
                update,
                file: file.clone(),
            });
        }
        accepted
    };
    if !accepted {
        log::info!("updater: the channel changed during the download; {version} is discarded");
        let _ = std::fs::remove_file(&file);
        return;
    }
    log::info!("updater: {version} downloaded and verified; waiting for Restart to Update");
    ui_changed(app);
    crate::bridge::show_notification(
        app,
        "Update ready — restart to install",
        &format!("Brainitive {version} is downloaded. Choose Restart to Update in the menu when it suits you."),
        None,
    );
}

fn write_private(file: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    crate::private_fs::write(file, bytes)
}

/// "Restart to Update": the user's click. Installs the waiting update (its signature is checked
/// again against the stored bytes) and restarts. Windows: the installer closes the app itself.
pub fn restart_to_update(app: &AppHandle) -> Result<(), String> {
    let pending = {
        let s = state(app);
        let mut inner = s.lock();
        let Some(pending) = inner
            .updater
            .pending
            .clone()
            .filter(|_| inner.updater.status.is_ready())
        else {
            return Err("No update is waiting.".into());
        };
        inner.updater.status = Status::Installing {
            version: pending.update.version.clone(),
        };
        pending
    };
    ui_changed(app);
    let h = app.clone();
    // never on the main thread: the macOS install may ask for an admin password on it
    tauri::async_runtime::spawn(async move {
        let version = pending.update.version.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let bytes = std::fs::read(&pending.file).map_err(|e| e.to_string())?;
            pending.update.install(bytes).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        match result {
            Ok(()) => {
                log::info!("updater: {version} installed, restarting");
                h.request_restart();
            }
            Err(e) => {
                log::warn!("updater: installing {version} failed: {e}");
                let generation = {
                    let s = state(&h);
                    let mut inner = s.lock();
                    inner.updater.pending = None;
                    inner.updater.generation
                };
                if let Some(dir) = staging_dir(&h) {
                    let _ = std::fs::remove_dir_all(dir);
                }
                failed(
                    &h,
                    generation,
                    Trigger::Menu,
                    "The update could not be installed. Nothing was changed.",
                );
            }
        }
    });
    Ok(())
}

/// The app's page in the Microsoft Store app (a constant, never an argument).
pub fn open_store_page(app: &AppHandle) -> Result<(), String> {
    tauri_plugin_opener::OpenerExt::opener(app)
        .open_url(crate::msstore::STORE_URL, None::<&str>)
        .map_err(|e| format!("The Microsoft Store could not be opened: {e}"))
}

/// The menu / tray item's click: restart when an update waits, otherwise check (the Store
/// build: open the Store page).
pub fn on_menu_click(app: &AppHandle) {
    let ready = state(app).lock().updater.status.is_ready();
    let result = if ready {
        restart_to_update(app)
    } else {
        check_now(app, Trigger::Menu)
    };
    if let Err(e) = result {
        log::info!("updater: {e}");
    }
}

/// Settings: another channel. A waiting update of the old channel is dropped, a check or
/// download still running for it is discarded when it ends ([`UpdaterState::switch_channel`]),
/// and an automatic check of the new channel runs soon.
pub fn set_channel(app: &AppHandle, channel: Channel) -> Result<(), String> {
    if state(app).lock().updater.availability == Availability::Store {
        return Err(Availability::Store.reason().unwrap_or_default().to_string());
    }
    let changed = {
        let s = state(app);
        let mut inner = s.lock();
        let changed = inner.settings.preferences.update_channel != channel;
        if changed {
            inner.settings.preferences.update_channel = channel;
            inner.save_settings();
            inner.updater.switch_channel(now_secs());
        }
        // an install already running reads its file from the staging folder
        changed && !matches!(inner.updater.status, Status::Installing { .. })
    };
    if changed {
        if let Some(dir) = staging_dir(app) {
            let _ = std::fs::remove_dir_all(dir);
        }
        ui_changed(app);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway key made in the test (never a real one): minisign, encoded the way
    // `tauri signer generate` prints it (base64 of the key file).
    struct TestKey {
        pubkey: String,
        secret: minisign::SecretKey,
    }

    fn test_key() -> TestKey {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let file = pair.pk.to_box().unwrap().to_string();
        TestKey {
            pubkey: STANDARD.encode(file),
            secret: pair.sk,
        }
    }

    /// A signature like `tauri build` writes it: base64 of the `.sig` file, the version in the
    /// trusted comment.
    fn sign(key: &TestKey, data: &[u8], version: &str) -> String {
        let comment = format!("timestamp:1790000000\tfile:bundle\tversion:{version}");
        let sig = minisign::sign(None, &key.secret, data, Some(&comment), None).unwrap();
        STANDARD.encode(sig.to_string())
    }

    #[test]
    fn channels_and_manifest_urls() {
        assert_eq!(Channel::default(), Channel::Stable);
        let stable = Channel::Stable.manifests();
        assert_eq!(
            stable.iter().map(Url::as_str).collect::<Vec<_>>(),
            ["https://github.com/armendzekjiri/brainitive-desktop/releases/latest/download/latest.json"]
        );
        let beta = Channel::Beta.manifests();
        assert_eq!(
            beta.iter().map(Url::as_str).collect::<Vec<_>>(),
            [
                "https://github.com/armendzekjiri/brainitive-desktop/releases/download/updater-beta/beta.json",
                "https://github.com/armendzekjiri/brainitive-desktop/releases/latest/download/latest.json",
            ]
        );
        for url in stable.iter().chain(&beta) {
            assert!(allowed_url(url), "{url}");
        }
        assert_eq!(serde_json::to_string(&Channel::Beta).unwrap(), "\"beta\"");
    }

    #[test]
    fn an_unknown_channel_in_the_file_is_stable() {
        #[derive(Deserialize)]
        struct P {
            #[serde(deserialize_with = "lenient_channel")]
            c: Channel,
        }
        let p: P = serde_json::from_str(r#"{"c":"beta"}"#).unwrap();
        assert_eq!(p.c, Channel::Beta);
        let p: P = serde_json::from_str(r#"{"c":"nightly"}"#).unwrap();
        assert_eq!(p.c, Channel::Stable);
        let p: P = serde_json::from_str(r#"{"c":7}"#).unwrap();
        assert_eq!(p.c, Channel::Stable);
    }

    #[test]
    fn only_https_github_hosts_and_few_redirects() {
        let u = |s: &str| Url::parse(s).unwrap();
        for ok in [
            "https://github.com/armendzekjiri/brainitive-desktop/releases/download/v1.0.0/latest.json",
            "https://objects.githubusercontent.com/github-production-release-asset/1/2?sig=x",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/2",
            "https://GitHub.com/x",
        ] {
            assert!(allowed_url(&u(ok)), "{ok}");
        }
        for bad in [
            "http://github.com/x",
            "https://github.com.evil.example/x",
            "https://evil.example/github.com/x",
            "https://gist.github.com/x",
            "https://raw.githubusercontent.com/x",
            "https://github.com:8443/x",
            "https://user:pw@github.com/x",
            "file:///etc/passwd",
        ] {
            assert!(!allowed_url(&u(bad)), "{bad}");
        }
        let cdn = u("https://release-assets.githubusercontent.com/a");
        assert!(follow_redirect(&cdn, 1));
        assert!(follow_redirect(&cdn, MAX_REDIRECTS));
        assert!(!follow_redirect(&cdn, MAX_REDIRECTS + 1));
        assert!(!follow_redirect(&u("https://evil.example/a"), 1));
    }

    #[test]
    fn the_placeholder_key_switches_the_updater_off() {
        let placeholder = "PLACEHOLDER-UPDATER-PUBLIC-KEY-run-npx-tauri-signer-generate-see-docs-RELEASING-md";
        assert_eq!(key_status(placeholder), KeyStatus::Placeholder);
        assert_eq!(key_status(""), KeyStatus::Placeholder);
        assert_eq!(key_status("  "), KeyStatus::Placeholder);
        assert_eq!(availability(Some(placeholder), false), Availability::PlaceholderKey);
        assert_eq!(availability(None, false), Availability::PlaceholderKey);
        assert!(!availability(Some(placeholder), false).is_available());
        assert!(availability(Some(placeholder), false).reason().is_some());

        // not a minisign key
        assert_eq!(key_status("not base64 !"), KeyStatus::Invalid);
        assert_eq!(key_status(&STANDARD.encode("hello")), KeyStatus::Invalid);
        assert_eq!(
            key_status(&STANDARD.encode("untrusted comment: x\nAAAA")),
            KeyStatus::Invalid
        );
        assert_eq!(availability(Some("xyz"), false), Availability::InvalidKey);

        // a real key (made here, thrown away)
        let key = test_key();
        assert_eq!(key_status(&key.pubkey), KeyStatus::Valid);
        assert_eq!(availability(Some(&key.pubkey), false), Availability::Available);
        assert_eq!(availability(Some(&key.pubkey), false).reason(), None);
        // … but never in a development build
        assert_eq!(availability(Some(&key.pubkey), true), Availability::DevBuild);
        assert!(Availability::default().reason().is_some());
    }

    #[test]
    fn the_committed_config_has_the_placeholder_or_a_real_key_never_garbage() {
        let path = format!("{}/tauri.conf.json", env!("CARGO_MANIFEST_DIR"));
        let config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let updater = &config["plugins"]["updater"];
        let key = updater["pubkey"].as_str().expect("plugins.updater.pubkey");
        assert_ne!(
            key_status(key),
            KeyStatus::Invalid,
            "the updater key is neither the placeholder nor a minisign key"
        );
        assert_eq!(updater["requireSignedVersion"], true);
        // the endpoints are code (channels), never an http one in the config
        for e in updater["endpoints"].as_array().map(Vec::as_slice).unwrap_or_default() {
            assert!(e.as_str().unwrap().starts_with("https://"));
        }
        for flag in [
            "dangerousInsecureTransportProtocol",
            "dangerousAcceptInvalidCerts",
            "dangerousAcceptInvalidHostnames",
            "allowDowngrades",
        ] {
            assert!(updater.get(flag).is_none_or(|v| v == false), "{flag}");
        }
        // never on: CI builds without the updater key and signs the bundles afterwards (BRAI-377)
        assert!(config["bundle"]
            .get("createUpdaterArtifacts")
            .is_none_or(|v| v == false));
    }

    #[test]
    fn only_newer_versions_are_updates() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert!(is_newer(&v("0.1.0"), &v("0.2.0")));
        assert!(is_newer(&v("1.0.0-beta.1"), &v("1.0.0-beta.2")));
        assert!(is_newer(&v("1.0.0-beta.2"), &v("1.0.0")));
        assert!(!is_newer(&v("1.0.0"), &v("1.0.0")));
        assert!(!is_newer(&v("1.0.0"), &v("0.9.9")));
        assert!(!is_newer(&v("1.0.0"), &v("1.0.0-beta.9")));
        // beta: the higher of the two manifests wins
        fn pick(items: Vec<&'static str>) -> Option<&'static str> {
            newest(items, |s| Version::parse(s).ok())
        }
        assert_eq!(pick(vec!["1.1.0-beta.1", "1.0.2"]), Some("1.1.0-beta.1"));
        assert_eq!(pick(vec!["1.1.0-beta.1", "1.1.0"]), Some("1.1.0"));
        assert_eq!(pick(vec![]), None);
    }

    #[test]
    fn checks_30_seconds_after_start_then_every_6_hours() {
        let t0 = 1_800_000_000;
        let mut s = Schedule::default();
        assert!(!s.due(t0), "not before start");
        s.start(t0);
        assert!(!s.due(t0 + 29));
        assert!(s.due(t0 + 30));
        s.record(t0 + 30, true);
        assert!(!s.due(t0 + 30 + 6 * 3600 - 1));
        assert!(s.due(t0 + 30 + 6 * 3600));
        // after a sleep of 2 days: due at once
        assert!(s.due(t0 + 2 * 86_400));
        // a failure retries after an hour
        s.record(t0 + 100, false);
        assert!(!s.due(t0 + 100 + 3599));
        assert!(s.due(t0 + 100 + 3600));
        // the clock went back: check rather than wait for a time that may never come
        s.record(t0 + 100, true);
        assert!(s.due(t0 + 50));
    }

    #[test]
    fn automatic_checks_need_the_setting_a_key_and_nothing_running() {
        let t0 = 1_800_000_000;
        let mut s = Schedule::default();
        s.start(t0);
        let now = t0 + 60;
        let idle = Status::Idle;
        assert!(should_auto_check(true, Availability::Available, &idle, &s, now));
        assert!(!should_auto_check(false, Availability::Available, &idle, &s, now));
        assert!(!should_auto_check(true, Availability::PlaceholderKey, &idle, &s, now));
        assert!(!should_auto_check(true, Availability::DevBuild, &idle, &s, now));
        assert!(!should_auto_check(
            true,
            Availability::Available,
            &Status::Checking,
            &s,
            now
        ));
        let ready = Status::Ready {
            version: "9.0.0".into(),
        };
        assert!(!should_auto_check(true, Availability::Available, &ready, &s, now));
        assert!(!should_auto_check(true, Availability::Available, &idle, &s, t0 + 10));
    }

    #[test]
    fn a_channel_change_discards_the_running_check_and_download() {
        let mut u = UpdaterState::default();
        let old = u.begin_check().unwrap();
        assert_eq!(u.begin_check(), None, "one check at a time");
        assert!(u.report(
            old,
            Status::Downloading {
                version: "1.1.0".into(),
                percent: Some(40)
            }
        ));
        // Settings → Beta while the stable download runs
        u.switch_channel(1_000_000);
        assert!(!u.is_current(old));
        assert_eq!(u.status, Status::Idle, "the old download no longer shows");
        assert!(u.pending.is_none());
        // the old download's progress and its "ready" are dropped
        assert!(!u.report(
            old,
            Status::Downloading {
                version: "1.1.0".into(),
                percent: Some(90)
            }
        ));
        assert!(!u.report(
            old,
            Status::Ready {
                version: "1.1.0".into()
            }
        ));
        assert_eq!(u.status, Status::Idle);
        // the new channel's check is due soon and reports normally
        assert!(u.schedule.due(1_000_000));
        let new = u.begin_check().unwrap();
        assert_ne!(new, old);
        assert!(u.report(
            new,
            Status::Ready {
                version: "1.2.0-beta.1".into()
            }
        ));
        assert!(u.status.is_ready());
        // an old failure arriving late doesn't hide the new update either
        assert!(!u.report(old, Status::Failed { message: "x".into() }));
        assert!(u.status.is_ready());
    }

    #[test]
    fn a_channel_change_leaves_a_running_install_alone() {
        let mut u = UpdaterState {
            status: Status::Installing {
                version: "1.1.0".into(),
            },
            ..UpdaterState::default()
        };
        u.switch_channel(5);
        assert!(matches!(u.status, Status::Installing { .. }));
        assert_eq!(u.generation, 1);
    }

    #[test]
    fn the_menu_item_restarts_only_when_an_update_is_ready() {
        let (text, enabled) = menu_item(Availability::PlaceholderKey, &Status::Idle);
        assert_eq!((text.as_str(), enabled), ("Check for Updates…", false));
        assert_eq!(
            menu_item(Availability::Available, &Status::Idle),
            ("Check for Updates…".into(), true)
        );
        assert!(menu_item(Availability::Available, &Status::UpToDate).1);
        assert!(!menu_item(Availability::Available, &Status::Checking).1);
        let downloading = Status::Downloading {
            version: "1.2.0".into(),
            percent: Some(40),
        };
        assert!(!menu_item(Availability::Available, &downloading).1);
        let ready = Status::Ready {
            version: "1.2.0".into(),
        };
        assert_eq!(
            menu_item(Availability::Available, &ready),
            ("Restart to Update (1.2.0)".into(), true)
        );
        assert_eq!(percent(50, Some(200)), Some(25));
        assert_eq!(percent(500, Some(200)), Some(100));
        assert_eq!(percent(5, None), None);
        assert_eq!(percent(5, Some(0)), None);
        assert_eq!(
            serde_json::to_value(&ready).unwrap(),
            serde_json::json!({"state": "ready", "version": "1.2.0"})
        );
    }

    // ---- the plugin itself, with the throwaway key -------------------------------------------

    fn mock_app(pubkey: &str) -> tauri::App<tauri::test::MockRuntime> {
        mock_app_at(pubkey, "0.1.0")
    }

    fn mock_app_at(pubkey: &str, version: &str) -> tauri::App<tauri::test::MockRuntime> {
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.package_info_mut().version = Version::parse(version).unwrap();
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({ "pubkey": pubkey, "requireSignedVersion": true }),
        );
        tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap()
    }

    /// A static manifest as CI writes it, for this machine's target (the mock app is 0.1.0).
    fn manifest(version: &str, url: &str, signature: &str) -> serde_json::Value {
        let target = tauri_plugin_updater::target().unwrap();
        serde_json::json!({
            "version": version,
            "notes": "test",
            "pub_date": "2026-11-01T00:00:00Z",
            "platforms": { target: { "url": url, "signature": signature } }
        })
    }

    #[test]
    fn the_plugin_refuses_bad_signatures_and_old_versions() {
        let key = test_key();
        let other = test_key();
        let app = mock_app(&key.pubkey);
        let updater = build_updater(app.handle(), stable_manifest_url(REPO), github_only).unwrap();
        let bundle = b"the new app".as_slice();
        let url = "https://github.com/armendzekjiri/brainitive-desktop/releases/download/v0.2.0/b.tar.gz";

        // signed with our key for this version: accepted
        let good = updater
            .restore_update(manifest("0.2.0", url, &sign(&key, bundle, "0.2.0")))
            .unwrap()
            .expect("0.2.0 is newer than 0.1.0");
        good.verify(bundle).unwrap();
        // other bytes than the signed ones
        assert!(good.verify(b"tampered").is_err());

        // signed with another key
        let foreign = updater
            .restore_update(manifest("0.2.0", url, &sign(&other, bundle, "0.2.0")))
            .unwrap()
            .unwrap();
        assert!(foreign.verify(bundle).is_err());

        // a valid signature of an older release announced as a newer version (downgrade trick)
        let replay = updater
            .restore_update(manifest("0.3.0", url, &sign(&key, bundle, "0.0.9")))
            .unwrap()
            .unwrap();
        assert!(replay.verify(bundle).is_err());

        // garbage instead of a signature
        let garbage = updater
            .restore_update(manifest("0.2.0", url, "bm90IGEgc2lnbmF0dXJl"))
            .unwrap()
            .unwrap();
        assert!(garbage.verify(bundle).is_err());

        // equal and older versions are no update at all
        for old in ["0.1.0", "0.0.9", "0.1.0-beta.3"] {
            let m = manifest(old, url, &sign(&key, bundle, old));
            assert!(updater.restore_update(m).unwrap().is_none(), "{old}");
        }
    }

    /// A one-shot loopback HTTP server: answers each request with the next response.
    async fn serve(responses: Vec<(&'static str, Vec<u8>)>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for (content_type, body) in responses {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let _ = socket.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            }
        });
        format!("http://{addr}")
    }

    fn loopback(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        builder.no_proxy()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_served_manifest_with_a_bad_signature_is_never_installed() {
        let key = test_key();
        let other = test_key();
        let app = mock_app(&key.pubkey);
        let bundle = b"bundle bytes".to_vec();

        for (signer, expect_ok) in [(&key, true), (&other, false)] {
            let bundle_server = serve(vec![("application/octet-stream", bundle.clone())]).await;
            let m = manifest(
                "0.2.0",
                &format!("{bundle_server}/b.tar.gz"),
                &sign(signer, &bundle, "0.2.0"),
            );
            let manifest_server = serve(vec![("application/json", serde_json::to_vec(&m).unwrap())]).await;
            let url = Url::parse(&format!("{manifest_server}/latest.json")).unwrap();
            let update = find_update(app.handle(), vec![url], loopback)
                .await
                .unwrap()
                .expect("0.2.0 is offered");
            assert_eq!(update.version, "0.2.0");
            let downloaded = update.download(|_, _| {}, || {}).await;
            assert_eq!(downloaded.is_ok(), expect_ok, "signed by the right key: {expect_ok}");
            if let Ok(bytes) = downloaded {
                assert_eq!(bytes, bundle);
            }
        }

        // an equal version served: no update
        let m = manifest("0.1.0", "https://github.com/x", &sign(&key, &bundle, "0.1.0"));
        let manifest_server = serve(vec![("application/json", serde_json::to_vec(&m).unwrap())]).await;
        let url = Url::parse(&format!("{manifest_server}/latest.json")).unwrap();
        assert!(find_update(app.handle(), vec![url], loopback).await.unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn beta_takes_the_newest_of_both_manifests_and_survives_one_failing() {
        let key = test_key();
        let app = mock_app(&key.pubkey);
        let sig = |v: &str| sign(&key, b"x", v);
        let beta = manifest("0.3.0-beta.1", "https://github.com/b", &sig("0.3.0-beta.1"));
        let stable = manifest("0.3.0", "https://github.com/s", &sig("0.3.0"));
        let b = serve(vec![("application/json", serde_json::to_vec(&beta).unwrap())]).await;
        let s = serve(vec![("application/json", serde_json::to_vec(&stable).unwrap())]).await;
        let urls = vec![
            Url::parse(&format!("{b}/beta.json")).unwrap(),
            Url::parse(&format!("{s}/latest.json")).unwrap(),
        ];
        let update = find_update(app.handle(), urls, loopback).await.unwrap().unwrap();
        assert_eq!(update.version, "0.3.0");

        // the beta manifest is missing (nothing listens there): the stable answer still counts
        let s = serve(vec![("application/json", serde_json::to_vec(&stable).unwrap())]).await;
        let urls = vec![
            Url::parse("http://127.0.0.1:9/beta.json").unwrap(),
            Url::parse(&format!("{s}/latest.json")).unwrap(),
        ];
        let update = find_update(app.handle(), urls, loopback).await.unwrap().unwrap();
        assert_eq!(update.version, "0.3.0");

        // nothing answers: an error (the schedule retries in an hour)
        let urls = vec![Url::parse("http://127.0.0.1:9/latest.json").unwrap()];
        assert!(find_update(app.handle(), urls, loopback).await.is_err());
    }

    /// CI, after `tauri build` with the signing secret: an updater bundle CI just signed must
    /// verify with the public key committed in `tauri.conf.json`, exactly as the shipped app
    /// checks it (plugin, `requireSignedVersion`, the app's version). A mismatched key pair would
    /// otherwise ship a release nobody can update to. Skipped without the variable.
    /// `BRAINITIVE_UPDATER_ARTIFACT=<bundle> cargo test --lib ci_signed_bundle -- --ignored`
    #[test]
    #[ignore]
    fn ci_signed_bundle_verifies_with_the_committed_key() {
        let Some(artifact) = std::env::var("BRAINITIVE_UPDATER_ARTIFACT")
            .ok()
            .filter(|a| !a.is_empty())
        else {
            eprintln!("BRAINITIVE_UPDATER_ARTIFACT not set: nothing to check");
            return;
        };
        let path = format!("{}/tauri.conf.json", env!("CARGO_MANIFEST_DIR"));
        let config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let pubkey = config["plugins"]["updater"]["pubkey"].as_str().unwrap();
        assert_eq!(
            key_status(pubkey),
            KeyStatus::Valid,
            "tauri.conf.json still has no real updater key"
        );
        let version = config["version"].as_str().unwrap();
        let bytes = std::fs::read(&artifact).expect("the bundle");
        let signature = std::fs::read_to_string(format!("{artifact}.sig")).expect("its .sig");
        let app = mock_app_at(pubkey, "0.0.0");
        let updater = build_updater(app.handle(), stable_manifest_url(REPO), github_only).unwrap();
        let update = updater
            .restore_update(manifest(version, "https://github.com/x", signature.trim()))
            .unwrap()
            .expect("the release version is above 0.0.0");
        update
            .verify(&bytes)
            .expect("the bundle's signature does not match the committed public key (or lacks the version)");
    }

    #[test]
    fn the_github_client_refuses_plain_http() {
        // https_only is part of the client every updater request uses
        let client = github_only(reqwest::Client::builder()).build().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(client.get("http://127.0.0.1:9/latest.json").send())
            .unwrap_err();
        assert!(err.is_builder() || err.to_string().contains("scheme"), "{err}");
    }

    // ---- the Microsoft Store build (BRAI-442) -------------------------------------------------

    #[test]
    fn the_store_build_never_updates_itself_whatever_the_key() {
        let key = test_key();
        let valid = key.pubkey.as_str();
        assert_eq!(build_availability(true, Some(valid), false), Availability::Store);
        assert_eq!(build_availability(true, None, false), Availability::Store);
        assert_eq!(build_availability(true, Some(valid), true), Availability::Store);
        // the normal build keeps its guard
        assert_eq!(build_availability(false, Some(valid), false), Availability::Available);
        assert_eq!(build_availability(false, None, false), Availability::PlaceholderKey);
        assert!(!Availability::Store.is_available());
        assert!(Availability::Store.reason().unwrap().contains("Microsoft Store"));
        // no automatic check ever starts
        let mut schedule = Schedule::default();
        schedule.start(0);
        assert!(!should_auto_check(
            true,
            Availability::Store,
            &Status::Idle,
            &schedule,
            1_000_000
        ));
    }

    #[test]
    fn this_binary_is_guarded_by_its_feature() {
        let key = test_key();
        let expected = if cfg!(feature = "msstore") {
            Availability::Store
        } else {
            Availability::Available
        };
        assert_eq!(
            build_availability(crate::msstore::STORE_BUILD, Some(key.pubkey.as_str()), false),
            expected
        );
    }

    #[test]
    fn the_store_build_menu_item_opens_the_store() {
        // clickable even though nothing is "available": the click opens the Store page
        for status in [Status::Idle, Status::UpToDate, Status::Failed { message: "x".into() }] {
            assert_eq!(
                menu_item(Availability::Store, &status),
                ("Check for Updates…".into(), true)
            );
        }
        // never "Restart to Update" in the Store build
        let ready = Status::Ready {
            version: "9.9.9".into(),
        };
        assert_eq!(
            menu_item(Availability::Store, &ready),
            ("Check for Updates…".into(), true)
        );
    }
}
