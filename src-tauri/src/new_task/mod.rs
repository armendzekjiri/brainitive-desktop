//! The New task window (`task.html`, window `task`, `capabilities/task.json`): a new issue on a
//! board, from any app. Opened by its global shortcut (⌃⌘T / Ctrl+Alt+T, off on Linux), the
//! Brainitive key's letter T, the tray and the menu-bar panel.
//!
//! Every API call is made here with the stored app token (`Session::request`); the page gets ids,
//! names and issue keys only — never a token or a URL. Commands take only ids of the last answers
//! for this server and sign-in ([`model::draft`]), and each one checks its caller first.
//!
//! - where to: `GET /api/v1/capture-destinations` (projects with boards, every workspace)
//! - types / priorities: `GET /api/v1/projects/{id}/config`
//! - create: `POST /api/v1/boards/{id}/items` with a `request_id` (reused for a retry of the
//!   same input, so a lost answer never makes a second issue)
//! - offline: the sync API's `issue.create` has no description / type, so the page offers "Save
//!   to Inbox instead" (the capture queue)

pub mod model;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, LogicalSize, Manager, Webview};

use crate::api::ApiRequest;
use crate::popup::{self, Popup};
use crate::queue::CaptureInput;
use crate::server::ServerOrigin;
use crate::state::{state, Inner, Phase};
use model::{Config, Created, Memory, Pending, Project, Selection, TaskError, TaskInput};

/// The destinations are fetched again when older than this (opening the window shows the kept
/// ones at once and refreshes behind them).
const FRESH: Duration = Duration::from_secs(60);

/// What the window keeps between calls, for one server and one sign-in.
#[derive(Debug, Default)]
pub struct TaskState {
    kept: Option<Kept>,
    /// A create whose answer never came (offline, server error): its `request_id` is reused when
    /// the same input is sent again.
    pending: Option<Pending>,
    /// The last created issue's page, for "Open" (never sent to the page).
    created: Option<(Ticket, String)>,
}

#[derive(Debug)]
struct Kept {
    ticket: Ticket,
    projects: Vec<Project>,
    configs: HashMap<u64, Config>,
    at: Instant,
    /// A board turned out to be gone: load the list again on the next view.
    outdated: bool,
}

/// One server + one sign-in (`account_epoch` changes on every sign-in / sign-out / switch).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ticket {
    origin: ServerOrigin,
    account: u64,
}

impl Ticket {
    fn now(inner: &Inner) -> Option<Self> {
        Some(Self {
            origin: inner.current_origin()?,
            account: inner.account_epoch,
        })
    }
}

/// Opens the window (or brings it to the front: the shortcut pressed while it is open focuses it,
/// the typed text stays).
pub fn show(app: &AppHandle) {
    popup::show(app, Popup::Task);
}

/// Sign-out / forget: nothing of the account stays (the window itself is destroyed by
/// `popup::close_account_windows`).
impl TaskState {
    /// Whether anything of an account is kept (projects, a create waiting, the created issue).
    pub fn holds_answers(&self) -> bool {
        self.kept.is_some() || self.pending.is_some() || self.created.is_some()
    }

    /// Answers kept for `origin` + `account` (tests of the server switch).
    #[cfg(test)]
    pub(crate) fn kept_for_test(origin: &ServerOrigin, account: u64) -> Self {
        Self {
            kept: Some(Kept {
                ticket: Ticket {
                    origin: origin.clone(),
                    account,
                },
                projects: Vec::new(),
                configs: HashMap::new(),
                at: Instant::now(),
                outdated: false,
            }),
            ..Self::default()
        }
    }
}

pub fn clear(app: &AppHandle) {
    state(app).lock().task = TaskState::default();
}

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), TaskError> {
    popup::ensure_caller(app, webview, Popup::Task).map_err(|m| TaskError::new("not_allowed", m))
}

/// The kept answers when they belong to this server and sign-in (anything else is dropped).
fn kept<'a>(inner: &'a mut Inner, ticket: &Ticket) -> Option<&'a mut Kept> {
    if inner.task.kept.as_ref().is_some_and(|k| &k.ticket != ticket) {
        inner.task = TaskState::default();
    }
    inner.task.kept.as_mut()
}

fn memory_of(inner: &Inner, ticket: &Ticket) -> Option<Memory> {
    inner.settings.preferences.new_task.get(ticket.origin.as_str()).copied()
}

/// The server can be asked: a server is chosen and there is a sign-in for it.
async fn signed_in_ticket(app: &AppHandle) -> Result<Ticket, TaskError> {
    let (ticket, in_app) = {
        let s = state(app);
        let inner = s.lock();
        (Ticket::now(&inner), inner.phase == Phase::InApp)
    };
    let ticket = ticket.ok_or_else(|| TaskError::new("no_server", "Connect Brainitive to a server first."))?;
    if !in_app && !state(app).session.has_stored_sign_in(&ticket.origin).await {
        return Err(TaskError::new(
            "signed_out",
            format!("Sign in to {} to create tasks.", ticket.origin.display_name()),
        ));
    }
    Ok(ticket)
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    /// "ready" | "signed_out" | "no_server"
    state: &'static str,
    /// "Cloud" / "brainitive.example.com"
    server: Option<String>,
    platform: &'static str,
    /// "⌃⌘T"; empty = off.
    shortcut: String,
    /// The kept projects (empty until loaded).
    projects: Vec<Project>,
    selection: Selection,
    /// The kept list is old (or missing): the page asks `task_load`.
    stale: bool,
}

