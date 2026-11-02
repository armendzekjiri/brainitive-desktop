fn main() {
    // An app ACL manifest: every app command needs an explicit permission (`allow-<command>`),
    // granted per origin in capabilities/ (local screens) and at runtime (the server's origin,
    // shell::allow_bridge_for). Without it Tauri would allow all app commands for local content.
    let attributes = windows_manifest_for_all_targets(tauri_build::Attributes::new());
    tauri_build::try_build(attributes.app_manifest(tauri_build::AppManifest::new().commands(&[
        "get_state",
        "check_server",
        "use_server",
        "start_sign_in",
        "cancel_sign_in",
        "retry_connect",
        "open_download_page",
        "log_client_error",
        "open_offline",
        "open_web_offline",
        "switch_server",
        "switch_to_server",
        "sign_out",
        "forget_server",
        "dismiss_notice",
        // the "Please update" screen (BRAI-373)
        "check_for_update",
        "restart_to_update",
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
        "settings_set_leader_letter",
        "settings_record_shortcut",
        "settings_set_toggle",
        "settings_set_voice_language",
        "settings_remove_voice_model",
        "settings_set_update_channel",
        "settings_check_for_updates",
        "settings_restart_to_update",
        // Settings → Servers
        "settings_servers",
        "settings_rename_server",
        "settings_move_server",
        "settings_switch_server",
        "settings_sign_out_server",
        "settings_forget_server",
        "settings_add_server",
        // the quick-capture window (capabilities/capture.json)
        "capture_context",
        "capture_submit",
        "capture_close",
        "capture_voice",
        // the quick-switcher window (capabilities/switcher.json)
        "switcher_context",
        "switcher_search",
        "switcher_open",
        "switcher_copy_snippet",
        "switcher_start_timer",
        "switcher_close",
        // the offline window (capabilities/offline.json, BRAI-366)
        "offline_view",
        "offline_set_done",
        "offline_my_day_add",
        "offline_add_task",
        "offline_update_task",
        "offline_add_capture",
        "offline_sync_now",
        "offline_dismiss_problem",
        "offline_open_app",
        // the voice-capture indicator (capabilities/voice.json, BRAI-439)
        "voice_view",
        "voice_start",
        "voice_stop",
        "voice_cancel",
        "voice_turn_on",
        "voice_download",
        "voice_open_privacy",
        "voice_close",
        "voice_release",
        // the Brainitive key overlay (capabilities/leader.json, BRAI-440)
        "leader_view",
        "leader_press",
        "leader_release",
        "leader_close",
        // the menu-bar panel (capabilities/panel.json)
        "panel_view",
        "panel_load",
        "panel_set_tab",
        "panel_set_done",
        "panel_open",
        "panel_mark_all_read",
        "panel_timer_start",
        "panel_timer_stop",
        "panel_timer_discard",
        "panel_run",
        "panel_resize",
        "panel_close",
        "panel_switch_server",
        // the New task window (capabilities/task.json)
        "task_view",
        "task_load",
        "task_config",
        "task_create",
        "task_open",
        "task_save_to_inbox",
        "task_open_app",
        "task_resize",
        "task_close",
    ])))
    .expect("failed to run tauri-build");
}

/// Tauri embeds its Windows manifest (Common-Controls v6) only into the app binary, so the unit
/// test binary starts without it and Windows refuses to load it (STATUS_ENTRYPOINT_NOT_FOUND,
/// `TaskDialogIndirect` exists only in v6). Linking the same manifest for every target fixes tests
/// and keeps the app unchanged.
fn windows_manifest_for_all_targets(attributes: tauri_build::Attributes) -> tauri_build::Attributes {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return attributes;
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    attributes.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
}
