//! Brainitive Desktop: a Tauri 2 shell around a Brainitive server (Cloud or self-hosted).
//! See docs/ARCHITECTURE.md (modules, flows) and docs/SECURITY.md (threat model).

pub mod api;
pub mod badge;
pub mod bridge;
pub mod capture;
pub mod commands;
pub mod diagnostics;
pub mod downloads;
pub mod flows;
pub mod hotkeys;
pub mod leader;
pub mod links;
pub mod logging;
pub mod menu;
pub mod msstore;
pub mod new_task;
pub mod offline;
pub mod offline_cache;
pub mod outbox;
pub mod panel;
pub mod pkce;
pub mod poll;
pub mod popup;
pub mod prefs;
pub mod private_fs;
pub mod problem;
pub mod queue;
pub mod reconnect;
pub mod redact;
pub mod secrets;
pub mod server;
pub mod servers;
pub mod session;
pub mod settings;
pub mod shell;
pub mod shortcuts;
pub mod startup;
pub mod state;
pub mod switcher;
pub mod tray;
pub mod updater;
pub mod version;
pub mod voice;

use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_notification::NotificationExt;

use crate::state::{state, AppState};

pub fn run() {
    startup::begin();
    let builder = tauri::Builder::default()
        // first: a second launch (or a deep link on Windows / Linux) goes to the running app
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            shell::focus_main(app)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // launch at login (BRAI-364): off until the user turns it on in Settings; the login
        // item starts the app with AUTOSTART_ARG so it can stay in the tray
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![prefs::AUTOSTART_ARG]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(hotkeys::on_shortcut)
                .build(),
        )
        .plugin(tauri_plugin_notification::init());
    // auto-updates (BRAI-373): Rust only (`updater.rs`), no webview gets its commands; the
    // public key is `plugins.updater.pubkey` (a placeholder switches the updater off). The
    // Microsoft Store build has no updater at all: the Store updates it (BRAI-442).
    #[cfg(not(feature = "msstore"))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    let app = builder
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // the bundled popups open centred; only app windows keep their size / position
                .with_denylist(&popup::labels())
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::check_server,
            commands::use_server,
            commands::start_sign_in,
            commands::cancel_sign_in,
            commands::retry_connect,
            commands::open_download_page,
            commands::log_client_error,
            commands::open_offline,
            commands::open_web_offline,
            commands::switch_server,
            commands::switch_to_server,
            commands::sign_out,
            commands::forget_server,
            commands::dismiss_notice,
            commands::check_for_update,
            commands::restart_to_update,
            bridge::brainitive_notify,
            bridge::brainitive_set_badge,
            bridge::brainitive_open_external,
            bridge::brainitive_sign_out,
            prefs::settings_get,
            prefs::settings_open_account,
            prefs::settings_reset_zoom,
            prefs::settings_close,
            prefs::settings_set_shortcut,
            prefs::settings_set_leader_letter,
            prefs::settings_record_shortcut,
            prefs::settings_set_toggle,
            prefs::settings_set_voice_language,
            prefs::settings_remove_voice_model,
            prefs::settings_set_update_channel,
            prefs::settings_check_for_updates,
            prefs::settings_restart_to_update,
            prefs::settings_servers,
            prefs::settings_rename_server,
            prefs::settings_move_server,
            prefs::settings_switch_server,
            prefs::settings_sign_out_server,
            prefs::settings_forget_server,
            prefs::settings_add_server,
            capture::capture_context,
            capture::capture_submit,
            capture::capture_close,
            capture::capture_voice,
            switcher::switcher_context,
            switcher::switcher_search,
            switcher::switcher_open,
            switcher::switcher_copy_snippet,
            switcher::switcher_start_timer,
            switcher::switcher_close,
            offline::offline_view,
            offline::offline_set_done,
            offline::offline_my_day_add,
            offline::offline_add_task,
            offline::offline_update_task,
            offline::offline_add_capture,
            offline::offline_sync_now,
            offline::offline_dismiss_problem,
            offline::offline_open_app,
            voice::voice_view,
            voice::voice_start,
            voice::voice_stop,
            voice::voice_cancel,
            voice::voice_turn_on,
            voice::voice_download,
            voice::voice_open_privacy,
            voice::voice_close,
            voice::voice_release,
            leader::leader_view,
            leader::leader_press,
            leader::leader_release,
            leader::leader_close,
            panel::panel_view,
            panel::panel_load,
            panel::panel_set_tab,
            panel::panel_set_done,
            panel::panel_open,
            panel::panel_mark_all_read,
            panel::panel_timer_start,
            panel::panel_timer_stop,
            panel::panel_timer_discard,
            panel::panel_run,
            panel::panel_resize,
            panel::panel_close,
            panel::panel_switch_server,
            new_task::task_view,
            new_task::task_load,
            new_task::task_config,
            new_task::task_create,
            new_task::task_open,
            new_task::task_save_to_inbox,
            new_task::task_open_app,
            new_task::task_resize,
            new_task::task_close,
        ])
        .setup(|app| {
            // first: everything after this is logged, and a panic leaves a trace
            match app.path().app_log_dir() {
                Ok(dir) => {
                    if let Err(e) = logging::init(&dir) {
                        eprintln!("could not open the log file in {}: {e}", dir.display());
                    }
                }
                Err(e) => eprintln!("no log directory: {e}"),
            }
            log::info!(
                "Brainitive Desktop {} starting ({}, {}{})",
                version::APP_VERSION,
                std::env::consts::OS,
                std::env::consts::ARCH,
                match (msstore::STORE_BUILD, msstore::package_family_name()) {
                    (false, _) => String::new(),
                    (true, Some(family)) => format!(", Microsoft Store build, package {family}"),
                    (true, None) => ", Microsoft Store build, not packaged".to_string(),
                }
            );
            startup::mark("setup entered");
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let data_dir = app.path().app_local_data_dir()?;
            let queue_path = data_dir.join("capture-queue.json");
            app.manage(AppState::new(settings_path, queue_path, data_dir.join("offline")));
            // voice capture (BRAI-439): its own state; the model lives in <local data>/voice/
            voice::init(app.handle(), data_dir.clone());
            {
                let s = state(app.handle());
                let mut inner = s.lock();
                // the MSIX startup task starts the app without `--autostart` (BRAI-442)
                let at_login = prefs::launched_at_login(std::env::args()) || msstore::launched_by_startup_task();
                inner.start_hidden = at_login && inner.settings.preferences.start_hidden;
            }
            startup::mark("settings and queue loaded");
            let handle = app.handle().clone();

            menu::install(&handle)?;
            app.on_menu_event(|app, event| {
                let id = event.id().as_ref();
                if id.starts_with("tray-") {
                    tray::on_menu_event(app, id);
                } else {
                    menu::on_menu_event(app, id);
                }
            });
            tray::install(&handle)?;
            tray::start_poller(&handle);
            updater::init(&handle);
            // a directory removal: off the setup path
            let cleaner = handle.clone();
            tauri::async_runtime::spawn_blocking(move || downloads::clean_staging(&cleaner));
            startup::mark("menu, tray and shortcuts ready");
            if let Err(e) = hotkeys::apply(&handle) {
                log::warn!("{e}");
            }

            // a click on one of our notifications
            let on_click = handle.clone();
            let _ = app.notification().on_action(move |action| {
                let id = action.notification().map(|n| n.id());
                bridge::on_notification_action(&on_click, action.action_id(), id);
            });

            // brainitive:// links (macOS: Apple events; Windows / Linux: forwarded argv)
            #[cfg(any(windows, target_os = "linux"))]
            if tauri::is_dev() {
                // installed builds register the scheme in the installer / .desktop file
                let _ = app.deep_link().register_all();
            }
            let on_link = handle.clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    let h = on_link.clone();
                    tauri::async_runtime::spawn(async move { flows::handle_deep_link(&h, url).await });
                }
            });

            flows::startup(&handle);
            startup::mark("main window created");

            // launched by a link
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for url in urls {
                    let h = handle.clone();
                    tauri::async_runtime::spawn(async move { flows::handle_deep_link(&h, url).await });
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Brainitive Desktop");

    app.run(|app, event| match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Destroyed,
            ..
        } if label == shell::MAIN => {
            shell::on_main_destroyed(app);
        }
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Focused(focused),
            ..
        } => {
            if focused && shell::is_app_window(&label) {
                state(app).lock().focused_app_window = Some(label);
            } else if !focused {
                popup::on_focus_lost(app, &label);
            }
        }
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::ThemeChanged(theme),
            ..
        } => popup::on_theme_changed(app, &label, theme),
        // "keep running in the tray": closing the main window hides it (the web app keeps
        // running, so its notifications still arrive); the tray / dock icon brings it back
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == shell::MAIN && state(app).lock().settings.preferences.keep_in_tray => {
            api.prevent_close();
            if let Some(w) = shell::main_window(app) {
                #[cfg(target_os = "macos")]
                if w.is_fullscreen().unwrap_or(false) {
                    let _ = w.set_fullscreen(false);
                }
                let _ = w.hide();
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => shell::focus_main(app),
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if popup::Popup::from_label(&label).is_some() => {
            // bundled windows are hidden, not destroyed: they open instantly next time
            api.prevent_close();
            if let Some(p) = popup::Popup::from_label(&label) {
                popup::dismiss(app, p);
            }
        }
        // the main window is being rebuilt with another server's data store
        RunEvent::ExitRequested { api, code: None, .. } if state(app).lock().rebuilding => {
            api.prevent_exit();
        }
        _ => {}
    });
}
