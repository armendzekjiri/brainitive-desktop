//! Unit-test support: a loopback fake of the server's App API with scripted answers per path,
//! and gates that hold a request until the test lets it through — so a test can pause a refresh,
//! an exchange or a logout at its `.await` and sign out in between (codex review, findings 3–5).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};

use crate::server::ServerOrigin;

#[derive(Debug, Clone)]
#[allow(dead_code)] // not every test reads every field
pub struct Hit {
    pub path: String,
    pub body: String,
    pub bearer: Option<String>,
}

#[derive(Default)]
struct Shared {
    /// Answers per path, in order; the last one repeats.
    replies: Mutex<HashMap<String, Vec<(u16, Value)>>>,
    gates: Mutex<HashMap<String, Arc<Semaphore>>>,
    hits: Mutex<Vec<Hit>>,
}

pub struct FakeServer {
    pub origin: ServerOrigin,
    shared: Arc<Shared>,
    arrivals: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
}

/// A token pair as `/auth/exchange` and `/auth/refresh` answer it.
pub fn tokens(n: u32) -> Value {
    json!({ "access_token": format!("bra_{n}"), "expires_in": 900, "refresh_token": format!("brr_{n}") })
}

impl FakeServer {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = ServerOrigin::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let accept = shared.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, accept.clone(), tx.clone()));
            }
        });
        Self {
            origin,
            shared,
            arrivals: tokio::sync::Mutex::new(rx),
        }
    }

    /// The next answer to `path` (the last one given repeats).
    pub fn reply(&self, path: &str, status: u16, body: Value) {
        self.shared
            .replies
            .lock()
            .unwrap()
            .entry(path.to_string())
            .or_default()
            .push((status, body));
    }

    /// Requests to `path` wait (after they arrived) until [`release`](Self::release).
    pub fn hold(&self, path: &str) {
        self.shared
            .gates
            .lock()
            .unwrap()
            .insert(path.to_string(), Arc::new(Semaphore::new(0)));
    }

    /// Lets one held request to `path` answer.
    pub fn release(&self, path: &str) {
        if let Some(gate) = self.shared.gates.lock().unwrap().get(path) {
            gate.add_permits(1);
        }
    }

    /// Waits until a request to `path` arrived.
    pub async fn arrived(&self, path: &str) {
        let mut rx = self.arrivals.lock().await;
        let wait = async {
            while let Some(p) = rx.recv().await {
                if p == path {
                    return;
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), wait)
            .await
            .expect("the request never arrived");
    }

    pub fn hits(&self, path: &str) -> Vec<Hit> {
        self.shared
            .hits
            .lock()
            .unwrap()
            .iter()
            .filter(|h| h.path == path)
            .cloned()
            .collect()
    }
}

async fn serve(mut socket: tokio::net::TcpStream, shared: Arc<Shared>, arrived: mpsc::UnboundedSender<String>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let Ok(n) = socket.read(&mut chunk).await else { return };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let length = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let Ok(n) = socket.read(&mut chunk).await else { return };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let path = target.split('?').next().unwrap_or("/").to_string();
    let bearer = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("authorization")
            .then(|| v.trim().strip_prefix("Bearer ").map(str::to_string))?
    });
    shared.hits.lock().unwrap().push(Hit {
        path: path.clone(),
        body: String::from_utf8_lossy(&buf[head_end..]).to_string(),
        bearer,
    });
    let _ = arrived.send(path.clone());
    let gate = shared.gates.lock().unwrap().get(&path).cloned();
    if let Some(gate) = gate {
        if let Ok(permit) = gate.acquire().await {
            permit.forget();
        }
    }
    let (status, body) = {
        let mut replies = shared.replies.lock().unwrap();
        match replies.get_mut(&path) {
            Some(list) if list.len() > 1 => list.remove(0),
            Some(list) if !list.is_empty() => list[0].clone(),
            _ => (404, json!({ "code": "not_found", "message": "Not found." })),
        }
    };
    let body = if status == 204 { String::new() } else { body.to_string() };
    let response = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
}
