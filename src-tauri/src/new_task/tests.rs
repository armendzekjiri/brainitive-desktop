use serde_json::json;

use super::*;

/// `GET /capture-destinations` as the server answers it (two workspaces).
fn destinations() -> Value {
    json!([
        {"id": 1, "name": "Acme", "slug": "acme", "bookmarks": [], "snippet_collections": [],
         "projects": [
            {"id": 10, "name": "Website", "key": "WEB", "icon": null, "color": null, "can_create_pages": true,
             "boards": [{"id": 100, "name": "Main Board", "type": "kanban", "columns": [{"id": 1, "name": "To do"}]},
                        {"id": 101, "name": "Bugs", "type": "kanban", "columns": []}],
             "task_lists": []},
            {"id": 11, "name": "Docs only", "key": "DOC", "boards": [], "task_lists": [{"id": 5, "name": "L"}]},
         ]},
        {"id": 2, "name": "Home", "slug": "home", "bookmarks": [], "snippet_collections": [],
         "projects": [
            {"id": 20, "name": "Garden\u{7}", "key": null, "boards": [{"id": 200, "name": "Plants"}], "task_lists": []},
            {"id": 0, "name": "broken", "boards": [{"id": 1}]},
         ]},
    ])
}

fn projects() -> Vec<Project> {
    parse_destinations(&destinations())
}

fn config() -> Config {
    parse_config(&json!({"data": {
        "project_id": 10,
        "types": [
            {"id": 1, "name": "Task", "slug": "task", "icon": null, "color": "#3b82f6", "is_default": true},
            {"id": 2, "name": "Bug", "slug": "bug", "icon": null, "color": "red; background:url(x)", "is_default": false}
        ],
        "priorities": [
            {"id": 7, "name": "Highest", "slug": "highest", "icon": null, "color": "#dc2626", "is_default": false},
            {"id": 8, "name": "High", "slug": "high", "icon": null, "color": null, "is_default": false},
            {"id": 9, "name": "Medium", "slug": "medium", "icon": null, "color": "#ca8a04", "is_default": true}
        ],
        "labels": [], "fields": [], "releases": [], "can": {"manage": false}
    }}))
}

fn input(title: &str) -> TaskInput {
    TaskInput {
        project_id: 10,
        board_id: Some(101),
        title: title.into(),
        ..TaskInput::default()
    }
}

#[test]
fn destinations_keep_projects_with_boards_in_the_servers_order() {
    let p = projects();
    assert_eq!(p.iter().map(|p| p.id).collect::<Vec<_>>(), [10, 20]);
    assert_eq!(p[0].workspace, "Acme");
    assert_eq!(p[0].key.as_deref(), Some("WEB"));
    assert_eq!(
        p[0].boards,
        [
            Board {
                id: 100,
                name: "Main Board".into()
            },
            Board {
                id: 101,
                name: "Bugs".into()
            }
        ]
    );
    // control characters dropped, a missing key is none, the workspace is the second one
    assert_eq!(
        (p[1].name.as_str(), p[1].key.as_deref(), p[1].workspace.as_str()),
        ("Garden", None, "Home")
    );
    // the {data: …} wrapper and odd answers
    assert_eq!(parse_destinations(&json!({"data": destinations()})).len(), 2);
    assert!(parse_destinations(&json!(null)).is_empty());
    assert!(parse_destinations(&json!({"message": "x"})).is_empty());
}

#[test]
fn the_form_starts_with_the_remembered_project_and_board_else_the_first() {
    let p = projects();
    // nothing remembered (the server has no recent destinations): the first project, its first board
    assert_eq!(
        pick(&p, None),
        Selection {
            project_id: Some(10),
            board_id: Some(100)
        }
    );
    let m = Memory {
        project_id: Some(10),
        board_id: Some(101),
        type_id: Some(2),
        at: 1,
    };
    assert_eq!(
        pick(&p, Some(&m)),
        Selection {
            project_id: Some(10),
            board_id: Some(101)
        }
    );
    // a remembered board of another project is ignored
    let m = Memory {
        project_id: Some(20),
        board_id: Some(101),
        ..Memory::default()
    };
    assert_eq!(pick(&p, Some(&m)).board_id, Some(200));
    // the project is gone (or no access any more): the first one
    let m = Memory {
        project_id: Some(99),
        board_id: Some(990),
        ..Memory::default()
    };
    assert_eq!(pick(&p, Some(&m)).project_id, Some(10));
    assert_eq!(pick(&[], Some(&m)), Selection::default());
}

