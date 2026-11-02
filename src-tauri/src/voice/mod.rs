//! Voice capture (BRAI-439): hold a global shortcut, speak, let go — the speech becomes text **on
//! this computer** (whisper.cpp) and opens in quick capture to check and save.
//!
//! - `recorder` — the state machine (pure); `audio` — the microphone, in memory only;
//!   `whisper` — the transcription (background threads); `model` — the model download / check /
//!   removal; `permission` — the OS microphone permission.
//! - This module is the glue: the shortcut handler ([`on_hotkey`]), the menu / tray / quick-capture
//!   entry ([`start_from_menu`]), the commands of the indicator window (`voice.html`, label
//!   `voice`, `capabilities/voice.json`) and the Settings window's voice part.
//!
//! The indicator only ever gets a view model (events `voice://view` and `voice://level`, sent to
//! that window only). Audio never leaves this process: no file, no network.

pub mod audio;
pub mod model;
pub mod permission;
pub mod recorder;
pub mod whisper;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Webview};

use crate::popup::{self, Popup};
use crate::state::state;
use audio::{AudioError, Capture};
use model::{ModelError, MODEL};
use permission::MicPermission;
use recorder::{Effect, Input, Phase, Recorder, MAX_LENGTH};

pub const AUTO_LANGUAGE: &str = "auto";

