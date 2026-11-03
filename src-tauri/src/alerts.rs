//! Native notifications for the server's notifications (mentions, assignments, replies …).
//!
//! The web app shows them in its bell only; the desktop app doesn't wait for it: the tray poller
//! already asks for the unread count every minute ([`crate::tray`]). When the count rises, this
//! module reads the newest notifications and shows each one it hasn't shown yet as an OS
//! notification (a Windows toast from the package identity in the Store build). A click opens the
//! notification's page in the active tab. It works with the main window hidden in the tray.
//!
//! The first answer after a sign-in only marks what is there as seen: no flood of old ones. Nothing
//! is shown while the main window is in front (the page shows them itself). Text is the panel's
//! wording (`panel::view`); the link goes through `links::resolve_deep_link_target` on click.

use std::collections::{HashSet, VecDeque};

use serde_json::Value;
use tauri::AppHandle;

use crate::api::ApiRequest;
use crate::panel::view;
use crate::state::state;
use crate::tray::PollTicket;

/// How many of the newest notifications are read when the count rises.
const PAGE: &str = "10";
/// More new ones than this at once: one summary instead of a toast each.
const MAX_SINGLE: usize = 3;
/// Ids remembered (oldest forgotten first).
const REMEMBERED: usize = 500;

/// What was already seen or shown for the current sign-in (reset with the tray's data).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Seen {
    /// The unread count of the last poll this module looked at (`None`: not yet).
    last_unread: Option<u64>,
    ids: HashSet<String>,
    order: VecDeque<String>,
}

/// One OS notification to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
    /// The page (raw, checked with `links::resolve_deep_link_target` before use).
    pub url: Option<String>,
}

/// What a poll's unread count asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Nothing new.
    Skip,
    /// The first count since the sign-in with unread ones: read them, show none.
    Prime,
    /// The count rose: read the newest, show the unseen unread ones.
    Show,
}

impl Seen {
    /// Called with every polled unread count.
    pub fn look(&mut self, unread: u64) -> Look {
        let before = self.last_unread.replace(unread);
        match before {
            None if unread > 0 => Look::Prime,
            None => Look::Skip,
            Some(b) if unread > b => Look::Show,
            Some(_) => Look::Skip,
        }
    }

