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
use crate::state::{state, AccountTicket, Phase};

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
    // the sign-in this capture belongs to: checked again before it is kept for later
    let ticket = AccountTicket::now(&state(&app).lock())
        .ok_or_else(|| ui("no_server", "Connect Brainitive to a server first."))?;
    let origin = ticket.origin.clone();
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
            keep_for_retry(&mut state(&app).lock(), &ticket, entry)?;
            Ok(CaptureResult {
                status: "queued",
                message: "You're offline. The capture is kept on this computer and sent when the connection is back."
                    .into(),
            })
        }
    }
}

/// The queued captures of `origin` that may be sent as the account signed in now; removes the
/// ones of another account (or of an earlier sign-in nobody can attribute). Returns them and how
/// many were removed.
pub(crate) fn sendable(inner: &mut crate::state::Inner, origin: &ServerOrigin) -> (Vec<QueuedCapture>, usize) {
    let current = inner.settings.sign_in_of(origin);
    let confirmed = confirmed_user(inner, origin);
    let mut send = Vec::new();
    let mut drop = Vec::new();
    for entry in inner.queue.pending_for(origin) {
        match queue::owner(&entry, current.as_deref(), confirmed) {
            queue::Owner::Send => send.push(entry),
            queue::Owner::Wait => {}
            queue::Owner::Drop => drop.push(entry.request_id),
        }
    }
    for id in &drop {
        inner.queue.remove(id);
    }
    (send, drop.len())
}

/// The account `GET /me` confirmed for `origin` in this sign-in (the offline data's check).
fn confirmed_user(inner: &crate::state::Inner, origin: &ServerOrigin) -> Option<u64> {
    let data = &inner.offline;
    (data.origin.as_ref() == Some(origin) && data.verified)
        .then(|| data.cache.as_ref().and_then(|c| c.user_id))
        .flatten()
}

/// Keeps a capture that could not be sent for a later try — only while the sign-in it was made
/// in is still current (codex review 5): a send that fails after a sign-out (or a switch) must
/// not write the text back into the queue the sign-out just deleted.
pub(crate) fn keep_for_retry(
    inner: &mut crate::state::Inner,
    ticket: &AccountTicket,
    entry: QueuedCapture,
) -> Result<(), UiError> {
    if !ticket.holds(inner) {
        log::info!("a capture that failed after a sign-out or switch was not kept");
        return Err(ui(
            "signed_out",
            "You signed out or switched servers while this was sent. It was not saved.",
        ));
    }
    // whose it is: the sign-in now, and the account when `GET /me` confirmed it (codex review 2)
    let entry = entry.owned_by(
        inner.settings.sign_in_of(&ticket.origin),
        confirmed_user(inner, &ticket.origin),
    );
    inner.queue.push(entry).map_err(|e| ui("queue_full", e.to_string()))?;
    inner.save_queue();
    Ok(())
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
    let (origin, pending, dropped) = {
        let s = state(app);
        let mut inner = s.lock();
        let Some(origin) = inner.current_origin() else { return };
        if inner.flushing {
            return;
        }
        // only captures of this sign-in, or of an earlier one `GET /me` confirmed as the same
        // account; another account's are removed unsent (codex review 2)
        let (pending, dropped) = sendable(&mut inner, &origin);
        if dropped > 0 {
            inner.save_queue();
        }
        if !pending.is_empty() {
            inner.flushing = true;
        }
        (origin, pending, dropped)
    };
    if dropped > 0 {
        log::info!("{dropped} queued capture(s) of another sign-in removed unsent");
        let body = if dropped == 1 {
            "1 capture made while another account was signed in to this server was not sent.".to_string()
        } else {
            format!("{dropped} captures made while another account was signed in to this server were not sent.")
        };
        crate::bridge::show_notification(app, "Captures not sent", &body, None);
    }
    if pending.is_empty() {
        return;
    }
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

/// Sign out / forget: the server's waiting captures are deleted with its sign-in. `false`: the
/// file could not be written (they are gone from memory; the sign-out cleanup is retried).
pub fn drop_queue(app: &AppHandle, origin: &ServerOrigin) -> bool {
    let s = state(app);
    let mut inner = s.lock();
    inner.queue.drop_origin(origin);
    match inner.queue.save(&inner.queue_path) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("queued captures of a signed-out server not deleted from disk: {e}");
            false
        }
    }
}

