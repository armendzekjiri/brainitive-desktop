//! The quick switcher (BRAI-401): a Spotlight-like window on a second global shortcut, backed by
//! `GET /api/v1/search` (the web's search: ranking, visibility, typo tolerance). Enter jumps to
//! the issue / doc / plan in the app window; on a snippet it copies the content
//! (`GET /api/v1/snippets/{id}` → the clipboard, in Rust); on an issue it starts the timer
//! (`POST /api/v1/me/timer/start`). All calls use the stored app token from Rust.

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Webview};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::api::ApiRequest;
use crate::flows::UiError;
use crate::popup::{self, Popup};
use crate::server::ServerOrigin;
use crate::session::CallError;
use crate::state::{state, Phase};

const LIMIT: &str = "12";
/// The `type` filters the window offers (search group keys).
pub const FILTERS: [&str; 4] = ["issues", "docs", "plans", "snippets"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub id: String,
    /// item | page | plan | snippet | …
    pub kind: String,
    /// "Issue", "Doc" …
    pub label: String,
    pub title: String,
    pub key: Option<String>,
    /// Where it lives: "Website · Main Board", else the project / workspace.
    pub context: Option<String>,
    pub url: Option<String>,
    pub source_id: u64,
    pub can_copy: bool,
    pub can_time: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Place {
    pub kind: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SearchView {
    pub results: Vec<Hit>,
    pub places: Vec<Place>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwitcherContext {
    server: Option<String>,
    signed_in: bool,
    platform: &'static str,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(|s| s.chars().filter(|c| !c.is_control()).take(300).collect::<String>())
        .filter(|s| !s.trim().is_empty())
}

/// Reads a `GET /search` answer; only links to the current server are kept.
pub fn parse_search(origin: &ServerOrigin, body: &Value) -> SearchView {
    let link = |v: &Value| {
        text(v, "url")
            .and_then(|u| crate::links::resolve_on_instance(origin, &u))
            .map(|u| u.to_string())
    };
    let results = body
        .get("results")
        .or_else(|| body.get("data"))
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let kind = text(r, "type")?;
                    let source_id = r.get("source_id").and_then(Value::as_u64)?;
                    let context = text(r, "context").or_else(|| {
                        r.get("project")
                            .and_then(|p| text(p, "name"))
                            .or_else(|| text(r, "workspace"))
                    });
                    Some(Hit {
                        id: text(r, "id").unwrap_or_else(|| format!("{kind}:{source_id}")),
                        label: text(r, "label").unwrap_or_else(|| kind.clone()),
                        title: text(r, "title").unwrap_or_else(|| "Untitled".into()),
                        key: text(r, "key"),
                        context,
                        url: link(r),
                        source_id,
                        can_copy: kind == "snippet",
                        can_time: kind == "item",
                        kind,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let places = body
        .get("places")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|p| {
                    Some(Place {
                        kind: text(p, "type").unwrap_or_default(),
                        title: text(p, "title")?,
                        subtitle: text(p, "subtitle"),
                        url: link(p)?,
                    })
                })
                .take(5)
                .collect()
        })
        .unwrap_or_default();
    SearchView {
        results,
        places,
        total: body.get("total").and_then(Value::as_u64).unwrap_or(0),
    }
}

/// `q` the search needs: 2–300 characters (one letter finds nothing).
pub fn search_query(q: &str) -> Option<String> {
    let q: String = q.chars().filter(|c| !c.is_control()).collect();
    let q = q.trim();
    (q.chars().count() >= 2).then(|| q.chars().take(300).collect())
}

fn signed_in_origin(app: &AppHandle) -> Result<ServerOrigin, UiError> {
    let s = state(app);
    let inner = s.lock();
    let origin = inner
        .current_origin()
        .ok_or_else(|| ui("no_server", "Connect Brainitive to a server first."))?;
    if inner.phase != Phase::InApp {
        return Err(ui("signed_out", format!("Sign in to {} first.", origin.display_name())));
    }
    Ok(origin)
}

fn ui(kind: &'static str, message: impl Into<String>) -> UiError {
    UiError {
        kind,
        message: message.into(),
    }
}

fn call_error(e: CallError) -> UiError {
    match e {
        CallError::Offline(_) => ui("network", "You're offline."),
        CallError::SignedOut(m) => ui("signed_out", m),
        e => ui("server_error", e.to_string()),
    }
}

fn caller(app: &AppHandle, webview: &Webview) -> Result<(), UiError> {
    popup::ensure_caller(app, webview, Popup::Switcher).map_err(|m| ui("not_allowed", m))
}

#[tauri::command]
pub fn switcher_context(app: AppHandle, webview: Webview) -> Result<SwitcherContext, UiError> {
    caller(&app, &webview)?;
    let s = state(&app);
    let inner = s.lock();
    Ok(SwitcherContext {
        server: inner.current_origin().map(|o| o.display_name()),
        signed_in: inner.phase == Phase::InApp,
        platform: crate::api::platform(),
    })
}

#[tauri::command]
pub async fn switcher_search(
    app: AppHandle,
    webview: Webview,
    q: String,
    filter: Option<String>,
) -> Result<SearchView, UiError> {
    caller(&app, &webview)?;
    let Some(q) = search_query(&q) else {
        return Ok(SearchView::default());
    };
    let origin = signed_in_origin(&app)?;
    let mut request = ApiRequest::get("/api/v1/search").query("q", q).query("limit", LIMIT);
    if let Some(f) = filter.as_deref().filter(|f| FILTERS.contains(f)) {
        request = request.query("type", f);
    }
    let response = state(&app)
        .session
        .request(&origin, &request)
        .await
        .map_err(call_error)?;
    // the list answer is `{data: [...], next_cursor, total, counts, places}`: not unwrapped
    Ok(parse_search(&origin, &response.body))
}

/// Enter: the page in the app window (the switcher closes without hiding the app).
#[tauri::command]
pub fn switcher_open(app: AppHandle, webview: Webview, url: String) -> Result<(), UiError> {
    caller(&app, &webview)?;
    let origin = signed_in_origin(&app)?;
    let page = crate::links::resolve_deep_link_target(&origin, &url)
        .ok_or_else(|| ui("invalid", "Not a page of this server."))?;
    popup::hide(&app, Popup::Switcher);
    crate::shell::navigate_to(&app, &origin, page);
    Ok(())
}

/// Copies a snippet's content to the clipboard; the switcher then hands the focus back to the
/// app the user came from, ready to paste.
#[tauri::command]
pub async fn switcher_copy_snippet(app: AppHandle, webview: Webview, id: u64) -> Result<String, UiError> {
    caller(&app, &webview)?;
    let origin = signed_in_origin(&app)?;
    let response = state(&app)
        .session
        .request(&origin, &ApiRequest::get(format!("/api/v1/snippets/{id}")))
        .await
        .map_err(call_error)?;
    let content = response.body.get("content").and_then(Value::as_str).unwrap_or("");
    app.clipboard()
        .write_text(content)
        .map_err(|e| ui("clipboard", format!("The clipboard is not available: {e}")))?;
    let title = text(&response.body, "title").unwrap_or_else(|| "Snippet".into());
    Ok(format!("Copied “{title}”."))
}

/// Starts the time-tracking timer on an issue (the web's timer: one per user).
#[tauri::command]
pub async fn switcher_start_timer(app: AppHandle, webview: Webview, issue_id: u64) -> Result<String, UiError> {
    caller(&app, &webview)?;
    let origin = signed_in_origin(&app)?;
    let request = ApiRequest::post("/api/v1/me/timer/start", json!({ "id": format!("issue:{issue_id}") }));
    match state(&app).session.request(&origin, &request).await {
        Ok(_) => {
            state(&app).poll_now.notify_one();
            Ok("Timer started. Stop it from the menu bar / tray or the web app.".into())
        }
        Err(CallError::Server { status: 409, .. }) => Err(ui(
            "conflict",
            "Another timer is running. Stop it first (tray / menu bar or the web app).",
        )),
        Err(CallError::Server { status: 403, .. }) => Err(ui("forbidden", "You can't log time on this issue.")),
        Err(e) => Err(call_error(e)),
    }
}

#[tauri::command]
pub fn switcher_close(app: AppHandle, webview: Webview) -> Result<(), UiError> {
    caller(&app, &webview)?;
    popup::dismiss(&app, Popup::Switcher);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_need_two_characters() {
        assert_eq!(search_query(" a "), None);
        assert_eq!(search_query("ab").as_deref(), Some("ab"));
        assert_eq!(search_query("  BRAI-12 ").as_deref(), Some("BRAI-12"));
        assert_eq!(search_query(&"x".repeat(400)).unwrap().len(), 300);
    }

    #[test]
    fn parses_results_and_drops_links_to_other_sites() {
        let origin = ServerOrigin::parse("brainitive.example.com").unwrap();
        let body = json!({
            "data": [
                {"id": "item:12", "type": "item", "group": "issues", "label": "Issue", "source_id": 12,
                 "key": "BRAI-12", "title": "Fix login", "context": "Website · Main Board",
                 "url": "https://brainitive.example.com/workspaces/a/projects/b/boards/c/items/12"},
                {"id": "snippet:4", "type": "snippet", "label": "Snippet", "source_id": 4, "title": "Signature",
                 "workspace": "Acme", "url": "/workspaces/a/snippets/x"},
                {"id": "bookmark:1", "type": "bookmark", "label": "Bookmark", "source_id": 1, "title": "Evil",
                 "url": "https://evil.example.com/"},
                {"type": "page", "title": "no source id"}
            ],
            "total": 3,
            "places": [
                {"type": "project", "id": 3, "title": "Website", "subtitle": "Acme", "url": "/workspaces/a/projects/b"},
                {"type": "project", "id": 4, "title": "Elsewhere", "url": "https://evil.example.com/x"}
            ]
        });
        let view = parse_search(&origin, &body);
        assert_eq!(view.total, 3);
        assert_eq!(view.results.len(), 3);
        let issue = &view.results[0];
        assert!(issue.can_time && !issue.can_copy);
        assert_eq!(issue.key.as_deref(), Some("BRAI-12"));
        assert_eq!(issue.context.as_deref(), Some("Website · Main Board"));
        let snippet = &view.results[1];
        assert!(snippet.can_copy && !snippet.can_time);
        assert_eq!(snippet.context.as_deref(), Some("Acme"));
        assert_eq!(
            snippet.url.as_deref(),
            Some("https://brainitive.example.com/workspaces/a/snippets/x")
        );
        assert_eq!(view.results[2].url, None, "a link to another site is dropped");
        assert_eq!(view.places.len(), 1);
        assert_eq!(view.places[0].title, "Website");
        assert_eq!(parse_search(&origin, &json!({})), SearchView::default());
    }
}