#[test]
fn types_and_priorities_with_safe_colours() {
    let c = config();
    assert_eq!(c.types.len(), 2);
    assert_eq!(c.types[0].color.as_deref(), Some("#3b82f6"));
    assert_eq!(c.types[1].color, None, "never a style injection");
    assert_eq!(
        c.priorities.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Highest", "High", "Medium"]
    );
    // the remembered type when the project has it, else its default, else none
    let m = |t| Memory {
        type_id: Some(t),
        ..Memory::default()
    };
    assert_eq!(pick_type(&c, Some(&m(2))), Some(2));
    assert_eq!(pick_type(&c, Some(&m(42))), Some(1));
    assert_eq!(pick_type(&c, None), Some(1));
    assert_eq!(pick_type(&Config::default(), Some(&m(2))), None);
    // without the wrapper too
    assert_eq!(parse_config(&json!({"types": [], "priorities": []})), Config::default());
}

#[test]
fn payload_maps_the_form_to_the_issue_body() {
    let (p, c) = (projects(), config());
    let mut i = input("  Fix the login  ");
    i.description = "\n  Steps:\n- open\n  ".into();
    i.priority_id = Some(8);
    i.type_id = Some(2);
    let d = draft(&i, &p, Some(&c)).unwrap();
    assert_eq!((d.project_id, d.board_id, d.type_id), (10, 101, Some(2)));
    assert_eq!(
        d.body,
        json!({"title": "Fix the login", "description": "Steps:\n- open", "priority_id": 8, "type_id": 2})
    );
    assert_eq!(d.with_request_id("r-1")["request_id"], "r-1");

    // no description, no priority, no type: left out (the server gives its defaults)
    let d = draft(&input("Plain"), &p, Some(&c)).unwrap();
    assert_eq!(d.body, json!({"title": "Plain"}));

    // one board: no board needed; several: one must be picked
    let one = TaskInput {
        project_id: 20,
        board_id: None,
        title: "x".into(),
        ..TaskInput::default()
    };
    assert_eq!(draft(&one, &p, None).unwrap().board_id, 200);
    let several = TaskInput {
        board_id: None,
        ..input("x")
    };
    assert_eq!(draft(&several, &p, None).unwrap_err().fields["board"], "Pick a board.");
}

#[test]
fn only_ids_of_the_last_answers_are_accepted() {
    let (p, c) = (projects(), config());
    let field = |i: &TaskInput, cfg: Option<&Config>| {
        let e = draft(i, &p, cfg).unwrap_err();
        assert_eq!(e.kind, "invalid");
        e.fields.into_keys().collect::<Vec<_>>()
    };
    assert_eq!(field(&input("   "), Some(&c)), ["title"]);
    assert_eq!(field(&input(&"x".repeat(256)), Some(&c)), ["title"]);
    assert!(draft(&input(&"é".repeat(255)), &p, Some(&c)).is_ok());
    // a project / board the server didn't offer (e.g. the docs-only project, a board of another one)
    assert_eq!(
        field(
            &TaskInput {
                project_id: 11,
                ..input("x")
            },
            Some(&c)
        ),
        ["project"]
    );
    assert_eq!(
        field(
            &TaskInput {
                board_id: Some(200),
                ..input("x")
            },
            Some(&c)
        ),
        ["board"]
    );
    // priorities / types of the project's config only (none known yet: refused)
    assert_eq!(
        field(
            &TaskInput {
                priority_id: Some(99),
                ..input("x")
            },
            Some(&c)
        ),
        ["priority"]
    );
    assert_eq!(
        field(
            &TaskInput {
                type_id: Some(1),
                ..input("x")
            },
            None
        ),
        ["type"]
    );
    let long = TaskInput {
        description: "x".repeat(MAX_DESCRIPTION + 1),
        ..input("x")
    };
    assert_eq!(field(&long, Some(&c)), ["description"]);
}