fn view(inner: &mut Inner, ticket: Option<&Ticket>, state_name: &'static str) -> TaskView {
    let shortcut = crate::shortcuts::label_of(inner.settings.preferences.task_shortcut.as_deref());
    let server = ticket.map(|t| inner.settings.label_of(&t.origin));
    let memory = ticket.and_then(|t| memory_of(inner, t));
    let (projects, stale) = match ticket.and_then(|t| kept(inner, t)) {
        Some(k) => (k.projects.clone(), k.outdated || k.at.elapsed() > FRESH),
        None => (Vec::new(), true),
    };
    TaskView {
        state: state_name,
        server,
        platform: crate::api::platform(),
        shortcut,
        selection: model::pick(&projects, memory.as_ref()),
        projects,
        stale,
    }
}

/// What the window shows at once: the state, and the kept projects with the starting choice.
#[tauri::command]
pub async fn task_view(app: AppHandle, webview: Webview) -> Result<TaskView, TaskError> {
    caller(&app, &webview)?;
    let result = signed_in_ticket(&app).await;
    let s = state(&app);
    let mut inner = s.lock();
    Ok(match result {
        Ok(ticket) => view(&mut inner, Some(&ticket), "ready"),
        Err(e) => {
            let ticket = Ticket::now(&inner);
            let name = if e.kind == "no_server" {
                "no_server"
            } else {
                "signed_out"
            };
            let mut v = view(&mut inner, ticket.as_ref(), name);
            v.projects.clear();
            v.selection = Selection::default();
            v
        }
    })
}

