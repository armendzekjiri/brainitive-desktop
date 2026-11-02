//! The saved servers (accounts) the user switches between: their ids, display names, order and
//! the menu shortcuts of the first nine. Pure logic (no Tauri), unit tested below; the list itself
//! lives in `settings.json` (`Settings::recent`) and the switch in `flows::switch_to`.
//!
//! Commands that act on a saved server take its **id** (never an origin from a page): 16 hex
//! characters of the origin's store id, looked up in the saved list.

use serde::{Deserialize, Serialize};

use crate::server::{self, ServerOrigin};
use crate::settings::{RecentServer, Settings};

/// How many servers the app keeps. More are refused ("Forget a server first") instead of
/// silently dropping one (that would leave its keychain entry and data behind).
pub const MAX_SERVERS: usize = 20;
/// The first nine servers get a menu shortcut (⌃⌘1…⌃⌘9 / Alt+1…Alt+9).
pub const SHORTCUT_SLOTS: usize = 9;
/// A server's display name, at most.
pub const MAX_NAME: usize = 60;
/// What a Brainitive server is called when nobody named it: not a name of its own.
const STOCK_NAME: &str = "Brainitive";

/// The id commands use for a saved server.
pub fn server_id(origin: &ServerOrigin) -> String {
    server::hex(&origin.store_id()[..8])
}

/// The default name: "Brainitive Cloud" for the cloud, else the server's own name from
/// `/api/v1/instance` (not the stock "Brainitive"), else the host (with a non-default port).
pub fn default_name(origin: &ServerOrigin, instance_name: &str) -> String {
    if origin.is_cloud() {
        return "Brainitive Cloud".to_string();
    }
    let name = clean_name(instance_name);
    if name.is_empty() || name.eq_ignore_ascii_case(STOCK_NAME) {
        origin.display_name()
    } else {
        name
    }
}

/// A name typed in Settings (or sent by a server): no control or format characters, runs of
/// whitespace become one space, trimmed, at most [`MAX_NAME`] characters. Empty = no name.
pub fn clean_name(input: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in input.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if c.is_control() || is_format(c) {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out.chars().take(MAX_NAME).collect::<String>().trim_end().to_string()
}

/// Bidi controls and other invisible format characters (a name like "Work\u{202E}…" would read
/// backwards in menus).
fn is_format(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// The position of the server with `id` in the saved list.
pub fn index_of(list: &[RecentServer], id: &str) -> Option<usize> {
    list.iter().position(|r| server_id(&r.origin) == id)
}

/// Up / down in Settings → Servers (the order is the ⌃⌘1…9 order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Move {
    Up,
    Down,
}

/// Moves the server one place; `false` when it is unknown or already at that end.
pub fn move_server(list: &mut [RecentServer], id: &str, direction: Move) -> bool {
    let Some(i) = index_of(list, id) else {
        return false;
    };
    let j = match direction {
        Move::Up if i > 0 => i - 1,
        Move::Down if i + 1 < list.len() => i + 1,
        _ => return false,
    };
    list.swap(i, j);
    true
}

/// The menu accelerator of the server at `index` (0-based): macOS ⌃⌘1…⌃⌘9, Windows / Linux
/// Alt+1…Alt+9 (Ctrl+Alt is AltGr there, which types `{ [ ] } @ ²…` on many layouts, and the web
/// editor uses Ctrl+Alt+1–6 / Ctrl+Shift+7–9). Written so both the menu (muda) and the
/// global-shortcut parser read it (tests compare them). Local to the app's menu: never global.
pub fn accelerator(index: usize, mac: bool) -> Option<String> {
    (index < SHORTCUT_SLOTS).then(|| {
        let n = index + 1;
        if mac {
            format!("Control+Super+Digit{n}")
        } else {
            format!("Alt+Digit{n}")
        }
    })
}

/// Whether a global shortcut (Settings → Shortcuts) would take the keys of a server's menu
/// shortcut on this kind of system: the global one would win and the switch never fire.
pub fn takes_server_keys(shortcut: &tauri_plugin_global_shortcut::Shortcut, mac: bool) -> bool {
    (0..SHORTCUT_SLOTS)
        .filter_map(|i| accelerator(i, mac))
        .filter_map(|a| a.parse::<tauri_plugin_global_shortcut::Shortcut>().ok())
        .any(|s| s.id() == shortcut.id())
}

/// What Settings and the panel show for that shortcut.
pub fn shortcut_label(index: usize, mac: bool) -> Option<String> {
    (index < SHORTCUT_SLOTS).then(|| {
        let n = index + 1;
        if mac {
            format!("⌃⌘{n}")
        } else {
            format!("Alt+{n}")
        }
    })
}

/// One saved server as the menus, the panel and Settings list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub id: String,
    pub name: String,
    /// The host (with a non-default port), "www.brainitive.app" for the cloud.
    pub host: String,
    pub current: bool,
    /// "⌃⌘1" / "Alt+1" for the first nine.
    pub shortcut: Option<String>,
    /// Unix seconds.
    pub last_used: u64,
}

