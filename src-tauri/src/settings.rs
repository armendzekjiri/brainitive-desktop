//! What the app remembers between launches (`settings.json` in the app config dir): the
//! current server, the saved servers in the user's order, webview data stores still to be deleted, and the
//! desktop preferences (shortcuts, tray, downloads, zoom). No secrets here — tokens live in the
//! keychain (`secrets.rs`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::server::ServerOrigin;

/// A saved server (an account the user switches to; `servers.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentServer {
    pub origin: ServerOrigin,
    /// The server's own name (`/api/v1/instance`), refreshed on every connect.
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub last_used: u64,
    /// The name the user gave it in Settings → Servers (`None`: [`crate::servers::default_name`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
}

impl RecentServer {
    /// What the title, menus, the panel and Settings call it.
    pub fn label(&self) -> String {
        match &self.custom_name {
            Some(name) => name.clone(),
            None => crate::servers::default_name(&self.origin, &self.name),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub current: Option<ServerOrigin>,
    /// The saved servers in the user's order (⌃⌘1…9; the JSON key is from when this was a
    /// "recent" list).
    #[serde(default)]
    pub recent: Vec<RecentServer>,
    /// Hex ids of webview data stores of forgotten servers, deleted once nothing uses them.
    #[serde(default)]
    pub pending_wipes: Vec<String>,
    /// Servers whose sign-out could not delete everything on this computer (keychain entry,
    /// offline files, queued captures; codex review 4): retried at every start, and nothing is
    /// restored from them meanwhile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_sign_outs: Vec<ServerOrigin>,
    /// Desktop preferences (Settings window). Launch at login is not stored here: the OS
    /// (LaunchAgent / registry / autostart file) is the truth for it.
    #[serde(default)]
    pub preferences: Preferences,
    /// Unix seconds of the last update check that got an answer (BRAI-373, shown in Settings).
    #[serde(default)]
    pub last_update_check: Option<u64>,
}

/// What the Settings window changes. Every field has a default, so older files still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// The Brainitive key (BRAI-440): one global shortcut, then a letter in its overlay;
    /// `None` = off.
    pub leader_shortcut: Option<String>,
    /// The letter of each action after the Brainitive key (A–Z, all different).
    #[serde(deserialize_with = "crate::leader::lenient_letters")]
    pub leader_letters: crate::leader::LeaderLetters,
    /// Which shortcut defaults the file was written with: 0 (missing) = before the Brainitive key,
    /// see [`Preferences::migrate`].
    #[serde(default)]
    pub shortcut_scheme: u32,
    /// Global quick-capture shortcut (accelerator string); `None` = off.
    pub capture_shortcut: Option<String>,
    /// Direct quick-switcher shortcut; `None` = off (the default since BRAI-440: ⌃⌘K K).
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
    /// Direct voice-capture shortcut (hold to talk); `None` = off (the default since BRAI-440:
    /// ⌃⌘K, then hold V).
    pub voice_shortcut: Option<String>,
    /// Spoken language: "auto" or a code of `voice::LANGUAGES`.
    pub voice_language: String,
    /// Look for updates 30 s after start and every 6 hours (BRAI-373). "Check for Updates…"
    /// works either way.
    pub auto_update_check: bool,
    /// Stable (default) or beta ("Get beta updates"); an unknown value in the file is stable.
    #[serde(deserialize_with = "crate::updater::lenient_channel")]
    pub update_channel: crate::updater::Channel,
    /// The menu-bar panel's last tab (`panel::view::Tab`; anything unknown opens Today).
    pub panel_tab: String,
    /// The New task window's global shortcut; `None` = off (⌃⌘T / Ctrl+Alt+T, off on Linux).
    pub task_shortcut: Option<String>,
    /// The New task window's last project / board / type per server (origin → ids).
    #[serde(deserialize_with = "crate::new_task::model::lenient_memory")]
    pub new_task: crate::new_task::model::MemoryMap,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            leader_shortcut: Some(crate::shortcuts::DEFAULT_LEADER.to_string()),
            leader_letters: crate::leader::LeaderLetters::default(),
            shortcut_scheme: SHORTCUT_SCHEME,
            capture_shortcut: Some(crate::shortcuts::DEFAULT_CAPTURE.to_string()),
            switcher_shortcut: None,
            start_hidden: true,
            keep_in_tray: true,
            ask_where_to_save: true,
            zoom: 1.0,
            voice_enabled: false,
            voice_shortcut: None,
            voice_language: crate::voice::AUTO_LANGUAGE.to_string(),
            auto_update_check: true,
            update_channel: crate::updater::Channel::Stable,
            panel_tab: "today".to_string(),
            task_shortcut: crate::shortcuts::DEFAULT_TASK.map(str::to_string),
            new_task: Default::default(),
        }
    }
}

