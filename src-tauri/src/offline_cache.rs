//! The offline copy of the user's own data on one server (BRAI-366): My Work, today's My Day,
//! the kept task lists with their tasks and the private capture inbox, filled from
//! `GET /api/v1/sync/changes` and stored as `offline/<store id>/cache.json` in the app data dir.
//! No token is ever in it.
//!
//! Runs (API.md → Sync): without a cursor a **full run** (`reset: true`): page while `has_more`,
//! then drop every record the run did not send, and keep the last `next_cursor`. With it a
//! **delta run**: upserts (applied by `updated_at`) and tombstones (`op: delete`). After every
//! page, records of projects not in `project_ids` are dropped (lost access). Wanting more task
//! lists than the cursor knows starts a full run again.
//!
//! Also the window's view model: the cache with the waiting changes (`outbox.rs`) laid over it.
//! Pure logic; the calls are in `offline.rs`.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::outbox::{self, Change, Outbox};
use crate::server::ServerOrigin;

pub const SCOPES: &str = "work,task_lists,my_day,captures";
pub const MAX_KEPT_LISTS: usize = 50;
pub const PAGE_LIMIT: u32 = 100;
/// At most this many pages per sync (100 records each); a longer run goes on next time.
pub const MAX_PAGES: u32 = 200;
pub const KINDS: [&str; 5] = ["work", "task_list", "task", "capture", "my_day"];
const MAX_VIEW_WORK: usize = 500;
const MAX_VIEW_CAPTURES: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub project_id: Option<u64>,
    #[serde(default)]
    pub updated_at: Option<String>,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cache {
    pub origin: ServerOrigin,
    /// Whose data this is (`GET /me`); another account signing in deletes it.
    #[serde(default)]
    pub user_id: Option<u64>,
    #[serde(default)]
    pub user_name: Option<String>,
    /// The cursor of the last finished run (`None`: the next run is a full one).
    #[serde(default)]
    pub cursor: Option<String>,
    /// The `task_lists` the cursor was started with.
    #[serde(default)]
    pub task_lists: Vec<u64>,
    #[serde(default)]
    pub project_ids: Vec<u64>,
    /// Unix seconds of the last finished run.
    #[serde(default)]
    pub synced_at: Option<u64>,
    /// `kind:id` → record.
    #[serde(default)]
    pub records: BTreeMap<String, Record>,
}

/// One change of the feed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FeedChange {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(deserialize_with = "text_id")]
    pub id: String,
    #[serde(default)]
    pub op: String,
    #[serde(default)]
    pub project_id: Option<u64>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub data: Value,
}

/// One page of `GET /api/v1/sync/changes`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FeedPage {
    #[serde(default)]
    pub data: Vec<Value>,
    pub next_cursor: String,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub reset: bool,
    #[serde(default)]
    pub project_ids: Vec<u64>,
}

fn text_id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    match Value::deserialize(d)? {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(serde::de::Error::custom("id")),
    }
}

/// A run in progress (memory only: an interrupted full run simply starts again).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    /// The cursor for the next request (`None`: start a full run with `task_lists`).
    pub cursor: Option<String>,
    pub task_lists: Vec<u64>,
    pub full: bool,
    pub pages: u32,
    seen: HashSet<String>,
}

impl Run {
    /// The query of the next request.
    pub fn query(&self) -> Vec<(String, String)> {
        let mut q = Vec::new();
        match &self.cursor {
            Some(c) => q.push(("cursor".to_string(), c.clone())),
            None => {
                q.push(("scopes".to_string(), SCOPES.to_string()));
                q.push((
                    "task_lists".to_string(),
                    self.task_lists.iter().map(u64::to_string).collect::<Vec<_>>().join(","),
                ));
            }
        }
        q.push(("limit".to_string(), PAGE_LIMIT.to_string()));
        q
    }
}

pub fn key(kind: &str, id: &str) -> String {
    format!("{kind}:{id}")
}

/// ISO-8601 Zulu strings of the API (`2026-11-01T09:16:04Z`) compare as text.
fn newer(existing: Option<&str>, incoming: Option<&str>) -> bool {
    matches!((existing, incoming), (Some(a), Some(b)) if a > b)
}

