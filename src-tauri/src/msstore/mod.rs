//! The Microsoft Store build (BRAI-442): `--features msstore` + `tauri.msstore.conf.json`,
//! packaged as an MSIX by `scripts/build-msix.ps1` (docs/MICROSOFT-STORE.md).
//!
//! What that build does differently:
//! - **No updater**: the plugin is not registered; the Store updates the app. "Check for
//!   Updates…", Settings → Updates and the "Please update" screen open the Store page instead
//!   ([`STORE_URL`]).
//! - **User agent**: the webview's ends in ` BrainitiveDesktop/<version> (MSStore)`, the API
//!   client's in `(windows; MSStore)`.
//! - **Open at login** (packaged): the manifest's startup task ([`STARTUP_TASK_ID`]) through the
//!   WinRT `StartupTask` API; MSIX keeps a registry Run key in the package's private hive, where
//!   Windows never reads it.
//! - **Notifications** (packaged): toasts use the package's AUMID (`<family>!Brainitive`); the
//!   plain identifier the plugin uses is not registered for a packaged app.
//! - **Paths for Explorer** (packaged): new files in AppData land in the package's private copy;
//!   "Open Logs Folder" opens that copy ([`outside_view`]).
//!
//! The rest of the app is the same binary logic. Pure parts here (tested on every OS); the WinRT
//! calls are in `packaged.rs` (Windows + `msstore` only). Without both, the runtime functions
//! below are inert (`None` / `false` / unchanged), so callers need no `cfg`.

use std::path::{Path, PathBuf};

/// Whether this binary is the Microsoft Store build.
pub const STORE_BUILD: bool = cfg!(feature = "msstore");

// The package identity Partner Center reserved (docs/MICROSOFT-STORE-LISTING.md § 7). The
// manifest template carries the same values; `tests/msix_manifest.rs` keeps the three in sync.
pub const PACKAGE_NAME: &str = "Ndize.al.Brainitive";
pub const PUBLISHER: &str = "CN=B7545416-53F6-4408-AB46-55AD313FFC15";
pub const PUBLISHER_DISPLAY_NAME: &str = "Ndize.al";
pub const PACKAGE_FAMILY_NAME: &str = "Ndize.al.Brainitive_x9qbckpnnsjh8";
pub const STORE_ID: &str = "9N08DHPCF7WB";
/// `<Application Id>` in the manifest: the AUMID is `<family name>!<this>`.
pub const APPLICATION_ID: &str = "Brainitive";
/// `<desktop:StartupTask TaskId>` in the manifest.
pub const STARTUP_TASK_ID: &str = "BrainitiveStartup";
/// The app's page in the Store app.
pub const STORE_URL: &str = "ms-windows-store://pdp/?productid=9N08DHPCF7WB";
/// What the user agents carry in the Store build.
pub const STORE_MARK: &str = "MSStore";

/// The marker the webview's user agent ends with: ` BrainitiveDesktop/<version>`, plus
/// ` (MSStore)` in the Store build. The web app matches ` BrainitiveDesktop/<semver>`, so the
/// suffix changes nothing for it.
pub fn webview_ua_marker(version: &str, store: bool) -> String {
    if store {
        format!(" BrainitiveDesktop/{version} ({STORE_MARK})")
    } else {
        format!(" BrainitiveDesktop/{version}")
    }
}

/// The API client's user agent: `BrainitiveDesktop/<version> (<platform>)`, `(windows; MSStore)`
/// in the Store build.
pub fn api_user_agent(version: &str, platform: &str, store: bool) -> String {
    if store {
        format!("BrainitiveDesktop/{version} ({platform}; {STORE_MARK})")
    } else {
        format!("BrainitiveDesktop/{version} ({platform})")
    }
}

/// Where "Download the update" (the "Please update" screen) leads: the Store page in the Store
/// build, the website's download page otherwise.
pub fn update_page(store: bool, download_url: &'static str) -> &'static str {
    if store {
        STORE_URL
    } else {
        download_url
    }
}

/// A packaged app's AUMID: `<package family name>!<application id>`.
pub fn aumid(family_name: &str) -> String {
    format!("{family_name}!{APPLICATION_ID}")
}

// ---- the startup task ("Open at login") ---------------------------------------------------------

/// `Windows.ApplicationModel.StartupTaskState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupState {
    Disabled,
    /// Turned off in Task Manager / Settings → Apps → Startup: only the user can turn it back on.
    DisabledByUser,
    Enabled,
    DisabledByPolicy,
    EnabledByPolicy,
    Unknown(i32),
}

