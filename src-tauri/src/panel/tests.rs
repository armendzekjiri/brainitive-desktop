//! The panel's pure parts: tray clicks, the toggle guard, each tab's mapping, the ids commands
//! may act on, and the offline decisions. Geometry has its own tests (`geometry.rs`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::view::{self, Tab};
use super::*;
use crate::outbox::{Change, Outbox};

fn none() -> HashMap<String, bool> {
    HashMap::new()
}

fn work(id: &str, key: Option<&str>, title: &str, done: bool, due: Option<&str>, project: Option<&str>) -> Value {
    json!({
        "id": id, "type": if id.starts_with("issue:") { "issue" } else { "task" },
        "key": key, "title": title, "done": done, "status": "To do", "priority": null,
        "due_date": due, "due_time": null,
        "project": project.map(|p| json!({"id": 1, "name": p, "key": "P", "color": "#22AA44"})),
        "workspace": {"slug": "acme", "name": "Acme"},
        "source": {"type": "board", "id": 3, "name": "Main"},
        "url": format!("https://brain.example.com/w/{id}"),
        "updated_at": "2026-11-02T08:00:00Z"
    })
}

// ── clicks ─────────────────────────────────────────────────────────────────────

#[test]
fn left_click_toggles_right_click_and_control_click_open_the_menu() {
    use super::{click_action as c, Button::*, ClickAction::*};
    // macOS: on mouse-down, like the system's menu-bar extras; ⌃-click is a right click
    assert_eq!(c("macos", Left, true, false), Some(TogglePanel));
    assert_eq!(c("macos", Left, false, false), None, "the up of the same click");
    assert_eq!(c("macos", Left, true, true), Some(ShowMenu));
    assert_eq!(
        c("macos", Right, true, false),
        Some(HidePanel),
        "the menu opens by itself"
    );
    // Windows: on mouse-up
    assert_eq!(c("windows", Left, false, false), Some(TogglePanel));
    assert_eq!(c("windows", Left, true, false), None);
    assert_eq!(c("windows", Right, true, false), Some(HidePanel));
    // Linux: no click events (the menu has "Open Panel…"), other buttons: nothing
    assert_eq!(c("linux", Left, true, false), None);
    assert_eq!(c("macos", Other, true, false), None);
}

#[test]
fn the_click_that_blurred_the_panel_does_not_reopen_it() {
    let now = Instant::now();
    assert_eq!(toggle_decision(true, None, now), Toggle::Hide);
    assert_eq!(toggle_decision(false, None, now), Toggle::Show);
    let just = now.checked_sub(Duration::from_millis(120)).unwrap();
    assert_eq!(toggle_decision(false, Some(just), now), Toggle::Ignore);
    let earlier = now.checked_sub(Duration::from_secs(3)).unwrap();
    assert_eq!(toggle_decision(false, Some(earlier), now), Toggle::Show);
}

// ── tabs ───────────────────────────────────────────────────────────────────────

#[test]
fn tabs_round_trip_and_an_unknown_remembered_tab_is_today() {
    for t in Tab::ALL {
        assert_eq!(Tab::parse(t.as_str()), Some(t));
    }
    assert_eq!(Tab::remembered("timer"), Tab::Timer);
    assert_eq!(Tab::remembered(""), Tab::Today);
    assert_eq!(Tab::remembered("../etc"), Tab::Today);
    assert_eq!(Tab::parse("Today"), None);
    assert_eq!(crate::settings::Preferences::default().panel_tab, "today");
}

