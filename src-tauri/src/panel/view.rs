//! What the menu-bar panel shows (pure): the API answers of `/me/today`, `/me/my-day`, `/me/work`,
//! `/notifications`, `/me/timer` and `/work-logs` mapped to rows for the page, the offline
//! fallback from the offline copy, the waiting outbox changes laid over them (pending badges), and
//! the decisions (API / outbox / disabled, sync status).
//!
//! Older servers: a notification row without `summary` / `url` gets the client's wording and the
//! payload's link; a timer answer without `today_logged_seconds` gets today's total from
//! `/work-logs`.
//!
//! URLs never go to the page: rows carry `has_link`, and a click comes back as a reference (tab +
//! id) that Rust resolves against the answer it keeps (`find_*`).

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::offline_cache;
use crate::session::CallError;
use crate::tray::is_work_id;

pub const MAX_EVENTS: usize = 8;
pub const MAX_DUE: usize = 30;
pub const MAX_TEXT: usize = 200;

/// The five tabs, in their order (⌘1–5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    Today,
    MyDay,
    Assigned,
    Notifications,
    Timer,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Today, Tab::MyDay, Tab::Assigned, Tab::Notifications, Tab::Timer];

    pub fn as_str(self) -> &'static str {
        match self {
            Tab::Today => "today",
            Tab::MyDay => "my_day",
            Tab::Assigned => "assigned",
            Tab::Notifications => "notifications",
            Tab::Timer => "timer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == value)
    }

    /// The remembered tab; anything unknown (an older or edited settings file) is Today.
    pub fn remembered(value: &str) -> Self {
        Self::parse(value).unwrap_or(Tab::Today)
    }
}

// ── small helpers ──────────────────────────────────────────────────────────────

/// Server text for the page: no control characters, trimmed, at most `max` characters.
pub fn clean(value: &Value, max: usize) -> Option<String> {
    let s = value.as_str()?;
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.chars().count() > max {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        return Some(format!("{}…", cut.trim_end()));
    }
    Some(s.to_string())
}

fn text(value: &Value) -> String {
    clean(value, MAX_TEXT).unwrap_or_default()
}

/// Only `#rgb` / `#rrggbb` colours reach the page (it sets them as a style).
pub fn safe_color(value: &Value) -> Option<String> {
    let s = value.as_str()?.trim();
    let hex = s.strip_prefix('#')?;
    (matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_lowercase())
}

fn plain_date(value: &Value) -> Option<String> {
    let s = value.as_str()?;
    let ok = s.len() == 10
        && s.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    ok.then(|| s.to_string())
}

/// Unix seconds of an ISO-8601 instant (`2026-11-01T09:16:04Z`, with fractions or an offset).
pub fn parse_instant(value: &str) -> Option<i64> {
    let b = value.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { value.get(from..to)?.parse().ok() };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &value[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let digits: String = rest[1..].chars().filter(|c| *c != ':').collect();
            if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            sign * (digits[..2].parse::<i64>().ok()? * 3600 + digits[2..].parse::<i64>().ok()? * 60)
        }
    };
    // days from the civil date (Howard Hinnant's algorithm)
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

// ── work rows (Today, My Day, Assigned) ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkRow {
    pub id: String,
    pub key: Option<String>,
    pub title: String,
    pub done: bool,
    /// A change of it waits in the outbox (made offline).
    pub pending: bool,
    pub due_date: Option<String>,
    pub due_time: Option<String>,
    pub overdue: bool,
    pub project: Option<String>,
    pub color: Option<String>,
    /// An issue (list tasks have no work log, so no timer).
    pub timeable: bool,
    pub has_link: bool,
}

/// One work item (`/me/work`, `/me/my-day`, `/me/today`), or `None` without a valid id.
pub fn work_row(v: &Value, today: Option<&str>, pending: &HashMap<String, bool>) -> Option<WorkRow> {
    let id = v["id"].as_str().filter(|id| is_work_id(id))?.to_string();
    let due_date = plain_date(&v["due_date"]);
    let done_server = v["done"] == true;
    let queued = pending.get(&id).copied();
    Some(WorkRow {
        key: clean(&v["key"], 40),
        title: text(&v["title"]),
        done: queued.unwrap_or(done_server),
        pending: queued.is_some(),
        overdue: matches!((&due_date, today), (Some(d), Some(t)) if d.as_str() < t) && !done_server,
        due_time: clean(&v["due_time"], 5),
        due_date,
        project: clean(&v["project"]["name"], 80),
        color: safe_color(&v["project"]["color"]),
        timeable: id.starts_with("issue:"),
        has_link: v["url"].as_str().is_some_and(|u| !u.is_empty()),
        id,
    })
}

