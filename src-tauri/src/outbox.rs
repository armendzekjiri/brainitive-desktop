//! Changes made in the offline window, waiting to be sent (BRAI-366): one file per server
//! (`offline/<store id>/outbox.json` in the app data dir), sent **in order** with
//! `POST /api/v1/sync` in batches of ≤ 50. Every change has its own `request_id`, so a batch that
//! is sent again (lost answer) is never applied twice.
//!
//! Each result is handled honestly (`verdict`): applied → done; conflict → done, the server kept
//! its value and the user's is shown as a problem; deleted / forbidden / invalid → dropped as a
//! problem that still shows the user's text; waiting → kept (dropped after a few tries);
//! rate_limited / error → kept, try later. A 4xx is never retried forever. Pure logic; the
//! sending is in `offline.rs`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::session::CallError;

pub const MAX_ENTRIES: usize = 1000;
pub const MAX_PROBLEMS: usize = 50;
pub const BATCH: usize = 50;
/// A `waiting` change (it needs an offline create that never arrives) is dropped after this many tries.
pub const MAX_WAITING: u32 = 5;
pub const MAX_TITLE: usize = 500;

/// One offline change. Ids are checked before they get here (`issue:<n>` / `task:<n>` work ids,
/// numeric list / task ids).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Change {
    /// The circle of My Work / My Day (`work.done`).
    WorkDone {
        work_id: String,
        done: bool,
        base_done: bool,
        #[serde(default)]
        title: String,
    },
    /// Plan it for today (`my_day.add`).
    MyDayAdd {
        work_id: String,
        #[serde(default)]
        title: String,
    },
    /// A new task in a kept list (`list_task.create`).
    TaskCreate { task_list_id: u64, title: String, done: bool },
    /// Rename / tick a task of a kept list (`list_task.update` with `base.fields`).
    TaskUpdate {
        task_list_id: u64,
        task_id: u64,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        done: Option<bool>,
        base_title: String,
        base_done: bool,
    },
    /// A capture for the private inbox (`capture.create`).
    CaptureCreate {
        text: String,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        intended_type: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub request_id: String,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: u64,
    /// Times it was in a batch that went out. A change that went out once is never edited any
    /// more (the server may already have it under this `request_id`).
    #[serde(default)]
    pub attempts: u32,
    pub change: Change,
}

/// A change the server did not take as it was: shown in the offline window until dismissed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub request_id: String,
    /// conflict | deleted | forbidden | invalid | rejected
    pub kind: String,
    /// What the change was ("Done: BRAI-12 Fix login").
    pub title: String,
    pub message: String,
    /// The user's own text / value, so it can be copied.
    #[serde(default)]
    pub mine: Option<String>,
    /// The server's value (conflicts).
    #[serde(default)]
    pub theirs: Option<String>,
    #[serde(default)]
    pub at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outbox {
    /// The account the changes were made as (`GET /me`), when known.
    #[serde(default)]
    pub user_id: Option<u64>,
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    #[error("Too many changes are waiting to be sent. Connect to the internet first.")]
    Full,
    #[error("This task is being sent right now. Try again in a moment.")]
    InFlight,
    #[error("That task is not waiting to be created any more.")]
    UnknownRef,
}

/// What to do with one change after the server's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Applied,
    /// Done, but (some) fields kept the server's value.
    Conflict,
    /// Dropped: `kind` for the problem.
    Drop(&'static str),
    /// Kept. `stop`: don't send more now (rate limit, server error).
    Keep { stop: bool },
}

/// One entry of the `POST /api/v1/sync` answer.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SyncResult {
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub conflicts: Option<Value>,
    #[serde(default)]
    pub errors: Option<Value>,
    #[serde(default)]
    pub entity: Option<Value>,
}

