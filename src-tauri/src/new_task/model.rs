//! The New task window's pure logic: where an issue can go (`GET /capture-destinations`), what a
//! project offers (`GET /projects/{id}/config`), the last choice per server, the request body of
//! `POST /boards/{id}/items`, the `request_id` of a retry, and the answers mapped to what the page
//! shows. No I/O here; the commands are in `mod.rs`.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};

use crate::session::CallError;

/// Servers whose last choice is kept (the oldest goes first).
pub const MAX_MEMORY: usize = 10;
/// `StoreIssueRequest`: title ≤ 255, description ≤ 100 000.
pub const MAX_TITLE: usize = 255;
pub const MAX_DESCRIPTION: usize = 100_000;
/// Names shown in the page are cut to this.
const MAX_NAME: usize = 200;
/// The window: fixed width, the height follows the page (`task_resize`).
pub const WIDTH: f64 = 580.0;
pub const MIN_HEIGHT: f64 = 180.0;
pub const MAX_HEIGHT: f64 = 620.0;

// ── memory per server ───────────────────────────────────────────────────────────

/// The last project / board / type a task was created in on one server (`settings.json`,
/// `preferences.new_task`, keyed by the server's origin). Ids only, no names, no tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    #[serde(default)]
    pub project_id: Option<u64>,
    #[serde(default)]
    pub board_id: Option<u64>,
    #[serde(default)]
    pub type_id: Option<u64>,
    /// Unix seconds of the last use (the oldest server is dropped beyond [`MAX_MEMORY`]).
    #[serde(default)]
    pub at: u64,
}

pub type MemoryMap = BTreeMap<String, Memory>;

/// `settings.json` → the memory. Entries that don't read are skipped (never the whole file).
pub fn lenient_memory<'de, D: Deserializer<'de>>(d: D) -> Result<MemoryMap, D::Error> {
    let raw = Value::deserialize(d)?;
    let mut out = MemoryMap::new();
    if let Some(map) = raw.as_object() {
        for (origin, value) in map {
            if let Ok(memory) = serde_json::from_value::<Memory>(value.clone()) {
                out.insert(origin.clone(), memory);
            }
        }
    }
    cap(&mut out);
    Ok(out)
}

/// Remembers `memory` for `origin` (the server's origin string).
pub fn remember(map: &mut MemoryMap, origin: &str, memory: Memory) {
    map.insert(origin.to_string(), memory);
    cap(map);
}

fn cap(map: &mut MemoryMap) {
    while map.len() > MAX_MEMORY {
        let oldest = map
            .iter()
            .min_by_key(|(origin, m)| (m.at, (*origin).clone()))
            .map(|(origin, _)| origin.clone());
        match oldest {
            Some(origin) => map.remove(&origin),
            None => break,
        };
    }
}

// ── destinations ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Board {
    pub id: u64,
    pub name: String,
}

/// A project the user may create issues in (it has at least one board they may write to).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Project {
    pub id: u64,
    pub name: String,
    pub key: Option<String>,
    /// The workspace's name (the list is grouped by it when there are several).
    pub workspace: String,
    pub boards: Vec<Board>,
}

impl Project {
    pub fn has_board(&self, id: u64) -> bool {
        self.boards.iter().any(|b| b.id == id)
    }
}

fn clean(text: &str) -> String {
    let text: String = text.chars().filter(|c| !c.is_control()).collect();
    text.trim().chars().take(MAX_NAME).collect()
}

fn id_of(v: &Value) -> Option<u64> {
    v.get("id").and_then(Value::as_u64).filter(|id| *id > 0)
}

fn name_of(v: &Value, key: &str) -> String {
    clean(v.get(key).and_then(Value::as_str).unwrap_or(""))
}