#[test]
fn a_retry_of_the_same_input_reuses_its_request_id() {
    let (p, c) = (projects(), config());
    let d = draft(&input("Fix"), &p, Some(&c)).unwrap();
    let mut n = 0;
    let mut fresh = || {
        n += 1;
        format!("id-{n}")
    };
    // first try: a new id
    let first = request_id_for(None, &d, &mut fresh);
    assert_eq!(first, "id-1");
    let pending = Pending {
        board_id: d.board_id,
        body: d.body.clone(),
        request_id: first.clone(),
    };
    // the answer was lost: the same input again → the same id (the server answers with the first issue)
    assert_eq!(request_id_for(Some(&pending), &d, &mut fresh), "id-1");
    // the title changed or another board: a new id (the same id with other input would be a 409)
    let other = draft(&input("Fix it"), &p, Some(&c)).unwrap();
    assert_eq!(request_id_for(Some(&pending), &other, &mut fresh), "id-2");
    let moved = draft(
        &TaskInput {
            board_id: Some(100),
            ..input("Fix")
        },
        &p,
        Some(&c),
    )
    .unwrap();
    assert_eq!(request_id_for(Some(&pending), &moved, &mut fresh), "id-3");
    // real ids are uuids
    let id = crate::queue::new_request_id();
    assert_eq!(id.len(), 36);
    assert_ne!(id, crate::queue::new_request_id());
}

#[test]
fn the_created_issue_and_its_page() {
    let body = json!({"data": {"id": 51, "work_id": "issue:51", "key": "WEB-12", "title": "Fix the login",
        "url": "https://brain.example.com/w/acme/p/website/boards/main?item=51"}});
    let c = parse_created(&body).unwrap();
    assert_eq!(
        (c.id, c.key.as_deref(), c.title.as_str()),
        (51, Some("WEB-12"), "Fix the login")
    );
    assert!(c.url.unwrap().ends_with("item=51"));
    assert_eq!(parse_created(&json!({"message": "x"})), None);
}

#[test]
fn field_errors_limit_and_access_are_mapped_for_the_form() {
    let e = error_answer(
        422,
        &json!({"message": "The title field is required. (and 1 more error)", "code": "validation_failed",
                "errors": {"title": ["The title field is required."], "type_id": ["The selected type is invalid."],
                           "fields.12": ["Pick one."]}}),
    );
    assert_eq!(e.kind, "invalid");
    assert_eq!(e.fields["title"], "The title field is required.");
    assert_eq!(e.fields["type"], "The selected type is invalid.");
    assert_eq!(e.fields["form"], "Pick one.");
    assert!(!e.inbox);
    // a single field error is the message
    let e = error_answer(422, &json!({"errors": {"priority_id": ["Bad priority."]}}));
    assert_eq!(
        (e.message.as_str(), e.fields["priority"].as_str()),
        ("Bad priority.", "Bad priority.")
    );

    let e = error_answer(
        402,
        &json!({"message": "Acme's storage is full (Free plan).", "code": "limit_reached",
                "limit": {"kind": "content", "limit": 1, "usage": 1, "plan": "Free", "workspace": "Acme", "billing_url": null}}),
    );
    assert_eq!(
        (e.kind, e.message.as_str()),
        ("limit", "Acme's storage is full (Free plan).")
    );
    assert!(!e.inbox, "the inbox is no way around a plan limit");
    assert_eq!(error_answer(402, &Value::Null).kind, "limit");
    assert_eq!(error_answer(403, &Value::Null).kind, "forbidden");
    assert_eq!(error_answer(404, &Value::Null).kind, "not_found");
    assert_eq!(error_answer(409, &Value::Null).kind, "conflict");
    let busy = error_answer(429, &Value::Null);
    assert!(busy.kind == "rate_limited" && busy.inbox);
}

#[test]
fn no_answer_offers_the_inbox_and_a_lost_sign_in_asks_to_sign_in() {
    let offline = call_error(&CallError::Offline("dns".into()), "Cloud");
    assert_eq!(offline.kind, "offline");
    assert!(offline.inbox);
    assert!(offline.message.contains("inbox"));
    let down = call_error(
        &CallError::Server {
            status: 503,
            code: String::new(),
            message: "x".into(),
        },
        "Cloud",
    );
    assert!(down.kind == "server" && down.inbox);
    assert!(call_error(&CallError::Failed("odd".into()), "Cloud").inbox);
    let out = call_error(&CallError::SignedOut("x".into()), "Cloud");
    assert_eq!(
        (out.kind, out.message.as_str()),
        ("signed_out", "Sign in to Cloud again.")
    );
    assert!(!out.inbox);
    // an error status without a kept body
    let gone = call_error(
        &CallError::Server {
            status: 404,
            code: "not_found".into(),
            message: "x".into(),
        },
        "Cloud",
    );
    assert_eq!(gone.kind, "not_found");
}