fn work_rows(items: &Value, today: Option<&str>, pending: &HashMap<String, bool>) -> Vec<WorkRow> {
    items
        .as_array()
        .map(|a| a.iter().filter_map(|v| work_row(v, today, pending)).collect())
        .unwrap_or_default()
}

/// The label a work item is known by ("BRAI-12 Fix login").
fn label(key: Option<&str>, title: &str) -> String {
    match key {
        Some(k) if !k.is_empty() => format!("{k} {title}"),
        _ => title.to_string(),
    }
}

// ── Today ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventRow {
    pub id: String,
    pub title: String,
    pub all_day: bool,
    /// ISO instant (timed) or `Y-m-d` (all day), formatted by the page in local time.
    pub start: Option<String>,
    pub end: Option<String>,
    pub location: Option<String>,
    /// Running right now.
    pub now: bool,
    pub color: Option<String>,
    /// A video call link exists (opened from the event's page).
    pub call: bool,
    pub has_link: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TodayView {
    pub date: Option<String>,
    pub events: Vec<EventRow>,
    pub more_events: usize,
    /// Open work due today or overdue.
    pub due: Vec<WorkRow>,
    pub more_due: usize,
    /// A connected calendar could not be read (`errors`).
    pub calendar_problem: bool,
    /// Built from the offline copy: no events.
    pub without_events: bool,
}

/// `GET /me/today` → the next events (ended ones left out, sorted, all-day first) and the work
/// due today / overdue (`events` holds calendar events and bookings only; dated work is in `work`).
pub fn today(body: &Value, now: i64, pending: &HashMap<String, bool>) -> TodayView {
    let date = plain_date(&body["date"]);
    let mut events: Vec<(i64, EventRow)> = body["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| event_row(e, now))
        .collect();
    events.sort_by_key(|(sort, _)| *sort);
    let total_events = events.len();
    let events: Vec<EventRow> = events.into_iter().map(|(_, e)| e).take(MAX_EVENTS).collect();
    let mut due = work_rows(&body["work"], date.as_deref(), pending);
    // ticked here (or queued offline): stays visible, struck through, until the next answer
    due.retain(|w| !w.done || w.pending);
    let total_due = due.len();
    due.truncate(MAX_DUE);
    TodayView {
        more_events: total_events - events.len(),
        events,
        more_due: total_due - due.len(),
        due,
        calendar_problem: body["errors"].as_array().is_some_and(|e| !e.is_empty()),
        without_events: false,
        date,
    }
}

/// One event with its sort key, or `None` when it is over (or broken).
fn event_row(e: &Value, now: i64) -> Option<(i64, EventRow)> {
    let id = clean(&e["id"], 300)?;
    let all_day = e["all_day"] == true;
    let start = e["start"].as_str().map(str::to_string);
    let end = e["end"].as_str().map(str::to_string);
    let (sort, running) = if all_day {
        (i64::MIN, false)
    } else {
        let s = start.as_deref().and_then(parse_instant)?;
        let f = end.as_deref().and_then(parse_instant).unwrap_or(s + 1);
        if f <= now {
            return None;
        }
        (s, s <= now)
    };
    Some((
        sort,
        EventRow {
            id,
            title: text(&e["title"]),
            all_day,
            start,
            end,
            location: clean(&e["location"], 120),
            now: running,
            color: safe_color(&e["color"]),
            call: e["conference_url"].as_str().is_some_and(|u| u.starts_with("https://")),
            has_link: e["url"].as_str().is_some_and(|u| !u.is_empty()),
        },
    ))
}

// ── My Day ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MyDayView {
    pub date: Option<String>,
    pub items: Vec<WorkRow>,
    pub done: usize,
    pub total: usize,
    /// Unfinished work of earlier days ("Bring over" in the web app).
    pub carry_over: u64,
}

/// `GET /me/my-day` → today's plan in the user's order.
pub fn my_day(body: &Value, pending: &HashMap<String, bool>) -> MyDayView {
    let date = plain_date(&body["date"]);
    let items = work_rows(&body["items"], date.as_deref(), pending);
    MyDayView {
        done: items.iter().filter(|i| i.done).count(),
        total: items.len(),
        carry_over: body["carry_over_count"].as_u64().unwrap_or(0),
        items,
        date,
    }
}