impl StartupState {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            0 => Self::Disabled,
            1 => Self::DisabledByUser,
            2 => Self::Enabled,
            3 => Self::DisabledByPolicy,
            4 => Self::EnabledByPolicy,
            other => Self::Unknown(other),
        }
    }

    /// What Settings shows as "Open Brainitive when you log in".
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled | Self::EnabledByPolicy)
    }
}

/// What turning "Open at login" on / off does with the startup task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStep {
    /// Already as wanted.
    Nothing,
    /// `RequestEnableAsync` (no prompt for a full-trust app).
    Enable,
    /// `Disable`.
    Disable,
    /// Windows doesn't let the app change it; the message says where to.
    Refuse(&'static str),
}

pub const STARTUP_TURNED_OFF_BY_USER: &str =
    "Windows has Brainitive turned off as a startup app. Turn it on in Settings → Apps → Startup, or in Task Manager → Startup apps.";
pub const STARTUP_POLICY_OFF: &str = "Your organization doesn't allow Brainitive to open at login.";
pub const STARTUP_POLICY_ON: &str = "Your organization opens Brainitive at login; it can't be turned off here.";
pub const STARTUP_UNKNOWN: &str = "Windows reported an unknown state for the startup task.";

pub fn startup_step(state: StartupState, want: bool) -> StartupStep {
    use StartupState::*;
    match (state, want) {
        (Enabled | EnabledByPolicy, true) | (Disabled | DisabledByUser | DisabledByPolicy, false) => {
            StartupStep::Nothing
        }
        (Disabled, true) => StartupStep::Enable,
        (Enabled, false) => StartupStep::Disable,
        (DisabledByUser, true) => StartupStep::Refuse(STARTUP_TURNED_OFF_BY_USER),
        (DisabledByPolicy, true) => StartupStep::Refuse(STARTUP_POLICY_OFF),
        (EnabledByPolicy, false) => StartupStep::Refuse(STARTUP_POLICY_ON),
        (Unknown(_), _) => StartupStep::Refuse(STARTUP_UNKNOWN),
    }
}

/// The answer of `RequestEnableAsync`: `Ok` when the task is on now, otherwise why not.
pub fn after_enable(state: StartupState) -> Result<(), &'static str> {
    match state {
        s if s.is_enabled() => Ok(()),
        StartupState::DisabledByUser => Err(STARTUP_TURNED_OFF_BY_USER),
        StartupState::DisabledByPolicy => Err(STARTUP_POLICY_OFF),
        _ => Err(STARTUP_UNKNOWN),
    }
}

// ---- AppData as other processes see it ----------------------------------------------------------

/// Where a file the packaged app wrote under AppData really is, for a process outside the
/// package (Explorer): MSIX writes new files under `%LOCALAPPDATA%` / `%APPDATA%` to
/// `%LOCALAPPDATA%\Packages\<family>\LocalCache\Local` / `…\Roaming`. `None` for a path outside
/// AppData (it is real as it is).
pub fn package_copy(path: &Path, local_appdata: &Path, roaming_appdata: &Path, family_name: &str) -> Option<PathBuf> {
    let cache = local_appdata.join("Packages").join(family_name).join("LocalCache");
    // Roaming first: it can't be under Local, but be exact about prefixes either way
    if let Ok(rest) = path.strip_prefix(roaming_appdata) {
        return Some(cache.join("Roaming").join(rest));
    }
    if path.starts_with(&cache) {
        return None;
    }
    path.strip_prefix(local_appdata)
        .ok()
        .map(|rest| cache.join("Local").join(rest))
}

/// The path to hand to Explorer: the package's copy when it exists (the app made the folder
/// while packaged), else the real one (made earlier by an unpackaged install, which Windows
/// keeps using).
pub fn outside_path(
    path: &Path,
    local_appdata: &Path,
    roaming_appdata: &Path,
    family_name: &str,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    match package_copy(path, local_appdata, roaming_appdata, family_name) {
        Some(copy) if exists(&copy) => copy,
        _ => path.to_path_buf(),
    }
}

