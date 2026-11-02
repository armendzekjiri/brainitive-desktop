# BRAI-439 — voice capture

Card: board `desktop-app`, BRAI-439 (id 759). Built on `main`, small commits.

## Spec (card + owner notes)

- A global hotkey (default ⇧⌘V / Ctrl+Shift+V, rebindable; **not final**: the owner may move it
  under a "Brainitive key" chord ⌥B → V later, so it is a normal `HotkeyAction`), a "Voice Capture"
  item in the tray, the menu and the quick-capture window.
- Hold to talk, let go to stop; a small always-on-top indicator (level meter, timer, max 2 min, Esc
  cancels).
- On-device speech to text: whisper.cpp through `whisper-rs`, a small multilingual model downloaded
  on first use (progress, pinned SHA-256, app data dir, "Remove voice model" in Settings).
- Audio only in memory: never written to disk, never sent anywhere.
- Off by default; Settings: on / off, language (auto or a list incl. German, English, Albanian),
  the hotkey. The first press while off explains and offers to turn it on.
- Microphone permission: `NSMicrophoneUsageDescription`; "denied" → a clear message and a button
  to the OS privacy settings.
- Result: `capture::open_quick_capture(app, Some(text))`; the user checks it and saves as usual.
- Transcription off the main thread; timing logged (no audio, no transcript).

## Plan

1. **Prefs + hotkey** — `Preferences.voice_enabled` (false), `voice_shortcut`
   (`CommandOrControl+Shift+KeyV`), `voice_language` ("auto"). `HotkeyAction::Voice` in
   `hotkeys::wanted`; the handler passes Pressed **and Released** to the voice module (the other
   actions stay press-only). `global-hotkey` 0.8 reports Released on macOS (Carbon
   `kEventHotKeyReleased`), Windows (key polled every 50 ms after `WM_HOTKEY`) and X11
   (detectable auto-repeat), so hold-to-talk is real; a short tap (< 0.5 s) latches toggle mode
   (tap again, Enter or Stop to finish) — the same code covers both.
2. **`voice/recorder.rs` (pure)** — the state machine: idle → recording → transcribing →
   done / cancelled / error; hold vs. toggle, Esc, the 2-minute cut-off, a session id so a late
   transcription of a cancelled recording is dropped.
3. **`voice/model.rs`** — the model manager: path in `<app local data>/voice/`, `.part` download
   with a running SHA-256, size + checksum check, atomic rename, remove, clean partial files;
   redirect rule for the download host (https, Hugging Face hosts only).
4. **`voice/audio.rs`** — `cpal` default input on its own thread (the stream is `!Send`), mono f32
   in memory (capped at 2 min), a level for the meter; `resample_16k` (pure: low-pass + linear).
5. **`voice/whisper.rs`** — load the context while the user is still speaking (background thread),
   transcribe on another background thread, Metal on macOS, greedy decoding, language or auto;
   silence gate (no whisper run for an all-quiet clip: it hallucinates "Thank you." on silence).
6. **`voice/permission.rs`** — macOS `AVCaptureDevice` authorization status / request (objc2);
   Windows / Linux: the stream's `PermissionDenied` error. Opens the privacy settings page.
7. **`voice/mod.rs` + `voice.html` / `src/voice.ts`** — the indicator window `voice` (own capability
   `capabilities/voice.json`, `popup::ensure_caller`), events `voice://view` / `voice://level` to that
   window only; intro / permission / download / recording / transcribing / error screens.
8. **Settings** — on / off, language, the shortcut row, model status + Remove.
9. **Tray / menu / capture window** — "Voice Capture…" items and a mic button in quick capture.
10. **Docs** — ARCHITECTURE, SECURITY (microphone, model download host, no audio retention),
    CHANGELOG, README, this file's result.

## Result

Built on `main` (commits `eb950ef` plan, `0a975f7` Rust, `33e7ad1` UI, `1d137c3` lock fix, then
docs).

**Works / verified (macOS 26, M4 Pro)**

- `cargo build` / `test` / `clippy -D warnings`, `npm run build` green. whisper.cpp compiles
  through CMake in ≈ 15–30 s (first build only).