pub fn verdict(r: &SyncResult) -> Verdict {
    match r.status.as_str() {
        "applied" => Verdict::Applied,
        "conflict" => Verdict::Conflict,
        "deleted" => Verdict::Drop("deleted"),
        "forbidden" => Verdict::Drop("forbidden"),
        "invalid" => Verdict::Drop("invalid"),
        "waiting" => Verdict::Keep { stop: false },
        "rate_limited" | "error" => Verdict::Keep { stop: true },
        // a status this app doesn't know yet: go by the HTTP status it stands for
        _ => match r.http_status.unwrap_or(500) {
            200..=299 => Verdict::Applied,
            409 => Verdict::Conflict,
            408 | 424 | 429 => Verdict::Keep { stop: true },
            400..=499 => Verdict::Drop("rejected"),
            _ => Verdict::Keep { stop: true },
        },
    }
}

/// What one send did, for the notification and the cache.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    /// Applied (incl. conflicts): the entry and the server's result.
    pub done: Vec<(Entry, SyncResult)>,
    /// New problems.
    pub problems: usize,
    /// Don't send more now.
    pub stop: bool,
    /// The batch was refused as a whole (4xx): send its changes one by one.
    pub split: bool,
}

/// `issue:12` → `("board", 12)`, `task:7` → `("list", 7)` (the sync API's `work_type`).
pub fn work_ref(work_id: &str) -> Option<(&'static str, u64)> {
    let (kind, n) = work_id.split_once(':')?;
    let n: u64 = if n.len() <= 12 && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) {
        n.parse().ok()?
    } else {
        return None;
    };
    match kind {
        "issue" => Some(("board", n)),
        "task" => Some(("list", n)),
        _ => None,
    }
}

fn status_of(done: bool) -> &'static str {
    if done {
        "done"
    } else {
        "todo"
    }
}

fn quote(text: &str) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    if clean.chars().count() > 60 {
        format!("“{}…”", clean.chars().take(59).collect::<String>().trim_end())
    } else {
        format!("“{clean}”")
    }
}

impl Change {
    /// A short description for the problem list and logs-free notifications.
    pub fn describe(&self) -> String {
        match self {
            Self::WorkDone { done: true, title, .. } => format!("Mark done: {}", quote(title)),
            Self::WorkDone { title, .. } => format!("Mark not done: {}", quote(title)),
            Self::MyDayAdd { title, .. } => format!("Add to My Day: {}", quote(title)),
            Self::TaskCreate { title, .. } => format!("New task {}", quote(title)),
            Self::TaskUpdate {
                title: Some(t), ..
            } => format!("Rename task to {}", quote(t)),
            Self::TaskUpdate {
                done: Some(true),
                base_title,
                ..
            } => format!("Mark done: {}", quote(base_title)),
            Self::TaskUpdate { base_title, .. } => format!("Change task {}", quote(base_title)),
            Self::CaptureCreate { text, .. } => format!("Capture {}", quote(text)),
        }
    }

    /// The text the user typed, kept when the server refuses the change.
    pub fn mine_text(&self) -> Option<String> {
        match self {
            Self::TaskCreate { title, .. } => Some(title.clone()),
            Self::TaskUpdate { title: Some(t), .. } => Some(t.clone()),
            Self::CaptureCreate { text, url, .. } => Some(match url {
                Some(u) => format!("{text}\n{u}"),
                None => text.clone(),
            }),
            _ => None,
        }
    }
}

impl Entry {
    pub fn new(change: Change) -> Self {
        Self {
            request_id: crate::queue::new_request_id(),
            created_at: now(),
            attempts: 0,
            change,
        }
    }

