//! Changes made in the offline window, waiting to be sent (BRAI-366): one file per server
//! (`offline/<store id>/outbox.json` in the app data dir), sent **in order** with
//! `POST /api/v1/sync` in batches of ≤ 50. Every change has its own `request_id`, so a batch that
//! is sent again (lost answer) is never applied twice.
//!
//! Each result is handled honestly (`verdict`): applied → done; conflict → done, the server kept
//! its value and the user's is shown as a problem; deleted / forbidden / invalid → dropped as a
//! problem that still shows the user's text; gone (a timer no longer running, a notification
//! deleted) → dropped quietly; waiting → kept (dropped after a few tries); rate_limited / error →
//! kept, try later. A 4xx is never retried forever. Two timer cases are special: a `timer.start`
//! conflict (another timer runs) is dropped with a problem, and an `invalid` `timer.stop` that
//! offers activities waits for the user's pick ([`Outbox::activity_needed`]). Pure logic; the
//! sending is in `offline.rs`.
//!
//! The timer, notification and `issue.create` description / type changes need a server with the
//! v1.49 client-gaps sync types (`offline_cache::Cache::extended_sync`); the callers check that.

use std::path::Path;
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
    TaskCreate {
        task_list_id: u64,
        title: String,
        done: bool,
    },
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
    /// Start the timer on an issue (`timer.start`); `started_at` = the device's clock (ISO).
    TimerStart {
        issue_id: u64,
        #[serde(default)]
        description: Option<String>,
        started_at: String,
        #[serde(default)]
        title: String,
    },
    /// Stop and log the running timer (`timer.stop`).
    TimerStop {
        timer_id: u64,
        stopped_at: String,
        #[serde(default)]
        activity_id: Option<u64>,
        #[serde(default)]
        title: String,
    },
    /// Stop without logging (`timer.discard`).
    TimerDiscard {
        timer_id: u64,
        #[serde(default)]
        title: String,
    },
    /// One notification read (`notification.read`).
    NotificationRead {
        notification_id: String,
        #[serde(default)]
        title: String,
    },
    /// "Mark all read" (`notifications.read_all`): only notifications created up to `before`.
    NotificationsReadAll { before: String },
    /// A new issue from the New task window (`issue.create`).
    IssueCreate {
        board_id: u64,
        title: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        item_type_id: Option<u64>,
        #[serde(default)]
        priority_id: Option<u64>,
    },
}

/// An activity a stopped timer can be logged as (`activities` of an `invalid` `timer.stop`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityChoice {
    pub id: u64,
    pub name: String,
}

/// A `timer.stop` the server refused for want of an activity: the timer runs on there; the panel
/// shows the choice and the stop goes out again with the picked `activity_id` (same `stopped_at`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityNeeded {
    pub timer_id: u64,
    pub stopped_at: String,
    #[serde(default)]
    pub title: String,
    pub activities: Vec<ActivityChoice>,
    #[serde(default)]
    pub default_activity_id: Option<u64>,
    #[serde(default)]
    pub message: String,
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
    /// A stop that waits for the user's activity (see [`ActivityNeeded`]).
    #[serde(default)]
    pub activity_needed: Option<ActivityNeeded>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    #[error("Too many changes are waiting to be sent. Connect to the internet first.")]
    Full,
    #[error("This task is being sent right now. Try again in a moment.")]
    InFlight,
    #[error("That task is not waiting to be created any more.")]
    UnknownRef,
    #[error("Pick one of the activities shown.")]
    UnknownActivity,
}

/// What to do with one change after the server's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Applied,
    /// Done, but (some) fields kept the server's value.
    Conflict,
    /// Dropped: `kind` for the problem.
    Drop(&'static str),
    /// Dropped without a problem: the timer / notification is no longer there, nothing was
    /// written (`gone`).
    Gone,
    /// Kept. `stop`: don't send more now (rate limit, server error).
    Keep {
        stop: bool,
    },
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
    /// What kind of feed record `entity` is (`work`, `task`, `task_list`, `capture`, `my_day`,
    /// `comment`; v2; `timer`, `notification`, `unread_count` with the client-gaps types).
    #[serde(default)]
    pub record_type: Option<String>,
    #[serde(default)]
    pub entity: Option<Value>,
    /// An `invalid` `timer.stop`: what can be picked (`[{id, name}]`) and the default.
    #[serde(default)]
    pub activities: Option<Value>,
    #[serde(default)]
    pub default_activity_id: Option<u64>,
}

