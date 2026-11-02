//! The recorder's state machine (pure, so it is tested): what a key-down, key-up, Enter, Esc, the
//! clock and the transcription result do. The glue in `voice/mod.rs` runs the [`Effect`]s
//! (microphone, whisper, windows).
//!
//! ```text
//! idle ──start──▶ recording ──release (held) / press again / Enter / 2 min──▶ transcribing ──▶ done
//!                     │  └─release after a short tap: stays recording (toggle mode)       └──▶ error
//!                     └──Esc──▶ cancelled                     transcribing ──Esc──▶ cancelled
//! ```

use std::time::{Duration, Instant};

/// The longest recording; it is cut off (and transcribed) there.
pub const MAX_LENGTH: Duration = Duration::from_secs(120);
/// A key held at least this long before it is released = hold to talk (release stops). A
/// shorter tap latches toggle mode: tap again, Enter or Stop to finish.
pub const HOLD_MIN: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Recording,
    Transcribing,
    Done,
    Cancelled,
    Error,
}

/// How the recording ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Started by the held hotkey: its release stops (unless it was only tapped).
    Hold,
    /// Tapped hotkey, tray, menu or the quick-capture button: press again / Enter / Stop.
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The hotkey went down (`hold`) or "Voice Capture" was chosen in a menu / button.
    Start {
        at: Instant,
        hold: bool,
    },
    /// The hotkey went down again while recording.
    HotkeyDown,
    HotkeyUp {
        at: Instant,
    },
    /// Enter / the Stop button.
    Stop,
    /// Esc / the Cancel button.
    Cancel,
    /// The clock (the indicator's timer): cuts off at [`MAX_LENGTH`].
    Tick {
        at: Instant,
    },
    /// The transcription of recording `session` finished.
    Transcribed {
        session: u64,
        text: String,
    },
    /// The microphone or the transcription of recording `session` failed.
    Failed {
        session: u64,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Open the microphone for recording `session` (and load the model meanwhile).
    StartRecording { session: u64 },
    /// Close the microphone and transcribe what it heard (`cut_off`: the 2-minute limit).
    Transcribe { session: u64, cut_off: bool },
    /// Close the microphone and forget the audio (and any transcription still running).
    Discard { session: u64 },
    /// Show the text in quick capture.
    Deliver(String),
    /// Show the problem in the indicator (a microphone still open is closed, its audio dropped).
    ShowError(String),
}

#[derive(Debug, Clone)]
pub struct Recorder {
    phase: Phase,
    mode: Mode,
    /// Increased on every start: results of an older recording are ignored.
    session: u64,
    started: Option<Instant>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            mode: Mode::Toggle,
            session: 0,
            started: None,
        }
    }
}

impl Recorder {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn session(&self) -> u64 {
        self.session
    }

    /// Recording or transcribing: a new start is not possible now.
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Recording | Phase::Transcribing)
    }

    /// How long the current recording has been running.
    pub fn elapsed(&self, now: Instant) -> Duration {
        match (self.phase, self.started) {
            (Phase::Recording, Some(t)) => now.saturating_duration_since(t).min(MAX_LENGTH),
            _ => Duration::ZERO,
        }
    }

    /// Feeds one input; returns what the glue has to do (if anything).
    pub fn handle(&mut self, input: Input) -> Option<Effect> {
        match (self.phase, input) {
            (Phase::Recording | Phase::Transcribing, Input::Start { .. }) => None,
            (_, Input::Start { at, hold }) => {
                self.session += 1;
                self.phase = Phase::Recording;
                self.mode = if hold { Mode::Hold } else { Mode::Toggle };
                self.started = Some(at);
                Some(Effect::StartRecording { session: self.session })
            }

            (Phase::Recording, Input::HotkeyUp { at }) if self.mode == Mode::Hold => {
                let held = self
                    .started
                    .map(|t| at.saturating_duration_since(t))
                    .unwrap_or_default();
                if held >= HOLD_MIN {
                    Some(self.transcribe(false))
                } else {
                    // a tap: keep recording until the next press / Enter
                    self.mode = Mode::Toggle;
                    None
                }
            }
            (Phase::Recording, Input::HotkeyDown | Input::Stop) => Some(self.transcribe(false)),
            (Phase::Recording, Input::Tick { at }) => {
                let started = self.started?;
                (at.saturating_duration_since(started) >= MAX_LENGTH).then(|| self.transcribe(true))
            }

            (Phase::Recording | Phase::Transcribing, Input::Cancel) => {
                self.phase = Phase::Cancelled;
                self.started = None;
                Some(Effect::Discard { session: self.session })
            }

            (Phase::Transcribing, Input::Transcribed { session, text }) if session == self.session => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    self.phase = Phase::Error;
                    Some(Effect::ShowError(NOTHING_HEARD.into()))
                } else {
                    self.phase = Phase::Done;
                    Some(Effect::Deliver(text))
                }
            }
            (Phase::Recording | Phase::Transcribing, Input::Failed { session, message }) if session == self.session => {
                self.phase = Phase::Error;
                self.started = None;
                Some(Effect::ShowError(message))
            }

            // everything else (a key-up after a tap, a late result of a cancelled recording,
            // Esc while idle, …) changes nothing
            _ => None,
        }
    }

    fn transcribe(&mut self, cut_off: bool) -> Effect {
        self.phase = Phase::Transcribing;
        self.started = None;
        Effect::Transcribe {
            session: self.session,
            cut_off,
        }
    }
}

