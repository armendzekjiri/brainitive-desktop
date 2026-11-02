//! Quick capture (BRAI-359): the commands of the capture window and the sending of queued
//! captures. `POST /api/v1/captures` is called from Rust with the stored app token; the window's
//! JavaScript only hands over the text.

use serde::Serialize;
use tauri::{AppHandle, Webview};

use crate::api::ApiRequest;
use crate::flows::UiError;
use crate::popup::{self, Popup};
use crate::queue::{self, CaptureInput, Outcome, QueuedCapture};
use crate::server::ServerOrigin;
use crate::session::CallError;
use crate::state::{state, Phase};

#[derive(Debug, Clone, Serialize)]
pub struct CaptureContext {
    /// "Cloud" / "brainitive.example.com"; `None` before a server is chosen.
    server: Option<String>,
    signed_in: bool,
    /// Captures of this server still waiting to be sent.
    queued: usize,
    /// "⇧⌘Space" etc.; empty when the shortcut is off.
    shortcut: String,
    platform: &'static str,
    /// Text to start with ([`open_quick_capture`]), handed over once.
    prefill: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureResult {
    /// "saved" | "queued"
    status: &'static str,
    message: String,
}

#[tauri::command]
pub async fn capture_context(app: AppHandle, webview: Webview) -> Result<CaptureContext, String> {
    popup::ensure_caller(&app, &webview, Popup::Capture)?;
    let (origin, phase, queued, shortcut, prefill) = {
        let s = state(&app);
        let mut inner = s.lock();
        let prefill = inner.capture_prefill.take();
        let origin = inner.current_origin();
        let queued = origin.as_ref().map(|o| inner.queue.count_for(o)).unwrap_or(0);
        (
            origin,
            inner.phase,
            queued,
            crate::shortcuts::label_of(inner.settings.preferences.capture_shortcut.as_deref()),
            prefill,
        )
    };
    let signed_in = match &origin {
        Some(o) => phase == Phase::InApp || state(&app).session.has_stored_sign_in(o).await,
        None => false,
    };
    Ok(CaptureContext {
        server: origin.map(|o| state(&app).lock().settings.label_of(&o)),
        signed_in,
        queued,
        shortcut,
        platform: crate::api::platform(),
        prefill,
    })
}

/// Opens the quick-capture window, optionally with text already in it (for other native
/// sources, e.g. a voice-capture shortcut that transcribes locally). The text is only handed to
/// the bundled capture page; nothing is sent until the user saves.
pub fn open_quick_capture(app: &AppHandle, prefill: Option<String>) {
    let prefill = prefill
        .map(|t| t.chars().take(queue::MAX_TEXT).collect::<String>())
        .filter(|t| !t.trim().is_empty());
    let has_prefill = prefill.is_some();
    state(app).lock().capture_prefill = prefill;
    popup::show(app, Popup::Capture);
    if has_prefill {
        // already in front (no focus event): ask the page to read its context again
        if let Some(w) = tauri::Manager::get_webview_window(app, Popup::Capture.label()) {
            let _ = w.eval("window.dispatchEvent(new Event('focus'))");
        }
    }
}

/// Saves a capture to the private inbox; offline (or a server problem) → queued on this
/// computer and sent later with the same `request_id`.
#[tauri::command]
pub async fn capture_submit(
    app: AppHandle,
    webview: Webview,
    text: String,
    url: Option<String>,
    intended_type: Option<String>,
) -> Result<CaptureResult, UiError> {
    popup::ensure_caller(&app, &webview, Popup::Capture).map_err(|m| ui("not_allowed", m))?;
    let input =
        CaptureInput::new(&text, url.as_deref(), intended_type.as_deref()).map_err(|e| ui("invalid", e.to_string()))?;
    save(&app, input).await
}

/// Sends a capture to the private inbox, or keeps it in the queue when the server can't be
/// reached (quick capture, and New task's "Save to Inbox instead").
pub async fn save(app: &AppHandle, input: CaptureInput) -> Result<CaptureResult, UiError> {
    let app = app.clone();
    let origin = state(&app)
        .lock()
        .current_origin()
        .ok_or_else(|| ui("no_server", "Connect Brainitive to a server first."))?;
    if !state(&app).session.has_stored_sign_in(&origin).await {
        return Err(ui("signed_out", format!("Sign in to {} first.", origin.display_name())));
    }
    let entry = QueuedCapture::new(&origin, input);
    let result = send(&app, &origin, &entry).await;
    match queue::outcome(&result) {
        Outcome::Sent => {
            // the connection works: send what waited
            let h = app.clone();
            tauri::async_runtime::spawn(async move { flush(&h).await });
            Ok(CaptureResult {
                status: "saved",
                message: "Saved to your inbox.".into(),
            })
        }
        Outcome::Rejected(message) => Err(ui("rejected", message)),
        Outcome::Retry => {
            if let Err(CallError::SignedOut(_)) = result {
                return Err(ui(
                    "signed_out",
                    format!("Sign in to {} again to capture.", origin.display_name()),
                ));
            }
            let s = state(&app);
            let mut inner = s.lock();
            inner.queue.push(entry).map_err(|e| ui("queue_full", e.to_string()))?;
            inner.save_queue();
            Ok(CaptureResult {
                status: "queued",
                message: "You're offline. The capture is kept on this computer and sent when the connection is back."
                    .into(),
            })
        }
    }
}

#[tauri::command]
pub fn capture_close(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Capture)?;
    popup::dismiss(&app, Popup::Capture);
    Ok(())
}