impl Cache {
    pub fn new(origin: &ServerOrigin) -> Self {
        Self {
            origin: origin.clone(),
            user_id: None,
            user_name: None,
            cursor: None,
            task_lists: Vec::new(),
            project_ids: Vec::new(),
            synced_at: None,
            records: BTreeMap::new(),
        }
    }

    /// A missing / unreadable file, or one of another server, is an empty cache.
    pub fn load(path: &Path, origin: &ServerOrigin) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
            .filter(|c| &c.origin == origin)
            .unwrap_or_else(|| Self::new(origin))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        outbox::write_atomic(path, &serde_json::to_vec(self).expect("serializable"))
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Record> + 'a {
        self.records.values().filter(move |r| r.kind == kind)
    }

    /// The task lists to keep offline: those of the user's open list tasks first, then the ones
    /// already kept; at most 50, sorted (the API's `task_lists`).
    pub fn wanted_lists(&self) -> Vec<u64> {
        let mut out: Vec<u64> = Vec::new();
        for r in self.of_kind("work") {
            let source = &r.data["source"];
            if source["type"] == "list" && r.data["done"] != true {
                if let Some(id) = source["id"].as_u64() {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
        }
        for r in self.of_kind("task_list") {
            if let Ok(id) = r.id.parse::<u64>() {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out.truncate(MAX_KEPT_LISTS);
        out.sort_unstable();
        out
    }

    /// The next run: a delta with the kept cursor, or a full run when there is none or more
    /// lists are wanted than the cursor knows (lists that went away don't restart anything).
    pub fn start_run(&self) -> Run {
        let wanted = self.wanted_lists();
        let more = wanted.iter().any(|id| !self.task_lists.contains(id));
        match (&self.cursor, more) {
            (Some(c), false) => Run {
                cursor: Some(c.clone()),
                task_lists: self.task_lists.clone(),
                ..Run::default()
            },
            _ => Run {
                cursor: None,
                task_lists: wanted,
                ..Run::default()
            },
        }
    }

    /// Applies one page; returns whether another page follows (call again with `run`).
    pub fn apply_page(&mut self, run: &mut Run, page: FeedPage, now: u64) -> bool {
        run.full |= page.reset;
        run.pages += 1;
        for raw in page.data {
            let Ok(change) = serde_json::from_value::<FeedChange>(raw) else { continue };
            if !KINDS.contains(&change.kind.as_str()) {
                continue;
            }
            let k = key(&change.kind, &change.id);
            if change.op == "delete" {
                self.records.remove(&k);
                run.seen.remove(&k);
                continue;
            }
            run.seen.insert(k.clone());
            let existing = self.records.get(&k).and_then(|r| r.updated_at.as_deref());
            if newer(existing, change.updated_at.as_deref()) {
                continue; // the runs overlap: an older copy of the same record
            }
            if change.kind == "my_day" {
                // one plan: today's (a new local day replaces yesterday's)
                let newest = self.of_kind("my_day").map(|r| r.id.clone()).max();
                if newest.as_deref().is_some_and(|n| n > change.id.as_str()) {
                    continue;
                }
                self.records.retain(|_, r| r.kind != "my_day");
            }
            self.records.insert(
                k,
                Record {
                    kind: change.kind,
                    id: change.id,
                    project_id: change.project_id.filter(|p| *p > 0),
                    updated_at: change.updated_at,
                    data: change.data,
                },
            );
        }
        // lost access is not a delete: drop what belongs to a project the user can't open now
        self.project_ids = page.project_ids;
        let projects: HashSet<u64> = self.project_ids.iter().copied().collect();
        self.records
            .retain(|_, r| r.project_id.is_none_or(|p| projects.contains(&p)));
        if page.has_more && run.pages < MAX_PAGES {
            run.cursor = Some(page.next_cursor);
            return true;
        }
        if page.has_more {
            // too long for one sync: the run is not finished, keep the old cursor
            return false;
        }
        if run.full {
            let seen = std::mem::take(&mut run.seen);
            self.records.retain(|k, _| seen.contains(k));
        }
        self.cursor = Some(page.next_cursor);
        self.task_lists = std::mem::take(&mut run.task_lists);
        self.synced_at = Some(now);
        false
    }

    /// After a change was applied: the cache shows it until the next pull brings the record.
    pub fn note_applied(&mut self, change: &Change) {
        match change {
            Change::WorkDone { work_id, done, .. } => self.set_work_done(work_id, *done),
            Change::TaskUpdate {
                task_id, title, done, ..
            } => {
                if let Some(r) = self.records.get_mut(&key("task", &task_id.to_string())) {
                    if let Some(t) = title {
                        r.data["title"] = Value::from(t.as_str());
                    }
                    if let Some(d) = done {
                        r.data["done"] = Value::from(*d);
                        r.data["status"] = Value::from(if *d { "done" } else { "todo" });
                    }
                }
                if let Some(d) = done {
                    self.set_work_done(&format!("task:{task_id}"), *d);
                }
            }
            _ => {}
        }
    }

    fn set_work_done(&mut self, work_id: &str, done: bool) {
        if let Some(r) = self.records.get_mut(&key("work", work_id)) {
            r.data["done"] = Value::from(done);
        }
        for r in self.records.values_mut().filter(|r| r.kind == "my_day") {
            if let Some(items) = r.data["items"].as_array_mut() {
                for i in items.iter_mut().filter(|i| i["id"] == work_id) {
                    i["done"] = Value::from(done);
                }
            }
        }
    }

    /// The current state of a work item (cache only), for a change's `base`.
    pub fn work(&self, work_id: &str) -> Option<ViewWork> {
        if let Some(r) = self.records.get(&key("work", work_id)) {
            return Some(ViewWork::from_json(&r.data));
        }
        self.my_day()
            .and_then(|d| d.data["items"].as_array().cloned())
            .and_then(|items| items.iter().find(|i| i["id"] == work_id).map(ViewWork::from_json))
    }

    pub fn task(&self, task_id: u64) -> Option<ViewTask> {
        self.records
            .get(&key("task", &task_id.to_string()))
            .map(|r| ViewTask::from_json(&r.data))
    }

    pub fn has_list(&self, list_id: u64) -> bool {
        self.records.contains_key(&key("task_list", &list_id.to_string()))
    }

    fn my_day(&self) -> Option<&Record> {
        self.of_kind("my_day").max_by(|a, b| a.id.cmp(&b.id))
    }
}

// ── the window's view ────────────────────────────────────────────────────────────

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().chars().take(2000).collect()
}

fn opt_text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(|s| s.chars().take(2000).collect())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewWork {
    pub id: String,
    pub key: Option<String>,
    pub title: String,
    pub done: bool,
    pub due_date: Option<String>,
    pub project: Option<String>,
    pub source: Option<String>,
    /// A change of it is waiting to be sent.
    pub pending: bool,
    pub in_my_day: bool,
}

impl ViewWork {
    fn from_json(v: &Value) -> Self {
        Self {
            id: text(&v["id"]),
            key: opt_text(&v["key"]),
            title: text(&v["title"]),
            done: v["done"] == true,
            due_date: opt_text(&v["due_date"]),
            project: opt_text(&v["project"]["name"]),
            source: opt_text(&v["source"]["name"]),
            pending: false,
            in_my_day: false,
        }
    }

    pub fn label(&self) -> String {
        match &self.key {
            Some(k) => format!("{k} {}", self.title),
            None => self.title.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewTask {
    /// The task id, or `ref:<request id>` for one created offline and not sent yet.
    pub id: String,
    pub title: String,
    pub done: bool,
    pub due_date: Option<String>,
    pub pending: bool,
    #[serde(skip)]
    position: f64,
}

impl ViewTask {
    fn from_json(v: &Value) -> Self {
        Self {
            id: match &v["id"] {
                Value::Number(n) => n.to_string(),
                other => text(other),
            },
            title: text(&v["title"]),
            done: v["done"] == true || v["status"] == "done",
            due_date: opt_text(&v["due_date"]),
            pending: false,
            position: v["position"].as_f64().unwrap_or(f64::MAX),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewList {
    pub id: u64,
    pub name: String,
    pub project: Option<String>,
    pub tasks: Vec<ViewTask>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewCapture {
    pub id: String,
    pub text: String,
    pub url: Option<String>,
    pub captured_at: Option<String>,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ViewDay {
    pub date: String,
    pub items: Vec<ViewWork>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct View {
    pub has_data: bool,
    pub user_name: Option<String>,
    pub synced_at: Option<u64>,
    pub pending: usize,
    pub my_day: Option<ViewDay>,
    pub work: Vec<ViewWork>,
    pub lists: Vec<ViewList>,
    pub captures: Vec<ViewCapture>,
    pub problems: Vec<outbox::Problem>,
}

/// The cache with the waiting changes laid over it (nothing is sent or stored here).
pub fn view(cache: &Cache, out: &Outbox) -> View {
    // work: open items (plus ones ticked here), soonest due first like My Work
    let mut work: Vec<ViewWork> = cache.of_kind("work").map(|r| ViewWork::from_json(&r.data)).collect();
    let day = cache.my_day();
    let mut day_items: Vec<ViewWork> = day
        .and_then(|d| d.data["items"].as_array())
        .map(|items| items.iter().map(ViewWork::from_json).collect())
        .unwrap_or_default();

    let mut lists: Vec<ViewList> = cache
        .of_kind("task_list")
        .filter_map(|r| {
            Some(ViewList {
                id: r.id.parse().ok()?,
                name: text(&r.data["name"]),
                project: opt_text(&r.data["project"]["name"]),
                tasks: Vec::new(),
            })
        })
        .collect();
    for r in cache.of_kind("task") {
        let list = r.data["list_id"].as_u64().or_else(|| r.data["task_list_id"].as_u64());
        if let Some(l) = lists.iter_mut().find(|l| Some(l.id) == list) {
            l.tasks.push(ViewTask::from_json(&r.data));
        }
    }

    let mut captures: Vec<ViewCapture> = cache
        .of_kind("capture")
        .filter(|r| r.data["state"] == "inbox" || r.data["state"].is_null())
        .map(|r| ViewCapture {
            id: r.id.clone(),
            text: text(&r.data["text"]),
            url: opt_text(&r.data["url"]),
            captured_at: opt_text(&r.data["captured_at"]),
            pending: false,
        })
        .collect();
    captures.sort_by(|a, b| b.captured_at.cmp(&a.captured_at));

    let mark_done = |items: &mut Vec<ViewWork>, id: &str, done: bool| {
        for w in items.iter_mut().filter(|w| w.id == id) {
            w.done = done;
            w.pending = true;
        }
    };
    let mut new_captures = Vec::new();
    for e in &out.entries {
        match &e.change {
            Change::WorkDone { work_id, done, .. } => {
                mark_done(&mut work, work_id, *done);
                mark_done(&mut day_items, work_id, *done);
                if let Some(n) = work_id.strip_prefix("task:") {
                    for t in lists.iter_mut().flat_map(|l| l.tasks.iter_mut()).filter(|t| t.id == n) {
                        t.done = *done;
                        t.pending = true;
                    }
                }
            }
            Change::MyDayAdd { work_id, .. } => {
                if !day_items.iter().any(|w| &w.id == work_id) {
                    if let Some(mut w) = cache.work(work_id) {
                        w.pending = true;
                        day_items.push(w);
                    }
                }
            }
            Change::TaskCreate {
                task_list_id,
                title,
                done,
            } => {
                if let Some(l) = lists.iter_mut().find(|l| l.id == *task_list_id) {
                    l.tasks.push(ViewTask {
                        id: format!("ref:{}", e.request_id),
                        title: title.clone(),
                        done: *done,
                        due_date: None,
                        pending: true,
                        position: f64::MAX,
                    });
                }
            }
            Change::TaskUpdate {
                task_id, title, done, ..
            } => {
                let id = task_id.to_string();
                for t in lists.iter_mut().flat_map(|l| l.tasks.iter_mut()).filter(|t| t.id == id) {
                    if let Some(title) = title {
                        t.title = title.clone();
                    }
                    if let Some(d) = done {
                        t.done = *d;
                    }
                    t.pending = true;
                }
                if let Some(d) = done {
                    mark_done(&mut work, &format!("task:{task_id}"), *d);
                    mark_done(&mut day_items, &format!("task:{task_id}"), *d);
                }
            }
            Change::CaptureCreate { text, url, .. } => new_captures.push(ViewCapture {
                id: format!("ref:{}", e.request_id),
                text: text.clone(),
                url: url.clone(),
                captured_at: None,
                pending: true,
            }),
        }
    }
    new_captures.reverse();
    new_captures.extend(captures);
    new_captures.truncate(MAX_VIEW_CAPTURES);

    let planned: HashSet<String> = day_items.iter().map(|w| w.id.clone()).collect();
    work.retain(|w| !w.done || w.pending);
    for w in work.iter_mut() {
        w.in_my_day = planned.contains(&w.id);
    }
    work.sort_by(|a, b| match (&a.due_date, &b.due_date) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.label().cmp(&b.label()),
    });
    work.truncate(MAX_VIEW_WORK);
    for w in day_items.iter_mut() {
        w.in_my_day = true;
    }

    for l in lists.iter_mut() {
        l.tasks
            .sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap_or(std::cmp::Ordering::Equal));
    }
    lists.sort_by_key(|a| a.name.to_lowercase());

    View {
        has_data: !cache.is_empty(),
        user_name: cache.user_name.clone(),
        synced_at: cache.synced_at,
        pending: out.len(),
        my_day: day.map(|d| ViewDay {
            date: d.id.clone(),
            items: day_items,
        }),
        work,
        lists,
        captures: new_captures,
        problems: out.problems.iter().rev().cloned().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn origin() -> ServerOrigin {
        ServerOrigin::parse("brain.example.com").unwrap()
    }

    fn work(id: &str, title: &str, project: u64, list: Option<u64>, updated: &str) -> Value {
        let source = match list {
            Some(l) => json!({"type": "list", "id": l, "name": "Groceries"}),
            None => json!({"type": "board", "id": 1, "name": "Main"}),
        };
        json!({"type": "work", "id": id, "op": "upsert", "project_id": project, "updated_at": updated,
            "data": {"id": id, "type": if list.is_some() {"task"} else {"issue"}, "key": if list.is_some() {Value::Null} else {json!("BRAI-1")},
                "title": title, "done": false, "status": "To do", "due_date": null,
                "project": {"id": project, "name": "Brainitive", "key": "BRAI"}, "source": source, "updated_at": updated}})
    }

    fn page(changes: Vec<Value>, cursor: &str, has_more: bool, reset: bool, projects: &[u64]) -> FeedPage {
        serde_json::from_value(json!({"data": changes, "next_cursor": cursor, "has_more": has_more,
            "reset": reset, "project_ids": projects}))
        .unwrap()
    }

    fn delete(kind: &str, id: &str) -> Value {
        json!({"type": kind, "id": id, "op": "delete", "project_id": 1, "updated_at": "2026-11-02T10:00:00Z", "data": null})
    }

    #[test]
    fn a_full_run_pages_then_drops_what_it_did_not_send() {
        let mut c = Cache::new(&origin());
        // something left from before that the server no longer sends
        c.records.insert(
            key("work", "issue:99"),
            Record {
                kind: "work".into(),
                id: "issue:99".into(),
                project_id: Some(1),
                updated_at: None,
                data: json!({}),
            },
        );
        let mut run = c.start_run();
        assert_eq!(run.cursor, None);
        let q = run.query();
        assert!(q.contains(&("scopes".into(), SCOPES.into())));
        assert!(q.contains(&("task_lists".into(), String::new())));
        assert!(q.contains(&("limit".into(), "100".into())));

        let more = c.apply_page(
            &mut run,
            page(vec![work("issue:1", "Fix", 1, None, "2026-11-01T09:00:00Z")], "c1", true, true, &[1]),
            10,
        );
        assert!(more);
        assert_eq!(run.query()[0], ("cursor".into(), "c1".into()));
        assert_eq!(c.cursor, None, "the cursor is kept only when the run is finished");
        let more = c.apply_page(
            &mut run,
            page(vec![work("task:5", "Milk", 1, Some(3), "2026-11-01T09:00:00Z")], "c2", false, false, &[1]),
            11,
        );
        assert!(!more);
        assert_eq!(c.cursor.as_deref(), Some("c2"));
        assert_eq!(c.synced_at, Some(11));
        let keys: Vec<_> = c.records.keys().cloned().collect();
        assert_eq!(keys, ["work:issue:1", "work:task:5"]);
    }

    #[test]
    fn delta_runs_apply_upserts_by_updated_at_and_tombstones() {
        let mut c = Cache::new(&origin());
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(
                vec![
                    work("issue:1", "Fix", 1, None, "2026-11-01T09:00:00Z"),
                    work("issue:2", "Ship", 1, None, "2026-11-01T09:00:00Z"),
                ],
                "c1",
                false,
                true,
                &[1],
            ),
            1,
        );
        let mut run = c.start_run();
        assert_eq!(run.cursor.as_deref(), Some("c1"), "a delta run");
        assert_eq!(run.query()[0], ("cursor".into(), "c1".into()));
        assert!(!run.query().iter().any(|(k, _)| k == "scopes"), "the cursor remembers the scopes");
        c.apply_page(
            &mut run,
            page(
                vec![
                    // an older copy (the runs overlap): ignored
                    work("issue:1", "Old title", 1, None, "2026-11-01T08:00:00Z"),
                    work("issue:2", "Ship it", 1, None, "2026-11-01T10:00:00Z"),
                    delete("work", "issue:1"),
                    // unknown kinds are skipped
                    json!({"type": "whiteboard", "id": "1", "op": "upsert", "data": {}}),
                ],
                "c2",
                false,
                false,
                &[1],
            ),
            2,
        );
        assert!(!c.records.contains_key("work:issue:1"));
        assert_eq!(c.records["work:issue:2"].data["title"], "Ship it");
        assert_eq!(c.cursor.as_deref(), Some("c2"));
        assert_eq!(c.records.len(), 1, "a delta run keeps what it did not send");
    }

    #[test]
    fn a_reset_answer_to_a_delta_run_becomes_a_full_run() {
        let mut c = Cache::new(&origin());
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(vec![work("issue:1", "A", 1, None, "2026-11-01T09:00:00Z")], "c1", false, true, &[1]),
            1,
        );
        // the cursor expired: the server starts over
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(vec![work("issue:2", "B", 1, None, "2026-11-01T09:00:00Z")], "c9", false, true, &[1]),
            2,
        );
        assert_eq!(c.records.keys().cloned().collect::<Vec<_>>(), ["work:issue:2"]);
    }

    #[test]
    fn records_of_projects_the_user_lost_are_dropped() {
        let mut c = Cache::new(&origin());
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(
                vec![
                    work("issue:1", "A", 1, None, "2026-11-01T09:00:00Z"),
                    work("issue:2", "B", 2, None, "2026-11-01T09:00:00Z"),
                    json!({"type": "capture", "id": "u-1", "op": "upsert", "project_id": null,
                        "updated_at": "2026-11-01T09:00:00Z", "data": {"id": "u-1", "text": "Idea", "state": "inbox"}}),
                ],
                "c1",
                false,
                true,
                &[1, 2],
            ),
            1,
        );
        let mut run = c.start_run();
        c.apply_page(&mut run, page(vec![], "c2", false, false, &[1]), 2);
        assert!(c.records.contains_key("work:issue:1"));
        assert!(!c.records.contains_key("work:issue:2"), "project 2 is out of reach");
        assert!(c.records.contains_key("capture:u-1"), "private records have no project");
        assert_eq!(c.project_ids, [1]);
    }

    #[test]
    fn task_lists_of_the_users_tasks_are_kept_and_new_ones_restart_the_run() {
        let mut c = Cache::new(&origin());
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(
                vec![
                    work("task:5", "Milk", 1, Some(3), "2026-11-01T09:00:00Z"),
                    work("task:6", "Bread", 1, Some(3), "2026-11-01T09:00:00Z"),
                    work("issue:1", "Fix", 1, None, "2026-11-01T09:00:00Z"),
                ],
                "c1",
                false,
                true,
                &[1],
            ),
            1,
        );
        assert_eq!(c.wanted_lists(), [3]);
        // list 3 is new for the cursor: a full run with it
        let run = c.start_run();
        assert_eq!(run.cursor, None);
        assert!(run.query().contains(&("task_lists".into(), "3".into())));
        let mut run = run;
        c.apply_page(
            &mut run,
            page(
                vec![
                    work("task:5", "Milk", 1, Some(3), "2026-11-01T09:00:00Z"),
                    json!({"type": "task_list", "id": "3", "op": "upsert", "project_id": 1, "updated_at": "2026-11-01T09:00:00Z",
                        "data": {"id": 3, "name": "Groceries", "project": {"id": 1, "name": "Home"}}}),
                    json!({"type": "task", "id": 5, "op": "upsert", "project_id": 1, "updated_at": "2026-11-01T09:00:00Z",
                        "data": {"id": 5, "list_id": 3, "title": "Milk", "status": "todo", "done": false, "position": 2}}),
                    json!({"type": "task", "id": "7", "op": "upsert", "project_id": 1, "updated_at": "2026-11-01T09:00:00Z",
                        "data": {"id": 7, "list_id": 3, "title": "Eggs", "status": "done", "done": true, "position": 1}}),
                ],
                "c2",
                false,
                true,
                &[1],
            ),
            2,
        );
        assert_eq!(c.task_lists, [3]);
        assert!(c.has_list(3));
        assert_eq!(c.start_run().cursor.as_deref(), Some("c2"), "stable now");
        // the user's task in list 3 was finished: the kept list stays, no new run
        c.records.remove("work:task:5");
        assert_eq!(c.start_run().cursor.as_deref(), Some("c2"));

        let v = view(&c, &Outbox::default());
        assert_eq!(v.lists.len(), 1);
        let titles: Vec<_> = v.lists[0].tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Eggs", "Milk"], "in the list's order");
        assert!(c.task(7).unwrap().done);
    }

    #[test]
    fn only_todays_my_day_is_kept() {
        let mut c = Cache::new(&origin());
        let day = |date: &str, title: &str| {
            json!({"type": "my_day", "id": date, "op": "upsert", "project_id": null, "updated_at": format!("{date}T08:00:00Z"),
                "data": {"date": date, "items": [{"id": "issue:1", "key": "BRAI-1", "title": title, "done": false}], "carry_over_count": 0}})
        };
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(vec![day("2026-11-01", "Yesterday"), day("2026-11-02", "Today")], "c1", false, true, &[]),
            1,
        );
        let mut run = c.start_run();
        c.apply_page(&mut run, page(vec![day("2026-11-01", "Late copy")], "c2", false, false, &[]), 2);
        let days: Vec<_> = c.of_kind("my_day").map(|r| r.id.clone()).collect();
        assert_eq!(days, ["2026-11-02"]);
        let v = view(&c, &Outbox::default());
        assert_eq!(v.my_day.unwrap().items[0].title, "Today");
    }

    #[test]
    fn the_view_shows_waiting_changes_on_top_of_the_cache() {
        let mut c = Cache::new(&origin());
        c.user_name = Some("Armend".into());
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(
                vec![
                    work("issue:1", "Fix", 1, None, "2026-11-01T09:00:00Z"),
                    work("task:5", "Milk", 1, Some(3), "2026-11-01T09:00:00Z"),
                    json!({"type": "task_list", "id": "3", "op": "upsert", "project_id": 1, "data": {"id": 3, "name": "Groceries"}}),
                    json!({"type": "task", "id": "5", "op": "upsert", "project_id": 1, "data": {"id": 5, "list_id": 3, "title": "Milk", "done": false}}),
                    json!({"type": "my_day", "id": "2026-11-02", "op": "upsert", "data": {"date": "2026-11-02", "items": []}}),
                    json!({"type": "capture", "id": "u-1", "op": "upsert", "data": {"id": "u-1", "text": "Old idea", "state": "inbox", "captured_at": "2026-11-01T09:00:00Z"}}),
                    json!({"type": "capture", "id": "u-2", "op": "upsert", "data": {"id": "u-2", "text": "Archived", "state": "archived"}}),
                ],
                "c1",
                false,
                true,
                &[1],
            ),
            1,
        );
        let mut out = Outbox::default();
        out.push(Change::WorkDone {
            work_id: "issue:1".into(),
            done: true,
            base_done: false,
            title: "Fix".into(),
        })
        .unwrap();
        out.push(Change::MyDayAdd {
            work_id: "task:5".into(),
            title: "Milk".into(),
        })
        .unwrap();
        let new_task = out
            .push(Change::TaskCreate {
                task_list_id: 3,
                title: "Bread".into(),
                done: false,
            })
            .unwrap()
            .unwrap();
        out.push(Change::TaskUpdate {
            task_list_id: 3,
            task_id: 5,
            title: Some("Oat milk".into()),
            done: None,
            base_title: "Milk".into(),
            base_done: false,
        })
        .unwrap();
        out.push(Change::CaptureCreate {
            text: "New idea".into(),
            url: None,
            intended_type: None,
        })
        .unwrap();

        let v = view(&c, &out);
        assert!(v.has_data);
        assert_eq!(v.pending, 5);
        assert_eq!(v.user_name.as_deref(), Some("Armend"));
        let fix = v.work.iter().find(|w| w.id == "issue:1").unwrap();
        assert!(fix.done && fix.pending, "ticked here: still listed, marked as waiting");
        let milk = v.work.iter().find(|w| w.id == "task:5").unwrap();
        assert!(milk.in_my_day);
        let day = v.my_day.unwrap();
        assert_eq!(day.items.len(), 1);
        assert!(day.items[0].pending);
        let tasks: Vec<_> = v.lists[0].tasks.iter().map(|t| (t.id.clone(), t.title.clone(), t.pending)).collect();
        assert_eq!(
            tasks,
            [
                ("5".to_string(), "Oat milk".to_string(), true),
                (format!("ref:{new_task}"), "Bread".to_string(), true)
            ]
        );
        let caps: Vec<_> = v.captures.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(caps, ["New idea", "Old idea"], "new first, archived ones not in the inbox");
    }

    #[test]
    fn applied_changes_show_until_the_next_pull() {
        let mut c = Cache::new(&origin());
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(
                vec![
                    work("issue:1", "Fix", 1, None, "2026-11-01T09:00:00Z"),
                    json!({"type": "my_day", "id": "2026-11-02", "op": "upsert",
                        "data": {"date": "2026-11-02", "items": [{"id": "issue:1", "title": "Fix", "done": false}]}}),
                ],
                "c1",
                false,
                true,
                &[1],
            ),
            1,
        );
        c.note_applied(&Change::WorkDone {
            work_id: "issue:1".into(),
            done: true,
            base_done: false,
            title: String::new(),
        });
        assert!(c.work("issue:1").unwrap().done);
        let v = view(&c, &Outbox::default());
        assert!(v.work.is_empty(), "done work leaves My Work");
        assert!(v.my_day.unwrap().items[0].done);
    }

    #[test]
    fn persists_per_server_without_secrets() {
        let dir = std::env::temp_dir().join(format!("brainitive-cache-{}", std::process::id()));
        let path = dir.join("cache.json");
        let mut c = Cache::new(&origin());
        c.user_id = Some(4);
        c.cursor = Some("c1".into());
        let mut run = c.start_run();
        c.apply_page(
            &mut run,
            page(vec![work("issue:1", "Fix", 1, None, "2026-11-01T09:00:00Z")], "c1", false, true, &[1]),
            1,
        );
        c.save(&path).unwrap();
        assert_eq!(Cache::load(&path, &origin()), c);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("bra_") && !raw.contains("brr_"));
        // another server's file is not used
        let other = ServerOrigin::parse("other.example.com").unwrap();
        assert!(Cache::load(&path, &other).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