/// The languages Settings offers (whisper's codes). whisper knows ~100; these are the ones our
/// users speak most. "auto" lets whisper detect it per recording.
pub const LANGUAGES: &[(&str, &str)] = &[
    (AUTO_LANGUAGE, "Detect automatically"),
    ("en", "English"),
    ("de", "German"),
    ("sq", "Albanian"),
    ("fr", "French"),
    ("it", "Italian"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("tr", "Turkish"),
    ("pl", "Polish"),
    ("sr", "Serbian"),
    ("hr", "Croatian"),
    ("bs", "Bosnian"),
];

pub fn is_language(code: &str) -> bool {
    LANGUAGES.iter().any(|(c, _)| *c == code)
}

/// What the indicator window shows (besides recording / transcribing, which come from the
/// recorder).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
pub enum Screen {
    /// Voice capture is off: what it does, "Turn on".
    Intro,
    /// The system asks for the microphone (macOS).
    Permission,
    /// The microphone is blocked: how to allow it.
    Denied { message: &'static str },
    Downloading { done: u64, total: u64 },
    DownloadFailed { message: String },
    /// Set up: "Hold ⇧⌘V and speak" / Start.
    Ready,
    Recording { elapsed_ms: u64, max_ms: u64, hold: bool },
    Transcribing,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceView {
    #[serde(flatten)]
    screen: Screen,
    /// "⇧⌘V"; empty when the shortcut is off.
    shortcut: String,
    platform: &'static str,
    model_name: &'static str,
    model_mb: u64,
    /// The OS has a privacy settings page to open.
    can_open_settings: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct Level {
    level: f32,
    elapsed_ms: u64,
}

struct Inner {
    recorder: Recorder,
    screen: Screen,
    /// The open microphone of the current session.
    capture: Option<Capture>,
    /// The model being loaded for `session` while the user speaks.
    loader: Option<(u64, JoinHandle<Result<whisper::Model, String>>)>,
    /// A model download in progress: its cancel flag and progress.
    download: Option<(Arc<AtomicBool>, u64)>,
}

/// Managed by Tauri next to `AppState` (its own lock: the audio callbacks and transcription
/// threads never touch the app state's lock).
pub struct Voice {
    inner: Mutex<Inner>,
    data_dir: PathBuf,
}

impl Voice {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn voice(app: &AppHandle) -> tauri::State<'_, Voice> {
    app.state::<Voice>()
}

/// Called in `setup()`: the state, whisper's log routing, and the removal of partial downloads.
pub fn init(app: &AppHandle, data_dir: PathBuf) {
    whisper::init_logging();
    let dir = data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || model::clean(&dir, &MODEL));
    app.manage(Voice {
        inner: Mutex::new(Inner {
            recorder: Recorder::default(),
            screen: Screen::Intro,
            capture: None,
            loader: None,
            download: None,
        }),
        data_dir,
    });
}

// ---- entry points -------------------------------------------------------------------------

/// The voice shortcut went down (`pressed`) or up.
pub fn on_hotkey(app: &AppHandle, pressed: bool) {
    if !pressed {
        feed(app, Input::HotkeyUp { at: Instant::now() });
    } else if voice(app).lock().recorder.busy() {
        feed(app, Input::HotkeyDown);
    } else {
        begin(app, true);
    }
}

/// Tray, menu, the quick-capture window's button, the indicator's Start: no key is held, so the
/// recording runs until Enter / Stop / the shortcut again. While recording: stops.
pub fn start_from_menu(app: &AppHandle) {
    if voice(app).lock().recorder.busy() {
        feed(app, Input::Stop);
    } else {
        begin(app, false);
    }
}

/// Everything a start needs, in order: turned on, model there, microphone allowed, then record.
fn begin(app: &AppHandle, hold: bool) {
    let enabled = state(app).lock().settings.preferences.voice_enabled;
    if !enabled {
        return show(app, Screen::Intro);
    }
    let v = voice(app);
    let downloading = v.lock().download.as_ref().map(|(_, done)| *done);
    if let Some(done) = downloading {
        return show(app, Screen::Downloading { done, total: MODEL.size });
    }
    if !model::is_ready(&v.data_dir, &MODEL) {
        return start_download(app);
    }
    match permission::status() {
        MicPermission::Denied | MicPermission::Restricted => show(app, denied()),
        MicPermission::NotDetermined => ask_permission(app),
        MicPermission::Granted | MicPermission::Unknown => {
            // the guard must be gone before `run` (it locks again): no `if let` on the lock
            let effect = v.lock().recorder.handle(Input::Start { at: Instant::now(), hold });
            if let Some(effect) = effect {
                show_window(app);
                run(app, effect);
            }
        }
    }
}

fn denied() -> Screen {
    Screen::Denied {
        message: if cfg!(target_os = "macos") {
            "Brainitive may not use the microphone. Allow it in System Settings → Privacy & Security → Microphone, then try again."
        } else if cfg!(windows) {
            "Brainitive may not use the microphone. Turn on \u{201c}Let desktop apps access your microphone\u{201d} in Settings → Privacy & security → Microphone, then try again."
        } else {
            "The microphone could not be opened. Check that one is connected, not muted, and that your user may use audio devices."
        },
    }
}

fn ask_permission(app: &AppHandle) {
    show(app, Screen::Permission);
    let h = app.clone();
    permission::request(move |granted| {
        log::info!("voice: microphone permission {}", if granted { "granted" } else { "denied" });
        set_screen(&h, if granted { Screen::Ready } else { denied() });
    });
}

/// After set-up steps: what the indicator says next.
fn next_step(app: &AppHandle) -> Screen {
    match permission::status() {
        MicPermission::Denied | MicPermission::Restricted => denied(),
        MicPermission::NotDetermined => {
            ask_permission(app);
            Screen::Permission
        }
        _ => Screen::Ready,
    }
}

// ---- the recorder's effects ---------------------------------------------------------------

fn feed(app: &AppHandle, input: Input) {
    let effect = voice(app).lock().recorder.handle(input);
    if let Some(effect) = effect {
        run(app, effect);
    }
}

fn run(app: &AppHandle, effect: Effect) {
    match effect {
        Effect::StartRecording { session } => {
            emit_view(app);
            let h = app.clone();
            let spawned = std::thread::Builder::new()
                .name("voice-start".into())
                .spawn(move || open_microphone(&h, session));
            if let Err(e) = spawned {
                feed(app, Input::Failed { session, message: format!("Voice capture could not start ({e}).") });
            }
        }
        Effect::Transcribe { session, cut_off } => {
            let capture = voice(app).lock().capture.take();
            emit_view(app);
            // no microphone yet (still opening): `open_microphone` hands it over itself
            if let Some(capture) = capture {
                spawn_transcription(app, session, capture, cut_off);
            }
        }
        Effect::Discard { .. } => {
            let (capture, _loader) = {
                let v = voice(app);
                let mut inner = v.lock();
                (inner.capture.take(), inner.loader.take())
            };
            // closing the stream joins its thread: not on the caller's (maybe the main) thread;
            // a model still loading finishes on its own thread and is dropped
            if let Some(c) = capture {
                std::thread::spawn(move || c.discard());
            }
            popup::dismiss(app, Popup::Voice);
        }
        Effect::Deliver(text) => {
            // quick capture first, while the indicator still has the focus (it inherits where to
            // return the focus), then the indicator goes
            crate::capture::open_quick_capture(app, Some(text));
            popup::hide(app, Popup::Voice);
        }
        Effect::ShowError(message) => {
            let capture = voice(app).lock().capture.take();
            if let Some(c) = capture {
                std::thread::spawn(move || c.discard());
            }
            set_screen(app, Screen::Error { message });
        }
    }
}

/// On its own thread: opens the microphone for `session`, starts loading the model and the
/// indicator's clock.
fn open_microphone(app: &AppHandle, session: u64) {
    let result = Capture::start(MAX_LENGTH);
    let v = voice(app);
    let capture = match result {
        Ok(c) => c,
        Err(AudioError::Denied) => {
            let current = v.lock().recorder.session() == session;
            if current {
                feed(app, Input::Failed { session, message: String::new() });
                set_screen(app, denied());
            }
            return;
        }
        Err(e) => {
            log::warn!("voice: microphone: {e}");
            feed(app, Input::Failed { session, message: e.to_string() });
            return;
        }
    };
    let phase = {
        let inner = v.lock();
        (inner.recorder.session() == session).then(|| inner.recorder.phase())
    };
    match phase {
        Some(Phase::Recording) => {
            let loader = spawn_loader(app);
            {
                let mut inner = v.lock();
                inner.capture = Some(capture);
                inner.loader = loader.map(|l| (session, l));
            }
            start_clock(app, session);
        }
        // released / stopped while the microphone was opening: transcribe what there is
        Some(Phase::Transcribing) => spawn_transcription(app, session, capture, false),
        // cancelled meanwhile
        _ => capture.discard(),
    }
}

fn spawn_loader(app: &AppHandle) -> Option<JoinHandle<Result<whisper::Model, String>>> {
    let path = model::path(&voice(app).data_dir, &MODEL);
    std::thread::Builder::new()
        .name("voice-model".into())
        .spawn(move || whisper::load(&path))
        .ok()
}

/// Every 80 ms while recording: the level meter + timer, the 2-minute cut-off, a device that
/// went away.
fn start_clock(app: &AppHandle, session: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(80)).await;
            let (level, failure) = {
                let v = voice(&app);
                let inner = v.lock();
                if inner.recorder.session() != session || inner.recorder.phase() != Phase::Recording {
                    return;
                }
                match &inner.capture {
                    Some(c) => (c.level(), c.failure()),
                    None => return,
                }
            };
            if let Some(f) = failure {
                log::warn!("voice: microphone stream: {f}");
                feed(&app, Input::Failed { session, message: "The microphone stopped working.".into() });
                return;
            }
            let elapsed = started.elapsed().min(MAX_LENGTH);
            let _ = app.emit_to(
                Popup::Voice.label(),
                "voice://level",
                Level { level, elapsed_ms: elapsed.as_millis() as u64 },
            );
            feed(&app, Input::Tick { at: Instant::now() });
        }
    });
}

