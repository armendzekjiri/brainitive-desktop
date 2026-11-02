//! Cold-start timing (BRAI-369). `begin()` is the first thing `run()` does; `mark` logs the time
//! since then, once per phase name, so the log file shows where the first seconds go:
//! `startup: shell painted +412ms`. Only phase names and milliseconds are logged.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static BEGIN: OnceLock<Instant> = OnceLock::new();
static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

pub fn begin() {
    let _ = BEGIN.set(Instant::now());
}

/// `Some(ms)` the first time a phase is marked, `None` for repeats (every page load calls it).
pub fn mark(phase: &'static str) -> Option<u128> {
    let begin = BEGIN.get()?;
    let first = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(phase);
    if !first {
        return None;
    }
    let ms = begin.elapsed().as_millis();
    log::info!("startup: {phase} +{ms}ms");
    Some(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_phase_is_reported_once() {
        begin();
        assert!(mark("test phase").is_some());
        assert!(mark("test phase").is_none());
        assert!(mark("another test phase").is_some());
    }
}
