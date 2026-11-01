//! Brainitive Desktop: a Tauri 2 shell around a Brainitive server (Cloud or self-hosted).
//! See docs/ARCHITECTURE.md (modules, flows) and docs/SECURITY.md (threat model).

pub mod api;
pub mod bridge;
pub mod commands;
pub mod flows;
pub mod links;
pub mod pkce;
pub mod secrets;
pub mod server;
pub mod session;
pub mod settings;
pub mod shell;
pub mod state;
pub mod version;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
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
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
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
        ])
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            app.manage(AppState::new(settings_path));
            let handle = app.handle().clone();

            install_menu(&handle)?;

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
        // the main window is being rebuilt with another server's data store
        RunEvent::ExitRequested { api, code: None, .. } if state(app).lock().rebuilding => {
            api.prevent_exit();
        }
        _ => {}
    });
}

/// The platform's default menu (Edit is required for copy / paste shortcuts on macOS) plus a
/// "Server" menu: the current server, Switch Server…, Sign Out, Reload.
fn install_menu(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    let current = MenuItem::with_id(app, "server-current", "Not connected", false, None::<&str>)?;
    let switch = MenuItem::with_id(app, "server-switch", "Switch Server…", true, None::<&str>)?;
    let sign_out = MenuItem::with_id(app, "server-sign-out", "Sign Out", true, None::<&str>)?;
    let reload = MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?;
    let server = Submenu::with_items(
        app,
        "Server",
        true,
        &[
            &current,
            &PredefinedMenuItem::separator(app)?,
            &switch,
            &sign_out,
            &PredefinedMenuItem::separator(app)?,
            &reload,
        ],
    )?;
    menu.append(&server)?;
    app.set_menu(menu)?;
    state(app).lock().server_menu_item = Some(current);

    app.on_menu_event(|app, event| match event.id().as_ref() {
        "server-switch" => flows::switch_server(app),
        "server-sign-out" => {
            let h = app.clone();
            tauri::async_runtime::spawn(async move { flows::sign_out(&h).await });
        }
        "reload" => {
            if let Some(w) = shell::main_window(app) {
                let _ = w.reload();
            }
        }
        _ => {}
    });
    Ok(())
}