// ── Assigned ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Group {
    pub project: String,
    pub color: Option<String>,
    pub items: Vec<WorkRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssignedView {
    pub groups: Vec<Group>,
    pub total: usize,
    /// The server has more (`next_cursor`): "Show all in My Work".
    pub more: bool,
}

pub const NO_PROJECT: &str = "No project";

/// `GET /me/work?status=open` → grouped by project in the order the projects first appear (the
/// list is soonest-due first).
pub fn assigned(body: &Value, today: Option<&str>, pending: &HashMap<String, bool>) -> AssignedView {
    let rows = work_rows(&body["data"], today, pending);
    let total = rows.len();
    let mut groups: Vec<Group> = Vec::new();
    for row in rows {
        let project = row.project.clone().unwrap_or_else(|| NO_PROJECT.to_string());
        match groups.iter_mut().find(|g| g.project == project) {
            Some(g) => g.items.push(row),
            None => groups.push(Group {
                project,
                color: row.color.clone(),
                items: vec![row],
            }),
        }
    }
    AssignedView {
        groups,
        total,
        more: body["next_cursor"].as_str().is_some_and(|c| !c.is_empty()),
    }
}

// ── Notifications ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteRow {
    pub id: String,
    /// The worded line ("Ana assigned you to BRAI-12"): the server's `summary`, else the client's.
    pub text: String,
    /// Marked read offline, waiting to sync.
    pub pending: bool,
    pub body: Option<String>,
    pub key: Option<String>,
    pub context: Option<String>,
    pub read: bool,
    pub created_at: Option<String>,
    pub has_link: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotificationsView {
    pub items: Vec<NoteRow>,
    pub unread: u64,
}

/// A notification id that may go into a path: the uuid the server sent.
pub fn is_notification_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// What one notification says and where it leads (raw, checked on click): the web bell's
/// wording (`NotificationMessage.tsx`, `describeNotification`) for the known types, the payload
/// contract (title, body, url, subject_key, project_name) for the others.
pub fn describe(n: &Value) -> (String, Option<String>, Option<String>, Option<String>, Option<String>) {
    let d = &n["data"];
    let kind = n["type"].as_str().or_else(|| d["type"].as_str()).unwrap_or("");
    let s = |k: &str| clean(&d[k], MAX_TEXT);
    let actor = s("actor_name")
        .or_else(|| s("contact_name"))
        .or_else(|| s("contact_email"));
    let who = actor.clone().unwrap_or_else(|| "Someone".to_string());
    let url = |k: &str| d[k].as_str().filter(|u| !u.is_empty()).map(str::to_string);
    let key = |k: &str| clean(&d[k], 40);
    match kind {
        "task_digest" => {
            let mut parts = Vec::new();
            if let Some(n) = d["due_today"].as_u64().filter(|n| *n > 0) {
                parts.push(format!("{n} due today"));
            }
            if let Some(n) = d["overdue"].as_u64().filter(|n| *n > 0) {
                parts.push(format!("{n} overdue"));
            }
            let summary = if parts.is_empty() {
                "nothing due".to_string()
            } else {
                parts.join(" · ")
            };
            let titles = d["titles"]
                .as_array()
                .map(|t| t.iter().filter_map(|v| clean(v, 80)).collect::<Vec<_>>().join(", "))
                .filter(|t| !t.is_empty());
            (format!("Your tasks: {summary}"), titles, None, None, url("url"))
        }
        "item_reminder" => {
            let item = key("item_key");
            let when = s("when_label").map(|w| format!(" {w}")).unwrap_or_default();
            let text = format!("{}{when}", item.clone().unwrap_or_else(|| "An item".into()));
            (text, s("item_title"), item, None, url("item_url"))
        }
        "mentioned" => {
            let place = if d["context_type"] == "page" { "on page" } else { "in" };
            let title = s("context_title").unwrap_or_default();
            (
                format!("{who} mentioned you {place} {title}").trim_end().to_string(),
                None,
                None,
                None,
                url("context_url"),
            )
        }
        "page_updated" => {
            let page = s("page_name").unwrap_or_default();
            (
                format!("{who} updated page {page}").trim_end().to_string(),
                None,
                None,
                None,
                url("page_url"),
            )
        }
        "item_assigned" => {
            let item = key("item_key");
            let text = format!("{who} assigned you to {}", item.clone().unwrap_or_default());
            (
                text.trim_end().to_string(),
                s("item_title"),
                item,
                None,
                url("item_url"),
            )
        }
        "item_status_changed" => {
            let item = key("item_key");
            let text = format!(
                "{who} moved {} from {} → {}",
                item.clone().unwrap_or_default(),
                s("old_column").unwrap_or_else(|| "?".into()),
                s("new_column").unwrap_or_else(|| "?".into())
            );
            (text, s("item_title"), item, None, url("item_url"))
        }
        "ticket_created" | "ticket_reply" | "ticket_assigned" => {
            let ticket = key("ticket_key");
            let t = ticket.clone().unwrap_or_default();
            let text = match kind {
                "ticket_created" => format!("{who} opened ticket {t}"),
                "ticket_reply" => format!("{who} replied to ticket {t}"),
                _ => format!("{who} assigned you to ticket {t}"),
            };
            let body = if kind == "ticket_reply" {
                s("reply_preview").or_else(|| s("ticket_subject"))
            } else {
                s("ticket_subject")
            };
            (text.trim_end().to_string(), body, ticket, None, url("ticket_url"))
        }
        "lead_created" => {
            let number = d["lead_number"]
                .as_u64()
                .map(|n| n.to_string())
                .or_else(|| s("lead_number"))
                .unwrap_or_default();
            let from = s("lead_name").map(|n| format!(" from {n}")).unwrap_or_default();
            (
                format!("New lead #{number}{from}"),
                None,
                None,
                s("form_name"),
                url("lead_url"),
            )
        }
        _ => {
            let text = match s("title") {
                Some(title) => title,
                None => {
                    let subject = key("subject_key")
                        .or_else(|| key("item_key"))
                        .or_else(|| key("ticket_key"))
                        .or_else(|| s("page_name"));
                    match subject {
                        Some(subject) => format!("{who} updated {subject}"),
                        None => format!("{who} sent you a notification"),
                    }
                }
            };
            (text, s("body"), key("subject_key"), s("project_name"), url("url"))
        }
    }
}

