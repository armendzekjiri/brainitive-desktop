//! Speech to text on this computer (BRAI-439): whisper.cpp through `whisper-rs`, Metal on macOS
//! (the `metal` feature; whisper.cpp falls back to the CPU by itself when there is no GPU), the
//! CPU elsewhere. Everything here blocks: it runs on background threads, never the main thread.
//!
//! The model is loaded while the user is still speaking ([`load`] on its own thread), so only
//! the transcription itself is waited for. It is dropped afterwards: nothing stays in memory
//! between recordings.

use std::path::Path;
use std::time::{Duration, Instant};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::audio::{self, Clip, WHISPER_RATE};

/// A loaded model.
pub struct Model {
    ctx: WhisperContext,
    pub load_time: Duration,
}

/// What a transcription took (logged; no audio, no text).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub audio: Duration,
    pub resample: Duration,
    pub transcribe: Duration,
}

/// Shorter than this is not speech (a tap of the key): not transcribed.
pub const MIN_SECONDS: f32 = 0.3;
/// whisper does better with at least a second; shorter clips get silence appended.
const PAD_TO_SECONDS: f32 = 1.2;

/// Routes whisper.cpp's and ggml's own output to the `log` crate (the app logs other crates at
/// warn and above only), instead of stderr. Once per process.
pub fn init_logging() {
    whisper_rs::install_logging_hooks();
}

/// Loads the model file (≈ 0.15 s warm; the very first load on a Mac also compiles the Metal
/// kernels, a few seconds once). Blocking.
pub fn load(path: &Path) -> Result<Model, String> {
    // use_gpu is on when built with a GPU feature (Metal on macOS), off elsewhere
    load_with(path, WhisperContextParameters::default())
}

fn load_with(path: &Path, params: WhisperContextParameters) -> Result<Model, String> {
    let started = Instant::now();
    let path = path.to_str().ok_or("the model path is not valid UTF-8")?;
    let ctx = WhisperContext::new_with_params(path, params)
        .map_err(|e| format!("the voice model could not be loaded ({e})"))?;
    Ok(Model {
        ctx,
        load_time: started.elapsed(),
    })
}

/// `language`: "auto" or a code of `voice::LANGUAGES`. Blocking. `Ok("")` = nothing recognised.
pub fn transcribe(model: &Model, clip: &Clip, language: &str) -> Result<(String, Timing), String> {
    let audio_len = Duration::from_secs_f32(clip.seconds());
    if clip.seconds() < MIN_SECONDS || audio::is_silent(&clip.samples) {
        return Ok((
            String::new(),
            Timing {
                audio: audio_len,
                resample: Duration::ZERO,
                transcribe: Duration::ZERO,
            },
        ));
    }
    let started = Instant::now();
    let mut samples = audio::resample_16k(&clip.samples, clip.rate);
    let min = (PAD_TO_SECONDS * WHISPER_RATE as f32) as usize;
    if samples.len() < min {
        samples.resize(min, 0.0);
    }
    let resample = started.elapsed();

    let started = Instant::now();
    let mut state = model.ctx.create_state().map_err(|e| format!("whisper state ({e})"))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(language));
    params.set_translate(false);
    params.set_n_threads(threads());
    params.set_no_context(true);
    params.set_no_timestamps(true);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    let result = state.full(params, &samples);
    // the resampled copy is wiped too
    samples.iter_mut().for_each(|s| *s = 0.0);
    result.map_err(|e| format!("the transcription failed ({e})"))?;
    let segments: Vec<String> = state
        .as_iter()
        .filter_map(|segment| segment.to_str_lossy().ok().map(|t| t.into_owned()))
        .collect();
    Ok((
        clean(segments.iter().map(String::as_str)),
        Timing {
            audio: audio_len,
            resample,
            transcribe: started.elapsed(),
        },
    ))
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 8) as i32)
        .unwrap_or(4)
}