    fn remember(&mut self, id: &str) -> bool {
        if !self.ids.insert(id.to_string()) {
            return false;
        }
        self.order.push_back(id.to_string());
        while self.order.len() > REMEMBERED {
            if let Some(old) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
        true
    }

    /// `GET /notifications` (newest first) → the unread ones not seen before, newest first;
    /// all of them are remembered. `prime`: remember only.
    pub fn fresh(&mut self, body: &Value, prime: bool) -> Vec<Alert> {
        let mut out = Vec::new();
        for n in body["data"].as_array().into_iter().flatten() {
            let Some(id) = n["id"].as_str().filter(|id| view::is_notification_id(id)) else {
                continue;
            };
            if !self.remember(id) || prime || n["read"] == true {
                continue;
            }
            let (_, detail, _, context, _) = view::describe(n);
            out.push(Alert {
                title: view::note_text(n),
                body: detail.or(context).unwrap_or_default(),
                url: view::note_url(n),
            });
        }
        out
    }
}

/// The toasts for `alerts`: one each, or one summary for many.
pub fn toasts(alerts: Vec<Alert>) -> Vec<Alert> {
    if alerts.len() <= MAX_SINGLE {
        return alerts;
    }
    vec![Alert {
        title: format!("{} new notifications", alerts.len()),
        body: alerts.first().map(|a| a.title.clone()).unwrap_or_default(),
        url: Some("/notifications".into()),
    }]
}

/// The tray poller's unread count for `ticket` (still current): shows what is new.
pub async fn on_unread(app: &AppHandle, ticket: &PollTicket, unread: u64) {
    let look = state(app).lock().alerts.look(unread);
    if look == Look::Skip {
        return;
    }
    let request = ApiRequest::get("/api/v1/notifications").query("limit", PAGE);
    let body = match crate::tray::get_cached_request(app, ticket, request).await {
        Ok(Some(body)) => body,
        Ok(None) => return,
        Err(e) => {
            log::info!("notifications not read: {e}");
            return;
        }
    };
    let alerts = {
        let s = state(app);
        let mut inner = s.lock();
        if !ticket.still_current(
            inner.current_origin().as_ref(),
            inner.account_epoch,
            inner.phase == crate::state::Phase::InApp,
        ) {
            return;
        }
        inner.alerts.fresh(&body, look == Look::Prime)
    };
    // counts only: never the text (the log can be shared)
    let front = in_front(app);
    log::info!(
        "notifications: unread {unread}, {look:?}, {} new{}",
        alerts.len(),
        if front && !alerts.is_empty() {
            " (window in front: not shown)"
        } else {
            ""
        }
    );
    if alerts.is_empty() || front {
        return;
    }
    for alert in toasts(alerts) {
        let target = alert
            .url
            .as_deref()
            .and_then(|u| crate::links::resolve_deep_link_target(&ticket.origin, u));
        crate::bridge::show_notification(app, &alert.title, &alert.body, target);
    }
}

/// The main window is on screen and has the focus: the page shows its notifications itself.
fn in_front(app: &AppHandle) -> bool {
    crate::shell::main_window(app).is_some_and(|w| {
        w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false) && w.is_focused().unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn note(id: &str, read: bool, summary: &str) -> Value {
        json!({"id": id, "read": read, "summary": summary, "type": "mentioned",
               "data": {"actor_name": "Ana", "context_title": "Plan"}, "url": "/pages/1"})
    }

    #[test]
    fn the_first_count_primes_and_only_a_rise_shows() {
        let mut s = Seen::default();
        assert_eq!(s.look(0), Look::Skip);
        assert_eq!(s.look(0), Look::Skip);
        assert_eq!(s.look(2), Look::Show);
        assert_eq!(s.look(1), Look::Skip);
        assert_eq!(s.look(1), Look::Skip);

        let mut s = Seen::default();
        assert_eq!(s.look(4), Look::Prime);
        assert_eq!(s.look(5), Look::Show);
    }

    #[test]
    fn only_unseen_unread_ones_are_shown_once() {
        let mut s = Seen::default();
        let first = json!({"data": [note("a1", false, "old one"), note("b2", true, "read one")]});
        assert!(s.fresh(&first, true).is_empty());

        let next = json!({"data": [
            note("c3", false, "Ana mentioned you on page Plan"),
            note("d4", true, "read already"),
            note("a1", false, "old one"),
        ]});
        let shown = s.fresh(&next, false);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title, "Ana mentioned you on page Plan");
        assert_eq!(shown[0].url.as_deref(), Some("/pages/1"));
        assert!(s.fresh(&next, false).is_empty());
    }

    #[test]
    fn rows_without_a_usable_id_are_skipped() {
        let mut s = Seen::default();
        let body = json!({"data": [note("../x", false, "bad"), note("", false, "empty"), json!({"read": false})]});
        assert!(s.fresh(&body, false).is_empty());
    }

    #[test]
    fn many_at_once_become_one_summary() {
        let few: Vec<Alert> = (0..3)
            .map(|i| Alert {
                title: format!("n{i}"),
                body: String::new(),
                url: None,
            })
            .collect();
        assert_eq!(toasts(few.clone()), few);
        let many: Vec<Alert> = (0..5)
            .map(|i| Alert {
                title: format!("n{i}"),
                body: String::new(),
                url: None,
            })
            .collect();
        let t = toasts(many);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].title, "5 new notifications");
        assert_eq!(t[0].body, "n0");
        assert_eq!(t[0].url.as_deref(), Some("/notifications"));
    }

    #[test]
    fn the_remembered_ids_are_bounded() {
        let mut s = Seen::default();
        for i in 0..(REMEMBERED + 20) {
            s.remember(&format!("{i:x}"));
        }
        assert_eq!(s.ids.len(), REMEMBERED);
        assert_eq!(s.order.len(), REMEMBERED);
        assert!(!s.ids.contains("0"));
    }
}