/// The shortcut defaults of this version: 1 = the Brainitive key (BRAI-440), 2 = New task.
pub const SHORTCUT_SCHEME: u32 = 2;
/// The quick switcher's default as files before BRAI-440 stored it (untouched, or "Default").
const OLD_DEFAULT_SWITCHER: &str = "CommandOrControl+Alt+K";
/// Voice capture's default as files before BRAI-440 stored it.
const OLD_DEFAULT_VOICE: &str = "CommandOrControl+Shift+KeyV";

impl Preferences {
    /// Brings a file of an older version to the current shortcut defaults; `true` = changed.
    ///
    /// Every field is written to the file, so "never changed" can't be told by a missing field.
    /// It is told by the **exact string**: a default is stored as its constant
    /// (`CommandOrControl+Alt+K`), recorded keys always in the canonical form (`alt+super+KeyK`).
    /// So only the old defaults of the quick switcher and voice capture become Off (the
    /// Brainitive key does that now); recorded keys and an explicit Off stay. The Brainitive key
    /// starts on, unless a kept direct shortcut already uses its keys.
    ///
    /// Scheme 2 (New task): a file without `task_shortcut` got the default when it was read; it
    /// starts Off instead when one of the other shortcuts already uses those keys.
    pub fn migrate(&mut self) -> bool {
        if self.shortcut_scheme >= SHORTCUT_SCHEME {
            return false;
        }
        if self.shortcut_scheme < 1 {
            self.migrate_to_leader();
        }
        let parse = |s: &Option<String>| s.as_deref().and_then(|a| crate::shortcuts::parse(a).ok());
        if let Some(task) = parse(&self.task_shortcut) {
            let taken = [
                &self.leader_shortcut,
                &self.capture_shortcut,
                &self.switcher_shortcut,
                &self.voice_shortcut,
            ]
            .into_iter()
            .filter_map(parse)
            .any(|s| s.id() == task.id());
            if taken {
                self.task_shortcut = None;
            }
        }
        self.shortcut_scheme = SHORTCUT_SCHEME;
        true
    }

    /// Scheme 0 → 1 (BRAI-440), see [`Preferences::migrate`].
    fn migrate_to_leader(&mut self) {
        if self.switcher_shortcut.as_deref() == Some(OLD_DEFAULT_SWITCHER) {
            self.switcher_shortcut = None;
        }
        if self.voice_shortcut.as_deref() == Some(OLD_DEFAULT_VOICE) {
            self.voice_shortcut = None;
        }
        let parse = |s: &Option<String>| s.as_deref().and_then(|a| crate::shortcuts::parse(a).ok());
        if let Some(leader) = parse(&self.leader_shortcut) {
            let taken = [&self.capture_shortcut, &self.switcher_shortcut, &self.voice_shortcut]
                .into_iter()
                .filter_map(parse)
                .any(|s| s.id() == leader.id());
            if taken {
                self.leader_shortcut = None;
            }
        }
    }

