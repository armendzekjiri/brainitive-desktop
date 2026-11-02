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
pub fn clean_name(suggested: &str) -> String {
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    let name: String = cleaned.chars().take(200).collect();
    if name.is_empty() {
        "download".into()
    } else {
        name
    }
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
                return true; // the webview's own default
            };
            *destination = path.clone();
            let s = state(app);
            let mut inner = s.lock();
            if inner.downloads.len() >= 50 {
                inner.downloads.remove(0);
            }
            inner.downloads.push(Pending {
                url: url.to_string(),
                name,
                path,
                staged,
            });
            true
        }
        DownloadEvent::Finished { url, path, success } => {
            let pending = take(app, &url, path.as_deref());
            match (pending, success) {
                (Some(p), true) if p.staged => ask_where(app, p),
                (Some(p), true) => crate::bridge::show_file_notification(
                    app,
                    "Download finished",
                    &format!("{} is in your Downloads folder.", p.name),
                    p.path,
                ),
                (pending, false) => {
                    if let Some(p) = pending.filter(|p| p.staged) {
                        remove_staged(&p.path);
                    }
                    crate::bridge::show_notification(app, "Download failed", "The file could not be downloaded.", None);
                }
                (None, true) => crate::bridge::show_notification(app, "Download finished", "The file was saved.", None),
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
    if let Some(w) = crate::shell::target_window(app) {
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
                let name = target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| pending.name.clone());
                crate::bridge::show_file_notification(&handle, "Download saved", &name, target);
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
}
