//! What the app remembers between launches (`settings.json` in the app config dir): the
//! current server, up to five recent servers, webview data stores still to be deleted, and the
//! desktop preferences (shortcuts, tray, downloads, zoom). No secrets here — tokens live in the
//! keychain (`secrets.rs`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::server::ServerOrigin;

pub const MAX_RECENT: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentServer {
    pub origin: ServerOrigin,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub last_used: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub current: Option<ServerOrigin>,
    #[serde(default)]
    pub recent: Vec<RecentServer>,
    /// Hex ids of webview data stores of forgotten servers, deleted once nothing uses them.
    #[serde(default)]
    pub pending_wipes: Vec<String>,
    /// Desktop preferences (Settings window). Launch at login is not stored here: the OS
    /// (LaunchAgent / registry / autostart file) is the truth for it.
    #[serde(default)]
    pub preferences: Preferences,
}

/// What the Settings window changes. Every field has a default, so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Global quick-capture shortcut (accelerator string); `None` = off.
    pub capture_shortcut: Option<String>,
    /// Global quick-switcher shortcut; `None` = off.
    pub switcher_shortcut: Option<String>,
    /// Launched at login: stay in the tray / menu bar instead of opening the window.
    pub start_hidden: bool,
    /// Closing the main window hides it; the app keeps running in the tray / menu bar.
    pub keep_in_tray: bool,
    /// Downloads open a Save dialog (off: straight to the Downloads folder).
    pub ask_where_to_save: bool,
    /// Zoom level of the app windows (1.0 = 100 %).
    pub zoom: f64,
    /// Voice capture (BRAI-439): off until the user turns it on (Settings, or the first press of
    /// its shortcut).
    pub voice_enabled: bool,
    /// Global voice-capture shortcut (hold to talk); `None` = off.
    pub voice_shortcut: Option<String>,
    /// Spoken language: "auto" or a code of `voice::LANGUAGES`.
    pub voice_language: String,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            capture_shortcut: Some(crate::shortcuts::DEFAULT_CAPTURE.to_string()),
            switcher_shortcut: Some(crate::shortcuts::DEFAULT_SWITCHER.to_string()),
            start_hidden: true,
            keep_in_tray: true,
            ask_where_to_save: true,
            zoom: 1.0,
            voice_enabled: false,
            voice_shortcut: Some(crate::shortcuts::DEFAULT_VOICE.to_string()),
            voice_language: crate::voice::AUTO_LANGUAGE.to_string(),
        }
    }
}

/// Browser-like zoom steps (View → Zoom In / Zoom Out / Actual Size).
pub const ZOOM_STEPS: [f64; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// The next zoom step up (`direction > 0`) or down (`< 0`); `0` resets to 100 %.
pub fn next_zoom(current: f64, direction: i8) -> f64 {
    let current = if current.is_finite() { current } else { 1.0 };
    match direction.signum() {
        0 => 1.0,
        1 => ZOOM_STEPS
            .iter()
            .copied()
            .find(|z| *z > current + 0.001)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1]),
        _ => ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|z| *z < current - 0.001)
            .unwrap_or(ZOOM_STEPS[0]),
    }
}

/// A zoom level read from the file: finite and within the steps' range.
pub fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        1.0
    }
}