    /// The voice shortcut that is **registered**: only while voice capture is on. Off, the keys
    /// belong to other apps (⇧⌘V / Ctrl+Shift+V = paste as plain text, paste in Linux terminals).
    pub fn active_voice_shortcut(&self) -> Option<String> {
        self.voice_shortcut.clone().filter(|_| self.voice_enabled)
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
                if s.preferences.migrate() {
                    log::info!("settings: shortcut defaults updated (scheme {SHORTCUT_SCHEME})");
                    if let Err(e) = s.save(path) {
                        log::warn!("could not save the migrated settings: {e}");
                    }
                }
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

    /// Makes `origin` current. A saved server keeps its place in the list (its name and version
    /// are refreshed); a new one goes to the end. The list never grows beyond
    /// [`crate::servers::MAX_SERVERS`]: callers check [`Settings::is_full_for`] first.
    pub fn remember(&mut self, origin: &ServerOrigin, name: &str, version: Option<String>) {
        let name: String = name.chars().take(120).collect();
        if let Some(r) = self.recent.iter_mut().find(|r| &r.origin == origin) {
            r.name = name;
            r.version = version;
            r.last_used = now();
        } else if self.recent.len() < crate::servers::MAX_SERVERS {
            self.recent.push(RecentServer {
                origin: origin.clone(),
                name,
                version,
                last_used: now(),
                custom_name: None,
            });
        } else {
            log::warn!("server list full: {origin} not saved");
        }
        self.current = Some(origin.clone());
        // a server chosen again must not lose its fresh data to an old wipe
        let id = crate::server::hex(&origin.store_id());
        self.pending_wipes.retain(|w| *w != id);
    }

    /// Whether adding `origin` would go beyond the limit (a saved server always fits).
    pub fn is_full_for(&self, origin: &ServerOrigin) -> bool {
        self.recent.len() >= crate::servers::MAX_SERVERS && !self.recent.iter().any(|r| &r.origin == origin)
    }

    /// Makes a saved server current (a switch): its last-used time, the order stays. `false` when
    /// it is not saved.
    pub fn touch(&mut self, origin: &ServerOrigin) -> bool {
        let Some(r) = self.recent.iter_mut().find(|r| &r.origin == origin) else {
            return false;
        };
        r.last_used = now();
        self.current = Some(origin.clone());
        let id = crate::server::hex(&origin.store_id());
        self.pending_wipes.retain(|w| *w != id);
        true
    }

    /// The instance answered with (possibly new) name and version: kept for the default name.
    pub fn update_instance(&mut self, origin: &ServerOrigin, name: &str, version: Option<String>) -> bool {
        let Some(r) = self.recent.iter_mut().find(|r| &r.origin == origin) else {
            return false;
        };
        let name: String = name.chars().take(120).collect();
        let changed = r.name != name || r.version != version;
        r.name = name;
        r.version = version;
        changed
    }

    /// Settings → Servers: a name of the user's own; empty = back to the default.
    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let Some(i) = crate::servers::index_of(&self.recent, id) else {
            return false;
        };
        let clean = crate::servers::clean_name(name);
        self.recent[i].custom_name = (!clean.is_empty()).then_some(clean);
        true
    }

    /// The saved server with this id (`servers::server_id`).
    pub fn server_by_id(&self, id: &str) -> Option<&RecentServer> {
        crate::servers::index_of(&self.recent, id).map(|i| &self.recent[i])
    }

    /// Removes the server from the list (and from `current`) and schedules its webview data.
    pub fn forget(&mut self, origin: &ServerOrigin) {
        self.recent.retain(|r| &r.origin != origin);
        self.preferences.new_task.remove(origin.as_str());
        if self.current.as_ref() == Some(origin) {
            self.current = None;
        }
        self.schedule_wipe(origin);
    }

    /// A sign-out of `origin` begins: marked until its local cleanup is done
    /// ([`Self::sign_out_done`]), so a crash or a refused deletion is retried at the next start.
    pub fn sign_out_started(&mut self, origin: &ServerOrigin) {
        if !self.pending_sign_outs.contains(origin) {
            self.pending_sign_outs.push(origin.clone());
        }
    }