/// The line a notification shows: the server's `summary` (the web bell's sentence, plain text),
/// else — an older server — the client's wording ([`describe`]).
pub fn note_text(n: &Value) -> String {
    clean(&n["summary"], MAX_TEXT).unwrap_or_else(|| describe(n).0)
}

/// Where a notification leads (raw, checked on click): the server's `url` when the row has the
/// field (`null` = nothing the user can open now), else — an older server — the payload's link.
pub fn note_url(n: &Value) -> Option<String> {
    match n.get("url") {
        Some(Value::String(u)) if !u.trim().is_empty() => Some(u.clone()),
        Some(_) => None,
        None => describe(n).4,
    }
}

/// Notifications marked read offline, not synced yet: single ones and the newest "mark all read"
/// (`before`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingReads {
    pub ids: HashSet<String>,
    pub before: Option<i64>,
}

impl PendingReads {
    pub fn of(outbox: &crate::outbox::Outbox) -> Self {
        let mut out = Self::default();
        for e in &outbox.entries {
            match &e.change {
                crate::outbox::Change::NotificationRead { notification_id, .. } => {
                    out.ids.insert(notification_id.clone());
                }
                crate::outbox::Change::NotificationsReadAll { before } => {
                    out.before = out.before.max(parse_instant(before));
                }
                _ => {}
            }
        }
        out
    }

    /// Whether row `n` (unread on the server) will be read once the outbox is sent.
    fn covers(&self, n: &Value) -> bool {
        let id = n["id"].as_str().unwrap_or("");
        let created = n["created_at"].as_str().and_then(parse_instant);
        self.ids.contains(id) || matches!((self.before, created), (Some(b), Some(c)) if c <= b)
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.before.is_none()
    }
}

/// The unread count with the waiting reads taken off: the server's count minus the shown unread
/// rows they cover (rows not loaded can't be known, so the count never goes below what is left).
pub fn unread_after(unread: u64, body: Option<&Value>, reads: &PendingReads) -> u64 {
    if reads.is_empty() {
        return unread;
    }
    let covered = body
        .and_then(|b| b["data"].as_array())
        .into_iter()
        .flatten()
        .filter(|n| n["read"] != true && reads.covers(n))
        .count() as u64;
    unread.saturating_sub(covered)
}

