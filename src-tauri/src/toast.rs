//! Windows: a notification that leads to a page carries `brainitive://open?url=<page>` as its
//! protocol activation, so a click opens the page whenever it happens — on the banner, or later in
//! the notification centre, even after the app was quit (Windows starts `Brainitive.exe` with the
//! link; the single-instance plugin hands it to a running app; `flows` opens it like any deep link,
//! through `links::resolve_deep_link_target`).
//!
//! The plugin's toasts report a click only while the banner is on screen. Other targets (a file, the
//! offline view) and the dev build keep using them: in a dev build `brainitive://` belongs to the
//! installed app, not to the one being developed.

use url::Url;

/// `brainitive://open?url=%2Fpath%3Fquery` for a page of the current server.
pub fn open_link(page: &Url) -> String {
    let mut path = page.path().to_string();
    if let Some(q) = page.query() {
        path.push('?');
        path.push_str(q);
    }
    let encoded: String = url::form_urlencoded::byte_serialize(path.as_bytes()).collect();
    format!("brainitive://open?url={encoded}")
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\n' => {}
            c => out.push(c),
        }
    }
    out
}

/// The toast's XML (ToastGeneric: title, then the body when there is one).
pub fn toast_xml(title: &str, body: &str, launch: &str) -> String {
    let body = if body.trim().is_empty() {
        String::new()
    } else {
        format!("<text>{}</text>", escape(body))
    };
    format!(
        "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text>{body}</binding></visual></toast>",
        escape(launch),
        escape(title)
    )
}

/// Shows a toast leading to `page`; `false`: not shown this way (not Windows, a dev build, an
/// error), the caller uses the plugin.
pub fn show_page(app: &tauri::AppHandle, title: &str, body: &str, page: &Url) -> bool {
    #[cfg(windows)]
    {
        if tauri::is_dev() {
            return false;
        }
        // packaged (Store): the package's identity; else the installer's Start-menu shortcut
        let app_id = crate::msstore::package_family_name()
            .map(|family| crate::msstore::aumid(&family))
            .unwrap_or_else(|| app.config().identifier.clone());
        let xml = toast_xml(title, body, &open_link(page));
        match show(&app_id, &xml) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("notification failed: {e}");
                false
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (app, title, body, page);
        false
    }
}

#[cfg(windows)]
fn show(app_id: &str, xml: &str) -> windows::core::Result<()> {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?.Show(&toast)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::{parse_deep_link, DeepLink};

    #[test]
    fn the_link_is_an_open_deep_link_back_to_the_same_page() {
        let page = Url::parse("https://brain.example.com/workspaces/a/items/12?tab=comments&x=1").unwrap();
        let link = open_link(&page);
        assert!(link.starts_with("brainitive://open?url=%2Fworkspaces%2Fa%2Fitems%2F12%3Ftab%3D"));
        assert_eq!(
            parse_deep_link(&Url::parse(&link).unwrap()),
            Some(DeepLink::Open {
                target: "/workspaces/a/items/12?tab=comments&x=1".into()
            })
        );
    }

    #[test]
    fn server_text_cannot_break_out_of_the_xml() {
        let xml = toast_xml(
            "Ana <b>mentioned</b> you & \"them\"",
            "it's\u{7}",
            "brainitive://open?url=%2Fa",
        );
        assert_eq!(
            xml,
            "<toast activationType=\"protocol\" launch=\"brainitive://open?url=%2Fa\"><visual><binding template=\"ToastGeneric\"><text>Ana &lt;b&gt;mentioned&lt;/b&gt; you &amp; &quot;them&quot;</text><text>it&apos;s</text></binding></visual></toast>"
        );
        assert!(!toast_xml("t", "  ", "l").contains("<text></text>"));
    }
}
