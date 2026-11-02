//! Quick captures waiting to be sent (BRAI-359): written while offline (or while the server has
//! a problem) to `capture-queue.json` in the app data dir and sent later with the **same
//! `request_id`**, so the server stores each capture once even when a send is repeated.
//!
//! Entries belong to a server origin and are only ever sent there. Signing out of a server or
//! forgetting it deletes its entries. Pure logic; the sending is in `capture.rs`.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::ServerOrigin;
use crate::session::CallError;

pub const MAX_ENTRIES: usize = 500;
pub const MAX_TEXT: usize = 20_000;
pub const MAX_URL: usize = 2048;
/// `source` of every capture from the app (the API's default would be the same).
pub const SOURCE: &str = "Brainitive Desktop";
/// `intended_type` values of `POST /api/v1/captures`.
pub const TYPES: [&str; 5] = ["issue", "task", "page", "bookmark", "snippet"];

/// What the capture window sends, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureInput {
    pub text: String,
    pub url: Option<String>,
    pub intended_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    #[error("Write something first.")]
    Empty,
    #[error("That's too long for a capture (at most 20,000 characters).")]
    TooLong,
    #[error("The link must start with http:// or https://.")]
    BadUrl,
    #[error("Unknown type.")]
    BadType,
}

impl CaptureInput {
    pub fn new(text: &str, url: Option<&str>, intended_type: Option<&str>) -> Result<Self, InputError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(InputError::Empty);
        }
        if text.chars().count() > MAX_TEXT {
            return Err(InputError::TooLong);
        }
        let url = match url.map(str::trim).filter(|u| !u.is_empty()) {
            None => None,
            Some(u) => {
                let parsed = url::Url::parse(u).map_err(|_| InputError::BadUrl)?;
                if u.len() > MAX_URL
                    || !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none_or(str::is_empty)
                {
                    return Err(InputError::BadUrl);
                }
                Some(parsed.to_string())
            }
        };
        let intended_type = match intended_type.map(str::trim).filter(|t| !t.is_empty()) {
            None => None,
            Some(t) if TYPES.contains(&t) => Some(t.to_string()),
            Some(_) => return Err(InputError::BadType),
        };
        Ok(Self {
            text: text.to_string(),
            url,
            intended_type,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedCapture {
    pub request_id: String,
    pub origin: ServerOrigin,
    pub text: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub intended_type: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub queued_at: u64,
    #[serde(default)]
    pub attempts: u32,
}

impl QueuedCapture {
    pub fn new(origin: &ServerOrigin, input: CaptureInput) -> Self {
        Self {
            request_id: new_request_id(),
            origin: origin.clone(),
            text: input.text,
            url: input.url,
            intended_type: input.intended_type,
            queued_at: now(),
            attempts: 0,
        }
    }

    /// The `POST /api/v1/captures` body.
    pub fn body(&self) -> Value {
        let mut body = json!({
            "text": self.text,
            "source": SOURCE,
            "request_id": self.request_id,
        });
        if let Some(url) = &self.url {
            body["url"] = json!(url);
        }
        if let Some(t) = &self.intended_type {
            body["intended_type"] = json!(t);
        }
        body
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureQueue {
    #[serde(default)]
    pub entries: Vec<QueuedCapture>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Too many captures are waiting to be sent. Connect to the internet first.")]
pub struct QueueFull;

/// What to do with a queued capture after one send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Stored on the server (201, or 200 for a repeated `request_id`): remove it.
    Sent,
    /// Offline, a server problem, signed out: keep it and stop for now (keeps the order).
    Retry,
    /// The server refused it for good (4xx): remove it and tell the user why.
    Rejected(String),
}

pub fn outcome<T>(result: &Result<T, CallError>) -> Outcome {
    match result {
        Ok(_) => Outcome::Sent,
        Err(e) if e.is_transient() || matches!(e, CallError::SignedOut(_)) => Outcome::Retry,
        Err(e) => Outcome::Rejected(e.to_string()),
    }
}

impl CaptureQueue {
    /// A missing or unreadable file is an empty queue (entries of an http origin someone typed
    /// into the file are dropped by `ServerOrigin`'s own checks).
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Atomic: a sibling temp file, then rename. An empty queue deletes the file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if self.entries.is_empty() {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        // 0600 file (and temp file) in a 0700 folder on Unix (BRAI-377)
        crate::private_fs::write_atomic(path, &serde_json::to_vec(self).expect("serializable"))
    }

    pub fn push(&mut self, entry: QueuedCapture) -> Result<(), QueueFull> {
        if self.entries.len() >= MAX_ENTRIES {
            return Err(QueueFull);
        }
        if !self.entries.iter().any(|e| e.request_id == entry.request_id) {
            self.entries.push(entry);
        }
        Ok(())
    }

    /// This server's entries, oldest first.
    pub fn pending_for(&self, origin: &ServerOrigin) -> Vec<QueuedCapture> {
        let mut out: Vec<_> = self.entries.iter().filter(|e| &e.origin == origin).cloned().collect();
        out.sort_by_key(|e| e.queued_at);
        out
    }

    pub fn count_for(&self, origin: &ServerOrigin) -> usize {
        self.entries.iter().filter(|e| &e.origin == origin).count()
    }

    pub fn remove(&mut self, request_id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.request_id != request_id);
        before != self.entries.len()
    }

    pub fn note_attempt(&mut self, request_id: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.request_id == request_id) {
            e.attempts = e.attempts.saturating_add(1);
        }
    }

    /// Sign out / forget: this server's entries go.
    pub fn drop_origin(&mut self, origin: &ServerOrigin) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| &e.origin != origin);
        before - self.entries.len()
    }
}

