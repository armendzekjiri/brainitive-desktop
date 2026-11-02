//! The log file and the panic hook (BRAI-368). Everything stays on this computer: a rotating
//! `brainitive.log` in the app's log directory (5 files × 2 MB), every line passed through
//! [`crate::redact`] before it is written. There is no network code here and no telemetry.
//!
//! Our own `log` implementation instead of a plugin: a few dozen lines, no new dependency, and
//! redaction sits exactly where the text is written.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::redact;

pub const FILE_NAME: &str = "brainitive.log";
/// The current file plus four rotated ones (`brainitive.log.1` … `.4`).
pub const FILE_COUNT: usize = 5;
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LINE_CHARS: usize = 2000;
const BACKTRACE_LINES: usize = 40;

/// A size-limited log file that rotates like `logrotate`: `log` → `log.1` → … → deleted.
pub struct RotatingFile {
    dir: PathBuf,
    file: File,
    size: u64,
    max_bytes: u64,
    files: usize,
}

impl RotatingFile {
    pub fn open(dir: &Path, max_bytes: u64, files: usize) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let (file, size) = open_append(&dir.join(FILE_NAME))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            size,
            max_bytes,
            files: files.max(1),
        })
    }

    pub fn write_line(&mut self, line: &str) -> io::Result<()> {
        let bytes = line.len() as u64 + 1;
        if self.size > 0 && self.size + bytes > self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.size += bytes;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let path = |n: usize| {
            if n == 0 {
                self.dir.join(FILE_NAME)
            } else {
                self.dir.join(format!("{FILE_NAME}.{n}"))
            }
        };
        let _ = fs::remove_file(path(self.files - 1));
        for n in (0..self.files - 1).rev() {
            if path(n).exists() {
                fs::rename(path(n), path(n + 1))?;
            }
        }
        let (file, size) = open_append(&path(0))?;
        self.file = file;
        self.size = size;
        Ok(())
    }
}

fn open_append(path: &Path) -> io::Result<(File, u64)> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(path)?;
    let size = file.metadata()?.len();
    Ok((file, size))
}

/// The last `n` lines over the current file and the rotated ones, oldest first.
pub fn tail_lines(dir: &Path, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for index in 0..FILE_COUNT {
        let path = if index == 0 {
            dir.join(FILE_NAME)
        } else {
            dir.join(format!("{FILE_NAME}.{index}"))
        };
        let Ok(bytes) = fs::read(&path) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let mut older: Vec<String> = text.lines().map(str::to_string).collect();
        older.append(&mut lines);
        lines = older;
        if lines.len() >= n {
            break;
        }
    }
    let skip = lines.len().saturating_sub(n);
    lines.split_off(skip)
}

/// `2026-11-02T00:51:24.809Z`
pub fn utc_timestamp(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// `<time> <LEVEL> <target> <message>`: redacted, one line, bounded.
pub fn format_line(time: SystemTime, level: Level, target: &str, message: &str) -> String {
    format!(
        "{} {:<5} {} {}",
        utc_timestamp(time),
        level,
        target,
        redact::log_line(message, MAX_LINE_CHARS)
    )
}

/// The message of a panic: thread, location and payload, no backtrace.
pub fn panic_message(thread: &str, location: Option<(&str, u32)>, payload: &str) -> String {
    match location {
        Some((file, line)) => format!("thread '{thread}' panicked at {file}:{line}: {payload}"),
        None => format!("thread '{thread}' panicked: {payload}"),
    }
}

struct Logger {
    sink: Mutex<RotatingFile>,
    app_level: LevelFilter,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

impl Logger {
    fn write(&self, level: Level, target: &str, message: &str, blocking: bool) {
        let line = format_line(SystemTime::now(), level, target, message);
        if cfg!(debug_assertions) {
            eprintln!("{line}");
        }
        // the panic hook must never wait on a lock the panicking thread may hold
        let guard = if blocking {
            Some(self.sink.lock().unwrap_or_else(|e| e.into_inner()))
        } else {
            self.sink.try_lock().ok()
        };
        if let Some(mut sink) = guard {
            let _ = sink.write_line(&line);
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let ours = metadata.target().starts_with("brainitive");
        metadata.level() <= if ours { self.app_level } else { LevelFilter::Warn }
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            self.write(record.level(), record.target(), &record.args().to_string(), true);
        }
    }

    fn flush(&self) {
        if let Ok(mut sink) = self.sink.lock() {
            let _ = sink.file.flush();
        }
    }
}

/// Opens the log file in `dir`, installs the `log` backend and the panic hook. Safe to call
/// once; a failure leaves the app running without a file (stderr in debug builds).
pub fn init(dir: &Path) -> io::Result<()> {
    let sink = RotatingFile::open(dir, MAX_FILE_BYTES, FILE_COUNT)?;
    let logger = LOGGER.get_or_init(|| Logger {
        sink: Mutex::new(sink),
        app_level: if cfg!(debug_assertions) {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        },
    });
    let _ = LOG_DIR.set(dir.to_path_buf());
    if log::set_logger(logger).is_ok() {
        log::set_max_level(LevelFilter::Debug);
    }
    install_panic_hook();
    Ok(())
}

/// The directory of the log files, once [`init`] ran.
pub fn log_dir() -> Option<&'static Path> {
    LOG_DIR.get().map(PathBuf::as_path)
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_string());
        let thread = std::thread::current();
        let location = info.location().map(|l| (l.file(), l.line()));
        let message = panic_message(thread.name().unwrap_or("unnamed"), location, &payload);
        if let Some(logger) = LOGGER.get() {
            logger.write(Level::Error, "panic", &message, false);
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            for frame in backtrace.lines().take(BACKTRACE_LINES) {
                logger.write(Level::Error, "panic", &format!("  {}", frame.trim_end()), false);
            }
        }
        // the app is `panic = "abort"` in release: this is the last chance to write
        previous(info);
    }));
}

