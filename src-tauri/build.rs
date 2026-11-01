fn main() {
    // An app ACL manifest: every app command needs an explicit permission (`allow-<command>`),
    // granted per origin in capabilities/ (local screens) and at runtime (the server's origin,
    // shell::allow_bridge_for). Without it Tauri would allow all app commands for local content.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "get_state",
            "check_server",
            "use_server",
            "start_sign_in",
            "cancel_sign_in",
            "retry_connect",
            "open_offline",
            "switch_server",
            "sign_out",
            "forget_server",
            "dismiss_notice",
            "brainitive_notify",
            "brainitive_set_badge",
            "brainitive_open_external",
            "brainitive_sign_out",
            // the Settings window (capabilities/settings.json)
            "settings_get",
            "settings_open_account",
            "settings_reset_zoom",
            "settings_close",
            "settings_set_shortcut",
            "settings_record_shortcut",
            // the quick-capture window (capabilities/capture.json)
            "capture_context",
            "capture_submit",
            "capture_close",
        ])),
    )
    .expect("failed to run tauri-build");
}
