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
    assert_eq!(enabled, ["local", "settings", "capture", "switcher", "offline"]);

    for (name, window, prefix) in [
        ("settings", "settings", "allow-settings-"),
        ("capture", "capture", "allow-capture-"),
        ("switcher", "switcher", "allow-switcher-"),
        ("offline", "offline", "allow-offline-"),
    ] {
        let cap = capability(name);
        assert_eq!(cap["identifier"], name);
        assert_eq!(cap["local"], true, "{name}");
        assert!(cap.get("remote").is_none(), "{name} must not reach remote content");
        assert_eq!(cap["windows"], serde_json::json!([window]), "{name}: exactly its own window");
        assert!(cap.get("webviews").is_none());
        for p in permissions(&cap) {
            // the offline window also listens for `offline://changed` (sent to it only)
            let event = name == "offline" && matches!(p.as_str(), "core:event:allow-listen" | "core:event:allow-unlisten");
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
    for name in ["local", "settings", "capture", "switcher", "offline"] {
        for p in permissions(&capability(name)) {
            assert!(!p.contains("brainitive-"), "{name}: bridge command {p}");
            for plugin in ["global-shortcut", "autostart", "dialog", "clipboard", "fs", "opener", "notification", "shell"] {
                assert!(!p.starts_with(&format!("{plugin}:")), "{name}: plugin permission {p}");
            }
            assert!(
                !p.starts_with("core:") || p.starts_with("core:event:"),
                "{name}: {p}"
            );
        }
    }
}

#[test]
fn every_command_of_a_capability_is_in_the_app_manifest() {
    let build = std::fs::read_to_string(format!("{}/build.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
    for name in ["local", "settings", "capture", "switcher", "offline"] {
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
    for name in ["local", "settings", "capture", "switcher"] {
        for p in permissions(&capability(name)) {
            assert!(!p.starts_with("allow-offline-"), "{name} grants {p}");
        }
    }
    // the main window's screens open it, and only they
    let local = permissions(&capability("local"));
    assert!(local.contains(&"allow-open-offline".to_string()));
    assert!(local.contains(&"allow-open-web-offline".to_string()));
}
