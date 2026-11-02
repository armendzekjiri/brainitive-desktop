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

/// Every capability file (= every bundled window + the main window's screens).
const ALL: [&str; 8] = [
    "local", "settings", "capture", "switcher", "offline", "voice", "leader", "panel",
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
        ["local", "settings", "capture", "switcher", "offline", "voice", "leader", "panel"]
    );

    for (name, window, prefix) in [
        ("settings", "settings", "allow-settings-"),
        ("capture", "capture", "allow-capture-"),
        ("switcher", "switcher", "allow-switcher-"),
        ("offline", "offline", "allow-offline-"),
        ("voice", "voice", "allow-voice-"),
        ("leader", "leader", "allow-leader-"),
        ("panel", "panel", "allow-panel-"),
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

    // the connect / sign-in screens: the main window only
    let local = capability("local");
    assert_eq!(local["windows"], serde_json::json!(["main"]));
    assert!(local.get("remote").is_none());
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