/// The saved servers in their order.
pub fn choices(settings: &Settings, mac: bool) -> Vec<Choice> {
    settings
        .recent
        .iter()
        .enumerate()
        .map(|(i, r)| Choice {
            id: server_id(&r.origin),
            name: r.label(),
            host: host_of(&r.origin),
            current: settings.current.as_ref() == Some(&r.origin),
            shortcut: shortcut_label(i, mac),
            last_used: r.last_used,
        })
        .collect()
}

/// The host part of an origin (with a non-default port): what a name alone doesn't tell.
pub fn host_of(origin: &ServerOrigin) -> String {
    let url = origin.url();
    let host = url.host_str().unwrap_or_default().to_string();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(s: &str) -> ServerOrigin {
        ServerOrigin::parse(s).unwrap()
    }

    fn saved(origins: &[&str]) -> Vec<RecentServer> {
        origins
            .iter()
            .map(|s| RecentServer {
                origin: o(s),
                name: String::new(),
                version: None,
                last_used: 0,
                custom_name: None,
                sign_in: None,
            })
            .collect()
    }

    #[test]
    fn default_names() {
        // the cloud is always "Brainitive Cloud", whatever the instance calls itself
        assert_eq!(default_name(&ServerOrigin::cloud(), "Brainitive"), "Brainitive Cloud");
        assert_eq!(default_name(&ServerOrigin::cloud(), ""), "Brainitive Cloud");
        // a self-hosted server with a name of its own
        assert_eq!(default_name(&o("brain.acme.com"), "Acme Brain"), "Acme Brain");
        assert_eq!(default_name(&o("brain.acme.com"), "  Acme \n Brain  "), "Acme Brain");
        // no name, or the stock one: the host (with its port)
        assert_eq!(default_name(&o("brain.acme.com"), ""), "brain.acme.com");
        assert_eq!(default_name(&o("brain.acme.com"), "Brainitive"), "brain.acme.com");
        assert_eq!(default_name(&o("brain.acme.com"), " brainitive "), "brain.acme.com");
        assert_eq!(default_name(&o("localhost:8000"), "Brainitive"), "localhost:8000");
        assert_eq!(
            default_name(&o("https://brain.acme.com:8443"), ""),
            "brain.acme.com:8443"
        );
    }

    #[test]
    fn names_are_cleaned_and_capped() {
        assert_eq!(clean_name("  Work  "), "Work");
        assert_eq!(clean_name("Work\tstuff\n"), "Work stuff");
        assert_eq!(clean_name("Wo\u{0}rk\u{7}"), "Work");
        assert_eq!(clean_name("Work\u{202E}gpj.exe"), "Workgpj.exe");
        assert_eq!(clean_name("\u{200B}"), "");
        assert_eq!(clean_name(&"x".repeat(200)).chars().count(), MAX_NAME);
        assert_eq!(
            clean_name(&format!("{} y", "x".repeat(MAX_NAME - 1))),
            "x".repeat(MAX_NAME - 1)
        );
        assert_eq!(clean_name("Privat 🏠"), "Privat 🏠");
    }

    #[test]
    fn ids_are_stable_hex_and_distinct() {
        let a = server_id(&ServerOrigin::cloud());
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(a, server_id(&ServerOrigin::cloud()));
        assert_ne!(a, server_id(&o("brain.acme.com")));
        let list = saved(&["a.example.com", "b.example.com"]);
        assert_eq!(index_of(&list, &server_id(&o("b.example.com"))), Some(1));
        assert_eq!(index_of(&list, "https://b.example.com"), None, "an origin is no id");
        assert_eq!(index_of(&list, &server_id(&o("c.example.com"))), None);
    }

    #[test]
    fn servers_move_up_and_down_within_the_list() {
        let mut list = saved(&["a.example.com", "b.example.com", "c.example.com"]);
        let id = |s: &str| server_id(&o(s));
        assert!(move_server(&mut list, &id("c.example.com"), Move::Up));
        assert_eq!(list[1].origin, o("c.example.com"));
        assert!(move_server(&mut list, &id("c.example.com"), Move::Up));
        assert_eq!(list[0].origin, o("c.example.com"));
        assert!(!move_server(&mut list, &id("c.example.com"), Move::Up), "already first");
        assert!(move_server(&mut list, &id("a.example.com"), Move::Down));
        assert_eq!(list[2].origin, o("a.example.com"));
        assert!(
            !move_server(&mut list, &id("a.example.com"), Move::Down),
            "already last"
        );
        assert!(!move_server(&mut list, &id("x.example.com"), Move::Down), "unknown");
        let order: Vec<_> = list.iter().map(|r| r.origin.as_str().to_string()).collect();
        assert_eq!(
            order,
            [
                "https://c.example.com",
                "https://b.example.com",
                "https://a.example.com"
            ]
        );
    }

    #[test]
    fn the_first_nine_get_a_shortcut() {
        for mac in [true, false] {
            for i in 0..SHORTCUT_SLOTS {
                assert!(accelerator(i, mac).is_some());
                assert!(shortcut_label(i, mac).is_some());
            }
            assert_eq!(accelerator(9, mac), None);
            assert_eq!(shortcut_label(12, mac), None);
        }
        assert_eq!(accelerator(0, true).as_deref(), Some("Control+Super+Digit1"));
        assert_eq!(accelerator(8, false).as_deref(), Some("Alt+Digit9"));
        assert_eq!(shortcut_label(0, true).as_deref(), Some("⌃⌘1"));
        assert_eq!(shortcut_label(2, false).as_deref(), Some("Alt+3"));
    }

    /// Parsed like the menu does (`Control+Super+Digit1`); `CmdOrCtrl` resolved per OS.
    fn shortcut(accelerator: &str, mac: bool) -> tauri_plugin_global_shortcut::Shortcut {
        let resolved = accelerator.replace("CmdOrCtrl", if mac { "Super" } else { "Control" });
        resolved.parse().unwrap_or_else(|_| panic!("{accelerator}"))
    }

    #[test]
    fn server_shortcuts_clash_with_no_menu_or_global_shortcut() {
        use crate::menu::MenuAction;
        for mac in [true, false] {
            let mut taken: Vec<(String, u32)> = MenuAction::ALL
                .iter()
                .filter_map(|a| a.accelerator())
                .map(|a| (a.to_string(), shortcut(a, mac).id()))
                .collect();
            // the global defaults of both platforms (a global shortcut wins over a menu one)
            let leader = if mac { "Control+Super+KeyK" } else { "Control+Alt+KeyK" };
            let switcher = if mac {
                "Alt+Super+KeyK"
            } else {
                "Control+Alt+Shift+KeyK"
            };
            let task = crate::shortcuts::default_task_for(if mac { "macos" } else { "windows" });
            for g in [
                Some(leader),
                Some(switcher),
                Some("CmdOrCtrl+Shift+Space"),
                Some("CmdOrCtrl+Shift+KeyV"),
                task,
            ]
            .into_iter()
            .flatten()
            {
                taken.push((g.to_string(), shortcut(g, mac).id()));
            }
            // the web editor's own (TipTap): headings Mod+Alt+0–6, lists Mod+Shift+7–9
            for n in 0..=9 {
                taken.push((
                    format!("CmdOrCtrl+Alt+Digit{n}"),
                    shortcut(&format!("CmdOrCtrl+Alt+Digit{n}"), mac).id(),
                ));
                taken.push((
                    format!("CmdOrCtrl+Shift+Digit{n}"),
                    shortcut(&format!("CmdOrCtrl+Shift+Digit{n}"), mac).id(),
                ));
            }
            let mut mine = Vec::new();
            for i in 0..SHORTCUT_SLOTS {
                let a = accelerator(i, mac).unwrap();
                let id = shortcut(&a, mac).id();
                assert!(!mine.contains(&id), "{a} twice");
                mine.push(id);
                if let Some((other, _)) = taken.iter().find(|(_, t)| *t == id) {
                    panic!("{a} clashes with {other} (mac: {mac})");
                }
            }
            // never Ctrl+Alt+digit on Windows / Linux: that is AltGr+digit ({ [ ] } on German layouts)
            if !mac {
                for i in 0..SHORTCUT_SLOTS {
                    let s = shortcut(&accelerator(i, mac).unwrap(), mac);
                    let altgr =
                        tauri_plugin_global_shortcut::Modifiers::CONTROL | tauri_plugin_global_shortcut::Modifiers::ALT;
                    assert!(!s.mods.contains(altgr));
                }
            }
        }
    }

    #[test]
    fn a_global_shortcut_on_a_server_key_is_noticed() {
        assert!(takes_server_keys(&shortcut("Control+Super+Digit3", true), true));
        assert!(!takes_server_keys(&shortcut("Control+Super+Digit3", true), false));
        assert!(takes_server_keys(&shortcut("Alt+Digit9", false), false));
        assert!(!takes_server_keys(&shortcut("Control+Super+Digit0", true), true));
        assert!(!takes_server_keys(&shortcut("Control+Super+KeyK", true), true));
    }

    #[test]
    fn choices_follow_the_saved_order_and_mark_the_current_one() {
        let mut s = Settings::default();
        s.remember(&o("work.acme.com"), "Acme", None);
        s.remember(&ServerOrigin::cloud(), "Brainitive", None);
        s.remember(&o("localhost:8000"), "Brainitive", None);
        let list = choices(&s, true);
        let names: Vec<_> = list.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Acme", "Brainitive Cloud", "localhost:8000"]);
        assert_eq!(list[1].host, "www.brainitive.app");
        assert_eq!(list[0].shortcut.as_deref(), Some("⌃⌘1"));
        assert!(list[2].current && !list[0].current);
        assert_eq!(list[0].id, server_id(&o("work.acme.com")));
    }
}