/// On its own thread: the clip → text, then the recorder gets the result.
fn spawn_transcription(app: &AppHandle, session: u64, capture: Capture, cut_off: bool) {
    let h = app.clone();
    let spawned = std::thread::Builder::new().name("voice-transcribe".into()).spawn(move || {
        let app = &h;
        let clip = capture.finish();
        let loader = {
            let v = voice(app);
            let mut inner = v.lock();
            match inner.loader.take() {
                Some((s, l)) if s == session => Some(l),
                _ => None,
            }
        };
        let language = state(app).lock().settings.preferences.voice_language.clone();
        let model = match loader {
            Some(l) => l.join().unwrap_or_else(|_| Err("the model loader stopped".into())),
            None => whisper::load(&model::path(&voice(app).data_dir, &MODEL)),
        };
        let result = model.and_then(|m| {
            let r = whisper::transcribe(&m, &clip, &language);
            if let Ok((_, t)) = &r {
                // durations only: never the audio or the text
                log::info!(
                    "voice: {:.1} s recorded{}, model load {} ms, resample {} ms, transcription {} ms ({})",
                    t.audio.as_secs_f32(),
                    if cut_off { " (cut off at 2 min)" } else { "" },
                    m.load_time.as_millis(),
                    t.resample.as_millis(),
                    t.transcribe.as_millis(),
                    if cfg!(target_os = "macos") { "metal" } else { "cpu" },
                );
            }
            r
        });
        drop(clip);
        match result {
            Ok((text, _)) => feed(app, Input::Transcribed { session, text }),
            Err(message) => {
                log::warn!("voice: {message}");
                feed(app, Input::Failed { session, message: format!("The speech could not be turned into text: {message}.") });
            }
        }
    });
    if let Err(e) = spawned {
        feed(app, Input::Failed { session, message: format!("Voice capture could not start ({e}).") });
    }
}

