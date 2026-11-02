//! Help → Copy Diagnostics / Open Logs Folder (BRAI-368). The text is built here (pure) and put on
//! the clipboard from Rust (`tauri-plugin-clipboard-manager`); nothing is sent anywhere. It holds
//! the app and OS versions, the webview version, the current server's **origin** (no path, no user
//! data), whether a sign-in is stored (yes / no) and the last 50 log lines — which were redacted
//! when written and are redacted again here.

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

use crate::redact;
use crate::state::{state, Phase};
use crate::{logging, session, version};

pub const LOG_LINES: usize = 50;

/// What goes into the report.
pub struct Report {
    pub app_version: String,
    pub debug_build: bool,
    pub os: String,
    pub arch: String,
    pub webview: Option<String>,
    /// The current server's origin, or `None` before one is chosen.
    pub server: Option<String>,
    pub server_version: Option<String>,
    pub signed_in: bool,
    pub phase: String,
    pub log_lines: Vec<String>,
}

pub fn render(report: &Report) -> String {
    let mut out = String::new();
    let mut line = |label: &str, value: &str| {
        out.push_str(&format!("{label}: {}\n", redact::redact(value)));
    };
    line(
        "Brainitive Desktop",
        &format!(
            "{}{}",
            report.app_version,
            if report.debug_build { " (debug build)" } else { "" }
        ),
    );
    line("OS", &format!("{} ({})", report.os, report.arch));
    line("Webview", report.webview.as_deref().unwrap_or("unknown"));
    line("Server", report.server.as_deref().unwrap_or("none chosen"));
    if let Some(v) = &report.server_version {
        line("Server version", v);
    }
    line("Signed in", if report.signed_in { "yes" } else { "no" });
    line("Screen", &report.phase);
    let lines: Vec<&String> = report
        .log_lines
        .iter()
        .rev()
        .take(LOG_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    out.push_str(&format!("\nLast {} log lines (redacted):\n", lines.len()));
    for l in lines {
        out.push_str(&redact::log_line(l, 2000));
        out.push('\n');
    }
    out
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Choose => "choose server",
        Phase::SignIn => "sign in",
        Phase::Waiting => "waiting for the browser",
        Phase::Connecting => "connecting",
        Phase::Unreachable => "server unreachable",
        Phase::UpdateRequired => "update required",
        Phase::InApp => "in the app",
    }
}

async fn collect(app: &AppHandle) -> Report {
    let (origin, phase, server_version) = {
        let s = state(app);
        let inner = s.lock();
        let origin = inner.current_origin();
        let version = inner
            .instance
            .as_ref()
            .filter(|(o, _)| Some(o) == origin.as_ref())
            .and_then(|(_, info)| info.version.clone());
        (origin, inner.phase, version)
    };
    let signed_in = match &origin {
        Some(o) => state(app).session.has_stored_sign_in(o).await,
        None => false,
    };
    Report {
        app_version: version::APP_VERSION.to_string(),
        debug_build: cfg!(debug_assertions),
        os: session::os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
        arch: std::env::consts::ARCH.to_string(),
        webview: tauri::webview_version().ok(),
        server: origin.map(|o| o.as_str().to_string()),
        server_version,
        signed_in,
        phase: phase_name(phase).to_string(),
        log_lines: logging::log_dir()
            .map(|dir| logging::tail_lines(dir, LOG_LINES))
            .unwrap_or_default(),
    }
}

/// Help → Copy Diagnostics.
pub async fn copy(app: &AppHandle) {
    let text = render(&collect(app).await);
    let (kind, message) = match app.clipboard().write_text(text) {
        Ok(()) => (
            MessageDialogKind::Info,
            "The diagnostics were copied to the clipboard. They contain no tokens, cookies or personal data; \
             you can paste them into a support request."
                .to_string(),
        ),
        Err(e) => {
            log::warn!("diagnostics could not be copied: {e}");
            (
                MessageDialogKind::Error,
                format!("The diagnostics could not be copied: {e}"),
            )
        }
    };
    app.dialog()
        .message(message)
        .title("Brainitive diagnostics")
        .kind(kind)
        .show(|_| {});
}

/// Help → Open Logs Folder.
pub fn open_logs_folder(app: &AppHandle) {
    let Some(dir) = logging::log_dir() else {
        app.dialog()
            .message("The log folder is not available.")
            .title("Brainitive")
            .kind(MessageDialogKind::Error)
            .show(|_| {});
        return;
    };
    // packaged (Microsoft Store build): Explorer sees the package's copy of AppData, not ours
    let dir = crate::msstore::outside_view(app, dir);
    if let Err(e) = app.opener().open_path(dir.to_string_lossy(), None::<&str>) {
        log::warn!("could not open the log folder: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        Report {
            app_version: "0.1.0".into(),
            debug_build: false,
            os: "macOS 15.4".into(),
            arch: "aarch64".into(),
            webview: Some("619.1.15".into()),
            server: Some("https://brain.example.com".into()),
            server_version: Some("1.52.0".into()),
            signed_in: true,
            phase: "in the app".into(),
            log_lines: vec!["2026-11-02T00:00:00.000Z INFO x started".into()],
        }
    }

    #[test]
    fn has_every_field_the_card_asks_for() {
        let text = render(&report());
        assert!(text.contains("Brainitive Desktop: 0.1.0\n"));
        assert!(text.contains("OS: macOS 15.4 (aarch64)\n"));
        assert!(text.contains("Webview: 619.1.15\n"));
        assert!(text.contains("Server: https://brain.example.com\n"));
        assert!(text.contains("Server version: 1.52.0\n"));
        assert!(text.contains("Signed in: yes\n"));
        assert!(text.contains("Last 1 log lines (redacted):\n2026-11-02T00:00:00.000Z INFO x started\n"));
    }

    #[test]
    fn unknowns_are_said_plainly() {
        let mut r = report();
        r.webview = None;
        r.server = None;
        r.server_version = None;
        r.signed_in = false;
        r.debug_build = true;
        let text = render(&r);
        assert!(text.contains("Webview: unknown\n"));
        assert!(text.contains("Server: none chosen\n"));
        assert!(!text.contains("Server version"));
        assert!(text.contains("Signed in: no\n"));
        assert!(text.contains("0.1.0 (debug build)"));
    }

    #[test]
    fn keeps_only_the_last_fifty_lines_and_redacts_them_again() {
        let mut r = report();
        r.log_lines = (0..80).map(|i| format!("line {i}")).collect();
        r.log_lines
            .push("callback https://x.example/cb?code=AuthCode1234567890 token=bra_0123456789abcdefghij".into());
        let text = render(&r);
        assert!(text.contains("Last 50 log lines"));
        assert!(!text.contains("line 30\n") && text.contains("line 31\n") && text.contains("line 79\n"));
        assert!(!text.contains("AuthCode1234567890") && !text.contains("bra_0123"));
        assert!(text.contains("https://x.example/cb?[redacted]"));
    }

    #[test]
    fn the_server_is_an_origin_only() {
        let mut r = report();
        r.server = Some("https://user:pw@brain.example.com/path?token=abc".into());
        let text = render(&r);
        assert!(!text.contains("user:pw") && !text.contains("abc"), "{text}");
    }
}