/// Joins whisper's segments: trimmed, single spaces, without the segments that are only a
/// non-speech marker ("[BLANK_AUDIO]", "(music)", "[Musik]", "*laughs*").
pub fn clean<'a>(segments: impl IntoIterator<Item = &'a str>) -> String {
    let mut words: Vec<&str> = Vec::new();
    for segment in segments {
        let t = segment.trim();
        let marker = (t.starts_with('[') && t.ends_with(']'))
            || (t.starts_with('(') && t.ends_with(')'))
            || (t.len() > 1 && t.starts_with('*') && t.ends_with('*'));
        if !marker {
            words.extend(t.split_whitespace());
        }
    }
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_are_joined_and_noise_markers_dropped() {
        assert_eq!(
            clean([" Call Anna tomorrow.\n", " And buy milk. "]),
            "Call Anna tomorrow. And buy milk."
        );
        assert_eq!(clean(["[BLANK_AUDIO]"]), "");
        assert_eq!(clean([" (music)", " Hello", " [Musik]", " *laughs* "]), "Hello");
        // brackets inside a sentence are speech
        assert_eq!(
            clean([" Ruf Anna an (morgen früh) bitte"]),
            "Ruf Anna an (morgen früh) bitte"
        );
        assert_eq!(clean(["unclosed [bracket"]), "unclosed [bracket");
        assert_eq!(clean([] as [&str; 0]), "");
    }

    #[test]
    fn too_short_or_silent_clips_are_not_transcribed() {
        // no model needed: these return before whisper runs
        let silent = Clip {
            samples: vec![0.0; 48_000 * 3],
            rate: 48_000,
        };
        assert!(silent.seconds() > MIN_SECONDS && audio::is_silent(&silent.samples));
        let tap = Clip {
            samples: vec![0.3; 1_000],
            rate: 48_000,
        };
        assert!(tap.seconds() < MIN_SECONDS);
    }

    /// A real transcription with the real model (downloaded once into the scratch folder named
    /// by `BRAINITIVE_VOICE_MODEL_DIR`) of a clip spoken by macOS `say`. Run by hand:
    /// `BRAINITIVE_VOICE_MODEL_DIR=/tmp/voice cargo test --lib real_transcription -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_transcription() {
        let dir =
            std::path::PathBuf::from(std::env::var("BRAINITIVE_VOICE_MODEL_DIR").expect("BRAINITIVE_VOICE_MODEL_DIR"));
        let spec = super::super::model::MODEL;
        if !super::super::model::is_ready(&dir, &spec) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let started = Instant::now();
            rt.block_on(super::super::model::download(&spec, &dir, &cancel, |_, _| {}))
                .expect("download");
            println!("download: {:?}", started.elapsed());
        }
        init_logging();
        let model = load(&super::super::model::path(&dir, &spec)).expect("load");
        println!("load: {:?}", model.load_time);
        let cpu = load_with(
            &super::super::model::path(&dir, &spec),
            WhisperContextParameters {
                use_gpu: false,
                ..Default::default()
            },
        )
        .expect("load cpu");
        for (lang, voice, sentence) in [
            ("en", "Samantha", "Remind me to call Anna tomorrow morning about the quarterly report and the new office plans."),
            ("de", "Anna", "Ich muss morgen früh Anna anrufen und den Bericht für das neue Büro fertig schreiben."),
            // about 15 seconds: the card's target is under 2 s for a 15 s clip on an M1
            ("en-15s", "Samantha", "Quick note for the team meeting on Thursday. We need to finish the voice capture feature for the desktop app, test it on Windows and Linux, update the security documentation, and ask Armend whether the new shortcut should move under the Brainitive key."),
        ] {
            let (lang, tag) = (lang.split('-').next().unwrap(), lang);
            let wav = std::env::temp_dir().join(format!("brainitive-voice-test-{lang}.wav"));
            let ok = std::process::Command::new("say")
                .args(["-v", voice, "-o"])
                .arg(&wav)
                .args(["--file-format=WAVE", "--data-format=LEI16@22050", sentence])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                println!("{lang}: `say` with voice {voice} not available, skipped");
                continue;
            }
            let clip = read_wav(&std::fs::read(&wav).unwrap());
            let _ = std::fs::remove_file(&wav);
            for language in [lang, "auto"] {
                let (text, timing) = transcribe(&model, &clip, language).expect("transcribe");
                println!(
                    "{tag} ({language}): {:.1} s audio → resample {:?}, transcribe {:?}: {text}",
                    timing.audio.as_secs_f32(),
                    timing.resample,
                    timing.transcribe
                );
                assert!(!text.is_empty());
            }
            let (_, timing) = transcribe(&cpu, &clip, lang).expect("transcribe on the cpu");
            println!("{tag} ({lang}, cpu only): transcribe {:?}", timing.transcribe);
        }
    }

    /// 16-bit PCM mono WAV → a clip (test helper; the app itself never reads or writes audio files).
    fn read_wav(bytes: &[u8]) -> Clip {
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let mut i = 12;
        while i + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
            if &bytes[i..i + 4] == b"data" {
                let data = &bytes[i + 8..(i + 8 + len).min(bytes.len())];
                let samples = data
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                    .collect();
                return Clip { samples, rate };
            }
            i += 8 + len + (len & 1);
        }
        panic!("no data chunk");
    }
}