/// The window's microphone button (BRAI-439): voice capture until Enter / Stop; the text comes
/// back into this window (added to what is already typed).
#[tauri::command]
pub fn capture_voice(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Capture)?;
    crate::voice::start_from_menu(&app);
    Ok(())
}

async fn send(app: &AppHandle, origin: &ServerOrigin, entry: &QueuedCapture) -> Result<(), CallError> {
    let request = ApiRequest::post("/api/v1/captures", entry.body());
    state(app).session.request(origin, &request).await.map(|_| ())
}

/// Sends the current server's queued captures, oldest first; stops at the first one that has
/// to wait (offline, server problem, signed out). One flush at a time.
pub async fn flush(app: &AppHandle) {
    let (origin, pending) = {
        let s = state(app);
        let mut inner = s.lock();
        let Some(origin) = inner.current_origin() else { return };
        let pending = inner.queue.pending_for(&origin);
        if pending.is_empty() || inner.flushing {
            return;
        }
        inner.flushing = true;
        (origin, pending)
    };
    let mut sent = 0usize;
    let mut rejected = Vec::new();
    for entry in pending {
        let result = send(app, &origin, &entry).await;
        let outcome = queue::outcome(&result);
        let s = state(app);
        let mut inner = s.lock();
        match outcome {
            Outcome::Sent => {
                inner.queue.remove(&entry.request_id);
                sent += 1;
            }
            Outcome::Rejected(reason) => {
                inner.queue.remove(&entry.request_id);
                rejected.push(reason);
            }
            Outcome::Retry => {
                inner.queue.note_attempt(&entry.request_id);
                break;
            }
        }
    }
    {
        let s = state(app);
        let mut inner = s.lock();
        inner.flushing = false;
        inner.save_queue();
    }
    if sent > 0 {
        let body = if sent == 1 {
            "1 capture from while you were offline is in your inbox now.".to_string()
        } else {
            format!("{sent} captures from while you were offline are in your inbox now.")
        };
        crate::bridge::show_notification(app, "Captures sent", &body, origin.url().join("/captures").ok());
    }
    if let Some(reason) = rejected.first() {
        crate::bridge::show_notification(app, "A capture could not be saved", reason, None);
    }
}

/// Sign out / forget: the server's waiting captures are deleted with its sign-in.
pub fn drop_queue(app: &AppHandle, origin: &ServerOrigin) {
    let s = state(app);
    let mut inner = s.lock();
    if inner.queue.drop_origin(origin) > 0 {
        inner.save_queue();
    }
}

fn ui(kind: &'static str, message: impl Into<String>) -> UiError {
    UiError {
        kind,
        message: message.into(),
    }
}