// ---- runtime (Windows + msstore; inert elsewhere) -----------------------------------------------

#[cfg(all(windows, feature = "msstore"))]
mod packaged;

#[cfg(all(windows, feature = "msstore"))]
pub use packaged::{
    launched_by_startup_task, outside_view, package_family_name, set_startup_task, show_toast, startup_task_state,
};

/// The package family name when the app runs packaged (MSIX); `None` otherwise.
#[cfg(not(all(windows, feature = "msstore")))]
pub fn package_family_name() -> Option<String> {
    None
}

/// The startup task's state when packaged; `None`: use the autostart plugin.
#[cfg(not(all(windows, feature = "msstore")))]
pub fn startup_task_state() -> Option<StartupState> {
    None
}

/// Turns the startup task on / off when packaged; `None`: use the autostart plugin.
#[cfg(not(all(windows, feature = "msstore")))]
pub fn set_startup_task(_on: bool) -> Option<Result<(), String>> {
    None
}

/// Whether Windows started the app through its startup task (packaged only).
#[cfg(not(all(windows, feature = "msstore")))]
pub fn launched_by_startup_task() -> bool {
    false
}

/// Shows a toast with the package's AUMID when packaged (`true`); `false`: use the plugin.
#[cfg(not(all(windows, feature = "msstore")))]
pub fn show_toast(_app: &tauri::AppHandle, _id: i32, _title: &str, _body: &str) -> bool {
    false
}