/// The list to unpack: `[…]`, or `{data: […]}` when the wrapper is still there.
fn items(body: &Value) -> &[Value] {
    body.as_array()
        .or_else(|| body.get("data").and_then(Value::as_array))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// `GET /capture-destinations` → the projects with boards, in the server's order (workspaces,
/// then their projects). Projects without a board (pages / task lists only) are left out.
pub fn parse_destinations(body: &Value) -> Vec<Project> {
    let mut out: Vec<Project> = Vec::new();
    for ws in items(body) {
        let workspace = name_of(ws, "name");
        for p in ws.get("projects").and_then(Value::as_array).into_iter().flatten() {
            let Some(id) = id_of(p) else { continue };
            if out.iter().any(|o| o.id == id) {
                continue;
            }
            let boards: Vec<Board> = p
                .get("boards")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    Some(Board {
                        id: id_of(b)?,
                        name: name_of(b, "name"),
                    })
                })
                .collect();
            if boards.is_empty() {
                continue;
            }
            let key = p
                .get("key")
                .and_then(Value::as_str)
                .map(clean)
                .filter(|k| !k.is_empty());
            out.push(Project {
                id,
                name: name_of(p, "name"),
                key,
                workspace: workspace.clone(),
                boards,
            });
        }
    }
    out
}

/// Which project and board the form starts with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Selection {
    pub project_id: Option<u64>,
    pub board_id: Option<u64>,
}

/// The remembered project when it is still in the list, else the first one; its remembered
/// board when it still has it, else its first board. (The server keeps no "recent destinations"
/// list, so there is nothing else to go by.)
pub fn pick(projects: &[Project], memory: Option<&Memory>) -> Selection {
    let project = memory
        .and_then(|m| m.project_id)
        .and_then(|id| projects.iter().find(|p| p.id == id))
        .or_else(|| projects.first());
    let Some(project) = project else {
        return Selection::default();
    };
    Selection {
        project_id: Some(project.id),
        board_id: Some(board_for(project, memory)),
    }
}

/// The board of `project` to start with: the remembered one when it is in this project.
pub fn board_for(project: &Project, memory: Option<&Memory>) -> u64 {
    memory
        .and_then(|m| m.board_id)
        .filter(|id| project.has_board(*id))
        .unwrap_or(project.boards[0].id)
}

// ── a project's types and priorities ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub id: u64,
    pub name: String,
    pub slug: String,
    /// `#rgb` / `#rrggbb` only (it goes into a style), else `None`.
    pub color: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Config {
    pub types: Vec<Choice>,
    pub priorities: Vec<Choice>,
}

fn safe_color(v: Option<&Value>) -> Option<String> {
    let c = v?.as_str()?;
    let hex = c.strip_prefix('#')?;
    (matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| c.to_string())
}

fn choices(v: Option<&Value>) -> Vec<Choice> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(Choice {
                id: id_of(c)?,
                name: name_of(c, "name"),
                slug: name_of(c, "slug"),
                color: safe_color(c.get("color")),
                is_default: c.get("is_default").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

/// `GET /projects/{id}/config` → the enabled types and the priorities.
pub fn parse_config(body: &Value) -> Config {
    let body = body.get("data").filter(|d| d.is_object()).unwrap_or(body);
    Config {
        types: choices(body.get("types")),
        priorities: choices(body.get("priorities")),
    }
}

/// The type to start with: the remembered one when this project has it, else the project's
/// default, else none (the server then gives its default).
pub fn pick_type(config: &Config, memory: Option<&Memory>) -> Option<u64> {
    memory
        .and_then(|m| m.type_id)
        .filter(|id| config.types.iter().any(|t| t.id == *id))
        .or_else(|| config.types.iter().find(|t| t.is_default).map(|t| t.id))
}

// ── the create ──────────────────────────────────────────────────────────────────

/// What the page sends. Ids must come from the last answers (checked in [`draft`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TaskInput {
    pub project_id: u64,
    #[serde(default)]
    pub board_id: Option<u64>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub priority_id: Option<u64>,
    #[serde(default)]
    pub type_id: Option<u64>,
}

/// A checked create: the board and the body without its `request_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub project_id: u64,
    pub board_id: u64,
    pub type_id: Option<u64>,
    pub body: Value,
}

