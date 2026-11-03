//! The app's keyboard shortcuts inside the main window's webviews (Windows).
//!
//! The app menu's accelerators are translated for the window that holds the menu. In the tabbed
//! main window every page is a child WebView2 whose unhandled keys go to that child, not to the
//! menu's window, so Ctrl+T, Ctrl+W, Ctrl+Tab, Ctrl+R … never fired. Each tab and the top bar
//! therefore watch WebView2's `AcceleratorKeyPressed` (keys with Ctrl / Alt and function keys,
//! before the page sees them) and run the same menu action. Only the menu's own shortcuts are
//! taken; everything else (copy, paste, the page's own keys) goes on to the page as before.

use tauri::{AppHandle, Webview};

/// A key combination: Ctrl (= `CmdOrCtrl` on Windows), Shift, Alt and the Windows virtual-key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub vk: u32,
}

/// `"CmdOrCtrl+Shift+N"` → a [`Combo`] (`None` for keys the menu doesn't use).
pub fn parse(accelerator: &str) -> Option<Combo> {
    let mut combo = Combo {
        ctrl: false,
        shift: false,
        alt: false,
        vk: 0,
    };
    for part in accelerator.split('+') {
        match part {
            "CmdOrCtrl" | "Ctrl" | "Control" | "CommandOrControl" => combo.ctrl = true,
            "Shift" => combo.shift = true,
            "Alt" | "Option" => combo.alt = true,
            key => combo.vk = vk_of(key)?,
        }
    }
    (combo.vk != 0).then_some(combo)
}

fn vk_of(key: &str) -> Option<u32> {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return match key {
            "Tab" => Some(0x09),
            "Plus" => Some(0xBB),
            _ => None,
        };
    };
    match c {
        'A'..='Z' | '0'..='9' => Some(c as u32),
        'a'..='z' => Some(c.to_ascii_uppercase() as u32),
        ',' => Some(0xBC),
        '=' => Some(0xBB),
        '-' => Some(0xBD),
        '[' => Some(0xDB),
        ']' => Some(0xDD),
        _ => None,
    }
}

/// The menu id a combination runs: the app menu's actions, then Servers ▸ (Alt+1…9).
pub fn menu_id_for(app: &AppHandle, pressed: Combo) -> Option<String> {
    for action in crate::menu::MenuAction::ALL {
        if action.accelerator().and_then(parse) == Some(pressed) {
            return Some(action.id().to_string());
        }
    }
    // numpad +/- for zoom, as in browsers
    if pressed.ctrl && !pressed.alt {
        match pressed.vk {
            0x6B => return Some(crate::menu::MenuAction::ZoomIn.id().to_string()),
            0x6D => return Some(crate::menu::MenuAction::ZoomOut.id().to_string()),
            _ => {}
        }
    }
    let choices = {
        let s = crate::state::state(app);
        let inner = s.lock();
        crate::servers::choices(&inner.settings, false)
    };
    choices.iter().enumerate().find_map(|(i, c)| {
        let accelerator = crate::servers::accelerator(i, false)?;
        (parse(&accelerator) == Some(pressed)).then(|| format!("{}{}", crate::menu::SERVER_PICK, c.id))
    })
}

/// Watches the webview's shortcut keys (Windows; elsewhere the menu gets them).
#[cfg(windows)]
pub fn attach(app: &AppHandle, webview: &Webview) {
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
    };

    #[link(name = "user32")]
    extern "system" {
        fn GetKeyState(vk: i32) -> i16;
    }
    fn down(vk: i32) -> bool {
        // SAFETY: GetKeyState only reads this thread's keyboard state
        unsafe { GetKeyState(vk) < 0 }
    }

    let app = app.clone();
    let result = webview.with_webview(move |platform| {
        let controller = platform.controller();
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
            let mut vk = 0u32;
            // SAFETY: plain getters of the event's arguments, on the UI thread that raised it
            unsafe {
                args.KeyEventKind(&mut kind)?;
                args.VirtualKey(&mut vk)?;
            }
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
                return Ok(());
            }
            let pressed = Combo {
                ctrl: down(0x11),
                shift: down(0x10),
                alt: down(0x12),
                vk,
            };
            if !pressed.ctrl && !pressed.alt {
                return Ok(());
            }
            if let Some(id) = menu_id_for(&app, pressed) {
                // SAFETY: as above
                unsafe { args.SetHandled(true)? };
                crate::menu::on_menu_event(&app, &id);
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: the controller is alive (we are inside its webview's callback); the handler
        // stays registered for the webview's lifetime
        if let Err(e) = unsafe { controller.add_AcceleratorKeyPressed(&handler, &mut token) } {
            log::warn!("keyboard shortcuts not attached: {e}");
        }
    });
    if let Err(e) = result {
        log::warn!("keyboard shortcuts not attached: {e}");
    }
}

#[cfg(not(windows))]
pub fn attach(_app: &AppHandle, _webview: &Webview) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::MenuAction;

    #[test]
    fn every_menu_accelerator_parses() {
        for action in MenuAction::ALL {
            if let Some(acc) = action.accelerator() {
                assert!(parse(acc).is_some(), "{acc}");
            }
        }
        assert_eq!(
            parse("CmdOrCtrl+T"),
            Some(Combo {
                ctrl: true,
                shift: false,
                alt: false,
                vk: 'T' as u32
            })
        );
        assert_eq!(parse("Ctrl+Shift+Tab").map(|c| (c.shift, c.vk)), Some((true, 0x09)));
        assert_eq!(parse("CmdOrCtrl+,").map(|c| c.vk), Some(0xBC));
        assert_eq!(parse("CmdOrCtrl+F13"), None);
    }

    #[test]
    fn shortcuts_are_unique_after_parsing() {
        let mut seen = Vec::new();
        for action in MenuAction::ALL {
            if let Some(c) = action.accelerator().and_then(parse) {
                assert!(!seen.contains(&c), "{action:?}");
                seen.push(c);
            }
        }
    }
}