/// `path` as a process outside the package sees it (unchanged when not packaged).
#[cfg(not(all(windows, feature = "msstore")))]
pub fn outside_view(_app: &tauri::AppHandle, path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_build_is_the_msstore_feature() {
        assert_eq!(STORE_BUILD, cfg!(feature = "msstore"));
    }

    #[test]
    fn the_identity_is_the_one_partner_center_reserved() {
        assert_eq!(PACKAGE_NAME, "Ndize.al.Brainitive");
        assert_eq!(PUBLISHER, "CN=B7545416-53F6-4408-AB46-55AD313FFC15");
        assert_eq!(PUBLISHER_DISPLAY_NAME, "Ndize.al");
        assert!(PACKAGE_FAMILY_NAME.starts_with(&format!("{PACKAGE_NAME}_")));
        assert!(STORE_URL.ends_with(&format!("productid={STORE_ID}")));
        assert!(STORE_URL.starts_with("ms-windows-store://pdp/"));
        assert_eq!(
            aumid(PACKAGE_FAMILY_NAME),
            "Ndize.al.Brainitive_x9qbckpnnsjh8!Brainitive"
        );
    }

    #[test]
    fn user_agents_carry_the_store_mark_only_in_the_store_build() {
        assert_eq!(webview_ua_marker("1.0.0", false), " BrainitiveDesktop/1.0.0");
        assert_eq!(webview_ua_marker("1.0.0", true), " BrainitiveDesktop/1.0.0 (MSStore)");
        assert_eq!(
            api_user_agent("1.0.0", "windows", false),
            "BrainitiveDesktop/1.0.0 (windows)"
        );
        assert_eq!(
            api_user_agent("1.0.0", "windows", true),
            "BrainitiveDesktop/1.0.0 (windows; MSStore)"
        );
        // the web app's pattern (resources/js/lib/desktop.ts) still finds the version
        let ua = format!(
            "Mozilla/5.0 (Windows NT 10.0){}",
            webview_ua_marker("1.2.3-beta.1", true)
        );
        let at = ua.find(" BrainitiveDesktop/").unwrap() + " BrainitiveDesktop/".len();
        assert_eq!(ua[at..].split(' ').next(), Some("1.2.3-beta.1"));
    }

    #[test]
    fn the_update_page_is_the_store_in_the_store_build() {
        let web = "https://www.brainitive.app/download";
        assert_eq!(update_page(false, web), web);
        assert_eq!(update_page(true, web), STORE_URL);
    }

    #[test]
    fn startup_states_map_from_winrt() {
        assert_eq!(StartupState::from_raw(0), StartupState::Disabled);
        assert_eq!(StartupState::from_raw(1), StartupState::DisabledByUser);
        assert_eq!(StartupState::from_raw(2), StartupState::Enabled);
        assert_eq!(StartupState::from_raw(3), StartupState::DisabledByPolicy);
        assert_eq!(StartupState::from_raw(4), StartupState::EnabledByPolicy);
        assert_eq!(StartupState::from_raw(9), StartupState::Unknown(9));
        assert!(StartupState::Enabled.is_enabled());
        assert!(StartupState::EnabledByPolicy.is_enabled());
        assert!(!StartupState::DisabledByUser.is_enabled());
        assert!(!StartupState::Unknown(9).is_enabled());
    }

    #[test]
    fn the_startup_task_decision() {
        use StartupState::*;
        assert_eq!(startup_step(Disabled, true), StartupStep::Enable);
        assert_eq!(startup_step(Enabled, false), StartupStep::Disable);
        assert_eq!(startup_step(Enabled, true), StartupStep::Nothing);
        assert_eq!(startup_step(EnabledByPolicy, true), StartupStep::Nothing);
        assert_eq!(startup_step(Disabled, false), StartupStep::Nothing);
        assert_eq!(startup_step(DisabledByUser, false), StartupStep::Nothing);
        assert_eq!(startup_step(DisabledByPolicy, false), StartupStep::Nothing);
        // only the user (or the policy) can change these: say where, don't pretend
        assert_eq!(
            startup_step(DisabledByUser, true),
            StartupStep::Refuse(STARTUP_TURNED_OFF_BY_USER)
        );
        assert_eq!(
            startup_step(DisabledByPolicy, true),
            StartupStep::Refuse(STARTUP_POLICY_OFF)
        );
        assert_eq!(
            startup_step(EnabledByPolicy, false),
            StartupStep::Refuse(STARTUP_POLICY_ON)
        );
        assert_eq!(startup_step(Unknown(7), true), StartupStep::Refuse(STARTUP_UNKNOWN));
        assert_eq!(after_enable(Enabled), Ok(()));
        assert_eq!(after_enable(DisabledByUser), Err(STARTUP_TURNED_OFF_BY_USER));
        assert_eq!(after_enable(DisabledByPolicy), Err(STARTUP_POLICY_OFF));
        assert_eq!(after_enable(Disabled), Err(STARTUP_UNKNOWN));
    }

    #[test]
    fn appdata_paths_map_to_the_package_copy() {
        let local = Path::new("/u/AppData/Local");
        let roaming = Path::new("/u/AppData/Roaming");
        let family = PACKAGE_FAMILY_NAME;
        let cache = local.join("Packages").join(family).join("LocalCache");
        assert_eq!(
            package_copy(&local.join("app.brainitive.desktop/logs"), local, roaming, family),
            Some(cache.join("Local").join("app.brainitive.desktop/logs"))
        );
        assert_eq!(
            package_copy(&roaming.join("app.brainitive.desktop"), local, roaming, family),
            Some(cache.join("Roaming").join("app.brainitive.desktop"))
        );
        // outside AppData, or already the package copy: real as it is
        assert_eq!(
            package_copy(Path::new("/u/Downloads/a.pdf"), local, roaming, family),
            None
        );
        assert_eq!(package_copy(&cache.join("Local/x"), local, roaming, family), None);
        // a sibling that only shares the prefix text is not inside
        assert_eq!(
            package_copy(Path::new("/u/AppData/LocalLow/x"), local, roaming, family),
            None
        );
    }

    #[test]
    fn explorer_gets_the_copy_that_exists() {
        let local = Path::new("/u/AppData/Local");
        let roaming = Path::new("/u/AppData/Roaming");
        let logs = local.join("app.brainitive.desktop/logs");
        let copy = package_copy(&logs, local, roaming, PACKAGE_FAMILY_NAME).unwrap();
        assert_eq!(outside_path(&logs, local, roaming, PACKAGE_FAMILY_NAME, |_| true), copy);
        // made by an earlier unpackaged install: Windows keeps using the real folder
        assert_eq!(
            outside_path(&logs, local, roaming, PACKAGE_FAMILY_NAME, |_| false),
            logs
        );
    }
}