/// `notifications.read_all`'s `before`: the newest notification the user saw in the panel, so one
/// that arrived after stays unread; nothing shown → now.
pub fn read_all_before(body: Option<&Value>, now: i64) -> String {
    let newest = body
        .and_then(|b| b["data"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|n| n["created_at"].as_str().and_then(parse_instant))
        .max();
    crate::outbox::iso_utc(newest.unwrap_or(now))
}

/// `GET /notifications` → rows, newest first. Rows without a usable id are left out; reads
/// waiting in the outbox show as read with a pending badge.
pub fn notifications(
    body: &Value,
    unread: u64,
    reads: &PendingReads,
    has_link: impl Fn(&str) -> bool,
) -> NotificationsView {
    let items = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["id"].as_str().filter(|id| is_notification_id(id))?.to_string();
            let (_, body, key, context, _) = describe(n);
            let read = n["read"] == true;
            let pending = !read && reads.covers(n);
            Some(NoteRow {
                id,
                text: note_text(n),
                pending,
                body,
                key,
                context,
                read: read || pending,
                created_at: n["created_at"]
                    .as_str()
                    .filter(|c| parse_instant(c).is_some())
                    .map(str::to_string),
                has_link: note_url(n).as_deref().is_some_and(&has_link),
            })
        })
        .collect();
    NotificationsView {
        items,
        unread: unread_after(unread, Some(body), reads),
    }
}

/// The URL a notification leads to (checked by the caller with `links::resolve_deep_link_target`).
pub fn find_notification(body: &Value, id: &str) -> Option<(bool, Option<String>)> {
    body["data"]
        .as_array()?
        .iter()
        .find(|n| n["id"] == id && is_notification_id(id))
        .map(|n| (n["read"] == true, note_url(n)))
}

/// A notification row as the server has it now (a sync result's `notification` record).
pub fn replace_note(body: &mut Value, row: &Value) {
    let Some(id) = row["id"].as_str().filter(|id| is_notification_id(id)) else {
        return;
    };
    if let Some(items) = body["data"].as_array_mut() {
        for n in items.iter_mut().filter(|n| n["id"] == id) {
            *n = row.clone();
        }
    }
}

/// The line a waiting change shows in the problem list ("Mark read: …").
pub fn note_title(body: &Value, id: &str) -> String {
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|n| n["id"] == id)
        .map(note_text)
        .unwrap_or_default()
}

/// After "read": the row is read in the kept answer too.
pub fn mark_read(body: &mut Value, id: Option<&str>) {
    if let Some(items) = body["data"].as_array_mut() {
        for n in items.iter_mut().filter(|n| id.is_none_or(|id| n["id"] == id)) {
            n["read"] = Value::Bool(true);
        }
    }
}

// ── Timer ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Activity {
    pub id: u64,
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunningTimer {
    pub label: String,
    pub key: Option<String>,
    pub title: String,
    /// What the user is doing (`timer.description`, newer servers).
    pub description: Option<String>,
    /// Started offline, waiting to sync.
    pub pending: bool,
    /// Started offline and not on the server yet: it can be discarded, not stopped (the stop needs
    /// the server's timer id).
    pub started_offline: bool,
    pub started_at: Option<String>,
    /// Seconds at the time of the answer; the page ticks on from there.
    pub elapsed_seconds: u64,
    pub activity_required: bool,
    pub default_activity_id: Option<u64>,
    pub activities: Vec<Activity>,
    pub has_link: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EntryRow {
    pub id: String,
    pub minutes: u64,
    pub label: String,
    pub activity: Option<String>,
    pub note: Option<String>,
    pub from_timer: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub id: String,
    pub label: String,
    pub in_my_day: bool,
}

/// A stop the server refused offline for want of an activity: the inline choice.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityNeededView {
    pub message: String,
    pub label: String,
    pub activities: Vec<Activity>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimerView {
    pub running: Option<RunningTimer>,
    /// `None`: not loaded (offline without an earlier answer), or the server sends the total
    /// itself (`today_seconds`).
    pub entries: Option<Vec<EntryRow>>,
    pub total_minutes: u64,
    /// Seconds logged today, without the running timer (`today_logged_seconds`; newer servers).
    pub today_seconds: Option<u64>,
    pub candidates: Vec<Candidate>,
    /// The server takes a description when starting.
    pub description_supported: bool,
    /// A stop / discard waits in the outbox ("Stopped — logs when synced").
    pub pending_note: Option<String>,
    pub activity_needed: Option<ActivityNeededView>,
}

/// The running timer of `GET /me/timer`. The elapsed time counts from `started_at` when it can
/// (a 304 gives back an older body whose `elapsed_seconds` is stale, BRAI-440), else from the
/// answer's `elapsed_seconds` plus the seconds since it came.
pub fn running(body: &Value, now: i64, age_seconds: u64) -> Option<RunningTimer> {
    if body["running"] != true {
        return None;
    }
    let t = body.get("timer").filter(|t| t.is_object())?;
    t.get("id").filter(|id| id.is_u64())?;
    let issue = t.get("issue").filter(|i| i.is_object());
    let key = issue.and_then(|i| clean(&i["key"], 40));
    let title = issue
        .and_then(|i| clean(&i["title"], MAX_TEXT))
        .unwrap_or_else(|| "Timer".to_string());
    let started_at = t["started_at"]
        .as_str()
        .filter(|s| parse_instant(s).is_some())
        .map(str::to_string);
    let elapsed = match started_at.as_deref().and_then(parse_instant) {
        Some(start) if now >= start => (now - start) as u64,
        _ => t["elapsed_seconds"].as_u64().unwrap_or(0) + age_seconds,
    };
    Some(RunningTimer {
        label: label(key.as_deref(), &title),
        key,
        title,
        description: clean(&t["description"], 255),
        pending: false,
        started_offline: false,
        started_at,
        elapsed_seconds: elapsed,
        activity_required: body["activity_required"] == true,
        default_activity_id: body["default_activity_id"].as_u64(),
        activities: activities(body),
        has_link: issue.and_then(|i| i["url"].as_str()).is_some_and(|u| !u.is_empty()),
    })
}

/// The activities the running timer can be logged as (`/me/timer`'s `activities`).
pub fn activities(body: &Value) -> Vec<Activity> {
    body["activities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(Activity {
                id: a["id"].as_u64()?,
                name: clean(&a["name"], 80)?,
                color: safe_color(&a["color"]),
            })
        })
        .collect()
}

