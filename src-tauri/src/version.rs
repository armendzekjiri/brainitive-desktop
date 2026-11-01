//! Version checks: the app against `min_versions.desktop` of `GET /api/v1/instance`.

use semver::Version;

/// The version of this build (`Cargo.toml` → `tauri.conf.json`).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

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
}