impl Settings {
    /// Unreadable or invalid files start fresh (worst case: choose the server again).
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Settings>(&bytes).ok())
            .map(|mut s| {
                s.preferences.zoom = clamp_zoom(s.preferences.zoom);
                if !crate::voice::is_language(&s.preferences.voice_language) {
                    s.preferences.voice_language = crate::voice::AUTO_LANGUAGE.to_string();
                }
                s
            })
            .unwrap_or_default()
    }

    /// Atomic: write a sibling temp file, then rename.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp: PathBuf = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).expect("serializable"))?;
        std::fs::rename(tmp, path)
    }

    /// Makes `origin` current and moves it to the top of the recent list.
    pub fn remember(&mut self, origin: &ServerOrigin, name: &str, version: Option<String>) {
        self.recent.retain(|r| &r.origin != origin);
        self.recent.insert(
            0,
            RecentServer {
                origin: origin.clone(),
                name: name.chars().take(120).collect(),
                version,
                last_used: now(),
            },
        );
        self.recent.truncate(MAX_RECENT);
        self.current = Some(origin.clone());
        // a server chosen again must not lose its fresh data to an old wipe
        let id = crate::server::hex(&origin.store_id());
        self.pending_wipes.retain(|w| *w != id);
    }

    /// Removes the server from the list (and from `current`) and schedules its webview data.
    pub fn forget(&mut self, origin: &ServerOrigin) {
        self.recent.retain(|r| &r.origin != origin);
        if self.current.as_ref() == Some(origin) {
            self.current = None;
        }
        let id = crate::server::hex(&origin.store_id());
        if !self.pending_wipes.contains(&id) {
            self.pending_wipes.push(id);
        }
    }

    pub fn name_of(&self, origin: &ServerOrigin) -> Option<&str> {
        self.recent
            .iter()
            .find(|r| &r.origin == origin)
            .map(|r| r.name.as_str())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(s: &str) -> ServerOrigin {
        ServerOrigin::parse(s).unwrap()
    }

    #[test]
    fn remembers_at_most_five_servers_most_recent_first() {
        let mut s = Settings::default();
        for i in 0..7 {
            s.remember(&o(&format!("s{i}.example.com")), &format!("S{i}"), None);
        }
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0].name, "S6");
        assert_eq!(s.current, Some(o("s6.example.com")));

        s.remember(&o("s3.example.com"), "S3 again", Some("1.49.0".into()));
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0].name, "S3 again");
        assert_eq!(s.recent.iter().filter(|r| r.origin == o("s3.example.com")).count(), 1);
    }

    #[test]
    fn forgetting_schedules_the_webview_data_and_choosing_again_cancels_it() {
        let mut s = Settings::default();
        let a = o("a.example.com");
        s.remember(&a, "A", None);
        s.forget(&a);
        assert!(s.current.is_none());
        assert!(s.recent.is_empty());
        assert_eq!(s.pending_wipes, vec![crate::server::hex(&a.store_id())]);
        s.forget(&a);
        assert_eq!(s.pending_wipes.len(), 1);
        s.remember(&a, "A", None);
        assert!(s.pending_wipes.is_empty());
    }

    #[test]
    fn preferences_default_when_missing_and_keep_an_explicit_off() {
        let s: Settings = serde_json::from_str(r#"{"current":null}"#).unwrap();
        assert_eq!(s.preferences, Preferences::default());
        assert!(s.preferences.keep_in_tray && s.preferences.ask_where_to_save);
        let s: Settings =
            serde_json::from_str(r#"{"preferences":{"capture_shortcut":null,"zoom":1.25}}"#).unwrap();
        assert_eq!(s.preferences.capture_shortcut, None);
        assert_eq!(s.preferences.switcher_shortcut, Preferences::default().switcher_shortcut);
        assert_eq!(s.preferences.zoom, 1.25);
    }

    #[test]
    fn voice_capture_is_off_by_default_and_old_files_get_its_defaults() {
        let p = Preferences::default();
        assert!(!p.voice_enabled);
        assert_eq!(p.voice_shortcut.as_deref(), Some(crate::shortcuts::DEFAULT_VOICE));
        assert_eq!(p.voice_language, "auto");
        // a file from before BRAI-439
        let s: Settings = serde_json::from_str(r#"{"preferences":{"zoom":1.1}}"#).unwrap();
        assert!(!s.preferences.voice_enabled);
        assert_eq!(s.preferences.voice_shortcut, p.voice_shortcut);
        // an explicit choice is kept
        let s: Settings = serde_json::from_str(
            r#"{"preferences":{"voice_enabled":true,"voice_shortcut":null,"voice_language":"sq"}}"#,
        )
        .unwrap();
        assert!(s.preferences.voice_enabled);
        assert_eq!(s.preferences.voice_shortcut, None);
        assert_eq!(s.preferences.voice_language, "sq");
    }

    #[test]
    fn an_unknown_voice_language_in_the_file_falls_back_to_auto() {
        let dir = std::env::temp_dir().join(format!("brainitive-desktop-voice-lang-{}", std::process::id()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, br#"{"preferences":{"voice_language":"../../etc"}}"#).unwrap();
        assert_eq!(Settings::load(&path).preferences.voice_language, "auto");
        std::fs::write(&path, br#"{"preferences":{"voice_language":"de"}}"#).unwrap();
        assert_eq!(Settings::load(&path).preferences.voice_language, "de");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zoom_steps() {
        assert_eq!(next_zoom(1.0, 1), 1.1);
        assert_eq!(next_zoom(1.0, -1), 0.9);
        assert_eq!(next_zoom(1.3, 1), 1.5);
        assert_eq!(next_zoom(1.3, -1), 1.25);
        assert_eq!(next_zoom(2.0, 1), 2.0);
        assert_eq!(next_zoom(0.5, -1), 0.5);
        assert_eq!(next_zoom(1.75, 0), 1.0);
        assert_eq!(next_zoom(f64::NAN, 1), 1.1);
        assert_eq!(clamp_zoom(9.0), 2.0);
        assert_eq!(clamp_zoom(f64::INFINITY), 1.0);
    }

    #[test]
    fn round_trips_and_survives_garbage() {
        let dir = std::env::temp_dir().join(format!("brainitive-desktop-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let mut s = Settings::default();
        s.remember(&o("a.example.com"), "A", None);
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        // an http origin someone typed into the file is rejected on load
        std::fs::write(&path, br#"{"current":"http://evil.example.com"}"#).unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