/// `today_logged_seconds` of `GET /me/timer` (finished logs of the user's day), `None` on an
/// older server (then `/work-logs` gives the total).
pub fn today_seconds(body: &Value) -> Option<u64> {
    body.get("today_logged_seconds").and_then(Value::as_u64)
}

/// The id of the running timer in a `GET /me/timer` answer.
pub fn timer_id(body: &Value) -> Option<u64> {
    (body["running"] == true)
        .then(|| body["timer"]["id"].as_u64())
        .flatten()
}

/// The waiting timer changes, in order: a start made offline, and the timers stopped / discarded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimerOverlay {
    /// (label, started_at, description, went out already)
    pub start: Option<(String, String, Option<String>, bool)>,
    /// Timer ids with a waiting stop (`true`) or discard (`false`).
    pub ended: Vec<(u64, bool)>,
}

impl TimerOverlay {
    pub fn of(outbox: &crate::outbox::Outbox) -> Self {
        let mut out = Self::default();
        for e in &outbox.entries {
            match &e.change {
                crate::outbox::Change::TimerStart {
                    title,
                    started_at,
                    description,
                    ..
                } => out.start = Some((title.clone(), started_at.clone(), description.clone(), e.attempts > 0)),
                crate::outbox::Change::TimerStop { timer_id, .. } => out.ended.push((*timer_id, true)),
                crate::outbox::Change::TimerDiscard { timer_id, .. } => out.ended.push((*timer_id, false)),
                _ => {}
            }
        }
        out
    }

    /// Lays the waiting changes over the server's running timer (`id`): a stopped / discarded one
    /// goes (with a note), a start made offline shows as running with a pending badge.
    pub fn apply(
        &self,
        running: Option<RunningTimer>,
        id: Option<u64>,
        now: i64,
    ) -> (Option<RunningTimer>, Option<String>) {
        let mut note = None;
        let mut running = running;
        if let Some((_, stop)) = id.and_then(|id| self.ended.iter().find(|(t, _)| *t == id)) {
            running = None;
            note = Some(
                if *stop {
                    "Timer stopped. The time is logged once Brainitive is back online."
                } else {
                    "Timer discarded. It goes away on the server once Brainitive is back online."
                }
                .to_string(),
            );
        }
        if let Some((title, started_at, description, sent)) = &self.start {
            if running.is_none() {
                let start = parse_instant(started_at);
                running = Some(RunningTimer {
                    label: title.clone(),
                    key: None,
                    title: title.clone(),
                    description: description.clone(),
                    pending: true,
                    started_offline: !sent,
                    started_at: start.map(|_| started_at.clone()),
                    elapsed_seconds: start.map(|s| (now - s).max(0) as u64).unwrap_or(0),
                    activity_required: false,
                    default_activity_id: None,
                    activities: Vec::new(),
                    has_link: false,
                });
            }
        }
        (running, note)
    }
}

