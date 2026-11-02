//! The Brainitive key (BRAI-440): one global shortcut (⌃⌘K on macOS, Ctrl+Alt+K on Windows /
//! Linux, `HotkeyAction::Leader`), then **one letter** for the action. The letter is read by the
//! overlay window (`leader.html`, label `leader`, `capabilities/leader.json`) that the shortcut
//! opens: no letter is registered globally, so nothing is taken from other apps but the key itself.
//!
//! The top half is pure (actions, letters, validation, texts) and tested; the bottom half is the
//! glue: the hotkey handler and the overlay's commands (each checks its caller,
//! [`popup::ensure_caller`]).

use serde::{Deserialize, Deserializer, Serialize};
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_global_shortcut::Shortcut;

use crate::popup::{self, Popup};
use crate::state::state;

/// What a letter after the Brainitive key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeaderAction {
    Capture,
    Switcher,
    /// Hold the letter to talk: key-down starts, key-up stops (like the direct voice shortcut).
    Voice,
    /// Show the main Brainitive window.
    Main,
    /// The New task window.
    Task,
}

impl LeaderAction {
    pub const ALL: [LeaderAction; 5] = [Self::Capture, Self::Task, Self::Switcher, Self::Voice, Self::Main];

    pub fn id(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Switcher => "switcher",
            Self::Voice => "voice",
            Self::Main => "main",
            Self::Task => "task",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Capture => "Quick capture",
            Self::Switcher => "Quick switcher",
            Self::Voice => "Voice capture",
            Self::Main => "Open Brainitive",
            Self::Task => "New task",
        }
    }

    /// C capture, K switcher (⌃⌘K K), V voice, O open, T new task.
    pub fn default_letter(self) -> char {
        match self {
            Self::Task => 'T',
            Self::Capture => 'C',
            Self::Switcher => 'K',
            Self::Voice => 'V',
            Self::Main => 'O',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LetterError {
    #[error("Use one letter, A–Z.")]
    NotALetter,
    #[error("{letter} is already used for {name}.")]
    Taken { letter: char, name: &'static str },
}

/// One letter A–Z (any case, surrounding spaces ignored) → upper case.
pub fn parse_letter(input: &str) -> Result<char, LetterError> {
    let mut chars = input.trim().chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Ok(c.to_ascii_uppercase()),
        _ => Err(LetterError::NotALetter),
    }
}

/// The letter of each action (Settings → Shortcuts). Always A–Z and all different.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LeaderLetters {
    pub capture: char,
    pub switcher: char,
    pub voice: char,
    pub main: char,
    pub task: char,
}

impl Default for LeaderLetters {
    fn default() -> Self {
        Self {
            capture: LeaderAction::Capture.default_letter(),
            switcher: LeaderAction::Switcher.default_letter(),
            voice: LeaderAction::Voice.default_letter(),
            main: LeaderAction::Main.default_letter(),
            task: LeaderAction::Task.default_letter(),
        }
    }
}

impl LeaderLetters {
    pub fn get(&self, action: LeaderAction) -> char {
        match action {
            LeaderAction::Capture => self.capture,
            LeaderAction::Switcher => self.switcher,
            LeaderAction::Voice => self.voice,
            LeaderAction::Main => self.main,
            LeaderAction::Task => self.task,
        }
    }

    fn slot(&mut self, action: LeaderAction) -> &mut char {
        match action {
            LeaderAction::Capture => &mut self.capture,
            LeaderAction::Switcher => &mut self.switcher,
            LeaderAction::Voice => &mut self.voice,
            LeaderAction::Main => &mut self.main,
            LeaderAction::Task => &mut self.task,
        }
    }

    /// The action of a pressed letter (any case).
    pub fn action_for(&self, letter: char) -> Option<LeaderAction> {
        let letter = letter.to_ascii_uppercase();
        LeaderAction::ALL.into_iter().find(|a| self.get(*a) == letter)
    }

    /// The letters with `action` on `input`, refused when it is not one letter A–Z or another
    /// action has it.
    pub fn with(&self, action: LeaderAction, input: &str) -> Result<Self, LetterError> {
        let letter = parse_letter(input)?;
        if let Some(other) = self.action_for(letter).filter(|o| *o != action) {
            return Err(LetterError::Taken {
                letter,
                name: other.name(),
            });
        }
        let mut next = *self;
        *next.slot(action) = letter;
        Ok(next)
    }

    /// A–Z and all different.
    pub fn is_valid(&self) -> bool {
        let all: Vec<char> = LeaderAction::ALL.iter().map(|a| self.get(*a)).collect();
        all.iter().all(|c| c.is_ascii_uppercase()) && all.iter().enumerate().all(|(i, c)| !all[..i].contains(c))
    }
}

/// `settings.json` → letters. A missing letter gets its default — New task's, when the user already
/// gave T to another action (a file from before New task), N, else the first free letter; a file someone edited into something invalid (not A–Z, twice the same) falls back to
/// all defaults instead of losing the rest of the settings.
pub fn lenient_letters<'de, D: Deserializer<'de>>(d: D) -> Result<LeaderLetters, D::Error> {
    let raw = serde_json::Value::deserialize(d)?;
    Ok(letters_from_value(&raw))
}

fn letters_from_value(raw: &serde_json::Value) -> LeaderLetters {
    let defaults = LeaderLetters::default();
    let Some(map) = raw.as_object() else {
        return defaults;
    };
    let mut out = defaults;
    let mut missing = Vec::new();
    for action in LeaderAction::ALL {
        match map.get(action.id()) {
            Some(value) => match value.as_str().map(parse_letter) {
                Some(Ok(letter)) => *out.slot(action) = letter,
                _ => return defaults,
            },
            None => missing.push(action),
        }
    }
    // only New task can be missing from a file the app wrote (every letter is written; New task
    // came later): it moves aside instead of resetting the user's letters
    for action in missing.into_iter().filter(|a| *a == LeaderAction::Task) {
        let used = |c: char| LeaderAction::ALL.iter().any(|a| *a != action && out.get(*a) == c);
        if used(out.get(action)) {
            let free = std::iter::once('N').chain('A'..='Z').find(|c| !used(*c));
            match free {
                Some(c) => *out.slot(action) = c,
                None => return defaults,
            }
        }
    }
    if out.is_valid() {
        out
    } else {
        defaults
    }
}

/// The letter of a shortcut's key when it is A–Z (⌃⌘K → K).
pub fn key_letter(shortcut: &Shortcut) -> Option<char> {
    let code = shortcut.key.to_string();
    let letter = code.strip_prefix("Key")?;
    let mut chars = letter.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_uppercase() => Some(c),
        _ => None,
    }
}