impl SyncResult {
    /// The live state this result carries (`timer`, `notification`, `unread_count`), shown by
    /// the tray and the panel.
    pub fn live_state(&self) -> Option<(&str, &Value)> {
        let kind = self.record_type.as_deref()?;
        let entity = self.entity.as_ref().filter(|e| e.is_object())?;
        matches!(kind, "timer" | "notification" | "unread_count").then_some((kind, entity))
    }

    /// `activities` of an `invalid` `timer.stop`, cleaned (id + name).
    fn activity_choices(&self) -> Vec<ActivityChoice> {
        self.activities
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let name: String = a["name"]
                    .as_str()?
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(80)
                    .collect();
                let name = name.trim().to_string();
                (!name.is_empty()).then_some(ActivityChoice {
                    id: a["id"].as_u64()?,
                    name,
                })
            })
            .collect()
    }
}

pub fn verdict(r: &SyncResult) -> Verdict {
    match r.status.as_str() {
        "applied" => Verdict::Applied,
        "conflict" => Verdict::Conflict,
        "deleted" => Verdict::Drop("deleted"),
        "gone" => Verdict::Gone,
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
    /// Results whose live state (timer, notification, unread count) the tray / panel show —
    /// also of changes that were dropped (a refused start brings the timer that runs).
    pub states: Vec<SyncResult>,
    /// Dropped quietly (`gone`): what they were, for the log.
    pub gone: Vec<String>,
    /// A stop now waits for an activity ([`Outbox::activity_needed`]).
    pub needs_activity: bool,
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
            Self::TaskUpdate { title: Some(t), .. } => format!("Rename task to {}", quote(t)),
            Self::TaskUpdate {
                done: Some(true),
                base_title,
                ..
            } => format!("Mark done: {}", quote(base_title)),
            Self::TaskUpdate { base_title, .. } => format!("Change task {}", quote(base_title)),
            Self::CaptureCreate { text, .. } => format!("Capture {}", quote(text)),
            Self::TimerStart { title, .. } => format!("Start timer: {}", quote(title)),
            Self::TimerStop { title, .. } => format!("Stop timer: {}", quote(title)),
            Self::TimerDiscard { title, .. } => format!("Discard timer: {}", quote(title)),
            Self::NotificationRead { title, .. } => format!("Mark read: {}", quote(title)),
            Self::NotificationsReadAll { .. } => "Mark all notifications read".to_string(),
            Self::IssueCreate { title, .. } => format!("New issue {}", quote(title)),
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
            Self::IssueCreate { title, description, .. } => Some(match description {
                Some(d) => format!("{title}\n\n{d}"),
                None => title.clone(),
            }),
            Self::TimerStart {
                description: Some(d), ..
            } => Some(d.clone()),
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
                (
                    "my_day.add",
                    None,
                    None,
                    json!({ "work_type": work_type, "work_id": id }),
                )
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
            Change::TimerStart {
                issue_id,
                description,
                started_at,
                ..
            } => {
                let mut data = json!({ "issue_id": issue_id, "started_at": started_at });
                if let Some(d) = description {
                    data["description"] = json!(d);
                }
                ("timer.start", None, None, data)
            }
            Change::TimerStop {
                timer_id,
                stopped_at,
                activity_id,
                ..
            } => {
                let mut data = json!({ "timer_id": timer_id, "stopped_at": stopped_at });
                if let Some(a) = activity_id {
                    data["activity_id"] = json!(a);
                }
                ("timer.stop", None, None, data)
            }
            Change::TimerDiscard { timer_id, .. } => ("timer.discard", None, None, json!({ "timer_id": timer_id })),
            Change::NotificationRead { notification_id, .. } => (
                "notification.read",
                None,
                None,
                json!({ "notification_id": notification_id }),
            ),
            Change::NotificationsReadAll { before } => {
                ("notifications.read_all", None, None, json!({ "before": before }))
            }
            Change::IssueCreate {
                board_id,
                title,
                description,
                item_type_id,
                priority_id,
            } => {
                let mut data = json!({ "board_id": board_id, "title": title });
                if let Some(d) = description {
                    data["description"] = json!(d);
                }
                if let Some(t) = item_type_id {
                    data["item_type_id"] = json!(t);
                }
                if let Some(p) = priority_id {
                    data["priority_id"] = json!(p);
                }
                ("issue.create", None, None, data)
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
        "timer_running" => format!(
            "{} Your offline start was not saved.",
            server.unwrap_or_else(|| "Another timer is already running.".into())
        ),
        "conflict" => "Changed on the server meanwhile. The server's version was kept.".to_string(),
        "deleted" => server.unwrap_or_else(|| "It was deleted or you can't open it any more.".into()),
        "forbidden" => "You are not allowed to make this change.".to_string(),
        "invalid" => errors
            .or(server)
            .unwrap_or_else(|| "The server did not accept this change.".into()),
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
        if self.entries.is_empty() && self.problems.is_empty() && self.activity_needed.is_none() {
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
            Change::NotificationRead { notification_id, .. } => {
                if let Some(i) = self.unsent(
                    |c| matches!(c, Change::NotificationRead { notification_id: n, .. } if n == notification_id),
                ) {
                    return Ok(Some(self.entries[i].request_id.clone()));
                }
            }
            Change::NotificationsReadAll { before } => {
                // a later "mark all read" covers the earlier one: it moves `before` forward
                if let Some(i) = self.unsent(|c| matches!(c, Change::NotificationsReadAll { .. })) {
                    if let Change::NotificationsReadAll { before: b } = &mut self.entries[i].change {
                        if before.as_str() > b.as_str() {
                            *b = before.clone();
                        }
                    }
                    return Ok(Some(self.entries[i].request_id.clone()));
                }
            }
            Change::TaskCreate { .. }
            | Change::CaptureCreate { .. }
            | Change::TimerStart { .. }
            | Change::TimerStop { .. }
            | Change::TimerDiscard { .. }
            | Change::IssueCreate { .. } => {}
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
    pub fn edit_pending_create(
        &mut self,
        request_id: &str,
        title: Option<String>,
        done: Option<bool>,
    ) -> Result<(), OutboxError> {
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

    /// The `done` a new change of `work_id` is based on: what the server will have once the
    /// changes already sent are applied (the cache doesn't show them yet), or the base of the
    /// unsent change it will be folded into.
    pub fn base_done(&self, work_id: &str, cached: bool) -> bool {
        let mut value = cached;
        for e in &self.entries {
            if let Change::WorkDone {
                work_id: w,
                done,
                base_done,
                ..
            } = &e.change
            {
                if w == work_id {
                    if e.attempts == 0 {
                        return *base_done;
                    }
                    value = *done;
                }
            }
        }
        value
    }

    /// Like [`Self::base_done`] for a task's title and done.
    pub fn task_base(&self, task_id: u64, cached_title: &str, cached_done: bool) -> (String, bool) {
        let mut value = (cached_title.to_string(), cached_done);
        for e in &self.entries {
            if let Change::TaskUpdate {
                task_id: t,
                title,
                done,
                base_title,
                base_done,
                ..
            } = &e.change
            {
                if *t == task_id {
                    if e.attempts == 0 {
                        return (base_title.clone(), *base_done);
                    }
                    if let Some(title) = title {
                        value.0 = title.clone();
                    }
                    if let Some(done) = done {
                        value.1 = *done;
                    }
                }
            }
        }
        value
    }

    /// The timer started offline that waits in the outbox (the last one), and whether it already
    /// went out (its answer lost: it can't be taken back any more).
    pub fn pending_start(&self) -> Option<(&Entry, bool)> {
        self.entries
            .iter()
            .rev()
            .find(|e| matches!(e.change, Change::TimerStart { .. }))
            .map(|e| (e, e.attempts > 0))
    }

    /// Discard of a timer started offline that wasn't sent yet: the start is taken back.
    /// `false` when there is none (or it went out already).
    pub fn cancel_unsent_start(&mut self) -> bool {
        match self.unsent(|c| matches!(c, Change::TimerStart { .. })) {
            Some(i) => {
                self.entries.remove(i);
                true
            }
            None => false,
        }
    }

    /// The user picked an activity for the stop that waited for one: the stop goes out again
    /// (a new change, same `stopped_at`) with it.
    pub fn resend_stop(&mut self, activity_id: u64) -> Result<Option<String>, OutboxError> {
        let needed = self.activity_needed.as_ref().ok_or(OutboxError::UnknownActivity)?;
        if !needed.activities.iter().any(|a| a.id == activity_id) {
            return Err(OutboxError::UnknownActivity);
        }
        let change = Change::TimerStop {
            timer_id: needed.timer_id,
            stopped_at: needed.stopped_at.clone(),
            activity_id: Some(activity_id),
            title: needed.title.clone(),
        };
        let id = self.push(change)?;
        self.activity_needed = None;
        Ok(id)
    }

    /// The stop that waited for an activity is no longer needed (`timer_id` stopped another way
    /// or isn't running; `None`: whatever it was).
    pub fn clear_activity_needed(&mut self, timer_id: Option<u64>) {
        if timer_id.is_none() || self.activity_needed.as_ref().map(|n| n.timer_id) == timer_id {
            self.activity_needed = None;
        }
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
            if result.live_state().is_some() {
                step.states.push(result.clone());
            }
            match verdict(result) {
                Verdict::Applied => {
                    let entry = self.entries.remove(i);
                    step.done.push((entry, result.clone()));
                }
                Verdict::Conflict if matches!(self.entries[i].change, Change::TimerStart { .. }) => {
                    // another timer runs: this start is dropped; its entity (the running timer)
                    // is shown through `states`
                    let entry = self.entries.remove(i);
                    self.add_problem(&entry, "timer_running", Some(result));
                    step.problems += 1;
                }
                Verdict::Conflict => {
                    let entry = self.entries.remove(i);
                    self.add_problem(&entry, "conflict", Some(result));
                    step.problems += 1;
                    step.done.push((entry, result.clone()));
                }
                Verdict::Drop("invalid") if self.waits_for_activity(i, result) => {
                    let entry = self.entries.remove(i);
                    if let Change::TimerStop {
                        timer_id,
                        stopped_at,
                        title,
                        ..
                    } = entry.change
                    {
                        let message = problem_message("invalid", Some(result));
                        self.activity_needed = Some(ActivityNeeded {
                            timer_id,
                            stopped_at,
                            title,
                            activities: result.activity_choices(),
                            default_activity_id: result.default_activity_id,
                            message,
                        });
                    }
                    step.needs_activity = true;
                }
                Verdict::Drop(kind) => {
                    let entry = self.entries.remove(i);
                    self.add_problem(&entry, kind, Some(result));
                    step.problems += 1;
                }
                Verdict::Gone => {
                    let entry = self.entries.remove(i);
                    step.gone.push(entry.change.describe());
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

    /// An `invalid` `timer.stop` that offers activities to pick from.
    fn waits_for_activity(&self, i: usize, result: &SyncResult) -> bool {
        matches!(self.entries[i].change, Change::TimerStop { .. }) && !result.activity_choices().is_empty()
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
        if let Some(i) = sent
            .first()
            .and_then(|id| self.entries.iter().position(|e| &e.request_id == id))
        {
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

/// Unix seconds → `2026-11-02T09:41:12Z` (the sync API's device times).
pub fn iso_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // the civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
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

/// A sibling temp file, then rename (a crash never leaves half a file). The temp file is
/// created with mode 0600 and the folder 0700 on Unix ([`crate::private_fs`]).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    crate::private_fs::write_atomic(path, bytes)
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
        let Change::TaskUpdate { title, done, .. } = &o.entries[0].change else {
            panic!()
        };
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
    fn a_new_change_is_based_on_what_was_already_sent() {
        let mut o = Outbox::default();
        assert!(!o.base_done("issue:1", false), "nothing waiting: the cache");
        o.push(done("issue:1", true, false)).unwrap();
        assert!(!o.base_done("issue:1", false), "unsent: folded in, same base");
        o.entries[0].attempts = 1;
        assert!(
            o.base_done("issue:1", false),
            "sent, answer lost: the server may have it done"
        );

        let update = |title: Option<&str>, d: Option<bool>| Change::TaskUpdate {
            task_list_id: 1,
            task_id: 5,
            title: title.map(str::to_string),
            done: d,
            base_title: "Old".into(),
            base_done: false,
        };
        assert_eq!(o.task_base(5, "Old", false), ("Old".to_string(), false));
        o.push(update(Some("New"), None)).unwrap();
        assert_eq!(o.task_base(5, "Old", false), ("Old".to_string(), false));
        o.entries[1].attempts = 1;
        assert_eq!(o.task_base(5, "Old", false), ("New".to_string(), false));
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
        assert_eq!(
            o.edit_pending_create(&id, None, Some(false)),
            Err(OutboxError::InFlight)
        );
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
        assert_eq!(verdict(&result("a", "gone", 410)), Verdict::Gone, "dropped quietly");
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
            "replayed": false, "entity_type": "issue", "record_type": "work", "entity": {"id": "issue:1"}, "conflicts": null, "applied": null,
            "message": null, "errors": null}]});
        let r = parse_results(&body);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].status, "applied");
        assert_eq!(r[0].record_type.as_deref(), Some("work"));
        assert_eq!(r[0].entity, Some(json!({"id": "issue:1"})));
        assert_eq!(parse_results(&body["data"]).len(), 1);
        assert!(parse_results(&json!({"message": "x"})).is_empty());
    }

    #[test]
    fn timer_inbox_and_issue_changes_become_the_new_sync_types() {
        let j = Entry::new(Change::TimerStart {
            issue_id: 412,
            description: Some("Reproduce the bug".into()),
            started_at: "2026-11-02T08:58:00Z".into(),
            title: "WEB-12 Fix login".into(),
        })
        .to_json();
        assert_eq!(j["type"], "timer.start");
        assert_eq!(
            j["data"],
            json!({"issue_id": 412, "description": "Reproduce the bug", "started_at": "2026-11-02T08:58:00Z"})
        );
        assert!(j.get("target").is_none() && j.get("base").is_none());
        let j = Entry::new(Change::TimerStart {
            issue_id: 412,
            description: None,
            started_at: "2026-11-02T08:58:00Z".into(),
            title: String::new(),
        })
        .to_json();
        assert_eq!(
            j["data"],
            json!({"issue_id": 412, "started_at": "2026-11-02T08:58:00Z"})
        );

        let stop = |activity_id| Change::TimerStop {
            timer_id: 77,
            stopped_at: "2026-11-02T09:40:00Z".into(),
            activity_id,
            title: "x".into(),
        };
        let j = Entry::new(stop(None)).to_json();
        assert_eq!(j["type"], "timer.stop");
        assert_eq!(j["data"], json!({"timer_id": 77, "stopped_at": "2026-11-02T09:40:00Z"}));
        let j = Entry::new(stop(Some(4))).to_json();
        assert_eq!(
            j["data"],
            json!({"timer_id": 77, "stopped_at": "2026-11-02T09:40:00Z", "activity_id": 4})
        );

        let j = Entry::new(Change::TimerDiscard {
            timer_id: 77,
            title: "x".into(),
        })
        .to_json();
        assert_eq!(
            (j["type"].clone(), j["data"].clone()),
            (json!("timer.discard"), json!({"timer_id": 77}))
        );

        let j = Entry::new(Change::NotificationRead {
            notification_id: "9b2c-41".into(),
            title: "Ada assigned you".into(),
        })
        .to_json();
        assert_eq!(j["type"], "notification.read");
        assert_eq!(j["data"], json!({"notification_id": "9b2c-41"}));

        let j = Entry::new(Change::NotificationsReadAll {
            before: "2026-11-02T09:41:12Z".into(),
        })
        .to_json();
        assert_eq!(j["type"], "notifications.read_all");
        assert_eq!(j["data"], json!({"before": "2026-11-02T09:41:12Z"}));

        let j = Entry::new(Change::IssueCreate {
            board_id: 9,
            title: "Offline task".into(),
            description: Some("**Steps**".into()),
            item_type_id: Some(2),
            priority_id: Some(3),
        })
        .to_json();
        assert_eq!(j["type"], "issue.create");
        assert_eq!(
            j["data"],
            json!({"board_id": 9, "title": "Offline task", "description": "**Steps**", "item_type_id": 2, "priority_id": 3})
        );
        let j = Entry::new(Change::IssueCreate {
            board_id: 9,
            title: "Plain".into(),
            description: None,
            item_type_id: None,
            priority_id: None,
        })
        .to_json();
        assert_eq!(
            j["data"],
            json!({"board_id": 9, "title": "Plain"}),
            "nothing unset is sent"
        );
    }

    #[test]
    fn reads_fold_and_a_later_read_all_moves_before_forward() {
        let mut o = Outbox::default();
        let read = |id: &str| Change::NotificationRead {
            notification_id: id.into(),
            title: String::new(),
        };
        let a = o.push(read("a")).unwrap();
        assert_eq!(o.push(read("a")).unwrap(), a, "read twice = once");
        o.push(read("b")).unwrap();
        let all = |before: &str| Change::NotificationsReadAll { before: before.into() };
        let r = o.push(all("2026-11-02T09:00:00Z")).unwrap();
        assert_eq!(o.push(all("2026-11-02T10:00:00Z")).unwrap(), r);
        assert_eq!(o.push(all("2026-11-02T08:00:00Z")).unwrap(), r, "never moves back");
        assert_eq!(o.len(), 3);
        assert_eq!(o.entries[2].change, all("2026-11-02T10:00:00Z"));
        // once it went out, a new "mark all read" is a new change
        o.entries[2].attempts = 1;
        o.push(all("2026-11-02T11:00:00Z")).unwrap();
        assert_eq!(o.len(), 4);
    }

    #[test]
    fn a_timer_started_offline_can_be_taken_back_until_it_went_out() {
        let mut o = Outbox::default();
        assert!(o.pending_start().is_none());
        o.push(Change::TimerStart {
            issue_id: 1,
            description: None,
            started_at: "2026-11-02T08:00:00Z".into(),
            title: "x".into(),
        })
        .unwrap();
        assert_eq!(o.pending_start().map(|(_, sent)| sent), Some(false));
        o.entries[0].attempts = 1;
        assert_eq!(o.pending_start().map(|(_, sent)| sent), Some(true));
        assert!(!o.cancel_unsent_start(), "the server may have it already");
        o.entries[0].attempts = 0;
        assert!(o.cancel_unsent_start());
        assert!(o.is_empty());
    }

    fn timer_result(id: &str, status: &str, http: u16) -> SyncResult {
        SyncResult {
            record_type: Some("timer".into()),
            ..result(id, status, http)
        }
    }

    #[test]
    fn timer_and_inbox_results_ok_conflict_invalid_with_activities_and_gone() {
        let mut o = Outbox::default();
        let push = |o: &mut Outbox, c| o.push(c).unwrap().unwrap();
        let start = push(
            &mut o,
            Change::TimerStart {
                issue_id: 412,
                description: None,
                started_at: "2026-11-02T08:58:00Z".into(),
                title: "WEB-12 Fix login".into(),
            },
        );
        let stop = push(
            &mut o,
            Change::TimerStop {
                timer_id: 77,
                stopped_at: "2026-11-02T09:40:00Z".into(),
                activity_id: None,
                title: "WEB-9 Docs".into(),
            },
        );
        let read = push(
            &mut o,
            Change::NotificationRead {
                notification_id: "n1".into(),
                title: "x".into(),
            },
        );
        let all = push(
            &mut o,
            Change::NotificationsReadAll {
                before: "2026-11-02T09:41:12Z".into(),
            },
        );
        let discard = push(
            &mut o,
            Change::TimerDiscard {
                timer_id: 5,
                title: "old".into(),
            },
        );
        let sent = ids(&o);

        let running = json!({"running": true, "timer": {"id": 76, "started_at": "2026-11-02T08:00:00Z"}, "today_logged_seconds": 5400});
        let mut conflict = timer_result(&start, "conflict", 409);
        conflict.entity = Some(running.clone());
        conflict.message = Some("A timer is already running — stop it first.".into());
        let mut invalid = result(&stop, "invalid", 422);
        invalid.errors = Some(json!({"activity_id": ["Pick an activity."]}));
        invalid.activities =
            Some(json!([{"id": 4, "name": "Programming"}, {"id": 5, "name": "\u{7}Review "}, {"name": "no id"}]));
        invalid.default_activity_id = Some(4);
        let mut gone = result(&read, "gone", 410);
        gone.record_type = Some("notification".into());
        let mut applied = result(&all, "applied", 200);
        applied.record_type = Some("unread_count".into());
        applied.entity = Some(json!({"unread_count": 1}));
        let results = vec![conflict, invalid, gone, applied, timer_result(&discard, "gone", 410)];
        let step = o.apply_results(&sent, &results);

        assert!(o.is_empty(), "every one is settled");
        // ok: done, its live state goes to the tray / panel
        assert_eq!(step.done.len(), 1);
        assert_eq!(step.done[0].0.request_id, all);
        // conflict: dropped with a clear problem, the running timer is shown
        assert_eq!(step.problems, 1);
        assert_eq!(o.problems.len(), 1);
        assert_eq!(o.problems[0].request_id, start);
        assert_eq!(o.problems[0].kind, "timer_running");
        assert_eq!(
            o.problems[0].message,
            "A timer is already running — stop it first. Your offline start was not saved."
        );
        let states: Vec<_> = step.states.iter().filter_map(SyncResult::live_state).collect();
        assert_eq!(
            states,
            vec![("timer", &running), ("unread_count", &json!({"unread_count": 1}))]
        );
        // invalid + activities: waits for the pick, no problem
        assert!(step.needs_activity);
        let needed = o.activity_needed.clone().unwrap();
        assert_eq!(needed.timer_id, 77);
        assert_eq!(needed.stopped_at, "2026-11-02T09:40:00Z");
        assert_eq!(needed.default_activity_id, Some(4));
        assert_eq!(needed.message, "Pick an activity.");
        assert_eq!(
            needed.activities,
            vec![
                ActivityChoice {
                    id: 4,
                    name: "Programming".into()
                },
                ActivityChoice {
                    id: 5,
                    name: "Review".into()
                }
            ]
        );
        // gone: dropped quietly, noted for the log
        assert_eq!(
            step.gone,
            vec!["Mark read: “x”".to_string(), "Discard timer: “old”".to_string()]
        );

        // the pick: the stop goes out again with the activity and the same stopped_at
        assert_eq!(o.resend_stop(9), Err(OutboxError::UnknownActivity));
        let again = o.resend_stop(4).unwrap().unwrap();
        assert_ne!(again, stop, "a new change");
        assert!(o.activity_needed.is_none());
        assert_eq!(
            o.entries[0].to_json()["data"],
            json!({"timer_id": 77, "stopped_at": "2026-11-02T09:40:00Z", "activity_id": 4})
        );
        assert_eq!(o.resend_stop(4), Err(OutboxError::UnknownActivity), "only once");

        // an invalid stop with nothing to pick is a problem like any other
        let mut o = Outbox::default();
        let stop = push(
            &mut o,
            Change::TimerStop {
                timer_id: 1,
                stopped_at: "2026-11-02T09:40:00Z".into(),
                activity_id: None,
                title: "x".into(),
            },
        );
        let mut invalid = result(&stop, "invalid", 422);
        invalid.activities = Some(json!([]));
        let step = o.apply_results(std::slice::from_ref(&stop), &[invalid]);
        assert_eq!((step.problems, step.needs_activity), (1, false));
        assert!(o.activity_needed.is_none());
    }

    #[test]
    fn a_stop_waiting_for_an_activity_is_kept_on_disk_and_can_be_cleared() {
        let dir = std::env::temp_dir().join(format!("brainitive-outbox-activity-{}", std::process::id()));
        let path = dir.join("outbox.json");
        let mut o = Outbox {
            activity_needed: Some(ActivityNeeded {
                timer_id: 7,
                stopped_at: "2026-11-02T09:40:00Z".into(),
                title: "x".into(),
                activities: vec![ActivityChoice {
                    id: 1,
                    name: "Dev".into(),
                }],
                default_activity_id: None,
                message: "Pick an activity.".into(),
            }),
            ..Outbox::default()
        };
        o.save(&path).unwrap();
        assert_eq!(Outbox::load(&path), o, "nothing waiting, but a pick is: the file stays");
        o.clear_activity_needed(Some(8));
        assert!(o.activity_needed.is_some(), "another timer: kept");
        o.clear_activity_needed(Some(7));
        assert!(o.activity_needed.is_none());
        o.save(&path).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn device_times_are_iso_utc() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_793_612_472), "2026-11-02T09:41:12Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
        for t in [0_i64, 951_782_400, 1_793_612_472, 4_102_444_799] {
            assert_eq!(crate::panel::view::parse_instant(&iso_utc(t)), Some(t), "{t}");
        }
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