// ---- the model download -------------------------------------------------------------------

fn start_download(app: &AppHandle) {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let v = voice(app);
        let mut inner = v.lock();
        if inner.download.is_some() {
            drop(inner);
            return show(app, Screen::Downloading { done: 0, total: MODEL.size });
        }
        inner.download = Some((Arc::clone(&cancel), 0));
    }
    show(app, Screen::Downloading { done: 0, total: MODEL.size });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let dir = voice(&app).data_dir.clone();
        let started = Instant::now();
        let mut last_percent = u64::MAX;
        let result = model::download(&MODEL, &dir, &cancel, |done, total| {
            let percent = done * 100 / total.max(1);
            if percent != last_percent {
                last_percent = percent;
                if let Some(d) = voice(&app).lock().download.as_mut() {
                    d.1 = done;
                }
                set_screen(&app, Screen::Downloading { done, total });
            }
        })
        .await;
        voice(&app).lock().download = None;
        match result {
            Ok(_) => {
                log::info!("voice: model downloaded and verified in {} s", started.elapsed().as_secs());
                let visible = app
                    .get_webview_window(Popup::Voice.label())
                    .is_some_and(|w| w.is_visible().unwrap_or(false));
                set_screen(&app, next_step(&app));
                if !visible {
                    let shortcut = shortcut_label(&app);
                    let body = if shortcut.is_empty() {
                        "Choose Voice Capture in the menu bar and speak.".to_string()
                    } else {
                        format!("Hold {shortcut} and speak.")
                    };
                    crate::bridge::show_notification(&app, "Voice capture is ready", &body, None);
                }
            }
            Err(ModelError::Cancelled) => log::info!("voice: model download cancelled"),
            Err(e) => {
                log::warn!("voice: model download failed: {e}");
                set_screen(&app, Screen::DownloadFailed { message: e.to_string() });
            }
        }
    });
}

// ---- the indicator window -----------------------------------------------------------------

fn shortcut_label(app: &AppHandle) -> String {
    crate::shortcuts::label_of(state(app).lock().settings.preferences.voice_shortcut.as_deref())
}

fn view(app: &AppHandle) -> VoiceView {
    let shortcut = shortcut_label(app);
    let screen = {
        let v = voice(app);
        let inner = v.lock();
        match inner.recorder.phase() {
            Phase::Recording => Screen::Recording {
                elapsed_ms: inner.recorder.elapsed(Instant::now()).as_millis() as u64,
                max_ms: MAX_LENGTH.as_millis() as u64,
                hold: inner.recorder.mode() == recorder::Mode::Hold,
            },
            Phase::Transcribing => Screen::Transcribing,
            _ => inner.screen.clone(),
        }
    };
    VoiceView {
        screen,
        shortcut,
        platform: crate::api::platform(),
        model_name: MODEL.name,
        model_mb: MODEL.size.div_ceil(1_000_000),
        can_open_settings: permission::privacy_settings_url().is_some(),
    }
}

fn emit_view(app: &AppHandle) {
    let v = view(app);
    let _ = app.emit_to(Popup::Voice.label(), "voice://view", v);
}

fn set_screen(app: &AppHandle, screen: Screen) {
    voice(app).lock().screen = screen;
    emit_view(app);
}

fn show_window(app: &AppHandle) {
    popup::show(app, Popup::Voice);
}

fn show(app: &AppHandle, screen: Screen) {
    set_screen(app, screen);
    show_window(app);
}

#[tauri::command]
pub fn voice_view(app: AppHandle, webview: Webview) -> Result<VoiceView, String> {
    popup::ensure_caller(&app, &webview, Popup::Voice)?;
    Ok(view(&app))
}

/// Enter / Stop: finish the recording and transcribe it.
#[tauri::command]
pub async fn voice_stop(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Voice)?;
    feed(&app, Input::Stop);
    Ok(())
}

/// The Start button (Ready screen): records until Enter / Stop.
#[tauri::command]
pub async fn voice_start(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Voice)?;
    start_from_menu(&app);
    Ok(())
}

/// Esc / Cancel: drop the recording (or close the window when nothing runs).
#[tauri::command]
pub async fn voice_cancel(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Voice)?;
    if voice(&app).lock().recorder.busy() {
        feed(&app, Input::Cancel);
    } else {
        popup::dismiss(&app, Popup::Voice);
    }
    Ok(())
}