- The real model download through `voice::model::download` (pinned URL → Hugging Face CDN redirect,
  size + SHA-256 verified, renamed into place) and real transcriptions of macOS `say` clips in the
  `#[ignore]` test `voice::whisper::tests::real_transcription`.
- Unit tests: recorder state machine (hold, tap → toggle, Enter / press again, Esc while recording
  and transcribing, the 2-minute cut-off, empty results, failures, stale sessions), model manager
  (paths, good download, checksum mismatch, short / oversized, cancelled → no part file, clean,
  remove, redirect hosts), mixing / meter / silence / resampling incl. anti-aliasing, transcript
  clean-up, languages, the view model, preferences (defaults, old files, unknown language), the
  hotkey registry entry (in `wanted`, default valid + distinct, duplicate refused, only voice gets
  the key-up), tray item, popup flags, capability files (own window, own commands, Info.plist key).

**Measured (whisper small q5_1)**

| Clip | Metal | CPU only (8 threads) |
|---|---|---|
| 4.8 s English | 0.21–0.33 s | 0.44 s |
| 4.9 s German | 0.21–0.33 s | 0.43 s |
| 13.7 s English | 0.35 s (0.43 s auto-detect) | 0.50 s |

Model load 0.14–0.28 s warm (it runs while the user speaks); the very first load after a build
took 5.6 s once (Metal kernels compiled and cached by macOS). Download 190 MB: 3.9 s here. The card's
target (< 2 s for 15 s on an M1) should hold with margin; not measured on an M1.

**Model choice**: whisper.cpp's multilingual `small` q5_1 (190 MB) instead of `base` (q8_0, 82 MB):
in the same test base turned "Ich muss morgen früh Anna anrufen" into "Ich muss Norge früh
aneranrufen"; small got German right, and German / Albanian are the users' languages. Above the
card's "75–150 MB", deliberately. Swapping is one constant (`voice::model::MODEL`).

**Size impact** (`brainitive-desktop` binary, before → after): debug 59.1 MB → 62.9 MB (+3.8 MB),
release 8.58 MB → 10.48 MB (+1.9 MB, +22 %). The model is not in the bundle.

**Decisions**

- Hold-to-talk is real (key-up on all three OSes, see plan); a tap < 0.5 s latches toggle mode, so
  the same shortcut also works as press-to-start / press-to-stop.
- The shortcut is registered while voice capture is off (spec: the first press explains). It takes
  ⇧⌘V / Ctrl+Shift+V from other apps (paste as plain text; paste in Linux terminals): see SECURITY
  known gaps — worth revisiting with the "Brainitive key" chord.
- The model loads while the user speaks and is dropped after each recording (≈ 0.15 s to load
  again; no ~200 MB kept in memory between uses).
- Silence / < 0.3 s clips are not transcribed (whisper invents "Thank you." on silence).
- Voice text is appended to what is already typed in quick capture (no draft is lost).
- `GGML_NATIVE=OFF` so release builds run on every Apple Silicon Mac / AVX2 PC.
- CMake became a build requirement (installed locally with Homebrew; CI runners have it); Linux
  CI gets `libasound2-dev`.

**Not tested**

- The running app: the indicator window, the hotkey press / release, the microphone stream and the
  macOS permission prompt were not exercised (the user's Brainitive.app was running; single
  instance). Needs a manual pass: hold ⇧⌘V, tap mode, Esc, 2-minute cut-off, denied permission →
  Open Privacy Settings, Turn on → download progress, Settings remove / language, the quick-capture
  mic button.
- Windows and Linux: nothing compiled or run here (no targets / toolchains); the CI matrix will
  build whisper.cpp with MSVC / gcc. Beta checklist (BRAI-376): WASAPI capture and the "denied"
  mapping, key-up polling on Windows, ALSA on Linux, Wayland has no global shortcuts (menu / tray
  work).
- Signed macOS builds need the `com.apple.security.device.audio-input` entitlement (signing isn't
  configured yet).
- Albanian accuracy (no Albanian `say` voice); Intel Macs.