/// The Brainitive key pressed again while the overlay is in front (its modifiers still held):
/// counts as its own letter (⌃⌘K K → quick switcher); `None` = close the overlay.
pub fn pressed_again(letters: &LeaderLetters, leader: Option<&Shortcut>) -> Option<LeaderAction> {
    leader.and_then(key_letter).and_then(|l| letters.action_for(l))
}

/// How voice capture is started, for the indicator and the "ready" notification: the direct
/// shortcut when there is one, else the Brainitive key and the letter, else the menu.
pub fn voice_how_to(direct: &str, leader: &str, letter: char) -> String {
    if !direct.is_empty() {
        format!("Hold {direct}")
    } else if !leader.is_empty() {
        format!("Press {leader}, then hold {letter}")
    } else {
        "Choose Voice Capture in the menu".to_string()
    }
}

// ---- the overlay -------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct LeaderView {
    /// "⌃⌘K"; empty when the Brainitive key is off (the overlay was opened from a menu).
    leader: String,
    platform: &'static str,
    actions: Vec<ActionView>,
}

#[derive(Debug, Clone, Serialize)]
struct ActionView {
    id: &'static str,
    name: &'static str,
    letter: char,
    enabled: bool,
    /// Why it is greyed out.
    hint: Option<&'static str>,
}

const VOICE_OFF_HINT: &str = "Off. Turn it on in Settings → Voice capture.";

fn view(app: &AppHandle) -> LeaderView {
    let prefs = state(app).lock().settings.preferences.clone();
    LeaderView {
        leader: crate::shortcuts::label_of(prefs.leader_shortcut.as_deref()),
        platform: crate::api::platform(),
        actions: LeaderAction::ALL
            .into_iter()
            .map(|a| {
                let enabled = a != LeaderAction::Voice || prefs.voice_enabled;
                ActionView {
                    id: a.id(),
                    name: a.name(),
                    letter: prefs.leader_letters.get(a),
                    enabled,
                    hint: (!enabled).then_some(VOICE_OFF_HINT),
                }
            })
            .collect(),
    }
}