    /// One change of the `POST /api/v1/sync` body.
    pub fn to_json(&self) -> Value {
        let (kind, target, base, data) = match &self.change {
            Change::WorkDone {
                work_id,
                done,
                base_done,
                ..
            } => {
                let (work_type, id) = work_ref(work_id).unwrap_or(("board", 0));
                (
                    "work.done",
                    None,
                    Some(json!({ "fields": { "done": base_done } })),
                    json!({ "work_type": work_type, "work_id": id, "done": done }),
                )
            }
            Change::MyDayAdd { work_id, .. } => {
                let (work_type, id) = work_ref(work_id).unwrap_or(("board", 0));
                ("my_day.add", None, None, json!({ "work_type": work_type, "work_id": id }))
            }
            Change::TaskCreate {
                task_list_id,
                title,
                done,
            } => (
                "list_task.create",
                None,
                None,
                json!({ "task_list_id": task_list_id, "title": title, "status": status_of(*done) }),
            ),
            Change::TaskUpdate {
                task_id,
                title,
                done,
                base_title,
                base_done,
                ..
            } => {
                let mut data = Map::new();
                let mut fields = Map::new();
                if let Some(t) = title {
                    data.insert("title".into(), json!(t));
                    fields.insert("title".into(), json!(base_title));
                }
                if let Some(d) = done {
                    data.insert("status".into(), json!(status_of(*d)));
                    fields.insert("status".into(), json!(status_of(*base_done)));
                }
                (
                    "list_task.update",
                    Some(json!({ "id": task_id })),
                    Some(json!({ "fields": fields })),
                    Value::Object(data),
                )
            }
            Change::CaptureCreate {
                text,
                url,
                intended_type,
            } => {
                let mut data = json!({ "text": text, "source": crate::queue::SOURCE });
                if let Some(u) = url {
                    data["url"] = json!(u);
                }
                if let Some(t) = intended_type {
                    data["intended_type"] = json!(t);
                }
                ("capture.create", None, None, data)
            }
        };
        let mut out = json!({ "request_id": self.request_id, "type": kind, "data": data });
        if let Some(t) = target {
            out["target"] = t;
        }
        if let Some(b) = base {
            out["base"] = b;
        }
        out
    }
}

/// `{field: {base, mine, theirs}}` → the user's value and the server's, as text.
fn conflict_values(conflicts: Option<&Value>) -> (Option<String>, Option<String>) {
    let Some(map) = conflicts.and_then(Value::as_object) else {
        return (None, None);
    };
    let field = map.get("title").or_else(|| map.values().next());
    let text = |v: Option<&Value>| match v {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Bool(true)) => Some("done".to_string()),
        Some(Value::Bool(false)) => Some("not done".to_string()),
        Some(Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    };
    match field {
        Some(f) => (text(f.get("mine")), text(f.get("theirs"))),
        None => (None, None),
    }
}

fn problem_message(kind: &str, result: Option<&SyncResult>) -> String {
    let server = result.and_then(|r| r.message.clone()).filter(|m| !m.trim().is_empty());
    let errors = result
        .and_then(|r| r.errors.as_ref())
        .and_then(Value::as_object)
        .and_then(|m| m.values().next())
        .and_then(|v| v.as_array().and_then(|a| a.first()).or(Some(v)))
        .and_then(Value::as_str)
        .map(str::to_string);
    let text = match kind {
        "conflict" => "Changed on the server meanwhile. The server's version was kept.".to_string(),
        "deleted" => server.unwrap_or_else(|| "It was deleted or you can't open it any more.".into()),
        "forbidden" => "You are not allowed to make this change.".to_string(),
        "invalid" => errors.or(server).unwrap_or_else(|| "The server did not accept this change.".into()),
        "waiting" => "It depended on another change that never arrived.".to_string(),
        _ => server.unwrap_or_else(|| "The server did not accept this change.".into()),
    };
    text.chars().take(300).collect()
}

