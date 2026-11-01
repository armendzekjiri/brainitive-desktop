//! When the tray polls the server (BRAI-360): sparingly, every 60 s while signed in; errors back
//! off up to 15 min; after the computer slept, a short grace for the network first. Pure logic;
//! the loop is in `tray.rs`.
//!
//! Sleep needs no timer of its own: the poller sleeps on the monotonic clock, which stops while
//! the computer sleeps, so nothing is polled then. A wake is recognised by the wall clock having
//! moved much further than the monotonic sleep ([`woke_from_sleep`]).

use std::time::Duration;

pub const INTERVAL: Duration = Duration::from_secs(60);
pub const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// My Day changes less often than the timer / unread count: every 5th poll (or after a change).
pub const MY_DAY_EVERY: u32 = 5;
/// After a wake from sleep, Wi-Fi needs a moment before the first request.
pub const WAKE_GRACE: Duration = Duration::from_secs(10);

/// Exponential backoff after failed polls: 60 s, 2 min, 4 min, 8 min, 15 min, 15 min …
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    pub fn delay(&self) -> Duration {
        let factor = 1u32 << self.failures.min(10);
        INTERVAL.saturating_mul(factor).min(MAX_BACKOFF)
    }

    pub fn failed(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }

    pub fn succeeded(&mut self) {
        self.failures = 0;
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }
}

/// The wall clock moved at least 2 minutes more than the (monotonic) wait: the computer slept.
pub fn woke_from_sleep(waited: Duration, wall_elapsed: Duration) -> bool {
    wall_elapsed > waited + Duration::from_secs(120)
}

/// Which endpoints one poll asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub unread: bool,
    pub timer: bool,
    pub my_day: bool,
}

/// `tick` counts polls since sign-in (0 = the first one).
pub fn plan(tick: u32, my_day_stale: bool) -> Plan {
    Plan {
        unread: true,
        timer: true,
        my_day: my_day_stale || tick % MY_DAY_EVERY == 0,
    }
}

/// "1:05" (h:mm) for a running timer; minutes only below an hour ("0:07").
pub fn clock(seconds: u64) -> String {
    let minutes = seconds / 60;
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_fifteen_minutes() {
        let mut b = Backoff::default();
        assert_eq!(b.delay(), Duration::from_secs(60));
        let mut delays = Vec::new();
        for _ in 0..7 {
            b.failed();
            delays.push(b.delay().as_secs());
        }
        assert_eq!(delays, [120, 240, 480, 900, 900, 900, 900]);
        for _ in 0..100 {
            b.failed();
        }
        assert_eq!(b.delay(), MAX_BACKOFF);
        b.succeeded();
        assert_eq!(b.delay(), INTERVAL);
        assert_eq!(b.failures(), 0);
    }

    #[test]
    fn wake_detection() {
        let wait = Duration::from_secs(60);
        assert!(!woke_from_sleep(wait, Duration::from_secs(61)));
        assert!(!woke_from_sleep(wait, Duration::from_secs(170)));
        assert!(woke_from_sleep(wait, Duration::from_secs(60 * 60)));
    }

    #[test]
    fn my_day_is_asked_every_fifth_poll_or_when_stale() {
        let asked: Vec<u32> = (0..11).filter(|t| plan(*t, false).my_day).collect();
        assert_eq!(asked, [0, 5, 10]);
        assert!(plan(3, true).my_day);
        assert!(plan(3, false).unread && plan(3, false).timer);
    }

    #[test]
    fn clock_format() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(7 * 60 + 59), "0:07");
        assert_eq!(clock(65 * 60), "1:05");
        assert_eq!(clock(25 * 3600), "25:00");
    }
}