/// At most `max` events per `window`: a page that loops on an error can't fill the log.
pub struct RateLimit {
    window: Duration,
    max: u32,
    started: Option<Instant>,
    count: u32,
}

impl RateLimit {
    pub const fn new(window: Duration, max: u32) -> Self {
        Self {
            window,
            max,
            started: None,
            count: 0,
        }
    }

    pub fn allow(&mut self, now: Instant) -> bool {
        match self.started {
            Some(t) if now.duration_since(t) < self.window => {}
            _ => {
                self.started = Some(now);
                self.count = 0;
            }
        }
        self.count += 1;
        self.count <= self.max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brainitive-log-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn timestamps_are_utc_iso() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + Duration::from_millis(1_782_953_484_809)),
            "2026-07-02T00:51:24.809Z"
        );
        // leap day
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + Duration::from_secs(1_709_164_800)),
            "2024-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn lines_are_redacted_and_flat() {
        let line = format_line(
            UNIX_EPOCH,
            Level::Warn,
            "brainitive_desktop_lib::flows",
            "failed https://x.example/cb?code=abc and token=bra_0123456789abcdefghij\nsecond line",
        );
        assert!(line.starts_with("1970-01-01T00:00:00.000Z WARN  brainitive_desktop_lib::flows failed "));
        assert!(!line.contains("abc") && !line.contains("bra_0123") && !line.contains('\n'));
        assert!(line.contains("?[redacted]") && line.contains("\\nsecond line"));
    }

    #[test]
    fn panic_text_has_thread_location_and_payload() {
        assert_eq!(
            panic_message("main", Some(("src/lib.rs", 7)), "boom"),
            "thread 'main' panicked at src/lib.rs:7: boom"
        );
        assert_eq!(panic_message("t", None, "x"), "thread 't' panicked: x");
        let line = format_line(
            UNIX_EPOCH,
            Level::Error,
            "panic",
            &panic_message("main", Some(("/Users/alice/app/src/lib.rs", 7)), "token=SuperSecret99"),
        );
        assert!(!line.contains("alice") && !line.contains("SuperSecret99"), "{line}");
    }

    #[test]
    fn rotates_at_the_size_limit_and_keeps_five_files() {
        let dir = temp_dir("rotate");
        let mut file = RotatingFile::open(&dir, 100, FILE_COUNT).unwrap();
        for i in 0..60 {
            file.write_line(&format!("line {i:03} {}", "x".repeat(20))).unwrap();
        }
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "brainitive.log",
                "brainitive.log.1",
                "brainitive.log.2",
                "brainitive.log.3",
                "brainitive.log.4"
            ]
        );
        for name in &names {
            assert!(fs::metadata(dir.join(name)).unwrap().len() <= 100, "{name}");
        }
        // the newest line is in the current file, older ones are in higher numbers
        assert!(fs::read_to_string(dir.join(FILE_NAME)).unwrap().contains("line 059"));
        let tail = tail_lines(&dir, 3);
        assert_eq!(tail.len(), 3);
        assert!(tail[2].contains("line 059") && tail[0].contains("line 057"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopening_appends_and_tail_spans_files() {
        let dir = temp_dir("tail");
        {
            let mut file = RotatingFile::open(&dir, 60, 3).unwrap();
            for i in 0..4 {
                file.write_line(&format!("old {i}")).unwrap();
            }
        }
        let mut file = RotatingFile::open(&dir, 1 << 20, FILE_COUNT).unwrap();
        file.write_line("new").unwrap();
        let tail = tail_lines(&dir, 50);
        assert_eq!(tail.last().map(String::as_str), Some("new"));
        assert!(tail.len() >= 2);
        assert_eq!(tail_lines(&dir, 1), ["new"]);
        assert!(tail_lines(&dir.join("missing"), 5).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_log_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("mode");
        let _file = RotatingFile::open(&dir, 100, 2).unwrap();
        let mode = fs::metadata(dir.join(FILE_NAME)).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rate_limit_allows_a_burst_then_waits_for_the_window() {
        let mut limit = RateLimit::new(Duration::from_secs(60), 3);
        let t0 = Instant::now();
        assert!((0..3).all(|_| limit.allow(t0)));
        assert!(!limit.allow(t0 + Duration::from_secs(10)));
        assert!(limit.allow(t0 + Duration::from_secs(61)));
    }
}