/// Loads the projects with boards again (`GET /capture-destinations`).
#[tauri::command]
pub async fn task_load(app: AppHandle, webview: Webview) -> Result<TaskView, TaskError> {
    caller(&app, &webview)?;
    let ticket = signed_in_ticket(&app).await?;
    let response = state(&app)
        .session
        .request(&ticket.origin, &ApiRequest::get("/api/v1/capture-destinations"))
        .await
        .map_err(|e| model::call_error(&e, &ticket.origin.display_name()))?;
    let projects = model::parse_destinations(&response.body);
    let s = state(&app);
    let mut inner = s.lock();
    if Ticket::now(&inner).as_ref() != Some(&ticket) {
        return Err(TaskError::new("signed_out", "The server or account changed meanwhile."));
    }
    // the configs are asked again too (types / priorities may have changed)
    inner.task.kept = Some(Kept {
        ticket: ticket.clone(),
        projects,
        configs: HashMap::new(),
        at: Instant::now(),
        outdated: false,
    });
    Ok(view(&mut inner, Some(&ticket), "ready"))
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigView {
    types: Vec<model::Choice>,
    priorities: Vec<model::Choice>,
    /// The type to start with (remembered, else the project's default).
    type_id: Option<u64>,
    /// The board to start with in this project (remembered, else the first).
    board_id: u64,
}

/// A project's types and priorities (`GET /projects/{id}/config`), kept per project. Only a
/// project of the kept list.
#[tauri::command]
pub async fn task_config(app: AppHandle, webview: Webview, project_id: u64) -> Result<ConfigView, TaskError> {
    caller(&app, &webview)?;
    let ticket = signed_in_ticket(&app).await?;
    let (project, cached, memory) = {
        let s = state(&app);
        let mut inner = s.lock();
        let memory = memory_of(&inner, &ticket);
        let k =
            kept(&mut inner, &ticket).ok_or_else(|| TaskError::field("project", "Pick a project from the list."))?;
        let project = k
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
            .ok_or_else(|| TaskError::field("project", "Pick a project from the list."))?;
        (project, k.configs.get(&project_id).cloned(), memory)
    };
    let config = match cached {
        Some(c) => c,
        None => {
            let path = format!("/api/v1/projects/{}/config", project.id);
            let response = state(&app)
                .session
                .request(&ticket.origin, &ApiRequest::get(path))
                .await
                .map_err(|e| model::call_error(&e, &ticket.origin.display_name()))?;
            let config = model::parse_config(&response.body);
            let s = state(&app);
            let mut inner = s.lock();
            if let Some(k) = kept(&mut inner, &ticket) {
                k.configs.insert(project.id, config.clone());
            }
            config
        }
    };
    // the remembered board / type count for the remembered project only
    let memory = memory.filter(|m| m.project_id == Some(project.id));
    Ok(ConfigView {
        type_id: model::pick_type(&config, memory.as_ref()),
        board_id: model::board_for(&project, memory.as_ref()),
        types: config.types,
        priorities: config.priorities,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatedView {
    key: Option<String>,
    title: String,
    /// "Open" works (the server sent the issue's page).
    can_open: bool,
}

/// Creates the issue (`POST /boards/{id}/items`). The ids must be of the kept answers.
#[tauri::command]
pub async fn task_create(app: AppHandle, webview: Webview, input: TaskInput) -> Result<CreatedView, TaskError> {
    caller(&app, &webview)?;
    let ticket = signed_in_ticket(&app).await?;
    let server = ticket.origin.display_name();
    let (draft, request_id) = {
        let s = state(&app);
        let mut inner = s.lock();
        let k =
            kept(&mut inner, &ticket).ok_or_else(|| TaskError::field("project", "Pick a project from the list."))?;
        let draft = model::draft(&input, &k.projects, k.configs.get(&input.project_id))?;
        let request_id = model::request_id_for(inner.task.pending.as_ref(), &draft, crate::queue::new_request_id);
        inner.task.pending = Some(Pending {
            board_id: draft.board_id,
            body: draft.body.clone(),
            request_id: request_id.clone(),
        });
        (draft, request_id)
    };
    let request = ApiRequest::post(
        format!("/api/v1/boards/{}/items", draft.board_id),
        draft.with_request_id(&request_id),
    )
    .error_body();
    let result = state(&app).session.request(&ticket.origin, &request).await;
    let s = state(&app);
    let mut inner = s.lock();
    let response = match result {
        Ok(r) => r,
        // no answer (or a server error): the pending request id stays for a retry
        Err(e) => return Err(model::call_error(&e, &server)),
    };
    if response.status >= 400 {
        // the server answered: this request id is spent, the next try gets a new one
        inner.task.pending = None;
        let e = model::error_answer(response.status, &response.body);
        if e.kind == "not_found" {
            // the board is gone: the list is loaded again on the next view
            if let Some(k) = kept(&mut inner, &ticket) {
                k.outdated = true;
            }
        }
        return Err(e);
    }
    inner.task.pending = None;
    let created: Created = model::parse_created(&response.body)
        .ok_or_else(|| TaskError::new("server", "The server sent an unexpected answer."))?;
    if Ticket::now(&inner).as_ref() == Some(&ticket) {
        let memory = Memory {
            project_id: Some(draft.project_id),
            board_id: Some(draft.board_id),
            type_id: draft.type_id,
            at: crate::outbox::now(),
        };
        model::remember(&mut inner.settings.preferences.new_task, ticket.origin.as_str(), memory);
        inner.save_settings();
        inner.task.created = created.url.clone().map(|u| (ticket.clone(), u));
    }
    Ok(CreatedView {
        can_open: inner.task.created.is_some(),
        key: created.key,
        title: created.title,
    })
}

/// "Open": the created issue in the main window (its URL from the server's answer, checked by
/// `links::resolve_deep_link_target` in `shell::open_page`).
#[tauri::command]
pub fn task_open(app: AppHandle, webview: Webview) -> Result<(), TaskError> {
    caller(&app, &webview)?;
    let url = {
        let s = state(&app);
        let inner = s.lock();
        let current = Ticket::now(&inner);
        inner
            .task
            .created
            .as_ref()
            .filter(|(t, _)| Some(t) == current.as_ref())
            .map(|(_, url)| url.clone())
    };
    let url = url.ok_or_else(|| TaskError::new("not_found", "That issue can't be opened from here any more."))?;
    popup::hide(&app, Popup::Task);
    crate::shell::open_page(&app, &url);
    Ok(())
}

/// "Save to Inbox instead" (offline, or the server can't take it now): the title and description
/// as a capture (`intended_type: issue`), sent now or kept in the capture queue.
#[tauri::command]
pub async fn task_save_to_inbox(
    app: AppHandle,
    webview: Webview,
    title: String,
    description: String,
) -> Result<crate::capture::CaptureResult, TaskError> {
    caller(&app, &webview)?;
    let text = model::inbox_text(&title, &description);
    let input = CaptureInput::new(&text, None, Some("issue")).map_err(|e| TaskError::new("invalid", e.to_string()))?;
    crate::capture::save(&app, input)
        .await
        .map_err(|e| TaskError::new(e.kind, e.message))
}

/// "Sign in to Brainitive" / "Open Brainitive": the main window.
#[tauri::command]
pub fn task_open_app(app: AppHandle, webview: Webview) -> Result<(), TaskError> {
    caller(&app, &webview)?;
    popup::hide(&app, Popup::Task);
    crate::shell::focus_main(&app);
    Ok(())
}

/// The page's content height (points): the window follows, the top stays put.
#[tauri::command]
pub fn task_resize(app: AppHandle, webview: Webview, height: f64) -> Result<(), TaskError> {
    caller(&app, &webview)?;
    if let Some(w) = app.get_webview_window(Popup::Task.label()) {
        let _ = w.set_size(LogicalSize::new(model::WIDTH, model::clamp_height(height)));
    }
    Ok(())
}

/// Esc / after "Done".
#[tauri::command]
pub fn task_close(app: AppHandle, webview: Webview) -> Result<(), TaskError> {
    caller(&app, &webview)?;
    popup::dismiss(&app, Popup::Task);
    Ok(())
}
