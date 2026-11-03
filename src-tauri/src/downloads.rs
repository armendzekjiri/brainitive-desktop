//! Downloads of the app windows (BRAI-363): exports, invoices, attachments.
//!
//! A webview has to be told the destination synchronously when a download starts, but a Save
//! dialog is asynchronous (and must not block the main thread). So, with "Ask where to save"
//! on (the default), the file first goes to a private staging folder in the app's cache; when
//! it is complete, a native **Save As** dialog (`tauri-plugin-dialog`) asks where it goes and the
//! file is moved there. Cancel deletes it. With the preference off it goes straight to the
//! Downloads folder under a unique name (never overwriting anything).

use std::path::{Path, PathBuf};

use tauri::webview::DownloadEvent;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use url::Url;

use crate::state::state;

/// A download in progress (macOS reports no path when it finishes: matched by URL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub url: String,
    pub name: String,
    pub path: PathBuf,
    /// In the staging folder, waiting for the Save dialog.
    pub staged: bool,
}

/// The file name a download is offered under: the last path component of what the server
/// suggested, without control / reserved characters, never empty or hidden.
/// BRAI-377: also without Unicode bidi and other format characters (a right-to-left override
/// turns `invoice_fdp.exe` into what reads as `invoice_exe.pdf`), without trailing dots and
/// spaces (Windows drops them), and never a Windows device name (`CON`, `NUL.txt`, `COM1`, …).
pub fn clean_name(suggested: &str) -> String {
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !is_format_char(*c) && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    let name: String = cleaned.chars().take(200).collect();
    let name = name.trim_end_matches(['.', ' ']).trim();
    if name.is_empty() {
        "download".into()
    } else if is_windows_device_name(name) {
        format!("_{name}")
    } else {
        name.to_string()
    }
}

/// Unicode format characters (general category Cf) and the line / paragraph separators:
/// invisible, and the bidi ones (U+200E/F, U+202A–202E, U+2066–2069, U+061C) reorder what the
/// user reads.
fn is_format_char(c: char) -> bool {
    matches!(c as u32,
        0x00AD
        | 0x0600..=0x0605
        | 0x061C
        | 0x06DD
        | 0x070F
        | 0x0890..=0x0891
        | 0x08E2
        | 0x180E
        | 0x200B..=0x200F
        | 0x2028..=0x202E
        | 0x2060..=0x2064
        | 0x2066..=0x206F
        | 0xFEFF
        | 0xFFF9..=0xFFFB
        | 0x110BD
        | 0x110CD
        | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A
        | 0xE0001
        | 0xE0020..=0xE007F)
}

/// `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9` (also `COM¹`–`³`), with or without
/// an extension and in any case: Windows opens the device instead of a file.
fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        s => {
            let mut chars = s.chars();
            let prefix: String = chars.by_ref().take(3).collect();
            let rest: String = chars.collect();
            matches!(prefix.as_str(), "COM" | "LPT")
                && matches!(
                    rest.as_str(),
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
        }
    }
}

/// The `com.apple.quarantine` value of a downloaded file (BRAI-377):
/// `flags;hex seconds;agent;UUID`, like browsers write it, so Gatekeeper checks an app or
/// script the user opens from it.
pub fn quarantine_value(unix_secs: u64, id: &str) -> String {
    format!("0081;{unix_secs:08x};Brainitive;{}", id.to_uppercase())
}

/// Marks a saved download as coming from the internet: the `com.apple.quarantine` attribute on
/// macOS (one already there, e.g. from WebKit, is kept), the `Zone.Identifier` stream (Mark of
/// the Web, zone 3) on Windows. Linux has no such flag. `false`: it could not be marked (the
/// user is warned, codex review 10).
pub fn mark_downloaded(path: &Path) -> bool {
    match quarantine(path) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("could not mark the download as downloaded: {e}");
            false
        }
    }
}

/// Downloads tracked at once. Never evicted while they run (an evicted one would finish without
/// its quarantine mark, codex review 10): a new one beyond this is refused.
pub const MAX_ACTIVE: usize = 50;

/// Whether a new download may start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    Yes,
    /// The same URL is downloading already: macOS reports no path when a download finishes, so
    /// two of them couldn't be told apart (one would stay unmarked).
    Busy,
    Full,
}

pub fn admit(active: &[Pending], url: &str) -> Admit {
    if active.iter().any(|p| p.url == url) {
        Admit::Busy
    } else if active.len() >= MAX_ACTIVE {
        Admit::Full
    } else {
        Admit::Yes
    }
}