impl Draft {
    /// `POST /api/v1/boards/{id}/items` body with the request id.
    pub fn with_request_id(&self, request_id: &str) -> Value {
        let mut body = self.body.clone();
        body["request_id"] = json!(request_id);
        body
    }
}

/// Checks the page's input against the last answers and builds the body. The title is trimmed
/// (required); an empty description is left out; priority / type only when chosen.
pub fn draft(input: &TaskInput, projects: &[Project], config: Option<&Config>) -> Result<Draft, TaskError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(TaskError::field("title", "Give the task a title."));
    }
    if title.chars().count() > MAX_TITLE {
        return Err(TaskError::field("title", "The title can be 255 characters at most."));
    }
    let description = input.description.trim();
    if description.chars().count() > MAX_DESCRIPTION {
        return Err(TaskError::field("description", "The description is too long."));
    }
    let project = projects
        .iter()
        .find(|p| p.id == input.project_id)
        .ok_or_else(|| TaskError::field("project", "Pick a project from the list."))?;
    let board_id = match input.board_id {
        Some(id) if project.has_board(id) => id,
        Some(_) => return Err(TaskError::field("board", "Pick a board of this project.")),
        None if project.boards.len() == 1 => project.boards[0].id,
        None => return Err(TaskError::field("board", "Pick a board.")),
    };
    let known = |list: Option<&Vec<Choice>>, id: u64| list.is_some_and(|l| l.iter().any(|c| c.id == id));
    if let Some(id) = input.priority_id {
        if !known(config.map(|c| &c.priorities), id) {
            return Err(TaskError::field("priority", "Pick a priority of this project."));
        }
    }
    if let Some(id) = input.type_id {
        if !known(config.map(|c| &c.types), id) {
            return Err(TaskError::field("type", "Pick a type of this project."));
        }
    }
    let mut body = Map::new();
    body.insert("title".into(), json!(title));
    if !description.is_empty() {
        body.insert("description".into(), json!(description));
    }
    if let Some(id) = input.priority_id {
        body.insert("priority_id".into(), json!(id));
    }
    if let Some(id) = input.type_id {
        body.insert("type_id".into(), json!(id));
    }
    Ok(Draft {
        project_id: project.id,
        board_id,
        type_id: input.type_id,
        body: Value::Object(body),
    })
}

/// A create that went out but whose answer said "try again" (offline, server error): the same
/// input again reuses its `request_id`, so the server answers with the issue it may already
/// have made instead of a second one.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub board_id: u64,
    pub body: Value,
    pub request_id: String,
}

/// The `request_id` for `draft`: the pending one when the input is the same, else `fresh()`.
pub fn request_id_for(pending: Option<&Pending>, draft: &Draft, fresh: impl FnOnce() -> String) -> String {
    match pending {
        Some(p) if p.board_id == draft.board_id && p.body == draft.body => p.request_id.clone(),
        _ => fresh(),
    }
}

/// The new issue (201, or 200 for a repeated `request_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub id: u64,
    pub key: Option<String>,
    pub title: String,
    /// Kept in Rust for "Open"; never sent to the page.
    pub url: Option<String>,
}

pub fn parse_created(body: &Value) -> Option<Created> {
    let body = body.get("data").filter(|d| d.is_object()).unwrap_or(body);
    Some(Created {
        id: id_of(body)?,
        key: body
            .get("key")
            .and_then(Value::as_str)
            .map(clean)
            .filter(|k| !k.is_empty()),
        title: name_of(body, "title"),
        url: body.get("url").and_then(Value::as_str).map(str::to_string),
    })
}

/// The text of "Save to Inbox instead": the title, then the description.
pub fn inbox_text(title: &str, description: &str) -> String {
    let (title, description) = (title.trim(), description.trim());
    let text = match (title.is_empty(), description.is_empty()) {
        (_, true) => title.to_string(),
        (true, false) => description.to_string(),
        (false, false) => format!("{title}\n\n{description}"),
    };
    text.chars().take(crate::queue::MAX_TEXT).collect()
}