/// Shown when the recording held no speech (or whisper returned nothing).
pub const NOTHING_HEARD: &str = "Nothing was heard. Check the microphone and speak a bit longer.";

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    fn start(r: &mut Recorder, at: Instant, hold: bool) -> u64 {
        match r.handle(Input::Start { at, hold }) {
            Some(Effect::StartRecording { session }) => session,
            other => panic!("expected a start, got {other:?}"),
        }
    }

    #[test]
    fn hold_to_talk_release_transcribes_and_the_text_is_delivered() {
        let t = t0();
        let mut r = Recorder::default();
        assert_eq!(r.phase(), Phase::Idle);
        let s = start(&mut r, t, true);
        assert_eq!((r.phase(), r.mode()), (Phase::Recording, Mode::Hold));
        assert!(r.busy());
        assert_eq!(r.elapsed(t + Duration::from_secs(3)), Duration::from_secs(3));
        assert_eq!(
            r.handle(Input::HotkeyUp {
                at: t + Duration::from_secs(3)
            }),
            Some(Effect::Transcribe {
                session: s,
                cut_off: false
            })
        );
        assert_eq!(r.phase(), Phase::Transcribing);
        assert_eq!(r.elapsed(t + Duration::from_secs(4)), Duration::ZERO);
        // pressing again while transcribing does nothing
        assert_eq!(r.handle(Input::Start { at: t, hold: true }), None);
        assert_eq!(r.handle(Input::HotkeyDown), None);
        assert_eq!(
            r.handle(Input::Transcribed {
                session: s,
                text: "  Call Anna tomorrow \n".into()
            }),
            Some(Effect::Deliver("Call Anna tomorrow".into()))
        );
        assert_eq!(r.phase(), Phase::Done);
        assert!(!r.busy());
    }

    #[test]
    fn a_short_tap_latches_toggle_mode() {
        let t = t0();
        let mut r = Recorder::default();
        let s = start(&mut r, t, true);
        assert_eq!(
            r.handle(Input::HotkeyUp {
                at: t + Duration::from_millis(200)
            }),
            None
        );
        assert_eq!((r.phase(), r.mode()), (Phase::Recording, Mode::Toggle));
        // a later key-up (of the second press) changes nothing; the press itself stops
        assert_eq!(
            r.handle(Input::HotkeyDown),
            Some(Effect::Transcribe {
                session: s,
                cut_off: false
            })
        );
        assert_eq!(
            r.handle(Input::HotkeyUp {
                at: t + Duration::from_secs(5)
            }),
            None
        );
        assert_eq!(r.phase(), Phase::Transcribing);
    }

    #[test]
    fn toggle_mode_from_a_menu_stops_with_enter_or_another_press() {
        let t = t0();
        let mut r = Recorder::default();
        let s = start(&mut r, t, false);
        assert_eq!(r.mode(), Mode::Toggle);
        // a key-up never stops a toggle recording
        assert_eq!(
            r.handle(Input::HotkeyUp {
                at: t + Duration::from_secs(9)
            }),
            None
        );
        assert_eq!(
            r.handle(Input::Stop),
            Some(Effect::Transcribe {
                session: s,
                cut_off: false
            })
        );

        let mut r = Recorder::default();
        let s = start(&mut r, t, false);
        assert_eq!(
            r.handle(Input::HotkeyDown),
            Some(Effect::Transcribe {
                session: s,
                cut_off: false
            })
        );
    }

    #[test]
    fn esc_cancels_while_recording_and_while_transcribing() {
        let t = t0();
        let mut r = Recorder::default();
        let s = start(&mut r, t, true);
        assert_eq!(r.handle(Input::Cancel), Some(Effect::Discard { session: s }));
        assert_eq!(r.phase(), Phase::Cancelled);
        // the release of the held key after Esc does nothing
        assert_eq!(
            r.handle(Input::HotkeyUp {
                at: t + Duration::from_secs(2)
            }),
            None
        );
        assert_eq!(r.phase(), Phase::Cancelled);

        let s = start(&mut r, t, false);
        r.handle(Input::Stop);
        assert_eq!(r.handle(Input::Cancel), Some(Effect::Discard { session: s }));
        // the transcription that was still running arrives late: ignored
        assert_eq!(
            r.handle(Input::Transcribed {
                session: s,
                text: "late".into()
            }),
            None
        );
        assert_eq!(r.phase(), Phase::Cancelled);
        // Esc when nothing runs
        assert_eq!(Recorder::default().handle(Input::Cancel), None);
    }

    #[test]
    fn the_recording_is_cut_off_at_two_minutes() {
        let t = t0();
        let mut r = Recorder::default();
        let s = start(&mut r, t, true);
        assert_eq!(
            r.handle(Input::Tick {
                at: t + Duration::from_secs(60)
            }),
            None
        );
        assert_eq!(
            r.handle(Input::Tick {
                at: t + MAX_LENGTH - Duration::from_millis(1)
            }),
            None
        );
        assert_eq!(r.elapsed(t + Duration::from_secs(500)), MAX_LENGTH);
        assert_eq!(
            r.handle(Input::Tick { at: t + MAX_LENGTH }),
            Some(Effect::Transcribe {
                session: s,
                cut_off: true
            })
        );
        // the key released afterwards changes nothing
        assert_eq!(
            r.handle(Input::HotkeyUp {
                at: t + MAX_LENGTH + Duration::from_secs(1)
            }),
            None
        );
        assert_eq!(r.phase(), Phase::Transcribing);
        // ticks after the cut-off do nothing
        assert_eq!(r.handle(Input::Tick { at: t + MAX_LENGTH * 2 }), None);
    }

    #[test]
    fn errors_and_empty_results() {
        let t = t0();
        let mut r = Recorder::default();
        let s = start(&mut r, t, true);
        // the microphone failed while recording: close it and show why
        assert_eq!(
            r.handle(Input::Failed {
                session: s,
                message: "no microphone".into()
            }),
            Some(Effect::ShowError("no microphone".into()))
        );
        assert_eq!(r.phase(), Phase::Error);
        assert!(!r.busy());

        let s = start(&mut r, t, true);
        r.handle(Input::Stop);
        assert_eq!(
            r.handle(Input::Transcribed {
                session: s,
                text: " \n ".into()
            }),
            Some(Effect::ShowError(NOTHING_HEARD.into()))
        );
        assert_eq!(r.phase(), Phase::Error);

        let s = start(&mut r, t, true);
        r.handle(Input::Stop);
        // a failure of an older session is ignored
        assert_eq!(
            r.handle(Input::Failed {
                session: s - 1,
                message: "old".into()
            }),
            None
        );
        assert_eq!(
            r.handle(Input::Failed {
                session: s,
                message: "model broken".into()
            }),
            Some(Effect::ShowError("model broken".into()))
        );
    }

    #[test]
    fn every_start_is_a_new_session() {
        let t = t0();
        let mut r = Recorder::default();
        let a = start(&mut r, t, true);
        r.handle(Input::Cancel);
        let b = start(&mut r, t, true);
        assert!(b > a);
        assert_eq!(r.session(), b);
        // a result for the cancelled one is not delivered into the new recording
        r.handle(Input::Stop);
        assert_eq!(
            r.handle(Input::Transcribed {
                session: a,
                text: "old".into()
            }),
            None
        );
        assert_eq!(r.phase(), Phase::Transcribing);
    }
}
