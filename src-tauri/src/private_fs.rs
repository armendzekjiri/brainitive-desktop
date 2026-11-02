//! Files and folders only this OS user may read (BRAI-377): the capture queue, the offline copy
//! and outbox, the downloaded update, the voice model folder.
//!
//! On Unix a file is **created** with mode 0600 (never written first and narrowed afterwards,
//! which would leave a window where another user could open it), and folders with 0700. On
//! Windows the user's profile ACLs already keep other users out; the calls are plain there.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Mode of private files on Unix.
pub const FILE_MODE: u32 = 0o600;
/// Mode of private folders on Unix.
pub const DIR_MODE: u32 = 0o700;

/// Creates `dir` (and missing parents) with mode 0700, and narrows `dir` itself to 0700 when it
/// already existed with a wider mode. Parents that already exist are left alone.
pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, DIR_MODE);
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir)?.permissions().mode() & 0o777;
        if mode != DIR_MODE {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(DIR_MODE))?;
        }
    }
    Ok(())
}

/// Opens a fresh file with mode 0600 from the start (`create_new`: an existing file, which may
/// have another mode, is removed first).
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, FILE_MODE);
    options.open(path)
}

/// Writes `bytes` to `path` (mode 0600 on Unix); the folder is created private when missing.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        ensure_dir(dir)?;
    }
    let mut file = create_private(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Like [`write`], but through a sibling temp file (`<name>.tmp`, also 0600) and a rename, so a
/// crash never leaves half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_path(path);
    if let Err(e) = write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
}

/// `cache.json` → `cache.json.tmp`.
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brainitive-private-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn temp_files_sit_next_to_the_target() {
        assert_eq!(
            temp_path(Path::new("/a/capture-queue.json")),
            PathBuf::from("/a/capture-queue.json.tmp")
        );
    }

    #[test]
    fn writes_private_files_in_private_folders() {
        let root = temp("write");
        let file = root.join("offline").join("abc").join("cache.json");
        write_atomic(&file, b"{}").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"{}");
        assert!(!temp_path(&file).exists(), "the temp file was renamed");
        #[cfg(unix)]
        {
            assert_eq!(mode(&file), 0o600);
            assert_eq!(mode(file.parent().unwrap()), 0o700);
            assert_eq!(mode(&root.join("offline")), 0o700, "created parents are private too");
        }
        // again over an existing file
        write_atomic(&file, b"[1]").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"[1]");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_leftover_temp_file_with_a_wide_mode_is_not_reused() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("leftover");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("capture-queue.json");
        let tmp = temp_path(&file);
        std::fs::write(&tmp, b"old").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&file, b"new").unwrap();
        assert_eq!(mode(&file), 0o600);
        // a plain (non-atomic) write over a wide file also ends up 0600
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&file, b"x").unwrap();
        assert_eq!(mode(&file), 0o600);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_wide_folder_is_narrowed() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("narrow");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_dir(&root).unwrap();
        assert_eq!(mode(&root), 0o700);
        let _ = std::fs::remove_dir_all(root);
    }
}