#[test]
fn instants_parse_with_fractions_and_offsets() {
    assert_eq!(view::parse_instant("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(view::parse_instant("2026-11-02T09:16:04Z"), Some(1_793_610_964));
    assert_eq!(view::parse_instant("2026-11-02T09:16:04.123456Z"), Some(1_793_610_964));
    assert_eq!(view::parse_instant("2026-11-02T10:16:04+01:00"), Some(1_793_610_964));
    assert_eq!(view::parse_instant("2026-11-02T04:16:04-0500"), Some(1_793_610_964));
    assert_eq!(view::parse_instant("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    for bad in [
        "",
        "2026-11-02",
        "2026-13-02T00:00:00Z",
        "2026-11-02T25:00:00Z",
        "2026-11-02T09:16:04+1",
        "x",
    ] {
        assert_eq!(view::parse_instant(bad), None, "{bad}");
    }
}

#[test]
fn today_shows_the_next_events_and_the_work_due() {
    let now = view::parse_instant("2026-11-02T10:00:00Z").unwrap();
    let body = json!({
        "date": "2026-11-02", "timezone": "Europe/Berlin",
        "work": [
            work("issue:1", Some("BRAI-1"), "Overdue thing", false, Some("2026-10-30"), Some("Website")),
            work("task:2", None, "Due today", false, Some("2026-11-02"), Some("Home")),
            work("issue:3", Some("BRAI-3"), "Done already", true, Some("2026-11-02"), Some("Website")),
            {"id": "evil/../1", "title": "no"}
        ],
        "my_day": [],
        "events": [
            {"id": "event:1:b", "type": "event", "title": "Standup", "all_day": false,
             "start": "2026-11-02T09:30:00Z", "end": "2026-11-02T10:15:00Z", "location": "Room 1",
             "conference_url": "https://meet.google.com/x", "color": "#4285F4", "url": null},
            {"id": "event:1:a", "type": "event", "title": "Breakfast", "all_day": false,
             "start": "2026-11-02T07:00:00Z", "end": "2026-11-02T08:00:00Z"},
            {"id": "event:1:c", "type": "event", "title": "Review", "all_day": false,
             "start": "2026-11-02T14:00:00Z", "end": "2026-11-02T15:00:00Z", "color": "red; background:url(x)",
             "conference_url": "javascript:alert(1)"},
            {"id": "booking:9", "type": "booking", "title": "Holiday", "all_day": true,
             "start": "2026-11-02", "end": "2026-11-02", "url": "/bookings/9"}
        ],
        "errors": [{"connection_id": 4, "message": "token expired"}]
    });
    let v = view::today(&body, now, &none());
    let titles: Vec<_> = v.events.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Holiday", "Standup", "Review"],
        "all-day first, the ended one left out"
    );
    assert!(v.events[1].now && v.events[1].call);
    assert_eq!(v.events[1].color.as_deref(), Some("#4285f4"));
    assert!(!v.events[2].now && !v.events[2].call, "a javascript: link is no call");
    assert_eq!(v.events[2].color, None, "only hex colours reach the page");
    assert!(v.events[0].has_link && !v.events[1].has_link);
    assert!(v.calendar_problem);
    let due: Vec<_> = v.due.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(due, ["issue:1", "task:2"], "done and broken ids left out");
    assert!(v.due[0].overdue && !v.due[1].overdue);
    assert!(v.due[0].timeable && !v.due[1].timeable);
    assert_eq!(v.due[0].project.as_deref(), Some("Website"));
    assert_eq!(v.date.as_deref(), Some("2026-11-02"));
}

#[test]
fn many_events_and_due_items_are_cut_with_a_count() {
    let now = 0;
    let events: Vec<Value> = (0..12)
        .map(|i| {
            json!({"id": format!("event:1:{i}"), "title": "E", "all_day": false,
            "start": format!("2026-11-02T{:02}:00:00Z", 8 + i), "end": format!("2026-11-02T{:02}:30:00Z", 8 + i)})
        })
        .collect();
    let items: Vec<Value> = (0..40)
        .map(|i| work(&format!("task:{i}"), None, "T", false, Some("2026-11-02"), None))
        .collect();
    let v = view::today(
        &json!({"date": "2026-11-02", "events": events, "work": items}),
        now,
        &none(),
    );
    assert_eq!((v.events.len(), v.more_events), (view::MAX_EVENTS, 4));
    assert_eq!((v.due.len(), v.more_due), (view::MAX_DUE, 10));
}

#[test]
fn my_day_counts_done_and_shows_queued_ticks() {
    let body = json!({"date": "2026-11-02", "carry_over_count": 3, "items": [
        work("issue:12", Some("BRAI-12"), "Fix login", false, None, Some("Website")),
        work("task:7", None, "Call Anna", true, None, None),
    ]});
    let v = view::my_day(&body, &none());
    assert_eq!((v.done, v.total, v.carry_over), (1, 2, 3));
    assert_eq!(v.items[0].key.as_deref(), Some("BRAI-12"));
    assert!(v.items[0].has_link);

    let pending = HashMap::from([("issue:12".to_string(), true)]);
    let v = view::my_day(&body, &pending);
    assert!(v.items[0].done && v.items[0].pending, "ticked offline, waiting to sync");
    assert_eq!(v.done, 2);
}

#[test]
fn assigned_work_is_grouped_by_project_in_list_order() {
    let body = json!({"data": [
        work("issue:1", Some("WEB-1"), "A", false, Some("2026-11-01"), Some("Website")),
        work("task:2", None, "B", false, None, Some("Home")),
        work("issue:3", Some("WEB-3"), "C", false, None, Some("Website")),
        work("task:4", None, "D", false, None, None),
    ], "next_cursor": "abc"});
    let v = view::assigned(&body, Some("2026-11-02"), &none());
    let groups: Vec<_> = v.groups.iter().map(|g| (g.project.as_str(), g.items.len())).collect();
    assert_eq!(groups, [("Website", 2), ("Home", 1), (view::NO_PROJECT, 1)]);
    assert_eq!(v.groups[0].color.as_deref(), Some("#22aa44"));
    assert!(v.groups[0].items[0].overdue);
    assert_eq!(v.total, 4);
    assert!(v.more);
    assert!(!view::assigned(&json!({"data": [], "next_cursor": null}), None, &none()).more);
}

#[test]
fn notifications_are_worded_like_the_web_bell() {
    let n = |kind: &str, data: Value| {
        json!({"id": "9b1c2d3e-0000-4000-8000-000000000001", "type": kind,
        "category": null, "data": data, "read": false, "read_at": null, "archived_at": null,
        "created_at": "2026-11-02T09:00:00Z"})
    };
    let text = |v: Value| view::describe(&v).0;
    assert_eq!(
        text(n(
            "item_assigned",
            json!({"actor_name": "Ana", "item_key": "BRAI-12", "item_title": "Fix login", "item_url": "/x"})
        )),
        "Ana assigned you to BRAI-12"
    );
    assert_eq!(
        text(n(
            "mentioned",
            json!({"actor_name": "Ana", "context_type": "page", "context_title": "Plan", "context_url": "/p"})
        )),
        "Ana mentioned you on page Plan"
    );
    assert_eq!(
        text(n(
            "item_status_changed",
            json!({"item_key": "BRAI-1", "old_column": "To do", "new_column": "Done"})
        )),
        "Someone moved BRAI-1 from To do → Done"
    );
    assert_eq!(
        text(n("task_digest", json!({"due_today": 2, "overdue": 1}))),
        "Your tasks: 2 due today · 1 overdue"
    );
    assert_eq!(
        text(n(
            "ticket_reply",
            json!({"contact_name": "Bob", "ticket_key": "HD-4", "reply_preview": "Thanks"})
        )),
        "Bob replied to ticket HD-4"
    );
    assert_eq!(
        text(n("lead_created", json!({"lead_number": 17, "lead_name": "Acme"}))),
        "New lead #17 from Acme"
    );
    // contract types: the server's title, else a generic line
    assert_eq!(
        text(n("release_published", json!({"title": "v2 is out", "url": "/r"}))),
        "v2 is out"
    );
    assert_eq!(
        text(n("x", json!({"actor_name": "Ana", "subject_key": "D-1"}))),
        "Ana updated D-1"
    );
    assert_eq!(text(n("x", json!({}))), "Someone sent you a notification");

    let body = json!({"data": [
        n("item_assigned", json!({"actor_name": "Ana", "item_key": "BRAI-12", "item_title": "Fix\nlogin", "item_url": "/w/1"})),
        {"id": "../../etc", "type": "x", "data": {}, "read": false},
        {"id": "9b1c2d3e-0000-4000-8000-000000000002", "type": "x", "data": {"url": "https://evil.example/x"}, "read": true, "created_at": "nope"}
    ], "next_cursor": null});
    let v = view::notifications(&body, 4, &view::PendingReads::default(), |url: &str| {
        url.starts_with('/')
    });
    assert_eq!(v.items.len(), 2, "an id that is not a uuid is dropped");
    assert_eq!(v.items[0].body.as_deref(), Some("Fix login"), "no control characters");
    assert!(v.items[0].has_link && !v.items[0].read);
    assert!(!v.items[1].has_link, "a link to another site is not offered");
    assert_eq!(v.items[1].created_at, None);
    assert_eq!(v.unread, 4);
}

#[test]
fn notifications_are_found_and_marked_read_by_id_only() {
    let id = "9b1c2d3e-0000-4000-8000-000000000001";
    let mut body = json!({"data": [{"id": id, "type": "item_assigned", "data": {"item_url": "/w/1"}, "read": false},
        {"id": "9b1c2d3e-0000-4000-8000-000000000002", "type": "x", "data": {}, "read": false}]});
    assert_eq!(view::find_notification(&body, id), Some((false, Some("/w/1".into()))));
    assert_eq!(
        view::find_notification(&body, "9b1c2d3e-0000-4000-8000-00000000000f"),
        None
    );
    assert_eq!(view::find_notification(&body, "x/../y"), None);
    view::mark_read(&mut body, Some(id));
    assert_eq!(body["data"][0]["read"], true);
    assert_eq!(body["data"][1]["read"], false);
    view::mark_read(&mut body, None);
    assert_eq!(body["data"][1]["read"], true);
    assert!(view::is_notification_id(id));
    assert!(
        !view::is_notification_id("") && !view::is_notification_id("a/b") && !view::is_notification_id(&"a".repeat(65))
    );
}

#[test]
fn the_timer_ticks_from_started_at_and_lists_its_activities() {
    let started = view::parse_instant("2026-11-02T09:00:00Z").unwrap();
    let body = json!({"running": true, "timer": {"id": 5, "started_at": "2026-11-02T09:00:00Z",
        "elapsed_seconds": 60, "elapsed_minutes": 1,
        "issue": {"id": 12, "work_id": "issue:12", "key": "BRAI-12", "title": "Fix login", "url": "/w/1", "workspace": "acme"}},
        "activities": [{"id": 3, "name": "Development", "color": "#00ff00"}, {"id": "x", "name": "Broken"}],
        "activity_required": true, "default_activity_id": null});
    let t = view::running(&body, started + 3725, 0).unwrap();
    assert_eq!(t.elapsed_seconds, 3725, "a 304 body's elapsed_seconds is stale");
    assert_eq!(t.label, "BRAI-12 Fix login");
    assert!(t.activity_required && t.default_activity_id.is_none());
    assert_eq!(t.activities.len(), 1);
    assert_eq!(t.activities[0].name, "Development");
    assert!(t.has_link);
    // without started_at: the answer's count plus its age
    let mut no_start = body.clone();
    no_start["timer"]["started_at"] = Value::Null;
    assert_eq!(view::running(&no_start, started, 30).unwrap().elapsed_seconds, 90);
    assert_eq!(view::running(&json!({"running": false, "timer": null}), 0, 0), None);
    assert_eq!(
        view::running(&json!({"running": true, "timer": {"id": "x"}}), 0, 0),
        None
    );
}

#[test]
fn todays_entries_add_up() {
    let body = json!({"data": [
        {"id": 1, "minutes": 90, "source": "timer", "note": "pairing",
         "issue": {"id": 12, "work_id": "issue:12", "key": "BRAI-12", "title": "Fix login", "url": null},
         "activity": {"id": 3, "name": "Development"}, "project": {"id": 1, "name": "Website", "key": "WEB"}},
        {"id": 2, "minutes": 15, "source": "manual", "issue": null, "activity": null,
         "project": {"id": 1, "name": "Website", "key": "WEB"}},
        {"id": "bad", "minutes": 999}
    ], "next_cursor": null});
    let (rows, total) = view::entries(&body);
    assert_eq!(rows.len(), 2);
    assert_eq!(total, 105);
    assert_eq!(rows[0].label, "BRAI-12 Fix login");
    assert!(rows[0].from_timer);
    assert_eq!(rows[0].activity.as_deref(), Some("Development"));
    assert_eq!(rows[1].label, "Website");
}

#[test]
fn a_timer_starts_only_on_open_issues_of_my_day_and_assigned_work() {
    let day = json!({"items": [
        work("issue:12", Some("BRAI-12"), "Fix login", false, None, None),
        work("task:7", None, "Call Anna", false, None, None),
        work("issue:13", Some("BRAI-13"), "Done", true, None, None),
    ]});
    let assigned = json!({"data": [
        work("issue:12", Some("BRAI-12"), "Fix login", false, None, None),
        work("issue:20", Some("WEB-20"), "Header", false, None, None),
    ]});
    let c = view::candidates(Some(&day), Some(&assigned));
    let ids: Vec<_> = c.iter().map(|c| (c.id.as_str(), c.in_my_day)).collect();
    assert_eq!(ids, [("issue:12", true), ("issue:20", false)]);
    assert_eq!(c[1].label, "WEB-20 Header");
    assert!(view::candidates(None, None).is_empty());
}

// ── what commands may act on ───────────────────────────────────────────────────

#[test]
fn commands_only_act_on_ids_the_server_sent() {
    let today = json!({"work": [work("issue:1", Some("BRAI-1"), "A", false, None, None)], "my_day": [],
        "events": [{"id": "event:4:x", "title": "Call", "url": "/calendar?e=1"}, {"id": "booking:2", "title": "B"}]});
    let my_day = json!({"items": [work("task:7", None, "Call Anna", true, None, None)]});
    let assigned = json!({"data": [work("issue:20", Some("WEB-20"), "Header", false, None, None)]});
    let bodies = [&today, &my_day, &assigned];
    let found = view::find_work(&bodies, "task:7").unwrap();
    assert_eq!((found.label.as_str(), found.done), ("Call Anna", true));
    assert_eq!(found.url.as_deref(), Some("https://brain.example.com/w/task:7"));
    assert!(view::find_work(&bodies, "issue:20").is_some());
    assert!(view::find_work(&bodies, "issue:999").is_none(), "not in an answer");
    assert!(view::find_work(&bodies, "issue:1/../2").is_none(), "not a work id");
    assert_eq!(
        view::find_event(&today, "event:4:x"),
        Some(Some("/calendar?e=1".into()))
    );
    assert_eq!(view::find_event(&today, "booking:2"), Some(None));
    assert_eq!(view::find_event(&today, "event:9:y"), None);

    let mut my_day = my_day;
    view::set_done(&mut my_day, "task:7", false);
    assert_eq!(my_day["items"][0]["done"], false);
}

#[test]
fn the_page_gets_flags_never_urls() {
    let v = view::my_day(
        &json!({"items": [work("issue:1", None, "A", false, None, None)]}),
        &none(),
    );
    let text = serde_json::to_string(&v).unwrap();
    assert!(!text.contains("https://") && !text.contains("\"url\""));
    let t = view::today(
        &json!({"events": [{"id": "e", "title": "x", "all_day": true, "start": "2026-11-02", "url": "/c",
            "conference_url": "https://meet/x"}]}),
        0,
        &none(),
    );
    let text = serde_json::to_string(&t).unwrap();
    assert!(!text.contains("meet/x") && !text.contains("/c\""));
}

#[test]
fn server_text_is_cleaned_and_shortened() {
    assert_eq!(view::clean(&json!("  a\u{0007}b  "), 10).as_deref(), Some("a b"));
    assert_eq!(view::clean(&json!("abcdefghij"), 5).as_deref(), Some("abcd…"));
    assert_eq!(view::clean(&json!("   "), 5), None);
    assert_eq!(view::clean(&json!(5), 5), None);
    assert_eq!(view::safe_color(&json!("#ABC")).as_deref(), Some("#abc"));
    for bad in ["red", "#12345", "#12345g", "url(x)", "#1234567"] {
        assert_eq!(view::safe_color(&json!(bad)), None, "{bad}");
    }
}

// ── offline ────────────────────────────────────────────────────────────────────

#[test]
fn ticks_go_to_the_outbox_offline_and_timer_actions_wait_for_the_server() {
    assert_eq!(view::done_route(false), view::DoneRoute::Api);
    assert_eq!(view::done_route(true), view::DoneRoute::Outbox);
    // a tick whose request found no server goes to the outbox; a refusal is shown
    assert!(view::queue_after(&CallError::Offline("dns".into())));
    for e in [
        CallError::SignedOut("x".into()),
        CallError::Failed("x".into()),
        CallError::Server {
            status: 403,
            code: "forbidden".into(),
            message: String::new(),
        },
    ] {
        assert!(!view::queue_after(&e), "{e:?}");
    }
    // an older server: timer / mark-read are off offline, with a hint
    assert!(view::needs_server(true, false).is_some());
    assert!(view::needs_server(false, false).is_none());
    // a server with the sync types: queued, no hint
    assert!(view::needs_server(true, true).is_none());
}

// ── server summary / url, timer total, waiting timer and inbox changes ─────────

#[test]
fn the_servers_summary_and_url_win_and_older_servers_fall_back() {
    let id = "9b1c2d3e-0000-4000-8000-000000000001";
    let data = json!({"actor_name": "Ana", "item_key": "BRAI-12", "item_title": "Fix login", "item_url": "/w/1"});
    // a newer server: its sentence and its link (absolute, same server)
    let new = json!({"id": id, "type": "item_assigned", "summary": "Ana assigned you to BRAI-12 \u{7}",
        "url": "https://brain.example.com/w/12", "data": data, "read": false, "created_at": "2026-11-02T09:00:00Z"});
    assert_eq!(view::note_text(&new), "Ana assigned you to BRAI-12");
    assert_eq!(view::note_url(&new).as_deref(), Some("https://brain.example.com/w/12"));
    // url null: the user can't open it now — no link, not even the payload's
    let closed =
        json!({"id": id, "type": "item_assigned", "summary": "Ana assigned you to BRAI-12", "url": null, "data": data});
    assert_eq!(view::note_url(&closed), None);
    // an empty summary: the client's wording
    let blank = json!({"id": id, "type": "item_assigned", "summary": "  ", "url": null, "data": data});
    assert_eq!(view::note_text(&blank), "Ana assigned you to BRAI-12");
    // an older server: no summary / url fields → the client's wording and the payload link
    let old = json!({"id": id, "type": "mentioned", "data": {"actor_name": "Bo", "context_title": "Plan", "context_url": "/p/1"}});
    assert_eq!(view::note_text(&old), "Bo mentioned you in Plan");
    assert_eq!(view::note_url(&old).as_deref(), Some("/p/1"));

    let body = json!({"data": [new, old]});
    let v = view::notifications(&body, 2, &view::PendingReads::default(), |u: &str| {
        u.starts_with("https://brain.example.com/") || u.starts_with('/')
    });
    assert_eq!(v.items[0].text, "Ana assigned you to BRAI-12");
    assert!(v.items[0].has_link);
    assert_eq!(
        view::find_notification(&body, id),
        Some((false, Some("https://brain.example.com/w/12".into())))
    );
    // a row from a sync result replaces the kept one
    let mut kept = body.clone();
    view::replace_note(&mut kept, &json!({"id": id, "summary": "x", "url": null, "read": true}));
    assert_eq!(kept["data"][0]["read"], true);
    view::replace_note(&mut kept, &json!({"id": "../x", "read": false}));
    assert_eq!(kept["data"][0]["read"], true, "a bad id changes nothing");
}

fn note(id: &str, at: &str, read: bool) -> Value {
    json!({"id": id, "type": "x", "summary": "Hello", "url": null, "data": {}, "read": read, "created_at": at})
}

#[test]
fn reads_made_offline_show_as_read_and_pending_and_read_all_uses_the_newest_seen() {
    let a = "00000000-0000-4000-8000-00000000000a";
    let b = "00000000-0000-4000-8000-00000000000b";
    let c = "00000000-0000-4000-8000-00000000000c";
    let body = json!({"data": [
        note(c, "2026-11-02T10:00:00.000000Z", false),
        note(b, "2026-11-02T09:00:00Z", false),
        note(a, "2026-11-02T08:00:00+01:00", true),
    ]});
    // `before` = the newest notification the user saw (fractions / offsets read), else now
    assert_eq!(view::read_all_before(Some(&body), 0), "2026-11-02T10:00:00Z");
    assert_eq!(
        view::read_all_before(Some(&json!({"data": []})), 1_793_612_472),
        "2026-11-02T09:41:12Z"
    );
    assert_eq!(view::read_all_before(None, 1_793_612_472), "2026-11-02T09:41:12Z");

    let mut o = Outbox::default();
    o.push(Change::NotificationRead {
        notification_id: b.into(),
        title: "Hello".into(),
    })
    .unwrap();
    let reads = view::PendingReads::of(&o);
    let v = view::notifications(&body, 5, &reads, |_: &str| false);
    assert!(!v.items[0].read && !v.items[0].pending);
    assert!(
        v.items[1].read && v.items[1].pending,
        "read offline: read, waiting to sync"
    );
    assert!(v.items[2].read && !v.items[2].pending, "already read on the server");
    assert_eq!(v.unread, 4);
    assert_eq!(view::unread_after(5, Some(&body), &reads), 4);
    assert_eq!(view::unread_after(5, Some(&body), &view::PendingReads::default()), 5);

    // read all up to 09:30: the 10:00 one stays unread
    o.push(Change::NotificationsReadAll {
        before: "2026-11-02T09:30:00Z".into(),
    })
    .unwrap();
    let reads = view::PendingReads::of(&o);
    let v = view::notifications(&body, 2, &reads, |_: &str| false);
    assert!(!v.items[0].read, "arrived after what the user saw");
    assert_eq!(v.unread, 1);
}

#[test]
fn todays_total_and_the_description_come_with_the_timer_on_newer_servers() {
    let new = json!({"running": true, "timer": {"id": 5, "started_at": "2026-11-02T09:00:00Z",
        "description": "Reproduce \nthe bug", "issue": {"key": "BRAI-12", "title": "Fix login"}},
        "activities": [], "today_logged_seconds": 5400});
    assert_eq!(view::today_seconds(&new), Some(5400));
    let t = view::running(&new, view::parse_instant("2026-11-02T09:10:00Z").unwrap(), 0).unwrap();
    assert_eq!(t.description.as_deref(), Some("Reproduce  the bug"));
    assert!(!t.pending && !t.started_offline);
    assert_eq!(view::timer_id(&new), Some(5));
    // an older server: no total (then `/work-logs`), no description
    let old = json!({"running": false, "timer": null});
    assert_eq!(view::today_seconds(&old), None);
    assert_eq!(view::timer_id(&old), None);
    assert_eq!(
        view::timer_id(&json!({"running": false, "timer": {"id": 5}})),
        None,
        "not running"
    );
}

#[test]
fn waiting_timer_changes_are_laid_over_the_running_timer() {
    let now = view::parse_instant("2026-11-02T09:10:00Z").unwrap();
    let body = json!({"running": true, "timer": {"id": 5, "started_at": "2026-11-02T09:00:00Z",
        "issue": {"key": "BRAI-12", "title": "Fix login"}}});
    let running = view::running(&body, now, 0);

    // nothing waiting: as the server has it
    let none = view::TimerOverlay::of(&Outbox::default());
    assert_eq!(none.apply(running.clone(), Some(5), now), (running.clone(), None));

    // stopped offline: gone, with a note
    let mut o = Outbox::default();
    o.push(Change::TimerStop {
        timer_id: 5,
        stopped_at: "2026-11-02T09:09:00Z".into(),
        activity_id: None,
        title: "BRAI-12 Fix login".into(),
    })
    .unwrap();
    let (r, note) = view::TimerOverlay::of(&o).apply(running.clone(), Some(5), now);
    assert!(r.is_none());
    assert!(note.unwrap().contains("logged once Brainitive is back online"));
    // another timer's stop doesn't touch this one
    let (r, note) = view::TimerOverlay::of(&o).apply(running.clone(), Some(6), now);
    assert!(r.is_some() && note.is_none());

    // then started offline on another issue: shown as running, pending, not stoppable yet
    o.push(Change::TimerStart {
        issue_id: 13,
        description: Some("Docs".into()),
        started_at: "2026-11-02T09:09:30Z".into(),
        title: "BRAI-13 Docs".into(),
    })
    .unwrap();
    let (r, _) = view::TimerOverlay::of(&o).apply(running.clone(), Some(5), now);
    let r = r.unwrap();
    assert_eq!(r.label, "BRAI-13 Docs");
    assert_eq!(r.description.as_deref(), Some("Docs"));
    assert_eq!(r.elapsed_seconds, 30);
    assert!(r.pending && r.started_offline && !r.has_link);
    o.entries[1].attempts = 1;
    let (r, _) = view::TimerOverlay::of(&o).apply(None, None, now);
    assert!(!r.unwrap().started_offline, "it went out: waiting for its answer");

    // discarded offline
    let mut o = Outbox::default();
    o.push(Change::TimerDiscard {
        timer_id: 5,
        title: String::new(),
    })
    .unwrap();
    let (r, note) = view::TimerOverlay::of(&o).apply(running, Some(5), now);
    assert!(r.is_none() && note.unwrap().starts_with("Timer discarded"));
}

#[test]
fn a_stop_waiting_for_an_activity_shows_the_choice_while_it_fits() {
    let needed = crate::outbox::ActivityNeeded {
        timer_id: 5,
        stopped_at: "2026-11-02T09:40:00Z".into(),
        title: "BRAI-12 Fix login".into(),
        activities: vec![crate::outbox::ActivityChoice {
            id: 4,
            name: "Programming".into(),
        }],
        default_activity_id: Some(4),
        message: String::new(),
    };
    let same = json!({"running": true, "timer": {"id": 5}});
    let other = json!({"running": true, "timer": {"id": 6}});
    let v = view::activity_needed_view(Some(&needed), Some(&same)).unwrap();
    assert_eq!(v.activities[0].id, 4);
    assert_eq!(v.label, "BRAI-12 Fix login");
    assert_eq!(v.message, "Choose what to log this time as.");
    assert!(
        view::activity_needed_view(Some(&needed), None).is_some(),
        "offline: nothing to compare"
    );
    assert!(view::activity_needed_view(Some(&needed), Some(&other)).is_none());
    assert!(view::activity_needed_view(Some(&needed), Some(&json!({"running": false}))).is_none());
    assert!(view::activity_needed_view(None, Some(&same)).is_none());
}

#[test]
fn offline_the_copy_offers_its_open_issues_to_time() {
    let view_of = |work: Vec<crate::offline_cache::ViewWork>, day: Vec<crate::offline_cache::ViewWork>| {
        crate::offline_cache::View {
            has_data: true,
            user_name: None,
            synced_at: None,
            pending: 0,
            my_day: Some(crate::offline_cache::ViewDay {
                date: "2026-11-02".into(),
                items: day,
            }),
            work,
            lists: Vec::new(),
            captures: Vec::new(),
            problems: Vec::new(),
        }
    };
    let w = |id: &str, done: bool| crate::offline_cache::ViewWork {
        id: id.into(),
        key: Some("K-1".into()),
        title: "Thing".into(),
        done,
        due_date: None,
        project: None,
        source: None,
        pending: false,
        in_my_day: false,
    };
    let v = view_of(
        vec![
            w("issue:1", false),
            w("task:2", false),
            w("issue:3", true),
            w("issue:4", false),
        ],
        vec![w("issue:4", false)],
    );
    let c = view::offline_candidates(&v);
    let ids: Vec<_> = c.iter().map(|c| (c.id.as_str(), c.in_my_day)).collect();
    assert_eq!(ids, [("issue:4", true), ("issue:1", false)]);
}

#[test]
fn the_header_dot() {
    assert_eq!(view::sync_status(true, true, 3), "offline");
    assert_eq!(view::sync_status(false, true, 0), "syncing");
    assert_eq!(view::sync_status(false, false, 0), "online");
    assert_eq!(view::sync_status(false, false, 2), "pending");
}

#[test]
fn queued_done_changes_are_laid_over_the_rows() {
    let mut outbox = Outbox::default();
    for (done, base) in [(true, false), (false, true)] {
        outbox
            .push(Change::WorkDone {
                work_id: "issue:1".into(),
                done,
                base_done: base,
                title: "A".into(),
            })
            .unwrap();
    }
    outbox
        .push(Change::WorkDone {
            work_id: "task:2".into(),
            done: true,
            base_done: false,
            title: "B".into(),
        })
        .unwrap();
    outbox
        .push(Change::CaptureCreate {
            text: "idea".into(),
            url: None,
            intended_type: None,
        })
        .unwrap();
    let pending = view::pending_done(&outbox);
    assert_eq!(pending.get("task:2"), Some(&true));
    assert_eq!(pending.len(), 1, "tick + untick folded away; captures are not ticks");
}

#[test]
fn offline_the_copy_fills_my_day_assigned_and_today() {
    let origin = crate::server::ServerOrigin::parse("brain.example.com").unwrap();
    let mut cache = crate::offline_cache::Cache::new(&origin);
    let page: crate::offline_cache::FeedPage = serde_json::from_value(json!({
        "data": [
            {"type": "work", "id": "issue:1", "op": "upsert", "project_id": 1, "updated_at": "2026-11-02T08:00:00Z",
             "data": work("issue:1", Some("WEB-1"), "Overdue", false, Some("2026-11-01"), Some("Website"))},
            {"type": "work", "id": "task:2", "op": "upsert", "project_id": 1, "updated_at": "2026-11-02T08:00:00Z",
             "data": work("task:2", None, "Later", false, Some("2026-11-20"), Some("Website"))},
            {"type": "my_day", "id": "2026-11-02", "op": "upsert", "project_id": null, "updated_at": "2026-11-02T08:00:00Z",
             "data": {"date": "2026-11-02", "items": [work("issue:1", Some("WEB-1"), "Overdue", false, Some("2026-11-01"), Some("Website"))], "carry_over_count": 0}}
        ],
        "next_cursor": "c1", "has_more": false, "reset": true, "project_ids": [1], "task_lists": []
    }))
    .unwrap();
    let mut run = cache.start_run();
    cache.apply_page(&mut run, page, 1);
    let mut outbox = Outbox::default();
    outbox
        .push(Change::WorkDone {
            work_id: "issue:1".into(),
            done: true,
            base_done: false,
            title: "WEB-1 Overdue".into(),
        })
        .unwrap();
    let copy = crate::offline_cache::view(&cache, &outbox);

    let day = view::offline_my_day(&copy).unwrap();
    assert_eq!(day.date.as_deref(), Some("2026-11-02"));
    assert_eq!((day.done, day.total), (1, 1));
    assert!(day.items[0].pending, "the queued tick shows");

    let assigned = view::offline_assigned(&copy, Some("2026-11-02"));
    assert_eq!(assigned.total, 2);
    assert_eq!(assigned.groups[0].project, "Website");
    assert!(!assigned.groups[0].items.iter().any(|w| w.has_link), "no links offline");

    let today = view::offline_today(&copy, Some("2026-11-02"));
    let due: Vec<_> = today.due.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(due, ["issue:1"], "due on or before today only");
    assert!(today.without_events && today.events.is_empty());
}

// ── the panel's data belongs to one server and one sign-in ─────────────────────

#[test]
fn another_server_or_sign_in_drops_the_kept_answers() {
    let dir = std::env::temp_dir().join(format!("brainitive-panel-{}", std::process::id()));
    let state = crate::state::AppState::new(dir.join("settings.json"), dir.join("q.json"), dir.join("offline"));
    let mut inner = state.lock();
    assert!(kept_for_current(&mut inner).is_none(), "no server");
    inner.settings.current = Some(crate::server::ServerOrigin::parse("a.example.com").unwrap());
    inner.phase = Phase::InApp;
    kept_for_current(&mut inner).unwrap().today = Some(Stored::new(json!({"date": "2026-11-02"})));
    // offline (unreachable): still the same owner, the data stays
    inner.phase = Phase::Unreachable;
    assert!(kept_for_current(&mut inner).unwrap().today.is_some());
    // signed out and in again: a new account epoch
    inner.phase = Phase::InApp;
    inner.account_epoch += 1;
    assert!(kept_for_current(&mut inner).unwrap().today.is_none());
    kept_for_current(&mut inner).unwrap().today = Some(Stored::new(json!({})));
    inner.settings.current = Some(crate::server::ServerOrigin::parse("b.example.com").unwrap());
    assert!(kept_for_current(&mut inner).unwrap().today.is_none(), "another server");
    inner.phase = Phase::SignIn;
    assert!(kept_for_current(&mut inner).is_none(), "signed out");
}

#[test]
fn the_page_sends_references_not_urls() {
    let t: OpenTarget = serde_json::from_value(json!({"kind": "work", "id": "issue:12"})).unwrap();
    assert!(matches!(t, OpenTarget::Work(id) if id == "issue:12"));
    let t: OpenTarget = serde_json::from_value(json!({"kind": "notification", "id": "9b1c"})).unwrap();
    assert!(matches!(t, OpenTarget::Notification(_)));
    for kind in ["timer", "my_work", "notifications", "calendar"] {
        assert!(
            serde_json::from_value::<OpenTarget>(json!({ "kind": kind })).is_ok(),
            "{kind}"
        );
    }
    // no way to hand over a URL or an unknown kind
    assert!(serde_json::from_value::<OpenTarget>(json!({"kind": "url", "id": "https://evil.example"})).is_err());
    assert!(serde_json::from_value::<OpenTarget>(json!({"kind": "work"})).is_err());
    assert_eq!(
        serde_json::from_value::<PanelAction>(json!("sign_in")).unwrap(),
        PanelAction::SignIn
    );
    assert!(serde_json::from_value::<PanelAction>(json!("quit")).is_err());
}

// ── answers that arrive after a switch (codex review, finding 9) ───────────────

#[test]
fn a_done_tick_confirmed_after_a_switch_does_not_touch_the_new_account() {
    let dir = std::env::temp_dir().join(format!("brainitive-panel-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let app_state = crate::state::AppState::new(dir.join("settings.json"), dir.join("q.json"), dir.join("offline"));
    let a = ServerOrigin::parse("a.example.com").unwrap();
    let b = ServerOrigin::parse("b.example.com").unwrap();
    let mut inner = app_state.lock();
    inner.settings.remember(&b, "B", None);
    inner.settings.remember(&a, "A", None);
    inner.phase = crate::state::Phase::InApp;
    let item = |done| crate::tray::DayItem {
        id: "issue:7".into(),
        key: None,
        title: "B's own issue 7".into(),
        done,
        url: None,
    };
    // `panel_set_done` on A is on the wire …
    let ticket_a = PollTicket::now(&inner).unwrap();
    // … the user switches to B, whose My Day has an item with the same id
    crate::flows::drop_account_state(&mut inner);
    assert!(inner.settings.touch(&b));
    inner.tray_data.my_day = Some(vec![item(false)]);
    inner.tray_data.my_day_stale = false;
    // A's success arrives: B's tray stays as it is
    assert!(!mark_tray_done(&mut inner, &ticket_a, "issue:7", true));
    assert_eq!(inner.tray_data.my_day, Some(vec![item(false)]));
    assert!(!inner.tray_data.my_day_stale);
    // the same account's answer still updates it
    let ticket_b = PollTicket::now(&inner).unwrap();
    assert!(mark_tray_done(&mut inner, &ticket_b, "issue:7", true));
    assert_eq!(inner.tray_data.my_day, Some(vec![item(true)]));
    // the same server, another sign-in: stale too
    inner.account_epoch += 1;
    assert!(!mark_tray_done(&mut inner, &ticket_b, "issue:7", false));
    assert_eq!(inner.tray_data.my_day, Some(vec![item(true)]));
    // a sync's live states belong to the sign-in it ran for (`apply_sync_states`)
    assert_ne!(ticket_b.account(), PollTicket::now(&inner).unwrap().account());
    drop(inner);
    let _ = std::fs::remove_dir_all(dir);
}