fn ui(kind: &'static str, message: impl Into<String>) -> UiError {
    UiError {
        kind,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    #[test]
    fn a_send_that_fails_after_the_sign_out_does_not_recreate_the_queue() {
        let dir = std::env::temp_dir().join(format!("brainitive-capture-epoch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let queue_path = dir.join("capture-queue.json");
        let app_state = AppState::new(dir.join("settings.json"), queue_path.clone(), dir.join("offline"));
        let a = ServerOrigin::parse("a.example.com").unwrap();
        let mut inner = app_state.lock();
        inner.settings.remember(&a, "A", None);
        let ticket = AccountTicket::now(&inner).unwrap();
        let entry = || QueuedCapture::new(&a, CaptureInput::new("private note", None, None).unwrap());

        // the send was on the wire when the user signed out (the epoch moves first)
        inner.account_epoch += 1;
        inner.queue.drop_origin(&a);
        let refused = keep_for_retry(&mut inner, &ticket, entry()).unwrap_err();
        assert_eq!(refused.kind, "signed_out");
        assert_eq!(inner.queue.count_for(&a), 0, "nothing in memory");
        assert!(!queue_path.exists(), "nothing on disk");
        // a later sign-in to the same server does not bring it back either
        inner.account_epoch += 1;
        assert!(keep_for_retry(&mut inner, &ticket, entry()).is_err());
        assert_eq!(inner.queue.count_for(&a), 0);

        // the same sign-in: kept for later as before
        let ticket = AccountTicket::now(&inner).unwrap();
        keep_for_retry(&mut inner, &ticket, entry()).unwrap();
        assert_eq!(inner.queue.count_for(&a), 1);
        assert!(queue_path.exists());
        drop(inner);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn captures_of_another_account_are_removed_unsent() {
        let dir = std::env::temp_dir().join(format!("brainitive-capture-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app_state = AppState::new(dir.join("settings.json"), dir.join("q.json"), dir.join("offline"));
        let a = ServerOrigin::parse("a.example.com").unwrap();
        let mut inner = app_state.lock();
        inner.settings.remember(&a, "A", None);
        inner.settings.set_sign_in(&a, Some("s-alice".into()));
        let note = |text: &str| QueuedCapture::new(&a, CaptureInput::new(text, None, None).unwrap());
        // Alice (user 1, confirmed) queued one; another was queued before any `/me`
        inner.offline.load(&a, Some("s-alice"));
        assert!(!inner.offline.adopt_user(1, None));
        let ticket = AccountTicket::now(&inner).unwrap();
        keep_for_retry(&mut inner, &ticket, note("alice confirmed")).unwrap();
        inner.offline.verified = false;
        keep_for_retry(&mut inner, &ticket, note("alice unconfirmed")).unwrap();
        let tags: Vec<_> = inner
            .queue
            .entries
            .iter()
            .map(|e| (e.sign_in.clone(), e.user_id))
            .collect();
        assert_eq!(
            tags,
            [(Some("s-alice".into()), Some(1)), (Some("s-alice".into()), None)]
        );

        // Alice's session expires; Bob signs in on the same server (no sign-out ran)
        crate::flows::drop_account_state(&mut inner);
        inner.settings.set_sign_in(&a, Some("s-bob".into()));
        // before `/me`: nothing is sent; the one nobody can attribute is removed
        let (send, dropped) = sendable(&mut inner, &a);
        assert!(send.is_empty());
        assert_eq!(dropped, 1);
        // `/me` says Bob (user 2): Alice's capture is removed, never sent as Bob
        inner.load_offline(&a);
        assert!(!inner.offline.adopt_user(2, None));
        let (send, dropped) = sendable(&mut inner, &a);
        assert!(send.is_empty());
        assert_eq!(dropped, 1);
        assert_eq!(inner.queue.count_for(&a), 0);
        drop(inner);
        let _ = std::fs::remove_dir_all(dir);
    }
}