fn in_front(app: &AppHandle) -> bool {
    app.get_webview_window(Popup::Leader.label())
        .is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

fn visible(app: &AppHandle) -> bool {
    app.get_webview_window(Popup::Leader.label())
        .is_some_and(|w| w.is_visible().unwrap_or(false))
}

/// The Brainitive key (global shortcut): opens the overlay; pressed again while it is in front,
/// it counts as its own letter (or closes the overlay).
pub fn on_hotkey(app: &AppHandle) {
    if !in_front(app) {
        return popup::show(app, Popup::Leader);
    }
    let (letters, leader) = {
        let s = state(app);
        let inner = s.lock();
        let prefs = &inner.settings.preferences;
        (
            prefs.leader_letters,
            prefs
                .leader_shortcut
                .as_deref()
                .and_then(|l| crate::shortcuts::parse(l).ok()),
        )
    };
    match pressed_again(&letters, leader.as_ref()) {
        // the global shortcut's key-up is not reported: voice runs until ↩ / Done here
        Some(action) => {
            if let Err(e) = run(app, action, None) {
                log::info!("Brainitive key: {e}");
            }
        }
        None => popup::dismiss(app, Popup::Leader),
    }
}

/// "Brainitive Key…" in the File menu / tray.
pub fn show(app: &AppHandle) {
    popup::show(app, Popup::Leader);
}

/// Runs an action. `hold`: the letter that is held down (voice: its release stops).
fn run(app: &AppHandle, action: LeaderAction, hold: Option<char>) -> Result<(), String> {
    match action {
        // the new window is shown while the overlay still has the focus, so it inherits where
        // the focus goes back to; then the overlay goes
        LeaderAction::Capture => crate::capture::open_quick_capture(app, None),
        LeaderAction::Switcher => popup::show(app, Popup::Switcher),
        LeaderAction::Main => crate::shell::focus_main(app),
        LeaderAction::Task => crate::new_task::show(app),
        LeaderAction::Voice => {
            if !state(app).lock().settings.preferences.voice_enabled {
                return Err("Voice capture is off. Turn it on in Settings → Voice capture.".into());
            }
            // the indicator takes the focus; the overlay hides when it loses it, or on the
            // key-up it may still get
            match hold {
                Some(letter) => crate::voice::start_from_leader(app, letter),
                None => crate::voice::start_from_menu(app),
            }
            return Ok(());
        }
    }
    popup::hide(app, Popup::Leader);
    Ok(())
}

#[tauri::command]
pub fn leader_view(app: AppHandle, webview: Webview) -> Result<LeaderView, String> {
    popup::ensure_caller(&app, &webview, Popup::Leader)?;
    Ok(view(&app))
}

/// A letter went down in the overlay (`letter`: what the page read, A–Z).
///
/// Synchronous on purpose, like `leader_release`: Tauri runs sync commands in order on the main
/// thread, so a quick tap's key-up can't overtake its key-down (async commands could). Nothing
/// here builds a webview on that thread (`popup::show` builds on the async runtime).
#[tauri::command]
pub fn leader_press(app: AppHandle, webview: Webview, letter: String) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Leader)?;
    let letter = parse_letter(&letter).map_err(|e| e.to_string())?;
    let letters = state(&app).lock().settings.preferences.leader_letters;
    let action = letters
        .action_for(letter)
        .ok_or_else(|| format!("{letter} does nothing here."))?;
    run(&app, action, Some(letter))
}

/// A letter went up in the overlay: the end of a held voice capture.
#[tauri::command]
pub fn leader_release(app: AppHandle, webview: Webview, letter: String) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Leader)?;
    let letter = parse_letter(&letter).map_err(|e| e.to_string())?;
    crate::voice::leader_released(&app, letter);
    if visible(&app) {
        popup::hide(&app, Popup::Leader);
    }
    Ok(())
}

