//! The microphone (BRAI-439): the default input device through `cpal`, mixed to mono f32 **in
//! memory only** — nothing is written to disk or sent anywhere. The stream lives on its own
//! thread (a `cpal::Stream` is not `Send` on every platform); [`Capture::finish`] stops it and
//! hands over the samples, [`Capture::discard`] stops it and wipes them.
//!
//! The pure parts (mixing, the meter level, resampling to the 16 kHz whisper needs, the silence
//! check) are tested.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample};

/// whisper's input rate.
pub const WHISPER_RATE: u32 = 16_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    #[error("No microphone was found. Connect one or choose an input device in the system settings.")]
    NoDevice,
    #[error("Brainitive is not allowed to use the microphone.")]
    Denied,
    #[error("The microphone could not be opened: {0}")]
    Other(String),
}

/// A finished recording, in memory. Wiped when dropped.
pub struct Clip {
    pub samples: Vec<f32>,
    pub rate: u32,
}

impl Clip {
    pub fn seconds(&self) -> f32 {
        if self.rate == 0 {
            0.0
        } else {
            self.samples.len() as f32 / self.rate as f32
        }
    }
}

impl Drop for Clip {
    fn drop(&mut self) {
        wipe(&mut self.samples);
    }
}

fn wipe(samples: &mut [f32]) {
    samples.iter_mut().for_each(|s| *s = 0.0);
}

struct Shared {
    samples: Mutex<Vec<f32>>,
    /// The meter level 0..1 (f32 bits) of the last block.
    level: AtomicU32,
    /// A stream error after the start (device unplugged, …).
    failure: Mutex<Option<String>>,
    max_samples: usize,
}

/// A running recording.
pub struct Capture {
    shared: Arc<Shared>,
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
    rate: u32,
}

impl Capture {
    /// Opens the default microphone and starts recording (at most `max_length`; later samples are
    /// dropped). Returns once the stream runs, or with why it could not start.
    pub fn start(max_length: Duration) -> Result<Self, AudioError> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, Arc<Shared>), AudioError>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("voice-microphone".into())
            .spawn(move || {
                let stream = match open(max_length) {
                    Ok((stream, rate, shared)) => {
                        let _ = ready_tx.send(Ok((rate, shared)));
                        stream
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                // until finish / discard (or the Capture is dropped: the sender goes away)
                let _ = stop_rx.recv();
                let _ = stream.pause();
                drop(stream);
            })
            .map_err(|e| AudioError::Other(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok((rate, shared))) => Ok(Self {
                shared,
                stop: stop_tx,
                thread: Some(thread),
                rate,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(AudioError::Other("the microphone did not start".into())),
        }
    }

    /// The meter level of the last audio block, 0..1.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    /// A stream error since the start (e.g. the device was unplugged).
    pub fn failure(&self) -> Option<String> {
        self.shared.failure.lock().ok().and_then(|f| f.clone())
    }

    fn stop(&mut self) {
        let _ = self.stop.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// Stops and hands over what was recorded.
    pub fn finish(mut self) -> Clip {
        self.stop();
        let samples = self
            .shared
            .samples
            .lock()
            .map(|mut s| std::mem::take(&mut *s))
            .unwrap_or_default();
        Clip {
            samples,
            rate: self.rate,
        }
    }

    /// Stops and wipes the audio (Esc).
    pub fn discard(self) {
        drop(self);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
        if let Ok(mut s) = self.shared.samples.lock() {
            wipe(&mut s);
            s.clear();
            s.shrink_to_fit();
        }
    }
}

type Opened = (cpal::Stream, u32, Arc<Shared>);

fn open(max_length: Duration) -> Result<Opened, AudioError> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or(AudioError::NoDevice)?;
    let config = device.default_input_config().map_err(classify)?;
    let rate = config.sample_rate();
    let channels = config.channels().max(1) as usize;
    let shared = Arc::new(Shared {
        samples: Mutex::new(Vec::with_capacity(rate as usize * 30)),
        level: AtomicU32::new(0f32.to_bits()),
        failure: Mutex::new(None),
        max_samples: rate as usize * max_length.as_secs() as usize,
    });
    let stream_config = config.config();
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, stream_config, channels, &shared),
        SampleFormat::I16 => build::<i16>(&device, stream_config, channels, &shared),
        SampleFormat::I32 => build::<i32>(&device, stream_config, channels, &shared),
        SampleFormat::U16 => build::<u16>(&device, stream_config, channels, &shared),
        SampleFormat::U8 => build::<u8>(&device, stream_config, channels, &shared),
        SampleFormat::I8 => build::<i8>(&device, stream_config, channels, &shared),
        other => return Err(AudioError::Other(format!("unsupported sample format {other}"))),
    }?;
    stream.play().map_err(classify)?;
    Ok((stream, rate, shared))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let data = Arc::clone(shared);
    let errors = Arc::clone(shared);
    let mut mono = Vec::new();
    device
        .build_input_stream::<T, _, _>(
            config,
            move |block: &[T], _| {
                mix_to_mono(block, channels, &mut mono);
                data.level.store(meter_level(&mono).to_bits(), Ordering::Relaxed);
                if let Ok(mut samples) = data.samples.lock() {
                    let room = data.max_samples.saturating_sub(samples.len());
                    samples.extend_from_slice(&mono[..mono.len().min(room)]);
                }
            },
            move |e| {
                if let Ok(mut f) = errors.failure.lock() {
                    f.get_or_insert_with(|| e.to_string());
                }
            },
            None,
        )
        .map_err(classify)
}