impl Outbox {
    /// A missing or unreadable file is an empty outbox.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Atomic (temp file + rename); nothing left → the file goes.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if self.entries.is_empty() && self.problems.is_empty() {
            return remove_file(path);
        }
        write_atomic(path, &serde_json::to_vec(self).expect("serializable"))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds a change, folding it into an unsent one for the same thing where possible.
    /// Returns the request id the change lives under (`None` when it cancelled an earlier one).
    pub fn push(&mut self, change: Change) -> Result<Option<String>, OutboxError> {
        match &change {
            Change::WorkDone { work_id, done, .. } => {
                if let Some(i) = self.unsent(|c| matches!(c, Change::WorkDone { work_id: w, .. } if w == work_id)) {
                    let Change::WorkDone { base_done, done: d, .. } = &mut self.entries[i].change else {
                        unreachable!()
                    };
                    if *base_done == *done {
                        // ticked and unticked while offline: nothing to send
                        return Ok(self.remove_at(i));
                    }
                    *d = *done;
                    return Ok(Some(self.entries[i].request_id.clone()));
                }
            }
            Change::MyDayAdd { work_id, .. } => {
                if let Some(i) = self.unsent(|c| matches!(c, Change::MyDayAdd { work_id: w, .. } if w == work_id)) {
                    return Ok(Some(self.entries[i].request_id.clone()));
                }
            }
            Change::TaskUpdate {
                task_id, title, done, ..
            } => {
                if let Some(i) = self.unsent(|c| matches!(c, Change::TaskUpdate { task_id: t, .. } if t == task_id)) {
                    let Change::TaskUpdate {
                        title: t,
                        done: d,
                        base_title,
                        base_done,
                        ..
                    } = &mut self.entries[i].change
                    else {
                        unreachable!()
                    };
                    if title.is_some() {
                        *t = title.clone();
                    }
                    if done.is_some() {
                        *d = *done;
                    }
                    // back to what the server has: drop that field
                    if t.as_deref() == Some(base_title.as_str()) {
                        *t = None;
                    }
                    if *d == Some(*base_done) {
                        *d = None;
                    }
                    if t.is_none() && d.is_none() {
                        return Ok(self.remove_at(i));
                    }
                    return Ok(Some(self.entries[i].request_id.clone()));
                }
            }
            Change::TaskCreate { .. } | Change::CaptureCreate { .. } => {}
        }
        if self.entries.len() >= MAX_ENTRIES {
            return Err(OutboxError::Full);
        }
        let entry = Entry::new(change);
        let id = entry.request_id.clone();
        self.entries.push(entry);
        Ok(Some(id))
    }

    /// Renames / ticks a task that was created offline and not sent yet (its create changes).
    pub fn edit_pending_create(&mut self, request_id: &str, title: Option<String>, done: Option<bool>) -> Result<(), OutboxError> {
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.request_id == request_id && matches!(e.change, Change::TaskCreate { .. }))
            .ok_or(OutboxError::UnknownRef)?;
        if entry.attempts > 0 {
            return Err(OutboxError::InFlight);
        }
        if let Change::TaskCreate { title: t, done: d, .. } = &mut entry.change {
            if let Some(title) = title {
                *t = title;
            }
            if let Some(done) = done {
                *d = done;
            }
        }
        Ok(())
    }

    fn unsent(&self, pred: impl Fn(&Change) -> bool) -> Option<usize> {
        self.entries.iter().position(|e| e.attempts == 0 && pred(&e.change))
    }

    fn remove_at(&mut self, i: usize) -> Option<String> {
        self.entries.remove(i);
        None
    }

    /// The next batch, oldest first.
    pub fn next_batch(&self, size: usize) -> Vec<Entry> {
        self.entries.iter().take(size.clamp(1, BATCH)).cloned().collect()
    }

    fn add_problem(&mut self, entry: &Entry, kind: &str, result: Option<&SyncResult>) {
        let (mut mine, theirs) = match kind {
            "conflict" => conflict_values(result.and_then(|r| r.conflicts.as_ref())),
            _ => (None, None),
        };
        if mine.is_none() {
            mine = entry.change.mine_text();
        }
        self.problems.push(Problem {
            request_id: entry.request_id.clone(),
            kind: kind.to_string(),
            title: entry.change.describe(),
            message: problem_message(kind, result),
            mine,
            theirs,
            at: now(),
        });
        if self.problems.len() > MAX_PROBLEMS {
            let extra = self.problems.len() - MAX_PROBLEMS;
            self.problems.drain(..extra);
        }
    }

    /// Applies the answer of a batch: one result per sent change (matched by `request_id`).
    pub fn apply_results(&mut self, sent: &[String], results: &[SyncResult]) -> Step {
        let mut step = Step::default();
        for id in sent {
            let Some(i) = self.entries.iter().position(|e| &e.request_id == id) else {
                continue;
            };
            self.entries[i].attempts = self.entries[i].attempts.saturating_add(1);
            let Some(result) = results.iter().find(|r| r.request_id.as_deref() == Some(id.as_str())) else {
                // no answer for it: keep it for the next sync
                continue;
            };
            match verdict(result) {
                Verdict::Applied => {
                    let entry = self.entries.remove(i);
                    step.done.push((entry, result.clone()));
                }
                Verdict::Conflict => {
                    let entry = self.entries.remove(i);
                    self.add_problem(&entry, "conflict", Some(result));
                    step.problems += 1;
                    step.done.push((entry, result.clone()));
                }
                Verdict::Drop(kind) => {
                    let entry = self.entries.remove(i);
                    self.add_problem(&entry, kind, Some(result));
                    step.problems += 1;
                }
                Verdict::Keep { stop } => {
                    if result.status == "waiting" && self.entries[i].attempts >= MAX_WAITING {
                        let entry = self.entries.remove(i);
                        self.add_problem(&entry, "waiting", Some(result));
                        step.problems += 1;
                    }
                    step.stop |= stop;
                }
            }
        }
        step
    }

    /// The whole batch failed. Offline / server trouble / signed out: keep everything, stop.
    /// Refused (4xx): split into single changes; a single change refused is dropped.
    pub fn apply_batch_error(&mut self, sent: &[String], error: &CallError) -> Step {
        let mut step = Step::default();
        for id in sent {
            if let Some(e) = self.entries.iter_mut().find(|e| &e.request_id == id) {
                e.attempts = e.attempts.saturating_add(1);
            }
        }
        if error.is_transient() || matches!(error, CallError::SignedOut(_)) {
            step.stop = true;
            return step;
        }
        if sent.len() > 1 {
            step.split = true;
            return step;
        }
        if let Some(i) = sent.first().and_then(|id| self.entries.iter().position(|e| &e.request_id == id)) {
            let entry = self.entries.remove(i);
            let result = SyncResult {
                message: Some(error.to_string()),
                ..SyncResult::default()
            };
            self.add_problem(&entry, "rejected", Some(&result));
            step.problems += 1;
        }
        step
    }

    pub fn dismiss(&mut self, request_id: Option<&str>) {
        match request_id {
            Some(id) => self.problems.retain(|p| p.request_id != id),
            None => self.problems.clear(),
        }
    }
}