/// The inline activity choice for a stop that waited for one, while it still fits what the server
/// shows: the same timer running, or nothing live to compare with (offline / not loaded).
pub fn activity_needed_view(
    needed: Option<&crate::outbox::ActivityNeeded>,
    live_timer: Option<&Value>,
) -> Option<ActivityNeededView> {
    let n = needed?;
    if live_timer.is_some_and(|b| timer_id(b) != Some(n.timer_id)) {
        return None;
    }
    Some(ActivityNeededView {
        message: if n.message.is_empty() {
            "Choose what to log this time as.".into()
        } else {
            n.message.clone()
        },
        label: n.title.chars().take(MAX_TEXT).collect(),
        activities: n
            .activities
            .iter()
            .map(|a| Activity {
                id: a.id,
                name: a.name.clone(),
                color: None,
            })
            .collect(),
    })
}

/// `GET /work-logs?from=D&to=D` → today's entries and their total.
pub fn entries(body: &Value) -> (Vec<EntryRow>, u64) {
    let rows: Vec<EntryRow> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let id = match &e["id"] {
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            let issue = e.get("issue").filter(|i| i.is_object());
            let label = match issue {
                Some(i) => label(clean(&i["key"], 40).as_deref(), &text(&i["title"])),
                None => clean(&e["project"]["name"], 80).unwrap_or_else(|| "Time entry".to_string()),
            };
            Some(EntryRow {
                id,
                minutes: e["minutes"].as_u64().unwrap_or(0),
                label,
                activity: clean(&e["activity"]["name"], 80),
                note: clean(&e["note"], 120),
                from_timer: e["source"] == "timer",
            })
        })
        .collect();
    let total = rows.iter().map(|r| r.minutes).sum();
    (rows, total)
}

/// What a timer can be started on: the issues of My Day first, then the assigned ones; open
/// only, each once (list tasks have no work log).
pub fn candidates(my_day: Option<&Value>, work: Option<&Value>) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let lists = [(my_day.map(|b| &b["items"]), true), (work.map(|b| &b["data"]), false)];
    for (items, in_my_day) in lists {
        for v in items.and_then(Value::as_array).into_iter().flatten() {
            let Some(id) = v["id"].as_str().filter(|id| id.starts_with("issue:") && is_work_id(id)) else {
                continue;
            };
            if v["done"] == true || !seen.insert(id.to_string()) {
                continue;
            }
            out.push(Candidate {
                id: id.to_string(),
                label: label(clean(&v["key"], 40).as_deref(), &text(&v["title"])),
                in_my_day,
            });
        }
    }
    out
}

// ── what the commands may act on ───────────────────────────────────────────────

/// A work item the panel showed: its label, done state and URL (raw, checked on open).
#[derive(Debug, Clone, PartialEq)]
pub struct WorkRef {
    pub label: String,
    pub done: bool,
    pub url: Option<String>,
}

/// Finds `id` in the kept answers (`/me/today`'s work and my_day, `/me/my-day`'s items,
/// `/me/work`'s data). Ids the server didn't send for this sign-in are unknown.
pub fn find_work(bodies: &[&Value], id: &str) -> Option<WorkRef> {
    if !is_work_id(id) {
        return None;
    }
    bodies.iter().find_map(|body| {
        ["work", "my_day", "items", "data"].iter().find_map(|list| {
            body[*list].as_array()?.iter().find(|v| v["id"] == id).map(|v| WorkRef {
                label: label(clean(&v["key"], 40).as_deref(), &text(&v["title"])),
                done: v["done"] == true,
                url: v["url"].as_str().filter(|u| !u.is_empty()).map(str::to_string),
            })
        })
    })
}

/// Sets `done` of `id` in a kept answer (after the server took the change).
pub fn set_done(body: &mut Value, id: &str, done: bool) {
    for list in ["work", "my_day", "items", "data"] {
        if let Some(items) = body[list].as_array_mut() {
            for v in items.iter_mut().filter(|v| v["id"] == id) {
                v["done"] = Value::Bool(done);
            }
        }
    }
}

/// An event of the kept `/me/today` answer: its page (raw, checked on open).
pub fn find_event(today: &Value, id: &str) -> Option<Option<String>> {
    today["events"]
        .as_array()?
        .iter()
        .find(|e| e["id"] == id)
        .map(|e| e["url"].as_str().filter(|u| !u.is_empty()).map(str::to_string))
}

// ── offline ────────────────────────────────────────────────────────────────────

/// Where a done tick goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoneRoute {
    /// `PATCH /me/my-day/items/{id}` or `/me/work/{id}` now.
    Api,
    /// The offline outbox (`work.done`, sent by `POST /api/v1/sync` later).
    Outbox,
}

pub fn done_route(offline: bool) -> DoneRoute {
    if offline {
        DoneRoute::Outbox
    } else {
        DoneRoute::Api
    }
}