fn classify(e: cpal::Error) -> AudioError {
    match e.kind() {
        ErrorKind::PermissionDenied => AudioError::Denied,
        ErrorKind::DeviceNotAvailable | ErrorKind::HostUnavailable => AudioError::NoDevice,
        _ => AudioError::Other(e.to_string()),
    }
}

/// Interleaved frames → the average of their channels, as f32.
pub fn mix_to_mono<T>(block: &[T], channels: usize, out: &mut Vec<f32>)
where
    T: Copy,
    f32: FromSample<T>,
{
    out.clear();
    let channels = channels.max(1);
    out.extend(
        block
            .chunks_exact(channels)
            .map(|frame| frame.iter().map(|s| <f32 as FromSample<T>>::from_sample_(*s)).sum::<f32>() / channels as f32),
    );
}

/// The meter: RMS of a block in dBFS, −60 dB … 0 dB mapped to 0 … 1.
pub fn meter_level(block: &[f32]) -> f32 {
    if block.is_empty() {
        return 0.0;
    }
    let rms = (block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32).sqrt();
    if rms <= 1e-6 {
        return 0.0;
    }
    ((20.0 * rms.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// No speech at all: whisper invents text ("Thank you.") for silence, so a clip this quiet is not
/// transcribed. Also what a microphone the OS blocks delivers on macOS (zeros).
pub fn is_silent(samples: &[f32]) -> bool {
    samples.iter().all(|s| s.abs() < 0.01)
}

/// Resamples mono audio to 16 kHz: a light low-pass (two box filters, i.e. a triangle) against
/// aliasing when going down, then linear interpolation. Plenty for speech recognition.
pub fn resample_16k(samples: &[f32], rate: u32) -> Vec<f32> {
    if rate == WHISPER_RATE || samples.is_empty() || rate == 0 {
        return samples.to_vec();
    }
    let ratio = rate as f64 / WHISPER_RATE as f64;
    let filtered;
    let src: &[f32] = if ratio > 1.0 {
        let width = ratio.round().max(1.0) as usize;
        filtered = box_filter(&box_filter(samples, width), width);
        &filtered
    } else {
        samples
    };
    let out_len = ((samples.len() as f64) / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let j = pos.floor() as usize;
            let frac = (pos - j as f64) as f32;
            let a = src[j.min(src.len() - 1)];
            let b = src[(j + 1).min(src.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Centred moving average of `width` samples.
fn box_filter(samples: &[f32], width: usize) -> Vec<f32> {
    if width <= 1 {
        return samples.to_vec();
    }
    let half = width / 2;
    let mut prefix = Vec::with_capacity(samples.len() + 1);
    prefix.push(0f64);
    for s in samples {
        prefix.push(prefix.last().unwrap() + *s as f64);
    }
    (0..samples.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + width - half).min(samples.len());
            ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, seconds: f32, amp: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn stereo_and_integer_samples_become_mono_f32() {
        let mut out = Vec::new();
        mix_to_mono(&[1.0f32, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, [0.5, 0.5]);
        mix_to_mono(&[i16::MAX, i16::MAX, 0, 0], 2, &mut out);
        assert!((out[0] - 1.0).abs() < 0.001 && out[1].abs() < 0.001);
        mix_to_mono(&[0.25f32, -0.25], 1, &mut out);
        assert_eq!(out, [0.25, -0.25]);
        // a broken frame at the end is dropped, zero channels don't divide by zero
        mix_to_mono(&[1.0f32, 1.0, 1.0], 2, &mut out);
        assert_eq!(out, [1.0]);
        mix_to_mono(&[0.5f32], 0, &mut out);
        assert_eq!(out, [0.5]);
    }

    #[test]
    fn meter_level_maps_decibels() {
        assert_eq!(meter_level(&[]), 0.0);
        assert_eq!(meter_level(&[0.0; 64]), 0.0);
        let full = meter_level(&[1.0, -1.0, 1.0, -1.0]);
        assert!((full - 1.0).abs() < 1e-4);
        let quiet = meter_level(&[0.001; 64]); // -60 dB
        assert!(quiet < 0.01);
        let mid = meter_level(&[0.0316; 64]); // about -30 dB
        assert!((mid - 0.5).abs() < 0.02, "{mid}");
    }

    #[test]
    fn silence_is_recognised() {
        assert!(is_silent(&[0.0; 1000]));
        assert!(is_silent(&sine(200.0, 16_000, 0.5, 0.005)));
        assert!(!is_silent(&sine(200.0, 16_000, 0.5, 0.2)));
        assert!(is_silent(&[]));
    }

    #[test]
    fn resampling_keeps_length_and_speech_band() {
        for rate in [48_000u32, 44_100, 32_000, 22_050, 8_000] {
            let input = sine(440.0, rate, 2.0, 0.5);
            let out = resample_16k(&input, rate);
            let expected = (input.len() as f64 * 16_000.0 / rate as f64) as usize;
            assert!((out.len() as i64 - expected as i64).abs() <= 1, "{rate}: {} vs {expected}", out.len());
            // a 440 Hz tone keeps (almost) its level
            let r = rms(&out[100..out.len() - 100]);
            assert!((r - 0.5 / 2f32.sqrt()).abs() < 0.03, "{rate}: rms {r}");
        }
        // 16 kHz in = unchanged
        let same = sine(300.0, 16_000, 0.1, 0.3);
        assert_eq!(resample_16k(&same, 16_000), same);
        assert!(resample_16k(&[], 48_000).is_empty());
    }

    #[test]
    fn resampling_damps_what_would_alias() {
        // 15 kHz at 48 kHz would fold back to 1 kHz at 16 kHz without the low-pass
        let input = sine(15_000.0, 48_000, 1.0, 0.5);
        let out = resample_16k(&input, 48_000);
        assert!(rms(&out) < 0.08, "aliased energy {}", rms(&out));
    }

    #[test]
    fn a_clip_knows_its_length_and_is_wiped_when_dropped() {
        let clip = Clip {
            samples: vec![0.5; 48_000],
            rate: 48_000,
        };
        assert_eq!(clip.seconds(), 1.0);
        let mut v = vec![0.5f32; 4];
        wipe(&mut v);
        assert_eq!(v, [0.0; 4]);
    }
}
