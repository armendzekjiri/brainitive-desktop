//! The capability files grant every bundled window its own commands only, never to the
//! server's pages (no `remote`), never a plugin's JavaScript API, never a bridge command.

use serde_json::Value;

fn capability(name: &str) -> Value {
    let path = format!("{}/capabilities/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("capability file")).expect("json")
}

fn permissions(cap: &Value) -> Vec<String> {
    cap["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect()
}

/// Every capability file (= every bundled window + the main window's screens and top bar).
const ALL: [&str; 10] = [
    "local", "settings", "capture", "switcher", "offline", "voice", "leader", "panel", "task", "topbar",
];

fn config() -> Value {
    let path = format!("{}/tauri.conf.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn every_bundled_window_has_its_own_local_only_capability() {
    let enabled: Vec<String> = config()["app"]["security"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        enabled,
        ["local", "settings", "capture", "switcher", "offline", "voice", "leader", "panel", "task", "topbar"]
    );

    for (name, window, prefix) in [
        ("settings", "settings", "allow-settings-"),
        ("capture", "capture", "allow-capture-"),
        ("switcher", "switcher", "allow-switcher-"),
        ("offline", "offline", "allow-offline-"),
        ("voice", "voice", "allow-voice-"),
        ("leader", "leader", "allow-leader-"),
        ("panel", "panel", "allow-panel-"),
        ("task", "task", "allow-task-"),
    ] {
        let cap = capability(name);
        assert_eq!(cap["identifier"], name);
        assert_eq!(cap["local"], true, "{name}");
        assert!(cap.get("remote").is_none(), "{name} must not reach remote content");
        assert_eq!(
            cap["windows"],
            serde_json::json!([window]),
            "{name}: exactly its own window"
        );
        assert!(cap.get("webviews").is_none());
        for p in permissions(&cap) {
            // the offline window listens for `offline://changed`, the voice indicator for
            // `voice://view` / `voice://level`, the panel for `panel://changed` (each sent to that
            // window only)
            let event = matches!(name, "offline" | "voice" | "panel")
                && matches!(p.as_str(), "core:event:allow-listen" | "core:event:allow-unlisten");
            assert!(p.starts_with(prefix) || event, "{name} grants {p}");
        }
    }

    // the connect / sign-in screens: the main window's `main` webview only — by webview label:
    // a capability by window would reach every webview of the window, the top bar too
    let local = capability("local");
    assert_eq!(local["webviews"], serde_json::json!(["main"]));
    assert!(local.get("windows").is_none());
    assert!(local.get("remote").is_none());
}

#[test]
fn the_top_bar_gets_its_own_commands_only() {
    let cap = capability("topbar");
    assert_eq!(cap["identifier"], "topbar");
    assert_eq!(cap["local"], true);
    assert!(cap.get("remote").is_none(), "the top bar must not reach remote content");
    assert!(cap.get("windows").is_none(), "by window it would reach the tabs too");
    assert_eq!(cap["webviews"], serde_json::json!(["topbar"]));
    for p in permissions(&cap) {
        let event = matches!(p.as_str(), "core:event:allow-listen" | "core:event:allow-unlisten");
        assert!(p.starts_with("allow-topbar-") || event, "topbar grants {p}");
    }
    // and nobody else gets them
    for name in ALL.iter().filter(|n| **n != "topbar") {
        for p in permissions(&capability(name)) {
            assert!(!p.starts_with("allow-topbar-"), "{name} grants {p}");
        }
    }
    // the bridge's runtime capability goes to the tabs by webview label, never by window
    let shell = std::fs::read_to_string(format!("{}/src/shell.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let bridge = shell
        .split("pub fn allow_bridge_for(")
        .nth(1)
        .unwrap()
        .split(
            "
}",
        )
        .next()
        .unwrap();
    assert!(bridge.contains(".webviews([MAIN.to_string(), format!(\"{APP_WINDOW_PREFIX}*\")])"));
    assert!(!bridge.contains(".windows("));
}

#[test]
fn no_capability_file_grants_plugins_or_the_bridge() {
    for name in ALL {
        for p in permissions(&capability(name)) {
            assert!(!p.contains("brainitive-"), "{name}: bridge command {p}");
            for plugin in [
                "global-shortcut",
                "autostart",
                "dialog",
                "clipboard",
                "fs",
                "opener",
                "notification",
                "shell",
                "updater",
            ] {
                assert!(!p.starts_with(&format!("{plugin}:")), "{name}: plugin permission {p}");
            }
            assert!(!p.starts_with("core:") || p.starts_with("core:event:"), "{name}: {p}");
        }
    }
}

#[test]
fn every_command_of_a_capability_is_in_the_app_manifest() {
    let build = std::fs::read_to_string(format!("{}/build.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
    for name in ALL {
        for p in permissions(&capability(name)) {
            if let Some(cmd) = p.strip_prefix("allow-") {
                let cmd = cmd.replace('-', "_");
                assert!(build.contains(&format!("\"{cmd}\"")), "{cmd} missing in build.rs");
            }
        }
    }
}

#[test]
fn only_the_offline_window_gets_the_offline_commands() {
    for name in ["local", "settings", "capture", "switcher", "voice", "leader"] {
        for p in permissions(&capability(name)) {
            assert!(!p.starts_with("allow-offline-"), "{name} grants {p}");
        }
    }
    // the main window's screens open it, and only they
    let local = permissions(&capability("local"));
    assert!(local.contains(&"allow-open-offline".to_string()));
    assert!(local.contains(&"allow-open-web-offline".to_string()));
}

#[test]
fn only_the_voice_indicator_gets_the_voice_commands() {
    for name in ["local", "settings", "capture", "switcher", "offline", "leader"] {
        for p in permissions(&capability(name)) {
            assert!(!p.starts_with("allow-voice-"), "{name} grants {p}");
        }
    }
    // the quick-capture window starts voice capture through its own command, Settings removes the
    // model through its own: the indicator's commands stay with the indicator
    assert!(permissions(&capability("capture")).contains(&"allow-capture-voice".to_string()));
    assert!(permissions(&capability("settings")).contains(&"allow-settings-remove-voice-model".to_string()));
    let voice = permissions(&capability("voice"));
    assert!(voice.contains(&"allow-voice-stop".to_string()) && voice.contains(&"allow-voice-cancel".to_string()));
}

#[test]
fn the_mac_app_explains_why_it_wants_the_microphone() {
    let plist = std::fs::read_to_string(format!("{}/Info.plist", env!("CARGO_MANIFEST_DIR"))).unwrap();
    assert!(plist.contains("<key>NSMicrophoneUsageDescription</key>"));
}

#[test]
fn only_the_settings_window_and_the_main_screens_get_the_update_commands() {
    // BRAI-373: checking / installing an update is a bundled window's command, never a remote
    // page's, and the updater plugin's own JS commands are granted to nobody (see above)
    let settings = permissions(&capability("settings"));
    for p in [
        "allow-settings-set-update-channel",
        "allow-settings-check-for-updates",
        "allow-settings-restart-to-update",
    ] {
        assert!(settings.contains(&p.to_string()), "{p}");
    }
    let local = permissions(&capability("local"));
    assert!(local.contains(&"allow-check-for-update".to_string()));
    assert!(local.contains(&"allow-restart-to-update".to_string()));
    for name in ["capture", "switcher", "offline", "voice", "leader"] {
        for p in permissions(&capability(name)) {
            let updater_command = ["for-update", "to-update", "update-channel"]
                .iter()
                .any(|u| p.contains(u));
            assert!(!updater_command, "{name} grants {p}");
        }
    }
    for name in ALL {
        for p in permissions(&capability(name)) {
            assert!(!p.starts_with("updater:"), "{name} grants the plugin's {p}");
        }
    }
}

#[test]
fn the_brainitive_key_overlay_gets_its_four_commands_only() {
    // BRAI-440: no events, no server, no other window's commands; the letters' actions run in Rust
    let leader = capability("leader");
    assert_eq!(
        permissions(&leader),
        [
            "allow-leader-view",
            "allow-leader-press",
            "allow-leader-release",
            "allow-leader-close"
        ]
    );
    for name in ["local", "settings", "capture", "switcher", "offline", "voice"] {
        for p in permissions(&capability(name)) {
            assert!(!p.starts_with("allow-leader-"), "{name} grants {p}");
        }
    }
    // the indicator's own key-up command (hold the letter to talk) stays with the indicator
    assert!(permissions(&capability("voice")).contains(&"allow-voice-release".to_string()));
}

#[test]
fn only_the_menu_bar_panel_gets_the_panel_commands() {
    // the panel's commands (API calls with the app token, opening pages, timer, ticks) belong to
    // the panel window; the panel gets nothing of the other windows
    let panel = permissions(&capability("panel"));
    for p in &panel {
        assert!(
            p.starts_with("allow-panel-") || p == "core:event:allow-listen" || p == "core:event:allow-unlisten",
            "panel grants {p}"
        );
    }
    for p in [
        "allow-panel-view",
        "allow-panel-load",
        "allow-panel-set-done",
        "allow-panel-open",
        "allow-panel-timer-stop",
        "allow-panel-close",
    ] {
        assert!(panel.contains(&p.to_string()), "{p}");
    }
    for name in ALL.into_iter().filter(|n| *n != "panel") {
        for p in permissions(&capability(name)) {
            assert!(!p.starts_with("allow-panel-"), "{name} grants {p}");
        }
    }
}

#[test]
fn every_panel_command_checks_its_caller() {
    // each `#[tauri::command]` of the panel starts with the caller check (popup::ensure_caller)
    let src = std::fs::read_to_string(format!("{}/src/panel/mod.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut commands = 0;
    for part in src.split("#[tauri::command]").skip(1) {
        commands += 1;
        let body = part.split_once('{').map(|(_, b)| b).unwrap_or("");
        let first = body.trim_start().lines().next().unwrap_or("");
        assert_eq!(
            first.trim(),
            "caller(&app, &webview)?;",
            "{}",
            part.lines().nth(1).unwrap_or("")
        );
    }
    assert_eq!(
        commands,
        permissions(&capability("panel")).len() - 2,
        "one permission per command"
    );
    assert!(src.contains("popup::ensure_caller(app, webview, Popup::Panel)"));
}

#[test]
fn only_the_new_task_window_gets_its_commands_and_nothing_else() {
    // creating issues (API calls with the app token, opening the created issue) belongs to the
    // New task window; it gets no events, no other window's commands
    let task = permissions(&capability("task"));
    assert_eq!(
        task,
        [
            "allow-task-view",
            "allow-task-load",
            "allow-task-config",
            "allow-task-create",
            "allow-task-open",
            "allow-task-save-to-inbox",
            "allow-task-open-app",
            "allow-task-resize",
            "allow-task-close"
        ]
    );
    for name in ALL.into_iter().filter(|n| *n != "task") {
        for p in permissions(&capability(name)) {
            assert!(!p.starts_with("allow-task-"), "{name} grants {p}");
        }
    }
    // the inbox fallback goes through its own command, not quick capture's
    assert!(!task.iter().any(|p| p.starts_with("allow-capture-")));
}

#[test]
fn every_new_task_command_checks_its_caller() {
    let src = std::fs::read_to_string(format!("{}/src/new_task/mod.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut commands = 0;
    for part in src.split("#[tauri::command]").skip(1) {
        commands += 1;
        let body = part.split_once('{').map(|(_, b)| b).unwrap_or("");
        let first = body.trim_start().lines().next().unwrap_or("");
        assert_eq!(
            first.trim(),
            "caller(&app, &webview)?;",
            "{}",
            part.lines().nth(1).unwrap_or("")
        );
    }
    assert_eq!(
        commands,
        permissions(&capability("task")).len(),
        "one permission per command"
    );
    assert!(src.contains("popup::ensure_caller(app, webview, Popup::Task)"));
}

#[test]
fn server_switching_commands_live_in_their_own_windows_files_only() {
    // a switch takes a saved server's id; adding a server stays the connect flow
    let settings = permissions(&capability("settings"));
    for p in [
        "allow-settings-servers",
        "allow-settings-rename-server",
        "allow-settings-move-server",
        "allow-settings-switch-server",
        "allow-settings-sign-out-server",
        "allow-settings-forget-server",
        "allow-settings-add-server",
    ] {
        assert!(settings.contains(&p.to_string()), "{p}");
    }
    assert!(permissions(&capability("panel")).contains(&"allow-panel-switch-server".to_string()));
    assert!(permissions(&capability("local")).contains(&"allow-switch-to-server".to_string()));
    for name in ALL {
        for p in permissions(&capability(name)) {
            let switching = p.ends_with("-server") || p.ends_with("-servers");
            if !switching {
                continue;
            }
            let own = match name {
                "settings" => p.starts_with("allow-settings-"),
                "panel" => p == "allow-panel-switch-server",
                // the connect screen's existing commands + the new id-based switch
                "local" => matches!(
                    p.as_str(),
                    "allow-check-server"
                        | "allow-use-server"
                        | "allow-switch-server"
                        | "allow-switch-to-server"
                        | "allow-forget-server"
                ),
                _ => false,
            };
            assert!(own, "{name} grants {p}");
        }
    }
}

/// The `#[tauri::command]` functions of `file` whose name matches `pick`, with the first line of
/// their body.
fn commands_in(file: &str, pick: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    let src = std::fs::read_to_string(format!("{}/src/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut out = Vec::new();
    for part in src.split("#[tauri::command]").skip(1) {
        let name = part
            .split("fn ")
            .nth(1)
            .and_then(|r| r.split('(').next())
            .unwrap_or("")
            .trim()
            .to_string();
        if !pick(&name) {
            continue;
        }
        let body = part
            .split_once(") -> ")
            .or_else(|| part.split_once(')'))
            .map(|(_, b)| b)
            .unwrap_or("");
        let body = body.split_once('{').map(|(_, b)| b).unwrap_or("");
        let first = body.trim_start().lines().next().unwrap_or("").trim().to_string();
        out.push((name, first));
    }
    out
}

#[test]
fn every_server_switching_command_checks_its_caller_first() {
    let settings = commands_in("prefs.rs", |n| n.ends_with("_server") || n == "settings_servers");
    assert_eq!(settings.len(), 7, "{settings:?}");
    for (name, first) in settings {
        assert_eq!(
            first, "popup::ensure_caller(&app, &webview, Popup::Settings)?;",
            "{name}"
        );
    }
    let local = commands_in("commands.rs", |n| n == "switch_to_server");
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].1, "ensure_main_screen(&app, &webview)?;");
    // the panel's command is covered by `every_panel_command_checks_its_caller`; it takes an id
    let panel = std::fs::read_to_string(format!("{}/src/panel/mod.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
    assert!(panel.contains("pub fn panel_switch_server(app: AppHandle, webview: Webview, id: String)"));
    // no switch command takes an origin / URL
    for file in ["prefs.rs", "commands.rs", "panel/mod.rs"] {
        let src = std::fs::read_to_string(format!("{}/src/{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        for part in src.split("#[tauri::command]").skip(1) {
            let signature = part.split('{').next().unwrap_or("");
            if signature.contains("switch") {
                assert!(
                    !signature.contains("origin") && !signature.contains("url"),
                    "{signature}"
                );
            }
        }
    }
}

/// The `.rs` files under `dir`, recursively.
fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_command_of_the_manifest_checks_its_caller_first() {
    // codex review 8: every app command (build.rs manifest) starts with an explicit check of the
    // calling window + origin, next to its capability file — none relies on the ACL alone
    let root = env!("CARGO_MANIFEST_DIR");
    let build = std::fs::read_to_string(format!("{root}/build.rs")).unwrap();
    let list = build
        .split(".commands(&[")
        .nth(1)
        .and_then(|r| r.split("])").next())
        .unwrap();
    let manifest: Vec<String> = list
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('"'))
        .map(|l| l.trim_end_matches(',').trim_matches('"').to_string())
        .collect();
    assert!(manifest.len() > 90, "{}", manifest.len());

    let mut files = Vec::new();
    rust_files(std::path::Path::new(&format!("{root}/src")), &mut files);
    let mut commands = std::collections::HashMap::new();
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        for part in src.split("#[tauri::command]").skip(1) {
            let after_fn = part.split_once("fn ").map(|(_, r)| r).unwrap_or("");
            let name = after_fn.split('(').next().unwrap_or("").trim().to_string();
            let body = after_fn.split_once('{').map(|(_, b)| b).unwrap_or("");
            let first = body.trim_start().lines().next().unwrap_or("").trim().to_string();
            let previous = commands.insert(name.clone(), (file.clone(), first));
            assert!(previous.is_none(), "{name} defined twice");
        }
    }

    const CHECKS: [&str; 7] = [
        "ensure_main_screen(&app, &webview)?;",
        "if ensure_main_screen(&app, &webview).is_err() {",
        "popup::ensure_caller(&app, &webview, Popup::",
        "caller(&app, &webview)?;",
        "ensure_current_server(&app, &webview)?;",
        "let origin = ensure_current_server(&app, &webview)?;",
        "topbar(&app, &webview)?;",
    ];
    for name in &manifest {
        let (file, first) = commands
            .get(name)
            .unwrap_or_else(|| panic!("{name} is in build.rs but no #[tauri::command]"));
        assert!(
            CHECKS.iter().any(|c| first.starts_with(c)),
            "{name} ({}) starts with `{first}`, not a caller check",
            file.display()
        );
        // a module's own `caller` helper is the popup check of that window
        if first.starts_with("caller(") {
            let src = std::fs::read_to_string(file).unwrap();
            let helper = src.split("fn caller(").nth(1).unwrap_or("");
            let helper = helper.split("\n}").next().unwrap_or("");
            assert!(
                helper.contains("popup::ensure_caller(app, webview, Popup::"),
                "{}: caller() is not the popup check",
                file.display()
            );
        }
    }
    // and no command exists outside the manifest (it would be unreachable or ungoverned)
    for name in commands.keys() {
        assert!(manifest.contains(name), "{name} is a command but not in build.rs");
    }
    // the top bar's check: the `topbar` webview on a bundled origin
    let tabs_rs = std::fs::read_to_string(format!("{root}/src/tabs.rs")).unwrap();
    let check = tabs_rs
        .split("fn topbar(")
        .nth(1)
        .unwrap()
        .split(
            "
}",
        )
        .next()
        .unwrap();
    assert!(check.contains("links::is_local_caller(webview.label(), &url, TOPBAR,"));
    // the main screens' check: the main window on a bundled origin
    let commands_rs = std::fs::read_to_string(format!("{root}/src/commands.rs")).unwrap();
    let check = commands_rs.split("fn ensure_main_screen(").nth(1).unwrap();
    assert!(check
        .split("\n}")
        .next()
        .unwrap()
        .contains("crate::links::is_local_caller("));
    assert!(check.contains("crate::shell::MAIN"));
    // the bridge's check: an app window on the current origin
    let bridge = std::fs::read_to_string(format!("{root}/src/bridge.rs")).unwrap();
    assert!(bridge.contains("if !is_bridge_caller(webview.label(), &url, &origin)"));
}
