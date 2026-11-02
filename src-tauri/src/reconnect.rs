//! When the app tries a server again after "can't be reached" (BRAI-367). Pure: the loop that
//! sleeps and calls `flows::restore` is in `flows.rs`.
//!
//! - Backoff: 5 s, 10 s, 20 s, 40 s, then every 60 s.
//! - Name resolution / no network: besides the backoff, the loop looks up the server's name
//!   every 3 s (a local lookup, no request to anyone) and retries the moment it resolves: that is
//!   "the network is back". A wake from sleep (wall clock far ahead of the monotonic wait, see
//!   `poll::woke_from_sleep`) retries at once as well.

use std::time::Duration;

pub const FIRST_DELAY: Duration = Duration::from_secs(5);
pub const MAX_DELAY: Duration = Duration::from_secs(60);
/// How often the loop looks around (name lookup, wake detection) while it waits.
pub const STEP: Duration = Duration::from_secs(3);

/// The wait before automatic attempt number `attempt` (0 = the first retry).
pub fn delay(attempt: u32) -> Duration {
    FIRST_DELAY.saturating_mul(1u32 << attempt.min(10)).min(MAX_DELAY)
}

/// The next sleep: one [`STEP`], or what is left of the wait when that is shorter.
pub fn next_sleep(remaining: Duration) -> Duration {
    remaining.min(STEP)
}

/// `host` and port of a server origin, for the lookup.
pub fn lookup_target(origin: &url::Url) -> Option<(String, u16)> {
    let host = origin.host_str()?.trim_matches(['[', ']']).to_string();
    Some((host, origin.port_or_known_default()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_a_minute() {
        let secs: Vec<u64> = (0..8).map(|a| delay(a).as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 60, 60, 60, 60]);
        assert_eq!(delay(u32::MAX), MAX_DELAY);
    }

    #[test]
    fn sleeps_in_steps_without_overshooting() {
        assert_eq!(next_sleep(Duration::from_secs(60)), STEP);
        assert_eq!(next_sleep(Duration::from_secs(1)), Duration::from_secs(1));
        assert_eq!(next_sleep(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn lookup_target_of_an_origin() {
        let u = |s| url::Url::parse(s).unwrap();
        assert_eq!(
            lookup_target(&u("https://brain.example.com")),
            Some(("brain.example.com".into(), 443))
        );
        assert_eq!(
            lookup_target(&u("http://localhost:8000")),
            Some(("localhost".into(), 8000))
        );
        assert_eq!(lookup_target(&u("http://[::1]:8000")), Some(("::1".into(), 8000)));
    }
}