/// "Turn on" on the intro screen: the preference, then the model download and the microphone
/// question.
#[tauri::command]
pub async fn voice_turn_on(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Voice)?;
    {
        let s = state(&app);
        let mut inner = s.lock();
        inner.settings.preferences.voice_enabled = true;
        inner.save_settings();
    }
    log::info!("voice: turned on");
    enabled_changed(&app);
    if model::is_ready(&voice(&app).data_dir, &MODEL) {
        set_screen(&app, next_step(&app));
    } else {
        if permission::status() == MicPermission::NotDetermined {
            // asked now, while the model downloads
            permission::request(|granted| log::info!("voice: microphone permission {}", if granted { "granted" } else { "denied" }));
        }
        start_download(&app);
    }
    Ok(())
}

/// "Try again" after a failed download.
#[tauri::command]
pub async fn voice_download(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Voice)?;
    start_download(&app);
    Ok(())
}

/// "Open Privacy Settings": a fixed OS settings link (never a page-supplied address).
#[tauri::command]
pub fn voice_open_privacy(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Voice)?;
    open_privacy_settings(&app)
}

#[tauri::command]
pub fn voice_close(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Voice)?;
    popup::dismiss(&app, Popup::Voice);
    Ok(())
}

fn open_privacy_settings(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let url = permission::privacy_settings_url().ok_or("There is no privacy settings page on this system.")?;
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// ---- Settings ------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    name: &'static str,
    size_mb: u64,
    ready: bool,
    /// 0–100 while downloading.
    downloading: Option<u64>,
}

pub fn model_status(app: &AppHandle) -> ModelStatus {
    let v = voice(app);
    let downloading = v.lock().download.as_ref().map(|(_, done)| done * 100 / MODEL.size.max(1));
    ModelStatus {
        name: MODEL.name,
        size_mb: MODEL.size.div_ceil(1_000_000),
        ready: model::is_ready(&v.data_dir, &MODEL),
        downloading,
    }
}

/// Settings turned voice capture off: a recording in progress is dropped.
pub fn turned_off(app: &AppHandle) {
    if voice(app).lock().recorder.busy() {
        feed(app, Input::Cancel);
    }
    log::info!("voice: turned off");
}

/// The "voice capture" preference changed: the global voice shortcut is registered only while it
/// is on, and the menu / tray labels follow.
pub fn enabled_changed(app: &AppHandle) {
    if let Err(e) = crate::hotkeys::apply(app) {
        log::warn!("{e}");
    }
    crate::menu::refresh_shortcut_labels(app);
    crate::tray::refresh(app);
}

/// "Remove voice model": stops a download, refuses while recording / transcribing.
pub fn remove_model(app: &AppHandle) -> Result<(), String> {
    let v = voice(app);
    {
        let inner = v.lock();
        if inner.recorder.busy() {
            return Err("Voice capture is in use. Try again when it has finished.".into());
        }
        if let Some((cancel, _)) = &inner.download {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    let freed = model::remove(&v.data_dir).map_err(|e| format!("The voice model could not be removed: {e}"))?;
    log::info!("voice: model removed ({} MB)", freed / 1_000_000);
    v.lock().screen = Screen::Intro;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_include_auto_german_english_albanian_and_are_unique() {
        for code in ["auto", "de", "en", "sq"] {
            assert!(is_language(code), "{code}");
        }
        assert!(!is_language("xx"));
        assert!(!is_language(""));
        assert!(!is_language("../etc"));
        let mut codes: Vec<&str> = LANGUAGES.iter().map(|(c, _)| *c).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), LANGUAGES.len());
        // whisper codes: 2 letters (or "auto")
        assert!(LANGUAGES.iter().all(|(c, _)| *c == "auto" || (c.len() == 2 && c.bytes().all(|b| b.is_ascii_lowercase()))));
    }

    #[test]
    fn the_view_model_is_tagged_by_screen() {
        let v = VoiceView {
            screen: Screen::Recording { elapsed_ms: 1500, max_ms: 120_000, hold: true },
            shortcut: "⇧⌘V".into(),
            platform: "macos",
            model_name: MODEL.name,
            model_mb: 82,
            can_open_settings: true,
        };
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["screen"], "recording");
        assert_eq!(json["elapsed_ms"], 1500);
        assert_eq!(json["hold"], true);
        assert_eq!(json["shortcut"], "⇧⌘V");
        let json = serde_json::to_value(Screen::DownloadFailed { message: "x".into() }).unwrap();
        assert_eq!(json["screen"], "download_failed");
        assert_eq!(serde_json::to_value(Screen::Intro).unwrap()["screen"], "intro");
    }
}
