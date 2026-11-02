//! The WinRT side of the Store build (Windows + `msstore` only), see `mod.rs`. Each function
//! first asks whether the app runs packaged (an MSIX install); an unpackaged run of the Store
//! binary (`target\release`) behaves like the normal build. None of these calls shows UI; the
//! asynchronous ones are waited for on a thread of their own, never on the UI thread.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tauri::{AppHandle, Manager};
use windows::core::HSTRING;
use windows::ApplicationModel::Activation::ActivationKind;
use windows::ApplicationModel::{AppInstance, Package, StartupTask};

use super::{after_enable, aumid, outside_path, startup_step, StartupState, StartupStep, STARTUP_TASK_ID};

/// Runs a WinRT call on a fresh thread (COM's multithreaded apartment, which windows-rs joins on
/// its own), so a wait on an async operation never blocks or re-enters the UI thread.
fn off_thread<T: Send + 'static>(f: impl FnOnce() -> windows::core::Result<T> + Send + 'static) -> Result<T, String> {
    std::thread::spawn(move || f().map_err(|e| e.to_string()))
        .join()
        .unwrap_or_else(|_| Err("the WinRT call failed".into()))
}

/// `Package.Current.Id.FamilyName`; `None` when the app runs without package identity. Asked
/// once per run (identity can't change while the process lives).
pub fn package_family_name() -> Option<String> {
    static FAMILY: OnceLock<Option<String>> = OnceLock::new();
    FAMILY
        .get_or_init(|| {
            off_thread(|| Package::Current()?.Id()?.FamilyName())
                .ok()
                .map(|family| family.to_string_lossy())
        })
        .clone()
}

fn startup_task() -> windows::core::Result<StartupTask> {
    StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK_ID))?.join()
}

pub fn startup_task_state() -> Option<StartupState> {
    package_family_name()?;
    match off_thread(|| startup_task()?.State()) {
        Ok(state) => Some(StartupState::from_raw(state.0)),
        Err(e) => {
            log::warn!("startup task {STARTUP_TASK_ID}: {e}");
            Some(StartupState::Unknown(-1))
        }
    }
}

pub fn set_startup_task(on: bool) -> Option<Result<(), String>> {
    package_family_name()?;
    let result = off_thread(move || {
        let task = startup_task()?;
        let state = StartupState::from_raw(task.State()?.0);
        Ok(match startup_step(state, on) {
            StartupStep::Nothing => Ok(()),
            // no prompt for a full-trust app; a task the user turned off stays off
            StartupStep::Enable => {
                after_enable(StartupState::from_raw(task.RequestEnableAsync()?.join()?.0)).map_err(str::to_string)
            }
            StartupStep::Disable => {
                task.Disable()?;
                Ok(())
            }
            StartupStep::Refuse(why) => Err(why.to_string()),
        })
    });
    Some(match result {
        Ok(outcome) => outcome,
        Err(e) => {
            log::warn!("startup task {STARTUP_TASK_ID}: {e}");
            Err(format!("The login item could not be changed: {e}"))
        }
    })
}

/// A startup task starts the app without arguments, so `--autostart` can't tell; the
/// activation kind can.
pub fn launched_by_startup_task() -> bool {
    if package_family_name().is_none() {
        return false;
    }
    off_thread(|| AppInstance::GetActivatedEventArgs()?.Kind())
        .map(|kind| kind == ActivationKind::StartupTask)
        .unwrap_or(false)
}

/// A toast from the package's identity (`<family>!Brainitive`). A click while it is on screen
/// does what a click on the plugin's notification does; a click in the notification centre
/// starts / focuses the app (single instance).
pub fn show_toast(app: &AppHandle, id: i32, title: &str, body: &str) -> bool {
    let Some(family) = package_family_name() else {
        return false;
    };
    let app = app.clone();
    let (title, body) = (title.to_string(), body.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        let on_click = app.clone();
        let toast = tauri_winrt_notification::Toast::new(&aumid(&family))
            .title(&title)
            .text1(&body)
            .on_activated(move |_action| {
                crate::bridge::on_notification_action(&on_click, "tap", Some(id));
                Ok(())
            });
        if let Err(e) = toast.show() {
            log::warn!("notification failed: {e}");
        }
    });
    true
}

/// `path` as Explorer sees it: new files the packaged app wrote under AppData are in the
/// package's private copy.
pub fn outside_view(app: &AppHandle, path: &Path) -> PathBuf {
    let paths = app.path();
    let (Some(family), Ok(local), Ok(roaming)) = (package_family_name(), paths.local_data_dir(), paths.data_dir())
    else {
        return path.to_path_buf();
    };
    outside_path(path, &local, &roaming, &family, Path::exists)
}