/// Parses the `POST /api/v1/sync` answer (`{data: [result]}`, unwrapped or not).
pub fn parse_results(body: &Value) -> Vec<SyncResult> {
    let list = body.as_array().or_else(|| body.get("data").and_then(Value::as_array));
    list.map(|items| {
        items
            .iter()
            .filter_map(|r| serde_json::from_value(r.clone()).ok())
            .collect()
    })
    .unwrap_or_default()
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn remove_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A sibling temp file, then rename (a crash never leaves half a file). Mode 0600 on Unix.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp: PathBuf = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(work: &str, done: bool, base: bool) -> Change {
        Change::WorkDone {
            work_id: work.into(),
            done,
            base_done: base,
            title: "Fix login".into(),
        }
    }

    fn result(id: &str, status: &str, http: u16) -> SyncResult {
        SyncResult {
            request_id: Some(id.into()),
            status: status.into(),
            http_status: Some(http),
            ..SyncResult::default()
        }
    }

    fn ids(o: &Outbox) -> Vec<String> {
        o.entries.iter().map(|e| e.request_id.clone()).collect()
    }

    #[test]
    fn work_refs() {
        assert_eq!(work_ref("issue:12"), Some(("board", 12)));
        assert_eq!(work_ref("task:7"), Some(("list", 7)));
        for bad in ["issue:", "page:1", "issue:1/2", "task:1234567890123", "x"] {
            assert_eq!(work_ref(bad), None, "{bad}");
        }
    }

    #[test]
    fn changes_become_the_sync_apis_mutations() {
        let e = Entry::new(done("issue:12", true, false));
        let j = e.to_json();
        assert_eq!(j["request_id"], e.request_id.as_str());
        assert_eq!(j["type"], "work.done");
        assert_eq!(j["data"], json!({"work_type": "board", "work_id": 12, "done": true}));
        assert_eq!(j["base"], json!({"fields": {"done": false}}));

        let j = Entry::new(Change::MyDayAdd {
            work_id: "task:3".into(),
            title: "x".into(),
        })
        .to_json();
        assert_eq!(j["type"], "my_day.add");
        assert_eq!(j["data"], json!({"work_type": "list", "work_id": 3}));
        assert!(j.get("base").is_none());

        let j = Entry::new(Change::TaskCreate {
            task_list_id: 4,
            title: "Milk".into(),
            done: false,
        })
        .to_json();
        assert_eq!(j["type"], "list_task.create");
        assert_eq!(j["data"], json!({"task_list_id": 4, "title": "Milk", "status": "todo"}));

        let j = Entry::new(Change::TaskUpdate {
            task_list_id: 4,
            task_id: 9,
            title: Some("New".into()),
            done: Some(true),
            base_title: "Old".into(),
            base_done: false,
        })
        .to_json();
        assert_eq!(j["type"], "list_task.update");
        assert_eq!(j["target"], json!({"id": 9}));
        assert_eq!(j["data"], json!({"title": "New", "status": "done"}));
        assert_eq!(j["base"], json!({"fields": {"title": "Old", "status": "todo"}}));

        let j = Entry::new(Change::CaptureCreate {
            text: "Idea".into(),
            url: Some("https://x.com/".into()),
            intended_type: None,
        })
        .to_json();
        assert_eq!(j["type"], "capture.create");
        assert_eq!(j["data"]["source"], crate::queue::SOURCE);
        assert_eq!(j["data"]["url"], "https://x.com/");
    }

    #[test]
    fn unsent_changes_fold_together() {
        let mut o = Outbox::default();
        let a = o.push(done("issue:1", true, false)).unwrap();
        assert!(a.is_some());
        // ticked back: nothing left to send
        assert_eq!(o.push(done("issue:1", false, true)).unwrap(), None);
        assert!(o.is_empty());

        // a change that already went out is never edited: a new one follows it
        o.push(done("issue:1", true, false)).unwrap();
        o.entries[0].attempts = 1;
        o.push(done("issue:1", false, true)).unwrap();
        assert_eq!(o.len(), 2);

        // task edits merge and drop fields that are back to the server's value
        let mut o = Outbox::default();
        let update = |title: Option<&str>, d: Option<bool>| Change::TaskUpdate {
            task_list_id: 1,
            task_id: 5,
            title: title.map(str::to_string),
            done: d,
            base_title: "Old".into(),
            base_done: false,
        };
        o.push(update(Some("New"), None)).unwrap();
        o.push(update(None, Some(true))).unwrap();
        assert_eq!(o.len(), 1);
        let Change::TaskUpdate { title, done, .. } = &o.entries[0].change else { panic!() };
        assert_eq!((title.as_deref(), *done), (Some("New"), Some(true)));
        o.push(update(Some("Old"), Some(false))).unwrap();
        assert!(o.is_empty());

        // My Day add twice = once
        o.push(Change::MyDayAdd {
            work_id: "issue:2".into(),
            title: String::new(),
        })
        .unwrap();
        o.push(Change::MyDayAdd {
            work_id: "issue:2".into(),
            title: String::new(),
        })
        .unwrap();
        assert_eq!(o.len(), 1);
    }

    #[test]
    fn an_unsent_new_task_is_edited_in_place() {
        let mut o = Outbox::default();
        let id = o
            .push(Change::TaskCreate {
                task_list_id: 1,
                title: "Mlik".into(),
                done: false,
            })
            .unwrap()
            .unwrap();
        o.edit_pending_create(&id, Some("Milk".into()), Some(true)).unwrap();
        assert_eq!(
            o.entries[0].change,
            Change::TaskCreate {
                task_list_id: 1,
                title: "Milk".into(),
                done: true
            }
        );
        o.entries[0].attempts = 1;
        assert_eq!(o.edit_pending_create(&id, None, Some(false)), Err(OutboxError::InFlight));
        assert_eq!(o.edit_pending_create("nope", None, None), Err(OutboxError::UnknownRef));
    }

    #[test]
    fn batches_keep_the_order_and_the_size() {
        let mut o = Outbox::default();
        for i in 0..60 {
            o.push(Change::CaptureCreate {
                text: format!("{i}"),
                url: None,
                intended_type: None,
            })
            .unwrap();
        }
        let batch = o.next_batch(BATCH);
        assert_eq!(batch.len(), 50);
        assert_eq!(batch[0].request_id, o.entries[0].request_id);
        assert_eq!(o.next_batch(500).len(), 50);
        assert_eq!(o.next_batch(0).len(), 1);
    }

    #[test]
    fn the_outbox_is_bounded() {
        let mut o = Outbox::default();
        for _ in 0..MAX_ENTRIES {
            o.push(Change::CaptureCreate {
                text: "x".into(),
                url: None,
                intended_type: None,
            })
            .unwrap();
        }
        assert_eq!(
            o.push(Change::CaptureCreate {
                text: "x".into(),
                url: None,
                intended_type: None
            }),
            Err(OutboxError::Full)
        );
    }

    #[test]
    fn verdicts_per_status() {
        assert_eq!(verdict(&result("a", "applied", 200)), Verdict::Applied);
        assert_eq!(verdict(&result("a", "conflict", 409)), Verdict::Conflict);
        assert_eq!(verdict(&result("a", "deleted", 410)), Verdict::Drop("deleted"));
        assert_eq!(verdict(&result("a", "forbidden", 403)), Verdict::Drop("forbidden"));
        assert_eq!(verdict(&result("a", "invalid", 422)), Verdict::Drop("invalid"));
        assert_eq!(verdict(&result("a", "waiting", 424)), Verdict::Keep { stop: false });
        assert_eq!(verdict(&result("a", "rate_limited", 429)), Verdict::Keep { stop: true });
        assert_eq!(verdict(&result("a", "error", 500)), Verdict::Keep { stop: true });
        // unknown statuses: by the HTTP status, a 4xx is never kept forever
        assert_eq!(verdict(&result("a", "gone_fishing", 418)), Verdict::Drop("rejected"));
        assert_eq!(verdict(&result("a", "later", 503)), Verdict::Keep { stop: true });
        assert_eq!(verdict(&result("a", "fine", 201)), Verdict::Applied);
    }

    #[test]
    fn results_are_handled_per_change() {
        let mut o = Outbox::default();
        let push = |o: &mut Outbox, c| o.push(c).unwrap().unwrap();
        let a = push(&mut o, done("issue:1", true, false));
        let b = push(
            &mut o,
            Change::TaskUpdate {
                task_list_id: 1,
                task_id: 2,
                title: Some("Mine".into()),
                done: None,
                base_title: "Base".into(),
                base_done: false,
            },
        );
        let c = push(
            &mut o,
            Change::CaptureCreate {
                text: "My idea".into(),
                url: None,
                intended_type: None,
            },
        );
        let d = push(&mut o, done("issue:4", true, false));
        let e = push(&mut o, done("task:5", true, false));
        let sent = ids(&o);
        let mut conflict = result(&b, "conflict", 409);
        conflict.conflicts = Some(json!({"title": {"base": "Base", "mine": "Mine", "theirs": "Theirs"}}));
        let mut invalid = result(&c, "invalid", 422);
        invalid.errors = Some(json!({"text": ["The text is too long."]}));
        let results = vec![
            result(&a, "applied", 200),
            conflict,
            invalid,
            result(&d, "deleted", 410),
            result(&e, "error", 500),
        ];
        let step = o.apply_results(&sent, &results);
        assert_eq!(step.done.len(), 2, "applied + conflict are done");
        assert_eq!(step.problems, 3);
        assert!(step.stop, "a server error: try later");
        assert_eq!(ids(&o), [e], "only the failed one stays");
        assert_eq!(o.entries[0].attempts, 1);

        let p = &o.problems;
        assert_eq!(p[0].kind, "conflict");
        assert_eq!(p[0].mine.as_deref(), Some("Mine"));
        assert_eq!(p[0].theirs.as_deref(), Some("Theirs"));
        assert!(p[0].message.contains("server's version was kept"));
        assert_eq!(p[1].kind, "invalid");
        assert_eq!(p[1].message, "The text is too long.");
        assert_eq!(p[1].mine.as_deref(), Some("My idea"), "the user's text is kept");
        assert_eq!(p[2].kind, "deleted");

        o.dismiss(Some(&b));
        assert_eq!(o.problems.len(), 2);
        o.dismiss(None);
        assert!(o.problems.is_empty());
    }

    #[test]
    fn waiting_changes_give_up_after_a_few_tries_and_missing_results_stay() {
        let mut o = Outbox::default();
        let a = o.push(done("issue:1", true, false)).unwrap().unwrap();
        let b = o.push(done("issue:2", true, false)).unwrap().unwrap();
        for _ in 0..MAX_WAITING - 1 {
            let step = o.apply_results(&[a.clone(), b.clone()], &[result(&a, "waiting", 424)]);
            assert!(!step.stop);
        }
        assert_eq!(o.len(), 2);
        let step = o.apply_results(std::slice::from_ref(&a), &[result(&a, "waiting", 424)]);
        assert_eq!(step.problems, 1);
        assert_eq!(ids(&o), [b]);
    }

    #[test]
    fn batch_errors_never_retry_a_4xx_forever() {
        let mut o = Outbox::default();
        let a = o.push(done("issue:1", true, false)).unwrap().unwrap();
        let b = o.push(done("issue:2", true, false)).unwrap().unwrap();
        let sent = vec![a.clone(), b.clone()];

        let step = o.apply_batch_error(&sent, &CallError::Offline("down".into()));
        assert!(step.stop && !step.split);
        assert_eq!(o.len(), 2);
        let step = o.apply_batch_error(&sent, &CallError::SignedOut("x".into()));
        assert!(step.stop);
        let server = |status| CallError::Server {
            status,
            code: "x".into(),
            message: "Nope".into(),
        };
        let step = o.apply_batch_error(&sent, &server(503));
        assert!(step.stop);

        let step = o.apply_batch_error(&sent, &server(422));
        assert!(step.split, "one by one");
        assert_eq!(o.len(), 2);
        let step = o.apply_batch_error(std::slice::from_ref(&a), &server(422));
        assert_eq!(step.problems, 1);
        assert_eq!(ids(&o), [b]);
        assert_eq!(o.problems[0].kind, "rejected");
        assert_eq!(o.problems[0].message, "Nope");
    }

    #[test]
    fn parses_the_batch_answer() {
        let body = json!({"data": [{"request_id": "a", "type": "work.done", "status": "applied", "http_status": 200,
            "replayed": false, "entity_type": "issue", "entity": {"id": 1}, "conflicts": null, "applied": null,
            "message": null, "errors": null}]});
        let r = parse_results(&body);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].status, "applied");
        assert_eq!(parse_results(&body["data"]).len(), 1);
        assert!(parse_results(&json!({"message": "x"})).is_empty());
    }

    #[test]
    fn persists_atomically_and_problems_are_bounded() {
        let dir = std::env::temp_dir().join(format!("brainitive-outbox-{}", std::process::id()));
        let path = dir.join("outbox.json");
        let mut o = Outbox {
            user_id: Some(7),
            ..Outbox::default()
        };
        o.push(done("issue:1", true, false)).unwrap();
        o.save(&path).unwrap();
        assert_eq!(Outbox::load(&path), o);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        o.entries.clear();
        o.save(&path).unwrap();
        assert!(!path.exists(), "nothing waiting, no problems: no file");
        std::fs::write(&path, b"{oops").unwrap();
        assert_eq!(Outbox::load(&path), Outbox::default());

        for i in 0..MAX_PROBLEMS + 5 {
            let e = Entry::new(done(&format!("issue:{}", i + 1), true, false));
            o.add_problem(&e, "deleted", None);
        }
        assert_eq!(o.problems.len(), MAX_PROBLEMS);
        let _ = std::fs::remove_dir_all(dir);
    }
}
