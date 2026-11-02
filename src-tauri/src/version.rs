//! Version checks: the app against `min_versions.desktop` of `GET /api/v1/instance`.

use std::time::{Duration, Instant};

use semver::Version;

/// The version of this build (`Cargo.toml` → `tauri.conf.json`).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where "Please update Brainitive Desktop" sends the user (opened from Rust, never from a page).
pub const DOWNLOAD_URL: &str = "https://www.brainitive.app/download";

/// Reads `1.2.3`, `v1.2.3`, `1.2` and `1` (missing parts are 0). Build metadata is ignored.
pub fn parse_lenient(raw: &str) -> Option<Version> {
    let raw = raw.trim().trim_start_matches(['v', 'V']);
    let core_end = raw.find(['-', '+']).unwrap_or(raw.len());
    let (core, rest) = raw.split_at(core_end);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let mut padded = parts.join(".");
    for _ in parts.len()..3 {
        padded.push_str(".0");
    }
    let pre = rest.split('+').next().unwrap_or("");
    Version::parse(&format!("{padded}{pre}")).ok()
}

/// How often the poller asks the server for `min_versions.desktop` again while the app runs.
pub const RECHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// After a failed re-check (the instance answer didn't come): try again after this.
pub const RECHECK_RETRY: Duration = Duration::from_secs(15 * 60);

/// When the minimum version is asked again (pure scheduling; the call is in `flows.rs`, the
/// loop in `tray.rs`): about every 6 hours, at once after the computer woke from sleep, and
/// 15 minutes after a failed try. A new server counts as just checked (connecting did it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Recheck {
    next: Option<Instant>,
}

impl Recheck {
    /// Whether to ask now. The first question for a server only starts the 6 hours.
    pub fn due(&mut self, now: Instant) -> bool {
        match self.next {
            None => {
                self.next = Some(now + RECHECK_EVERY);
                false
            }
            Some(at) => now >= at,
        }
    }

    pub fn checked(&mut self, now: Instant) {
        self.next = Some(now + RECHECK_EVERY);
    }

    pub fn failed(&mut self, now: Instant) {
        self.next = Some(now + RECHECK_RETRY);
    }

    /// The computer woke from sleep: the next chance asks.
    pub fn woke(&mut self, now: Instant) {
        self.next = Some(now);
    }

    /// Another server (or none): start over.
    pub fn reset(&mut self) {
        self.next = None;
    }
}

/// Whether `app` is at least `min`. An unreadable minimum blocks nobody (the server's mistake
/// must not lock users out); an unreadable app version never happens (it is ours).
pub fn meets_minimum(app: &str, min: &str) -> bool {
    match (parse_lenient(app), parse_lenient(min)) {
        (Some(app), Some(min)) => app >= min,
        (_, None) => true,
        (None, Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lenient_parsing() {
        assert_eq!(parse_lenient("1.2.3"), Some(Version::new(1, 2, 3)));
        assert_eq!(parse_lenient("v1.2"), Some(Version::new(1, 2, 0)));
        assert_eq!(parse_lenient(" 2 "), Some(Version::new(2, 0, 0)));
        assert_eq!(parse_lenient("1.2.3+build.5"), Some(Version::new(1, 2, 3)));
        assert!(parse_lenient("1.2.3-beta.1").unwrap() < Version::new(1, 2, 3));
        assert_eq!(parse_lenient(""), None);
        assert_eq!(parse_lenient("1..2"), None);
        assert_eq!(parse_lenient("1.2.3.4"), None);
        assert_eq!(parse_lenient("latest"), None);
    }

    #[test]
    fn prerelease_tags_follow_semver_precedence() {
        let order = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for pair in order.windows(2) {
            let (low, high) = (parse_lenient(pair[0]).unwrap(), parse_lenient(pair[1]).unwrap());
            assert!(low < high, "{} < {}", pair[0], pair[1]);
            assert!(
                !meets_minimum(pair[0], pair[1]),
                "{} must not meet {}",
                pair[0],
                pair[1]
            );
            assert!(meets_minimum(pair[1], pair[0]), "{} meets {}", pair[1], pair[0]);
        }
        // a short core with a tag, a v prefix and build metadata
        assert_eq!(parse_lenient("1.2-beta.1"), Version::parse("1.2.0-beta.1").ok());
        assert_eq!(parse_lenient("v2.0.0-rc.1+build.7"), Version::parse("2.0.0-rc.1").ok());
        // a pre-release of the next version is still above the previous release
        assert!(meets_minimum("0.2.0-alpha.1", "0.1.9"));
        assert!(!meets_minimum("0.2.0-alpha.1", "0.2.0"));
        // build metadata never matters
        assert!(meets_minimum("1.0.0+a", "1.0.0+b"));
        // an unreadable tag is an unreadable minimum
        assert!(meets_minimum("0.1.0", "1.0.0-"));
        assert!(meets_minimum("0.1.0", "1.0.0-be ta"));
    }

    #[test]
    fn minimum_versions() {
        assert!(meets_minimum("0.1.0", "0.1.0"));
        assert!(meets_minimum("0.2.0", "0.1.9"));
        assert!(meets_minimum("1.0.0", "0.9"));
        assert!(!meets_minimum("0.1.0", "0.2.0"));
        assert!(!meets_minimum("0.9.9", "1"));
        assert!(!meets_minimum("1.0.0-beta.1", "1.0.0"));
        // numeric, not lexicographic
        assert!(meets_minimum("0.10.0", "0.9.0"));
        // a broken minimum does not lock anyone out
        assert!(meets_minimum("0.1.0", "soon"));
        assert!(meets_minimum(APP_VERSION, "0.1.0"));
    }

    #[test]
    fn the_minimum_is_asked_again_about_every_six_hours() {
        let t0 = Instant::now();
        let hours = |h: u64| t0 + Duration::from_secs(h * 3600);
        let mut r = Recheck::default();
        // the first question only starts the clock (connecting just checked)
        assert!(!r.due(t0));
        assert!(!r.due(hours(5)));
        assert!(r.due(hours(6)));
        assert!(r.due(hours(7)), "still due until it was done");
        r.checked(hours(7));
        assert!(!r.due(hours(12)));
        assert!(r.due(hours(13)));
    }

    #[test]
    fn a_wake_asks_at_once_and_a_failure_retries_soon() {
        let t0 = Instant::now();
        let mut r = Recheck::default();
        assert!(!r.due(t0));
        r.woke(t0 + Duration::from_secs(60));
        assert!(r.due(t0 + Duration::from_secs(61)));
        r.failed(t0 + Duration::from_secs(61));
        assert!(!r.due(t0 + Duration::from_secs(61 + 14 * 60)));
        assert!(r.due(t0 + Duration::from_secs(61 + 15 * 60)));
        // another server: the clock starts over
        r.reset();
        assert!(!r.due(t0 + Duration::from_secs(10_000)));
        assert!(!r.due(t0 + Duration::from_secs(10_000 + 5 * 3600)));
        assert!(r.due(t0 + Duration::from_secs(10_000 + 6 * 3600)));
    }
}