/// A tick that failed because the server can't be reached goes to the outbox; other failures
/// (no right, gone, signed out) are shown.
pub fn queue_after(error: &CallError) -> bool {
    matches!(error, CallError::Offline(_))
}

/// Timer start / stop / discard and marking notifications read: queued in the outbox offline when
/// the server has their sync types (`queues`), else disabled with this hint (an older server).
pub fn needs_server(offline: bool, queues: bool) -> Option<&'static str> {
    (offline && !queues).then_some("You're offline. This works again once Brainitive can reach the server.")
}

/// What a timer can be started on from the offline copy (no live answer this session): its open
/// issues, My Day first.
pub fn offline_candidates(view: &offline_cache::View) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    let day = view.my_day.iter().flat_map(|d| d.items.iter()).map(|w| (w, true));
    let work = view.work.iter().map(|w| (w, false));
    day.chain(work)
        .filter(|(w, _)| w.id.starts_with("issue:") && is_work_id(&w.id) && !w.done)
        .filter(|(w, _)| seen.insert(w.id.clone()))
        .map(|(w, in_my_day)| Candidate {
            id: w.id.clone(),
            label: w.label(),
            in_my_day,
        })
        .collect()
}

/// The dot in the header.
pub fn sync_status(offline: bool, syncing: bool, pending: usize) -> &'static str {
    match (offline, syncing, pending) {
        (true, _, _) => "offline",
        (false, true, _) => "syncing",
        (false, false, 0) => "online",
        (false, false, _) => "pending",
    }
}

/// The waiting `work.done` changes of the outbox: id → the value it will have.
pub fn pending_done(outbox: &crate::outbox::Outbox) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    for e in &outbox.entries {
        if let crate::outbox::Change::WorkDone { work_id, done, .. } = &e.change {
            out.insert(work_id.clone(), *done);
        }
    }
    out
}

fn offline_row(w: &offline_cache::ViewWork, today: Option<&str>) -> Option<WorkRow> {
    is_work_id(&w.id).then(|| WorkRow {
        id: w.id.clone(),
        key: w.key.clone(),
        title: w.title.chars().take(MAX_TEXT).collect(),
        done: w.done,
        pending: w.pending,
        overdue: matches!((&w.due_date, today), (Some(d), Some(t)) if d.as_str() < t) && !w.done,
        due_date: w.due_date.clone(),
        due_time: None,
        project: w.project.clone(),
        color: None,
        timeable: w.id.starts_with("issue:"),
        has_link: false,
    })
}

/// My Day from the offline copy (the outbox already laid over it).
pub fn offline_my_day(view: &offline_cache::View) -> Option<MyDayView> {
    let day = view.my_day.as_ref()?;
    let items: Vec<WorkRow> = day
        .items
        .iter()
        .filter_map(|w| offline_row(w, Some(&day.date)))
        .collect();
    Some(MyDayView {
        date: Some(day.date.clone()),
        done: items.iter().filter(|i| i.done).count(),
        total: items.len(),
        carry_over: 0,
        items,
    })
}

/// Assigned work from the offline copy, grouped like online.
pub fn offline_assigned(view: &offline_cache::View, today: Option<&str>) -> AssignedView {
    let mut groups: Vec<Group> = Vec::new();
    let mut total = 0;
    for row in view.work.iter().filter_map(|w| offline_row(w, today)) {
        if row.done && !row.pending {
            continue;
        }
        total += 1;
        let project = row.project.clone().unwrap_or_else(|| NO_PROJECT.to_string());
        match groups.iter_mut().find(|g| g.project == project) {
            Some(g) => g.items.push(row),
            None => groups.push(Group {
                project,
                color: None,
                items: vec![row],
            }),
        }
    }
    AssignedView {
        groups,
        total,
        more: false,
    }
}

/// Today from the offline copy: open work due on or before `date` (no events: they come from
/// the calendars live).
pub fn offline_today(view: &offline_cache::View, date: Option<&str>) -> TodayView {
    let mut due: Vec<WorkRow> = view
        .work
        .iter()
        .filter(|w| matches!((&w.due_date, date), (Some(d), Some(t)) if d.as_str() <= t))
        .filter_map(|w| offline_row(w, date))
        .filter(|w| !w.done || w.pending)
        .collect();
    let total = due.len();
    due.truncate(MAX_DUE);
    TodayView {
        date: date.map(str::to_string),
        events: Vec::new(),
        more_events: 0,
        more_due: total - due.len(),
        due,
        calendar_problem: false,
        without_events: true,
    }
}