/// The page's content height → the window's (points).
pub fn clamp_height(height: f64) -> f64 {
    if height.is_finite() {
        height.round().clamp(MIN_HEIGHT, MAX_HEIGHT)
    } else {
        MIN_HEIGHT
    }
}

// ── errors ──────────────────────────────────────────────────────────────────────

/// What went wrong, for the page: `kind` to branch on, a sentence, per-field messages
/// (`title`, `description`, `project`, `board`, `priority`, `type`) and whether "Save to Inbox
/// instead" makes sense (the server could not be asked).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskError {
    /// invalid | limit | forbidden | not_found | conflict | offline | server | rate_limited |
    /// signed_out | no_server | not_allowed
    pub kind: &'static str,
    pub message: String,
    pub fields: BTreeMap<String, String>,
    pub inbox: bool,
}

impl TaskError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            fields: BTreeMap::new(),
            inbox: false,
        }
    }

    pub fn field(field: &str, message: &str) -> Self {
        let mut e = Self::new("invalid", message);
        e.fields.insert(field.to_string(), message.to_string());
        e
    }

    fn with_inbox(mut self) -> Self {
        self.inbox = true;
        self
    }
}

/// The server's field name → the form's.
fn form_field(server: &str) -> &'static str {
    match server {
        "title" => "title",
        "description" => "description",
        "type_id" => "type",
        "priority_id" => "priority",
        "column_id" | "board" | "board_id" => "board",
        _ => "form",
    }
}

fn first_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.first().and_then(first_text),
        _ => None,
    }
}

fn server_message(body: &Value) -> Option<String> {
    body.get("message")
        .and_then(Value::as_str)
        .map(|m| m.chars().take(300).collect::<String>())
        .filter(|m| !m.trim().is_empty())
}

/// A 4xx answer of the create (its body kept by `ApiRequest::error_body`).
pub fn error_answer(status: u16, body: &Value) -> TaskError {
    match status {
        422 => {
            let mut e = TaskError::new(
                "invalid",
                server_message(body).unwrap_or_else(|| "Check the highlighted fields.".into()),
            );
            if let Some(errors) = body.get("errors").and_then(Value::as_object) {
                for (field, messages) in errors {
                    let Some(text) = first_text(messages) else { continue };
                    let text: String = text.chars().take(300).collect();
                    let form = form_field(field.split('.').next().unwrap_or(field));
                    e.fields.entry(form.to_string()).or_insert(text);
                }
            }
            // one field error: that sentence is the message
            if let Some(only) = e.fields.values().next().filter(|_| e.fields.len() == 1) {
                e.message = only.clone();
            }
            e
        }
        402 => TaskError::new(
            "limit",
            server_message(body).unwrap_or_else(|| "This workspace has reached a limit of its plan.".into()),
        ),
        403 => TaskError::new("forbidden", "You can't create issues on this board."),
        404 => TaskError::new(
            "not_found",
            "This board is gone or you can't open it any more. The list was reloaded.",
        ),
        409 => TaskError::new("conflict", "That didn't go through. Try again."),
        408 | 429 => TaskError::new("rate_limited", "The server is busy. Try again in a moment.").with_inbox(),
        _ => TaskError::new(
            "server",
            server_message(body).unwrap_or_else(|| format!("The server answered with an error ({status}).")),
        ),
    }
}

/// A failed call (no answer, signed out, or an error status without a kept body).
pub fn call_error(e: &CallError, server: &str) -> TaskError {
    match e {
        CallError::SignedOut(_) => TaskError::new("signed_out", format!("Sign in to {server} again.")),
        CallError::Offline(_) => TaskError::new(
            "offline",
            "You're offline. Issues can't be created offline yet: save it to your inbox instead.",
        )
        .with_inbox(),
        CallError::Failed(_) => {
            TaskError::new("server", "The server could not be reached properly. Try again.").with_inbox()
        }
        CallError::Server { status, .. } if *status >= 500 => TaskError::new(
            "server",
            "The server has a problem right now. Try again, or save it to your inbox.",
        )
        .with_inbox(),
        CallError::Server { status, .. } => error_answer(*status, &Value::Null),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