/// A random (v4) UUID for `request_id`.
pub fn new_request_id() -> String {
    let mut b = [0u8; 16];
    rand::fill(&mut b[..]);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = crate::server::hex(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(s: &str) -> ServerOrigin {
        ServerOrigin::parse(s).unwrap()
    }

    fn entry(origin: &ServerOrigin, text: &str, at: u64) -> QueuedCapture {
        let mut e = QueuedCapture::new(origin, CaptureInput::new(text, None, None).unwrap());
        e.queued_at = at;
        e
    }

    #[test]
    fn input_is_checked() {
        let ok = CaptureInput::new("  Idea  ", Some(" https://example.com/a "), Some("issue")).unwrap();
        assert_eq!(ok.text, "Idea");
        assert_eq!(ok.url.as_deref(), Some("https://example.com/a"));
        assert_eq!(ok.intended_type.as_deref(), Some("issue"));
        assert_eq!(CaptureInput::new("x", Some(""), Some("")).unwrap().url, None);
        assert_eq!(CaptureInput::new("   ", None, None), Err(InputError::Empty));
        assert_eq!(
            CaptureInput::new(&"a".repeat(MAX_TEXT + 1), None, None),
            Err(InputError::TooLong)
        );
        for bad in ["javascript:alert(1)", "file:///etc", "example.com", "https://"] {
            assert_eq!(
                CaptureInput::new("x", Some(bad), None),
                Err(InputError::BadUrl),
                "{bad}"
            );
        }
        assert_eq!(
            CaptureInput::new("x", None, Some("whiteboard")),
            Err(InputError::BadType)
        );
    }

    #[test]
    fn request_ids_are_v4_uuids() {
        let a = new_request_id();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(a, new_request_id());
    }

    #[test]
    fn body_carries_the_request_id_and_source() {
        let e = QueuedCapture::new(
            &o("a.example.com"),
            CaptureInput::new("Idea", Some("https://x.com/"), Some("task")).unwrap(),
        );
        let b = e.body();
        assert_eq!(b["text"], "Idea");
        assert_eq!(b["url"], "https://x.com/");
        assert_eq!(b["intended_type"], "task");
        assert_eq!(b["source"], SOURCE);
        assert_eq!(b["request_id"], e.request_id.as_str());
        let plain = QueuedCapture::new(&o("a.example.com"), CaptureInput::new("x", None, None).unwrap()).body();
        assert!(plain.get("url").is_none() && plain.get("intended_type").is_none());
    }

    #[test]
    fn entries_stay_with_their_server_in_order() {
        let (a, b) = (o("a.example.com"), o("b.example.com"));
        let mut q = CaptureQueue::default();
        q.push(entry(&a, "second", 20)).unwrap();
        q.push(entry(&b, "other server", 10)).unwrap();
        q.push(entry(&a, "first", 10)).unwrap();
        let pending: Vec<_> = q.pending_for(&a).into_iter().map(|e| e.text).collect();
        assert_eq!(pending, ["first", "second"]);
        assert_eq!(q.count_for(&b), 1);

        let id = q.pending_for(&a)[0].request_id.clone();
        q.note_attempt(&id);
        assert_eq!(q.pending_for(&a)[0].attempts, 1);
        assert!(q.remove(&id));
        assert!(!q.remove(&id));
        assert_eq!(q.drop_origin(&a), 1);
        assert_eq!(q.count_for(&a), 0);
        assert_eq!(q.count_for(&b), 1);
    }

    #[test]
    fn the_queue_is_bounded_and_ignores_a_repeated_id() {
        let a = o("a.example.com");
        let mut q = CaptureQueue::default();
        let e = entry(&a, "x", 1);
        q.push(e.clone()).unwrap();
        q.push(e).unwrap();
        assert_eq!(q.entries.len(), 1);
        for i in 1..MAX_ENTRIES {
            q.push(entry(&a, "x", i as u64)).unwrap();
        }
        assert_eq!(q.push(entry(&a, "x", 0)), Err(QueueFull));
    }

    #[test]
    fn send_outcomes() {
        assert_eq!(outcome::<()>(&Ok(())), Outcome::Sent);
        assert_eq!(outcome::<()>(&Err(CallError::Offline("down".into()))), Outcome::Retry);
        assert_eq!(outcome::<()>(&Err(CallError::SignedOut("x".into()))), Outcome::Retry);
        let server = |status| CallError::Server {
            status,
            code: "x".into(),
            message: "Nope".into(),
        };
        assert_eq!(outcome::<()>(&Err(server(500))), Outcome::Retry);
        assert_eq!(outcome::<()>(&Err(server(429))), Outcome::Retry);
        assert_eq!(outcome::<()>(&Err(server(422))), Outcome::Rejected("Nope".into()));
        assert_eq!(outcome::<()>(&Err(server(403))), Outcome::Rejected("Nope".into()));
    }

    #[test]
    fn persists_atomically_and_removes_the_file_when_empty() {
        let dir = std::env::temp_dir().join(format!("brainitive-queue-{}", std::process::id()));
        let path = dir.join("capture-queue.json");
        let mut q = CaptureQueue::default();
        q.push(entry(&o("a.example.com"), "x", 1)).unwrap();
        q.save(&path).unwrap();
        assert_eq!(CaptureQueue::load(&path), q);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600, "the queue holds the user's text: 0600");
            assert_eq!(mode(&dir), 0o700);
        }
        q.entries.clear();
        q.save(&path).unwrap();
        assert!(!path.exists());
        q.save(&path).unwrap(); // nothing to delete: fine
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, b"garbage").unwrap();
        assert_eq!(CaptureQueue::load(&path), CaptureQueue::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
