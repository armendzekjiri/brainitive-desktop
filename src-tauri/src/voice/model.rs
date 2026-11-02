//! The speech model (BRAI-439): downloaded on first use, not bundled, so the installer stays
//! small. One file in `<app local data>/voice/`, fetched from a pinned URL (an exact commit of the
//! official whisper.cpp model repository) and accepted only with the exact size and SHA-256 below.
//!
//! The download goes to `<model>.part` while it is hashed; only a complete file with the right
//! checksum is renamed to the model name. A cancelled, failed or wrong download leaves nothing
//! behind ([`Writer`] removes its part file when dropped unfinished).

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use url::Url;

/// What is downloaded and how it is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    /// Shown in Settings.
    pub name: &'static str,
    /// File name in the voice folder.
    pub file: &'static str,
    /// Pinned to a commit of the repository: the bytes behind it cannot change.
    pub url: &'static str,
    /// Lowercase hex SHA-256 of the file.
    pub sha256: &'static str,
    pub size: u64,
}

/// whisper.cpp's multilingual **small** model, 5-bit quantized (q5_1). Chosen over `base`
/// (82 MB as q8_0): base garbled German in the test ("Norge früh aneranrufen" for "morgen früh Anna
/// anrufen"), small got it right, and German / Albanian are what our users speak; small-q5_1 is
/// still fast (≈ 0.2–0.35 s for a 5 s clip with Metal on an M4 Pro) and the q5_1 file is 40 % of
/// the full small model. See docs/tasks/brai-439.md. Source: <https://huggingface.co/ggerganov/whisper.cpp>, the official model
/// repository of whisper.cpp, at commit 5359861c (the checksum is the repository's own LFS oid).
pub const MODEL: ModelSpec = ModelSpec {
    name: "Whisper small, multilingual (190 MB)",
    file: "ggml-small-q5_1.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small-q5_1.bin",
    sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
    size: 190_085_487,
};

/// The folder of the model, inside the app's local data dir.
pub const FOLDER: &str = "voice";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("The voice model could not be saved: {0}")]
    Io(String),
    #[error("The download could not be completed: {0}")]
    Network(String),
    #[error("The download was incomplete or too large ({got} of {expected} bytes). Try again.")]
    Size { expected: u64, got: u64 },
    #[error("The downloaded voice model is damaged (checksum mismatch) and was deleted. Try again.")]
    Checksum,
    #[error("The download was cancelled.")]
    Cancelled,
}

impl From<std::io::Error> for ModelError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join(FOLDER)
}

pub fn path(data_dir: &Path, spec: &ModelSpec) -> PathBuf {
    folder(data_dir).join(spec.file)
}

fn part_path(data_dir: &Path, spec: &ModelSpec) -> PathBuf {
    folder(data_dir).join(format!("{}.part", spec.file))
}

/// The model is there and complete (the checksum was verified when it was downloaded; the file
/// is only ever created by a verified rename).
pub fn is_ready(data_dir: &Path, spec: &ModelSpec) -> bool {
    std::fs::metadata(path(data_dir, spec))
        .map(|m| m.is_file() && m.len() == spec.size)
        .unwrap_or(false)
}

/// "Remove voice model": deletes the model and anything else in the voice folder (partial
/// downloads, models of older app versions). Returns the bytes freed.
pub fn remove(data_dir: &Path) -> std::io::Result<u64> {
    let dir = folder(data_dir);
    let mut freed = 0;
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    freed += entry.metadata().map(|m| m.len()).unwrap_or(0);
                    std::fs::remove_file(&p)?;
                }
            }
            let _ = std::fs::remove_dir(&dir);
            Ok(freed)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