/// Esc.
#[tauri::command]
pub fn leader_close(app: AppHandle, webview: Webview) -> Result<(), String> {
    popup::ensure_caller(&app, &webview, Popup::Leader)?;
    // a late Esc of a hidden overlay must not move the focus
    if visible(&app) {
        popup::dismiss(&app, Popup::Leader);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_letters_c_k_v_o_t() {
        let l = LeaderLetters::default();
        assert_eq!((l.capture, l.switcher, l.voice, l.main), ('C', 'K', 'V', 'O'));
        assert_eq!(l.task, 'T');
        assert_eq!(l.action_for('t'), Some(LeaderAction::Task));
        assert_eq!(LeaderAction::Task.name(), "New task");
        assert!(l.is_valid());
        assert_eq!(l.action_for('c'), Some(LeaderAction::Capture));
        assert_eq!(l.action_for('K'), Some(LeaderAction::Switcher));
        assert_eq!(l.action_for('v'), Some(LeaderAction::Voice));
        assert_eq!(l.action_for('O'), Some(LeaderAction::Main));
        assert_eq!(l.action_for('X'), None);
        for a in LeaderAction::ALL {
            assert_eq!(LeaderAction::from_id(a.id()), Some(a));
        }
        assert_eq!(LeaderAction::from_id("nope"), None);
    }

    #[test]
    fn letters_are_a_to_z_only() {
        assert_eq!(parse_letter("j"), Ok('J'));
        assert_eq!(parse_letter(" Q "), Ok('Q'));
        for bad in ["", " ", "1", "ab", "ä", "ß", "-", "\u{0}", "KK", "Ж"] {
            assert_eq!(parse_letter(bad), Err(LetterError::NotALetter), "{bad:?}");
        }
    }

    #[test]
    fn letters_must_be_unique_among_actions() {
        let l = LeaderLetters::default();
        let next = l.with(LeaderAction::Capture, "n").unwrap();
        assert_eq!(next.capture, 'N');
        assert_eq!(next.action_for('C'), None);
        assert_eq!(
            l.with(LeaderAction::Capture, "v"),
            Err(LetterError::Taken {
                letter: 'V',
                name: "Voice capture"
            })
        );
        assert_eq!(
            l.with(LeaderAction::Capture, "v").unwrap_err().to_string(),
            "V is already used for Voice capture."
        );
        // its own letter again is fine
        assert_eq!(l.with(LeaderAction::Switcher, "k"), Ok(l));
        assert_eq!(l.with(LeaderAction::Main, "42"), Err(LetterError::NotALetter));
        // swapping goes through a free letter
        let a = l.with(LeaderAction::Capture, "x").unwrap();
        let b = a.with(LeaderAction::Voice, "c").unwrap();
        let c = b.with(LeaderAction::Capture, "v").unwrap();
        assert_eq!((c.capture, c.voice), ('V', 'C'));
        assert!(c.is_valid());
    }

    #[test]
    fn letters_load_leniently() {
        use serde_json::json;
        assert_eq!(letters_from_value(&json!(null)), LeaderLetters::default());
        assert_eq!(letters_from_value(&json!("C")), LeaderLetters::default());
        let partial = letters_from_value(&json!({"capture": "n"}));
        assert_eq!(partial.capture, 'N');
        assert_eq!(partial.voice, 'V');
        // invalid or duplicate: everything back to the defaults
        assert_eq!(letters_from_value(&json!({"capture": "1"})), LeaderLetters::default());
        assert_eq!(letters_from_value(&json!({"capture": 3})), LeaderLetters::default());
        assert_eq!(letters_from_value(&json!({"capture": "v"})), LeaderLetters::default());
        // a file from before New task: its letters stay, task gets T
        let old = letters_from_value(&json!({"capture": "N", "switcher": "K", "voice": "V", "main": "O"}));
        assert_eq!((old.capture, old.task), ('N', 'T'));
        // T already given to another action: that one keeps it, task gets N …
        let old = letters_from_value(&json!({"capture": "T", "switcher": "K", "voice": "V", "main": "O"}));
        assert_eq!((old.capture, old.task), ('T', 'N'));
        assert!(old.is_valid());
        // … or, N taken too, the first free letter
        let old = letters_from_value(&json!({"capture": "T", "switcher": "N", "voice": "A", "main": "O"}));
        assert_eq!((old.capture, old.switcher, old.voice, old.task), ('T', 'N', 'A', 'B'));
        // customizable like the others
        let l = LeaderLetters::default().with(LeaderAction::Task, "n").unwrap();
        assert_eq!(l.task, 'N');
        assert_eq!(
            l.with(LeaderAction::Task, "c").unwrap_err().to_string(),
            "C is already used for Quick capture."
        );
        // serialized as one-letter strings and read back
        let l = LeaderLetters::default().with(LeaderAction::Main, "b").unwrap();
        let json = serde_json::to_value(l).unwrap();
        assert_eq!(json["main"], "B");
        assert_eq!(letters_from_value(&json), l);
    }

    #[test]
    fn the_brainitive_key_again_counts_as_its_letter() {
        use crate::shortcuts::parse;
        let l = LeaderLetters::default();
        let k = parse("Control+Super+KeyK").unwrap();
        assert_eq!(key_letter(&k), Some('K'));
        assert_eq!(pressed_again(&l, Some(&k)), Some(LeaderAction::Switcher));
        // a key that is no action's letter closes the overlay
        let j = parse("Control+Alt+KeyJ").unwrap();
        assert_eq!(pressed_again(&l, Some(&j)), None);
        let f5 = parse("Ctrl+F5").unwrap();
        assert_eq!(key_letter(&f5), None);
        assert_eq!(pressed_again(&l, Some(&f5)), None);
        assert_eq!(pressed_again(&l, None), None);
        assert_eq!(key_letter(&parse("Control+Alt+Digit1").unwrap()), None);
    }

    #[test]
    fn voice_says_how_to_start() {
        assert_eq!(voice_how_to("⇧⌘V", "⌃⌘K", 'V'), "Hold ⇧⌘V");
        assert_eq!(voice_how_to("", "⌃⌘K", 'V'), "Press ⌃⌘K, then hold V");
        assert_eq!(voice_how_to("", "Ctrl+Alt+K", 'X'), "Press Ctrl+Alt+K, then hold X");
        assert_eq!(voice_how_to("", "", 'V'), "Choose Voice Capture in the menu");
    }
}