#[test]
fn the_inbox_keeps_title_and_description() {
    assert_eq!(inbox_text(" Fix ", "\n Steps \n"), "Fix\n\nSteps");
    assert_eq!(inbox_text("Fix", "  "), "Fix");
    assert_eq!(inbox_text("", "Only notes"), "Only notes");
    assert_eq!(
        inbox_text("x", &"y".repeat(30_000)).chars().count(),
        crate::queue::MAX_TEXT
    );
}

#[test]
fn memory_per_server_is_capped_and_survives_odd_files() {
    let mut map = MemoryMap::new();
    for i in 0..(MAX_MEMORY as u64 + 3) {
        remember(
            &mut map,
            &format!("https://s{i}.example.com"),
            Memory {
                project_id: Some(i),
                at: 100 + i,
                ..Memory::default()
            },
        );
    }
    assert_eq!(map.len(), MAX_MEMORY);
    // the oldest went
    assert!(!map.contains_key("https://s0.example.com"));
    assert!(map.contains_key("https://s12.example.com"));
    // used again: replaced, not added
    remember(
        &mut map,
        "https://s12.example.com",
        Memory {
            project_id: Some(1),
            at: 999,
            ..Memory::default()
        },
    );
    assert_eq!(map.len(), MAX_MEMORY);
    assert_eq!(map["https://s12.example.com"].project_id, Some(1));

    // a broken entry is skipped, the others stay
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(deserialize_with = "lenient_memory")]
        m: MemoryMap,
    }
    let f: File = serde_json::from_value(json!({"m": {
        "https://a.example.com": {"project_id": 3, "board_id": 4, "type_id": null, "at": 5},
        "https://b.example.com": {"project_id": "nope"},
        "https://c.example.com": {}
    }}))
    .unwrap();
    assert_eq!(f.m.len(), 2);
    assert_eq!(f.m["https://a.example.com"].board_id, Some(4));
    assert_eq!(f.m["https://c.example.com"], Memory::default());
    let f: File = serde_json::from_value(json!({"m": "garbage"})).unwrap();
    assert!(f.m.is_empty());
}

#[test]
fn the_window_follows_the_page_within_limits() {
    assert_eq!(clamp_height(300.4), 300.0);
    assert_eq!(clamp_height(10.0), MIN_HEIGHT);
    assert_eq!(clamp_height(5000.0), MAX_HEIGHT);
    assert_eq!(clamp_height(f64::NAN), MIN_HEIGHT);
    assert_eq!(clamp_height(f64::INFINITY), MIN_HEIGHT);
}

#[test]
fn offline_the_same_create_goes_to_the_outbox_with_description_and_type() {
    let (p, c) = (projects(), config());
    let mut i = input("  Fix the login  ");
    i.description = "Steps:\n- open".into();
    i.priority_id = Some(8);
    i.type_id = Some(2);
    let d = draft(&i, &p, Some(&c)).unwrap();
    let change = offline_change(&d);
    assert_eq!(
        change,
        crate::outbox::Change::IssueCreate {
            board_id: 101,
            title: "Fix the login".into(),
            description: Some("Steps:\n- open".into()),
            item_type_id: Some(2),
            priority_id: Some(8),
        }
    );
    // the sync API's names: `item_type_id`, not `type_id`
    let j = crate::outbox::Entry::new(change).to_json();
    assert_eq!(
        j["data"],
        json!({"board_id": 101, "title": "Fix the login", "description": "Steps:\n- open", "item_type_id": 2, "priority_id": 8})
    );
    let plain = offline_change(&draft(&input("Plain"), &p, Some(&c)).unwrap());
    assert_eq!(
        crate::outbox::Entry::new(plain).to_json()["data"],
        json!({"board_id": 101, "title": "Plain"})
    );
}

#[test]
fn with_an_etag_the_destinations_are_asked_again_on_every_open_else_once_a_minute() {
    use std::time::Duration;
    let minute = Duration::from_secs(60);
    // a newer server (ETag): always revalidated, a 304 costs nothing
    assert!(stale(false, true, Duration::from_secs(1), minute));
    // an older server (no ETag): the 1-minute throttle stays
    assert!(!stale(false, false, Duration::from_secs(30), minute));
    assert!(stale(false, false, Duration::from_secs(61), minute));
    // a board turned out gone: always
    assert!(stale(true, false, Duration::ZERO, minute));
}