/// At start: partial downloads (the app quit mid-download) and models the app no longer uses
/// are deleted; the current model stays.
pub fn clean(data_dir: &Path, spec: &ModelSpec) {
    let Ok(entries) = std::fs::read_dir(folder(data_dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && p.file_name().and_then(|n| n.to_str()) != Some(spec.file) {
            if let Err(e) = std::fs::remove_file(&p) {
                log::warn!("voice: could not delete a leftover file: {e}");
            }
        }
    }
}

/// The download's redirects may only go to Hugging Face's own hosts, over https (the file is
/// served from their CDN). Anything else is refused before a byte is read; the checksum would
/// refuse a wrong file anyway.
pub fn allowed_url(url: &Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    if url.port().is_some_and(|p| p != 443) {
        return false;
    }
    let Some(host) = url.host_str() else { return false };
    let host = host.to_ascii_lowercase();
    host == "huggingface.co" || host.ends_with(".huggingface.co") || host == "hf.co" || host.ends_with(".hf.co")
}

/// Writes a download to the part file and hashes it on the way.
pub struct Writer {
    file: Option<File>,
    hasher: Sha256,
    written: u64,
    part: PathBuf,
    target: PathBuf,
    spec: ModelSpec,
}

impl Writer {
    /// Starts a fresh part file (an older one is truncated).
    pub fn create(data_dir: &Path, spec: &ModelSpec) -> Result<Self, ModelError> {
        std::fs::create_dir_all(folder(data_dir))?;
        let part = part_path(data_dir, spec);
        let file = File::create(&part)?;
        Ok(Self {
            file: Some(file),
            hasher: Sha256::new(),
            written: 0,
            part,
            target: path(data_dir, spec),
            spec: *spec,
        })
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), ModelError> {
        let got = self.written + chunk.len() as u64;
        if got > self.spec.size {
            return Err(ModelError::Size {
                expected: self.spec.size,
                got,
            });
        }
        let file = self.file.as_mut().ok_or(ModelError::Cancelled)?;
        file.write_all(chunk)?;
        self.hasher.update(chunk);
        self.written = got;
        Ok(())
    }

    /// Size and checksum right → the part file becomes the model. Wrong → it is deleted.
    pub fn finish(mut self) -> Result<PathBuf, ModelError> {
        let file = self.file.take().ok_or(ModelError::Cancelled)?;
        file.sync_all()?;
        drop(file);
        if self.written != self.spec.size {
            return Err(ModelError::Size {
                expected: self.spec.size,
                got: self.written,
            });
        }
        let digest = crate::server::hex(&std::mem::take(&mut self.hasher).finalize());
        if digest != self.spec.sha256 {
            return Err(ModelError::Checksum);
        }
        std::fs::rename(&self.part, &self.target)?;
        Ok(self.target.clone())
    }
}

impl Drop for Writer {
    /// Unfinished (cancelled, failed, wrong checksum): the part file goes.
    fn drop(&mut self) {
        self.file.take();
        if self.part.exists() {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

/// Downloads and verifies the model. `progress(done, total)` is called as bytes arrive;
/// `cancel` stops it (the part file is deleted).
pub async fn download(
    spec: &ModelSpec,
    data_dir: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, ModelError> {
    let url = Url::parse(spec.url).map_err(|e| ModelError::Network(e.to_string()))?;
    debug_assert!(allowed_url(&url));
    let client = reqwest::Client::builder()
        .user_agent(format!("BrainitiveDesktop/{}", crate::version::APP_VERSION))
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if !allowed_url(attempt.url()) {
                attempt.error("redirect to a host that is not allowed")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| ModelError::Network(e.to_string()))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| ModelError::Network(short_error(&e)))?;
    if let Some(len) = response.content_length().filter(|l| *l != spec.size) {
        return Err(ModelError::Size {
            expected: spec.size,
            got: len,
        });
    }
    let mut writer = Writer::create(data_dir, spec)?;
    progress(0, spec.size);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ModelError::Cancelled);
        }
        match response.chunk().await {
            Ok(Some(chunk)) => {
                writer.write(&chunk)?;
                progress(writer.written(), spec.size);
            }
            Ok(None) => break,
            Err(e) => return Err(ModelError::Network(short_error(&e))),
        }
    }
    writer.finish()
}

/// A reqwest error without the URL (it carries a signed CDN query string).
fn short_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "the connection timed out".into()
    } else if e.is_connect() {
        "no connection to the download server".into()
    } else if let Some(status) = e.status() {
        format!("the download server answered {status}")
    } else if e.is_redirect() {
        "the download was redirected to a host that is not allowed".into()
    } else {
        "the connection was interrupted".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("brainitive-voice-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A tiny "model" with its real checksum.
    fn spec_for(bytes: &[u8]) -> ModelSpec {
        let sha: &'static str = Box::leak(crate::server::hex(&Sha256::digest(bytes)).into_boxed_str());
        ModelSpec {
            name: "test",
            file: "test-model.bin",
            url: "https://huggingface.co/x/y/resolve/abc/test-model.bin",
            sha256: sha,
            size: bytes.len() as u64,
        }
    }

    #[test]
    fn the_pinned_model_is_well_formed() {
        assert_eq!(MODEL.sha256.len(), 64);
        assert!(MODEL
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        let url = Url::parse(MODEL.url).unwrap();
        assert!(allowed_url(&url));
        // pinned to a commit, not a moving branch
        assert!(!MODEL.url.contains("/resolve/main/"));
        assert!(MODEL.url.ends_with(MODEL.file));
        const { assert!(MODEL.size > 50_000_000 && MODEL.size < 250_000_000) };
    }

    #[test]
    fn paths_live_in_the_voice_folder() {
        let data = Path::new("/data");
        assert_eq!(path(data, &MODEL), Path::new("/data/voice/ggml-small-q5_1.bin"));
        assert_eq!(
            part_path(data, &MODEL),
            Path::new("/data/voice/ggml-small-q5_1.bin.part")
        );
    }

    #[test]
    fn a_complete_download_with_the_right_checksum_becomes_the_model() {
        let dir = TempDir::new("ok");
        let bytes = b"ggml model bytes for a test".repeat(100);
        let spec = spec_for(&bytes);
        assert!(!is_ready(&dir.0, &spec));
        let mut w = Writer::create(&dir.0, &spec).unwrap();
        for chunk in bytes.chunks(97) {
            w.write(chunk).unwrap();
        }
        let p = w.finish().unwrap();
        assert_eq!(p, path(&dir.0, &spec));
        assert_eq!(std::fs::read(&p).unwrap(), bytes);
        assert!(is_ready(&dir.0, &spec));
        assert!(!part_path(&dir.0, &spec).exists());
    }

    #[test]
    fn a_checksum_mismatch_is_deleted_and_never_becomes_the_model() {
        let dir = TempDir::new("mismatch");
        let good = b"the real model".to_vec();
        let spec = spec_for(&good);
        let mut w = Writer::create(&dir.0, &spec).unwrap();
        w.write(b"the fake model").unwrap(); // same length, other bytes
        assert_eq!(w.finish().unwrap_err(), ModelError::Checksum);
        assert!(!is_ready(&dir.0, &spec));
        assert!(!path(&dir.0, &spec).exists());
        assert!(!part_path(&dir.0, &spec).exists(), "the part file is gone");
    }

    #[test]
    fn short_or_oversized_downloads_are_refused() {
        let dir = TempDir::new("size");
        let spec = spec_for(b"0123456789");
        let mut w = Writer::create(&dir.0, &spec).unwrap();
        w.write(b"01234").unwrap();
        assert_eq!(w.finish().unwrap_err(), ModelError::Size { expected: 10, got: 5 });
        assert!(!part_path(&dir.0, &spec).exists());

        let mut w = Writer::create(&dir.0, &spec).unwrap();
        w.write(b"0123456789").unwrap();
        assert_eq!(w.write(b"x").unwrap_err(), ModelError::Size { expected: 10, got: 11 });
        drop(w);
        assert!(!part_path(&dir.0, &spec).exists());
        // a file of the wrong size is not a model
        std::fs::write(path(&dir.0, &spec), b"short").unwrap();
        assert!(!is_ready(&dir.0, &spec));
    }

    #[test]
    fn a_cancelled_download_leaves_no_part_file() {
        let dir = TempDir::new("cancel");
        let spec = spec_for(b"0123456789");
        let mut w = Writer::create(&dir.0, &spec).unwrap();
        w.write(b"0123").unwrap();
        assert!(part_path(&dir.0, &spec).exists());
        drop(w); // cancelled / failed mid-way
        assert!(!part_path(&dir.0, &spec).exists());
        assert!(!is_ready(&dir.0, &spec));
    }

    #[test]
    fn clean_removes_partials_and_old_models_but_keeps_the_current_one() {
        let dir = TempDir::new("clean");
        let spec = spec_for(b"model");
        std::fs::create_dir_all(folder(&dir.0)).unwrap();
        std::fs::write(path(&dir.0, &spec), b"model").unwrap();
        std::fs::write(part_path(&dir.0, &spec), b"mo").unwrap(); // the app quit mid-download
        std::fs::write(folder(&dir.0).join("ggml-old.bin"), b"old").unwrap();
        clean(&dir.0, &spec);
        assert!(is_ready(&dir.0, &spec));
        assert!(!part_path(&dir.0, &spec).exists());
        assert!(!folder(&dir.0).join("ggml-old.bin").exists());
        // no folder yet: nothing happens
        clean(&dir.0.join("nope"), &spec);
    }

    #[test]
    fn remove_deletes_the_model_and_its_folder() {
        let dir = TempDir::new("remove");
        let spec = spec_for(b"model!");
        std::fs::create_dir_all(folder(&dir.0)).unwrap();
        std::fs::write(path(&dir.0, &spec), b"model!").unwrap();
        std::fs::write(part_path(&dir.0, &spec), b"mo").unwrap();
        assert_eq!(remove(&dir.0).unwrap(), 8);
        assert!(!is_ready(&dir.0, &spec));
        assert!(!folder(&dir.0).exists());
        // removing again is fine
        assert_eq!(remove(&dir.0).unwrap(), 0);
    }

    #[test]
    fn only_hugging_face_hosts_over_https_are_followed() {
        for ok in [
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/abc/x.bin",
            "https://us.aws.cdn.hf.co/xet-bridge-us/1/2?Expires=1",
            "https://cas-bridge.xethub.hf.co/x",
            "https://cdn-lfs.huggingface.co/x",
            "https://hf.co/x",
        ] {
            assert!(allowed_url(&Url::parse(ok).unwrap()), "{ok}");
        }
        for bad in [
            "http://huggingface.co/x",
            "https://huggingface.co.evil.com/x",
            "https://evilhuggingface.co/x",
            "https://evilhf.co/x",
            "https://user:pw@huggingface.co/x",
            "https://huggingface.co:8443/x",
            "https://example.com/x",
            "ftp://huggingface.co/x",
        ] {
            assert!(!allowed_url(&Url::parse(bad).unwrap()), "{bad}");
        }
    }
}