    /// Everything of that sign-out is gone from this computer (or a new sign-in owns it now).
    pub fn sign_out_done(&mut self, origin: &ServerOrigin) {
        self.pending_sign_outs.retain(|o| o != origin);
    }

    /// The server's webview data store is deleted once no window uses it (Forget, and a
    /// sign-out of a server that is not the current one).
    pub fn schedule_wipe(&mut self, origin: &ServerOrigin) {
        let id = crate::server::hex(&origin.store_id());
        if !self.pending_wipes.contains(&id) {
            self.pending_wipes.push(id);
        }
    }

    /// The display name of `origin`: its saved label, else the default for an unnamed server.
    pub fn label_of(&self, origin: &ServerOrigin) -> String {
        self.recent
            .iter()
            .find(|r| &r.origin == origin)
            .map(RecentServer::label)
            .unwrap_or_else(|| crate::servers::default_name(origin, ""))
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
    fn saved_servers_keep_the_users_order() {
        let mut s = Settings::default();
        for i in 0..3 {
            s.remember(&o(&format!("s{i}.example.com")), &format!("S{i}"), None);
        }
        assert_eq!(s.current, Some(o("s2.example.com")));
        // used again: refreshed, but it keeps its place (⌃⌘1…9 don't move under the user)
        s.remember(&o("s0.example.com"), "S0 again", Some("1.49.0".into()));
        let order: Vec<_> = s.recent.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(order, ["S0 again", "S1", "S2"]);
        assert_eq!(s.current, Some(o("s0.example.com")));
        assert_eq!(s.recent[0].version.as_deref(), Some("1.49.0"));
        // a switch: current + last used, the order stays
        s.recent[2].last_used = 0;
        assert!(s.touch(&o("s2.example.com")));
        assert!(s.recent[2].last_used > 0);
        assert_eq!(s.current, Some(o("s2.example.com")));
        assert_eq!(s.recent[0].origin, o("s0.example.com"));
        assert!(!s.touch(&o("unknown.example.com")), "only saved servers");
        assert_eq!(s.current, Some(o("s2.example.com")));
    }

    #[test]
    fn the_list_stops_at_its_limit_instead_of_dropping_a_server() {
        use crate::servers::MAX_SERVERS;
        let mut s = Settings::default();
        for i in 0..MAX_SERVERS {
            assert!(!s.is_full_for(&o(&format!("s{i}.example.com"))));
            s.remember(&o(&format!("s{i}.example.com")), "S", None);
        }
        assert_eq!(s.recent.len(), MAX_SERVERS);
        assert!(s.is_full_for(&o("new.example.com")));
        assert!(!s.is_full_for(&o("s3.example.com")), "a saved one always fits");
        s.remember(&o("new.example.com"), "N", None);
        assert_eq!(s.recent.len(), MAX_SERVERS, "nothing dropped silently");
        assert_eq!(s.recent[0].origin, o("s0.example.com"));
    }

    #[test]
    fn names_rename_and_survive_a_reconnect() {
        let mut s = Settings::default();
        let work = o("brain.acme.com");
        s.remember(&work, "Brainitive", None);
        s.remember(&ServerOrigin::cloud(), "Brainitive", None);
        assert_eq!(s.label_of(&work), "brain.acme.com");
        assert_eq!(s.label_of(&ServerOrigin::cloud()), "Brainitive Cloud");
        let id = crate::servers::server_id(&work);
        assert!(s.rename(&id, "  Work "));
        assert_eq!(s.label_of(&work), "Work");
        // the server's own name changes on the next connect: the user's name stays
        assert!(s.update_instance(&work, "Acme Brain", Some("1.50.0".into())));
        s.remember(&work, "Acme Brain", None);
        assert_eq!(s.label_of(&work), "Work");
        // empty = the default again (now the server's own name)
        assert!(s.rename(&id, "   "));
        assert_eq!(s.label_of(&work), "Acme Brain");
        assert!(!s.rename("0000000000000000", "x"));
        // written and read back; files without the field load
        s.rename(&id, "Work");
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.label_of(&work), "Work");
        let old: Settings = serde_json::from_str(
            r#"{"current":"brain.acme.com","recent":[{"origin":"brain.acme.com","name":"Acme","last_used":1}]}"#,
        )
        .unwrap();
        assert_eq!(old.recent[0].custom_name, None);
        assert_eq!(old.label_of(&work), "Acme");
        // not saved: the default for an unnamed server
        assert_eq!(s.label_of(&o("x.example.com")), "x.example.com");
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
        let s: Settings = serde_json::from_str(r#"{"preferences":{"capture_shortcut":null,"zoom":1.25}}"#).unwrap();
        assert_eq!(s.preferences.capture_shortcut, None);
        assert_eq!(
            s.preferences.switcher_shortcut,
            Preferences::default().switcher_shortcut
        );
        assert_eq!(s.preferences.zoom, 1.25);
    }

    #[test]
    fn voice_capture_is_off_by_default_and_old_files_get_its_defaults() {
        let p = Preferences::default();
        assert!(!p.voice_enabled);
        // BRAI-440: no direct shortcut by default (⌃⌘K, then hold V)
        assert_eq!(p.voice_shortcut, None);
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

    /// A whole `preferences` object as a version before BRAI-440 wrote it.
    fn old_file(switcher: &str, voice: &str, capture: &str) -> Preferences {
        let json = format!(
            r#"{{"capture_shortcut":{capture},"switcher_shortcut":{switcher},"start_hidden":true,"keep_in_tray":true,"ask_where_to_save":true,"zoom":1.0,"voice_enabled":true,"voice_shortcut":{voice},"voice_language":"auto","auto_update_check":true,"update_channel":"stable"}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn new_installs_get_the_brainitive_key_and_capture_only() {
        let p = Preferences::default();
        assert_eq!(p.leader_shortcut.as_deref(), Some(crate::shortcuts::DEFAULT_LEADER));
        assert_eq!(p.leader_letters, crate::leader::LeaderLetters::default());
        assert_eq!(p.capture_shortcut.as_deref(), Some(crate::shortcuts::DEFAULT_CAPTURE));
        assert_eq!(p.switcher_shortcut, None);
        assert_eq!(p.voice_shortcut, None);
        assert_eq!(p.shortcut_scheme, SHORTCUT_SCHEME);
        let mut again = p.clone();
        assert!(!again.migrate(), "nothing to migrate");
        assert_eq!(again, p);
        // a written file reads back without a migration
        let s: Settings = serde_json::from_str(&serde_json::to_string(&Settings::default()).unwrap()).unwrap();
        assert_eq!(s.preferences.shortcut_scheme, SHORTCUT_SCHEME);
    }

    #[test]
    fn migration_turns_off_only_the_old_defaults() {
        // untouched (or "Default"): the constants as they were stored
        let mut p = old_file(
            r#""CommandOrControl+Alt+K""#,
            r#""CommandOrControl+Shift+KeyV""#,
            r#""CommandOrControl+Shift+Space""#,
        );
        assert_eq!(p.shortcut_scheme, 0, "missing = before BRAI-440");
        assert!(p.migrate());
        assert_eq!(p.switcher_shortcut, None);
        assert_eq!(p.voice_shortcut, None);
        assert_eq!(p.capture_shortcut.as_deref(), Some("CommandOrControl+Shift+Space"));
        assert_eq!(p.leader_shortcut.as_deref(), Some(crate::shortcuts::DEFAULT_LEADER));
        assert!(p.voice_enabled, "other preferences are kept");
        assert_eq!(p.shortcut_scheme, SHORTCUT_SCHEME);
        // once only: a later "Default" press (the same string) is not undone at the next start
        p.voice_shortcut = Some("CommandOrControl+Shift+KeyV".into());
        assert!(!p.migrate());
        assert!(p.voice_shortcut.is_some());

        // recorded by the user (canonical form, even on the same keys) and explicit Off: kept
        let mut p = old_file(r#""alt+super+KeyK""#, "null", r#""control+shift+KeyJ""#);
        assert!(p.migrate());
        assert_eq!(p.switcher_shortcut.as_deref(), Some("alt+super+KeyK"));
        assert_eq!(p.voice_shortcut, None);
        assert_eq!(p.capture_shortcut.as_deref(), Some("control+shift+KeyJ"));
        assert!(p.leader_shortcut.is_some());
    }

    #[test]
    fn migration_never_gives_the_brainitive_key_keys_the_user_chose_for_another_shortcut() {
        let leader = crate::shortcuts::parse(crate::shortcuts::DEFAULT_LEADER).unwrap();
        let recorded = format!("\"{}\"", crate::shortcuts::canonical(&leader));
        let mut p = old_file(&recorded, "null", r#""CommandOrControl+Shift+Space""#);
        assert!(p.migrate());
        assert_eq!(
            p.leader_shortcut, None,
            "starts off rather than taking the switcher's keys"
        );
        assert!(p.switcher_shortcut.is_some());
    }

    #[test]
    fn migration_runs_at_load_and_is_saved() {
        let dir = std::env::temp_dir().join(format!("brainitive-desktop-migrate-{}", std::process::id()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            br#"{"current":"brain.example.com","preferences":{"switcher_shortcut":"CommandOrControl+Alt+K","voice_shortcut":"CommandOrControl+Shift+KeyV"}}"#,
        )
        .unwrap();
        let s = Settings::load(&path);
        assert!(s.current.is_some());
        assert_eq!(s.preferences.switcher_shortcut, None);
        assert_eq!(s.preferences.voice_shortcut, None);
        let on_disk: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk["preferences"]["shortcut_scheme"], SHORTCUT_SCHEME);
        assert_eq!(on_disk["preferences"]["leader_letters"]["voice"], "V");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A whole `preferences` object as version 1 (BRAI-440) wrote it: no `task_shortcut`.
    fn scheme_1_file(leader: &str, capture: &str) -> Preferences {
        let json = format!(
            r#"{{"leader_shortcut":{leader},"leader_letters":{{"capture":"C","switcher":"K","voice":"V","main":"O"}},"shortcut_scheme":1,"capture_shortcut":{capture},"switcher_shortcut":null,"start_hidden":true,"keep_in_tray":true,"ask_where_to_save":true,"zoom":1.0,"voice_enabled":false,"voice_shortcut":null,"voice_language":"auto","auto_update_check":true,"update_channel":"stable","panel_tab":"today"}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn new_task_gets_its_default_unless_another_shortcut_uses_those_keys() {
        let leader = format!("\"{}\"", crate::shortcuts::DEFAULT_LEADER);
        // untouched file: New task gets this OS's default (off on Linux), T as its letter
        let mut p = scheme_1_file(&leader, r#""CommandOrControl+Shift+Space""#);
        assert_eq!(p.shortcut_scheme, 1);
        assert!(p.migrate());
        assert_eq!(p.task_shortcut.as_deref(), crate::shortcuts::DEFAULT_TASK);
        assert_eq!(p.leader_letters.task, 'T');
        assert_eq!(p.shortcut_scheme, SHORTCUT_SCHEME);
        assert!(!p.migrate(), "once only");
        assert!(p.new_task.is_empty());

        // a shortcut the user recorded on those keys keeps them; New task starts off
        if let Some(task) = crate::shortcuts::DEFAULT_TASK {
            let recorded = format!(
                "\"{}\"",
                crate::shortcuts::canonical(&crate::shortcuts::parse(task).unwrap())
            );
            let mut p = scheme_1_file(&leader, &recorded);
            assert!(p.migrate());
            assert_eq!(p.task_shortcut, None);
            assert_eq!(p.capture_shortcut.as_deref(), Some(&recorded[1..recorded.len() - 1]));
            // also when it is the Brainitive key
            let mut p = scheme_1_file(&recorded, "null");
            assert!(p.migrate());
            assert_eq!(p.task_shortcut, None);
            assert!(p.leader_shortcut.is_some());
        }

        // a file from before the Brainitive key goes through both steps
        let mut p = old_file(
            r#""CommandOrControl+Alt+K""#,
            "null",
            r#""CommandOrControl+Shift+Space""#,
        );
        assert!(p.migrate());
        assert_eq!(p.switcher_shortcut, None);
        assert_eq!(p.task_shortcut.as_deref(), crate::shortcuts::DEFAULT_TASK);

        // after the migration an explicit Off stays off
        let mut p = Preferences {
            task_shortcut: None,
            ..Preferences::default()
        };
        assert!(!p.migrate());
        assert_eq!(p.task_shortcut, None);
        let s: Settings =
            serde_json::from_str(r#"{"preferences":{"shortcut_scheme":2,"task_shortcut":null}}"#).unwrap();
        assert_eq!(s.preferences.task_shortcut, None);
    }

    #[test]
    fn the_last_new_task_choice_is_kept_per_server_and_goes_with_forget() {
        use crate::new_task::model::{remember, Memory};
        let mut s = Settings::default();
        let a = o("a.example.com");
        let b = o("b.example.com");
        s.remember(&a, "A", None);
        s.remember(&b, "B", None);
        let m = |p| Memory {
            project_id: Some(p),
            board_id: Some(p * 10),
            type_id: None,
            at: p,
        };
        remember(&mut s.preferences.new_task, a.as_str(), m(1));
        remember(&mut s.preferences.new_task, b.as_str(), m(2));
        // written and read back
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.preferences.new_task[a.as_str()], m(1));
        assert_eq!(back.preferences.new_task[b.as_str()], m(2));
        // forgetting a server forgets its choice, the other server's stays
        s.forget(&a);
        assert!(!s.preferences.new_task.contains_key(a.as_str()));
        assert_eq!(s.preferences.new_task[b.as_str()], m(2));
        // a broken entry doesn't lose the file
        let s: Settings = serde_json::from_str(
            r#"{"current":"brain.example.com","preferences":{"new_task":{"https://brain.example.com":{"project_id":-1}}}}"#,
        )
        .unwrap();
        assert!(s.current.is_some());
        assert!(s.preferences.new_task.is_empty());
    }

    #[test]
    fn odd_letters_in_the_file_do_not_lose_the_settings() {
        let s: Settings = serde_json::from_str(
            r#"{"current":"brain.example.com","preferences":{"leader_letters":{"capture":"??","voice":"V"}}}"#,
        )
        .unwrap();
        assert!(s.current.is_some());
        assert_eq!(s.preferences.leader_letters, crate::leader::LeaderLetters::default());
    }

    #[test]
    fn update_preferences_default_to_automatic_stable_and_survive_odd_values() {
        use crate::updater::Channel;
        let p = Preferences::default();
        assert!(p.auto_update_check);
        assert_eq!(p.update_channel, Channel::Stable);
        // a file from before BRAI-373
        let s: Settings = serde_json::from_str(r#"{"preferences":{"zoom":1.1}}"#).unwrap();
        assert!(s.preferences.auto_update_check);
        assert_eq!(s.preferences.update_channel, Channel::Stable);
        assert_eq!(s.last_update_check, None);
        // explicit choices are kept; an unknown channel does not lose the rest of the file
        let s: Settings = serde_json::from_str(
            r#"{"current":"brain.example.com","last_update_check":1800000000,"preferences":{"auto_update_check":false,"update_channel":"beta"}}"#,
        )
        .unwrap();
        assert!(!s.preferences.auto_update_check);
        assert_eq!(s.preferences.update_channel, Channel::Beta);
        assert_eq!(s.last_update_check, Some(1_800_000_000));
        let s: Settings =
            serde_json::from_str(r#"{"current":"brain.example.com","preferences":{"update_channel":"nightly"}}"#)
                .unwrap();
        assert_eq!(s.preferences.update_channel, Channel::Stable);
        assert!(s.current.is_some());
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
