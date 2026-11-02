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

(filled in at the end)