/// What happens when a download ends (pure part of [`on_download`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finish {
    /// Staged: the Save dialog, then marked where the user put it.
    AskWhere(Pending),
    /// Saved: mark this file.
    Saved { path: PathBuf, name: String },
    /// Saved, but nobody knows where (no record, no path): the user is warned.
    Unmarked,
    /// Failed (a staged file is deleted).
    Failed { staged: Option<PathBuf> },
}

/// `reported`: the path the webview reports (Windows / Linux; none on macOS).
pub fn finish(pending: Option<Pending>, success: bool, reported: Option<PathBuf>) -> Finish {
    match (pending, success) {
        (Some(p), true) if p.staged => Finish::AskWhere(p),
        (Some(p), true) => Finish::Saved {
            path: p.path,
            name: p.name,
        },
        (None, true) => match reported {
            Some(path) => Finish::Saved {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path,
            },
            None => Finish::Unmarked,
        },
        (p, false) => Finish::Failed {
            staged: p.filter(|p| p.staged).map(|p| p.path),
        },
    }
}

const UNMARKED: &str = "Brainitive could not mark it as downloaded from the internet: be careful when opening it.";

#[cfg(target_os = "macos")]
fn quarantine(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let file = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let name = c"com.apple.quarantine";
    // SAFETY: both strings are NUL-terminated and outlive the calls; a null buffer of size 0
    // only asks for the attribute's size.
    let existing = unsafe { libc::getxattr(file.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    if existing > 0 {
        return Ok(());
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let value = quarantine_value(secs, &crate::queue::new_request_id());
    // SAFETY: `value` is a live buffer of `value.len()` bytes.
    let result = unsafe { libc::setxattr(file.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn quarantine(path: &Path) -> std::io::Result<()> {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":Zone.Identifier");
    std::fs::write(PathBuf::from(stream), b"[ZoneTransfer]\r\nZoneId=3\r\n")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn quarantine(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// `name.ext`, `name (1).ext`, … — never overwrites an existing file.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let name = clean_name(name);
    let candidate = dir.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    (1..10_000)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(candidate)
}

/// Moves a finished download to where the user chose: a rename, or copy + delete when the
/// target is on another volume.
pub fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if from == to {
        return Ok(());
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

fn staging_root(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_cache_dir().ok().map(|d| d.join("downloads"))
}

/// Leftovers of a previous run (the app quit while a Save dialog was open).
pub fn clean_staging(app: &AppHandle) {
    if let Some(root) = staging_root(app) {
        let _ = std::fs::remove_dir_all(root);
    }
}

/// The `on_download` handler of every app window.
pub fn on_download(app: &AppHandle, event: DownloadEvent<'_>) -> bool {
    match event {
        DownloadEvent::Requested { url, destination } => {
            let name = clean_name(
                &destination
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            let ask = state(app).lock().settings.preferences.ask_where_to_save;
            let target = if ask {
                // a folder per download: the same name twice never collides
                staging_root(app).and_then(|root| {
                    let dir = root.join(crate::queue::new_request_id());
                    std::fs::create_dir_all(&dir).ok().map(|_| (dir.join(&name), true))
                })
            } else {
                None
            }
            .or_else(|| {
                app.path()
                    .download_dir()
                    .ok()
                    .map(|dir| (unique_path(&dir, &name), false))
            });
            let Some((path, staged)) = target else {
                // no folder of ours to put it in: the webview's own default could not be
                // tracked to its end (no quarantine mark) — refuse instead
                crate::bridge::show_notification(
                    app,
                    "Download not started",
                    "Brainitive has no folder to save downloads in.",
                    None,
                );
                return false;
            };
            let refused = {
                let s = state(app);
                let mut inner = s.lock();
                match admit(&inner.downloads, url.as_str()) {
                    Admit::Yes => {
                        inner.downloads.push(Pending {
                            url: url.to_string(),
                            name,
                            path: path.clone(),
                            staged,
                        });
                        None
                    }
                    Admit::Busy => Some("This file is being downloaded already."),
                    Admit::Full => Some("Too many downloads at once. Wait for some to finish."),
                }
            };
            if let Some(reason) = refused {
                if staged {
                    remove_staged(&path);
                }
                crate::bridge::show_notification(app, "Download not started", reason, None);
                return false;
            }
            *destination = path;
            true
        }
        DownloadEvent::Finished { url, path, success } => {
            let pending = take(app, &url, path.as_deref());
            match finish(pending, success, path) {
                Finish::AskWhere(p) => ask_where(app, p),
                Finish::Saved { path, name } => {
                    let body = if mark_downloaded(&path) {
                        format!("{name} is in your Downloads folder.")
                    } else {
                        format!("{name} is in your Downloads folder. {UNMARKED}")
                    };
                    crate::bridge::show_file_notification(app, "Download finished", &body, path)
                }
                Finish::Unmarked => {
                    log::warn!("a download finished that was not tracked: not marked");
                    crate::bridge::show_notification(
                        app,
                        "Download finished",
                        &format!("The file was saved. {UNMARKED}"),
                        None,
                    )
                }
                Finish::Failed { staged } => {
                    if let Some(path) = staged {
                        remove_staged(&path);
                    }
                    crate::bridge::show_notification(app, "Download failed", "The file could not be downloaded.", None);
                }
            }
            true
        }
        _ => true,
    }
}

fn take(app: &AppHandle, url: &Url, path: Option<&Path>) -> Option<Pending> {
    let s = state(app);
    let mut inner = s.lock();
    let index = inner
        .downloads
        .iter()
        .position(|p| path.is_some_and(|path| p.path == path))
        .or_else(|| inner.downloads.iter().position(|p| p.url == url.as_str()))?;
    Some(inner.downloads.remove(index))
}

fn remove_staged(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The native Save As dialog for a finished, staged download.
fn ask_where(app: &AppHandle, pending: Pending) {
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save Download")
        .set_file_name(&pending.name);
    if let Ok(dir) = app.path().download_dir() {
        dialog = dialog.set_directory(dir);
    }
    if let Some(w) = crate::shell::main_window(app) {
        dialog = dialog.set_parent(&w);
    }
    let handle = app.clone();
    dialog.save_file(move |chosen| {
        let Some(target) = chosen.and_then(|p| p.into_path().ok()) else {
            remove_staged(&pending.path);
            return;
        };
        match move_file(&pending.path, &target) {
            Ok(()) => {
                remove_staged(&pending.path);
                let marked = mark_downloaded(&target);
                let name = target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| pending.name.clone());
                let body = if marked { name } else { format!("{name}. {UNMARKED}") };
                crate::bridge::show_file_notification(&handle, "Download saved", &body, target);
            }
            Err(e) => {
                log::warn!("could not move the download: {e}");
                remove_staged(&pending.path);
                crate::bridge::show_notification(
                    &handle,
                    "Download not saved",
                    "The file could not be saved there. Try another folder.",
                    None,
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brainitive-dl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names_stay_inside_the_folder_and_readable() {
        assert_eq!(clean_name("report.pdf"), "report.pdf");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("..\\..\\win.ini"), "win.ini");
        assert_eq!(clean_name("a:b*c?.txt"), "abc.txt");
        assert_eq!(clean_name(".hidden"), "hidden");
        assert_eq!(clean_name(""), "download");
        assert_eq!(clean_name("  \u{0007} "), "download");
        assert_eq!(clean_name(&"x".repeat(500)).len(), 200);
    }

    #[test]
    fn names_lose_bidi_and_format_characters() {
        // "invoice_\u{202E}fdp.exe" reads as "invoice_exe.pdf"
        assert_eq!(clean_name("invoice_\u{202E}fdp.exe"), "invoice_fdp.exe");
        for c in [
            '\u{200E}',
            '\u{200F}',
            '\u{202A}',
            '\u{202B}',
            '\u{202C}',
            '\u{202D}',
            '\u{202E}',
            '\u{2066}',
            '\u{2067}',
            '\u{2068}',
            '\u{2069}',
            '\u{200B}',
            '\u{FEFF}',
            '\u{00AD}',
            '\u{061C}',
            '\u{2028}',
            '\u{E0041}',
        ] {
            assert_eq!(clean_name(&format!("a{c}b.pdf")), "ab.pdf", "U+{:04X}", c as u32);
        }
        // ordinary non-ASCII stays
        assert_eq!(clean_name("Übersicht – März.pdf"), "Übersicht – März.pdf");
        assert_eq!(clean_name("報告.pdf"), "報告.pdf");
    }

    #[test]
    fn names_are_never_windows_devices_or_end_in_dots() {
        for device in [
            "CON",
            "con",
            "PRN",
            "AUX",
            "NUL",
            "nul.txt",
            "COM1",
            "com9.log",
            "LPT1",
            "lpt9.tar.gz",
            "COM¹",
            "Con .txt",
        ] {
            let cleaned = clean_name(device);
            assert!(cleaned.starts_with('_'), "{device} → {cleaned}");
        }
        assert_eq!(clean_name("NUL.txt"), "_NUL.txt");
        for fine in [
            "CONSOLE.txt",
            "COM10",
            "COM0",
            "LPT",
            "nullable.rs",
            "aux-notes.md",
            "icon.png",
        ] {
            assert_eq!(clean_name(fine), fine);
        }
        assert_eq!(clean_name("report.pdf. . "), "report.pdf");
        assert_eq!(clean_name("report..."), "report");
        assert_eq!(clean_name("..."), "download");
        assert_eq!(clean_name(". . ."), "download");
    }

    #[test]
    fn the_quarantine_value_has_the_browser_format() {
        assert_eq!(
            quarantine_value(0x6543_21ab, "0f1e2d3c-aaaa-4bbb-8ccc-0123456789ab"),
            "0081;654321ab;Brainitive;0F1E2D3C-AAAA-4BBB-8CCC-0123456789AB"
        );
        assert_eq!(quarantine_value(5, "x"), "0081;00000005;Brainitive;X");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn saved_downloads_are_quarantined() {
        let dir = temp("quarantine");
        let file = dir.join("setup.dmg");
        std::fs::write(&file, b"x").unwrap();
        mark_downloaded(&file);
        let out = std::process::Command::new("xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(&file)
            .output()
            .unwrap();
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(value.starts_with("0081;"), "{value:?}");
        assert!(value.contains(";Brainitive;"), "{value:?}");
        // a second mark keeps the first
        mark_downloaded(&file);
        let again = std::process::Command::new("xattr")
            .args(["-p", "com.apple.quarantine"])
            .arg(&file)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&again.stdout).trim(), value);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn downloads_never_overwrite() {
        let dir = temp("unique");
        assert_eq!(unique_path(&dir, "a.pdf"), dir.join("a.pdf"));
        std::fs::write(dir.join("a.pdf"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.pdf"), dir.join("a (1).pdf"));
        assert_eq!(unique_path(&dir, "../../etc/a.pdf"), dir.join("a (1).pdf"));
        std::fs::write(dir.join("README"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "README"), dir.join("README (1)"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn moves_the_staged_file() {
        let dir = temp("move");
        let staged = dir.join("staging").join("x.csv");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"a,b").unwrap();
        let target = dir.join("x.csv");
        move_file(&staged, &target).unwrap();
        assert!(!staged.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"a,b");
        assert!(move_file(&dir.join("missing"), &dir.join("y")).is_err());
        move_file(&target, &target).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_successful_download_with_a_known_path_is_marked() {
        let p = |url: &str, staged: bool| Pending {
            url: url.into(),
            name: "setup.dmg".into(),
            path: PathBuf::from(format!("/staging/{url}/setup.dmg")),
            staged,
        };
        // tracked: marked where it is, or after the Save dialog
        assert_eq!(
            finish(Some(p("a", false)), true, None),
            Finish::Saved {
                path: PathBuf::from("/staging/a/setup.dmg"),
                name: "setup.dmg".into()
            }
        );
        assert_eq!(finish(Some(p("a", true)), true, None), Finish::AskWhere(p("a", true)));
        // untracked but the webview says where (Windows / Linux): marked too, not just "saved"
        assert_eq!(
            finish(None, true, Some(PathBuf::from("/Users/me/Downloads/x.exe"))),
            Finish::Saved {
                path: PathBuf::from("/Users/me/Downloads/x.exe"),
                name: "x.exe".into()
            }
        );
        // nobody knows where: the user is warned instead of a plain "saved"
        assert_eq!(finish(None, true, None), Finish::Unmarked);
        assert_eq!(
            finish(Some(p("a", true)), false, None),
            Finish::Failed {
                staged: Some(PathBuf::from("/staging/a/setup.dmg"))
            }
        );
        assert_eq!(finish(None, false, None), Finish::Failed { staged: None });
    }

    #[test]
    fn running_downloads_are_never_evicted() {
        let mut active: Vec<Pending> = Vec::new();
        for i in 0..MAX_ACTIVE {
            let url = format!("https://brainitive.example.com/f/{i}");
            assert_eq!(admit(&active, &url), Admit::Yes);
            active.push(Pending {
                url,
                name: format!("{i}.pdf"),
                path: PathBuf::from(format!("/downloads/{i}.pdf")),
                staged: false,
            });
        }
        // the 51st is refused; the first 50 all stay tracked until they finish
        assert_eq!(admit(&active, "https://brainitive.example.com/f/new"), Admit::Full);
        assert_eq!(active.len(), MAX_ACTIVE);
        // the same URL twice can't be told apart on macOS (no path when it finishes)
        active.truncate(3);
        assert_eq!(admit(&active, "https://brainitive.example.com/f/1"), Admit::Busy);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_that_cannot_be_marked_is_reported() {
        assert!(!mark_downloaded(Path::new("/nonexistent-brainitive-dir/setup.dmg")));
        let dir = temp("mark-ok");
        let file = dir.join("ok.pdf");
        std::fs::write(&file, b"x").unwrap();
        assert!(mark_downloaded(&file));
        let _ = std::fs::remove_dir_all(dir);
    }
}
