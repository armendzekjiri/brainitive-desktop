//! Brainitive Desktop: a Tauri 2 shell around a Brainitive server (Cloud or self-hosted).
//! See docs/ARCHITECTURE.md (modules, flows) and docs/SECURITY.md (threat model).

pub mod api;
pub mod badge;
pub mod bridge;
pub mod capture;
pub mod commands;
pub mod flows;
pub mod hotkeys;
pub mod links;
pub mod menu;
pub mod pkce;
pub mod poll;
pub mod popup;
pub mod prefs;
pub mod queue;
pub mod secrets;
pub mod server;
pub mod session;
pub mod settings;
pub mod shell;
pub mod shortcuts;
pub mod state;
pub mod tray;
pub mod version;

use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_notification::NotificationExt;

use crate::state::{state, AppState};

pub fn run() {
    let app = tauri::Builder::default()
        // first: a second launch (or a deep link on Windows / Linux) goes to the running app
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            shell::focus_main(app)
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(hotkeys::on_shortcut)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
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
            commands::open_offline,
            commands::switch_server,
            commands::sign_out,
            commands::forget_server,
            commands::dismiss_notice,
            bridge::brainitive_notify,
            bridge::brainitive_set_badge,
            bridge::brainitive_open_external,
            bridge::brainitive_sign_out,
            prefs::settings_get,
            prefs::settings_open_account,
            prefs::settings_reset_zoom,
            prefs::settings_close,
            prefs::settings_set_shortcut,
            prefs::settings_record_shortcut,
            prefs::settings_set_toggle,
            capture::capture_context,
            capture::capture_submit,
            capture::capture_close,
        ])
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let queue_path = app.path().app_local_data_dir()?.join("capture-queue.json");
            app.manage(AppState::new(settings_path, queue_path));
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
