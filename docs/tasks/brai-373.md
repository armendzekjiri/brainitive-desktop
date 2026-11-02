# BRAI-373 — auto-updater with signed bundles and stable / beta channels

Card: board `desktop-app`, BRAI-373 (id 653): "tauri-plugin-updater, update key pair kept offline +
in CI secrets, manifests on GitHub Releases. Beta channel tracks pre-release, stable tracks main."
Built on `main`, small commits, no remote yet (planned repo `armendzekjiri/brainitive-desktop`).

## Plan

1. **Dependency**: `tauri-plugin-updater = "=2.14.0"` (2026-10-14; 2.14.1 is younger than two
   weeks). Used from Rust only: no capability grants `updater:*` (`tests/capabilities.rs`).
2. **Key handling**: the public key lives in `tauri.conf.json` → `plugins.updater.pubkey` (the plugin
   verifies with exactly that value). The repo carries a clearly marked **placeholder**; the
   updater's `availability()` guard refuses to run (log line, disabled menu / tray / Settings) unless
   the value decodes to a real minisign Ed25519 public key. Debug builds never update either (the
   owner's running dev app must not be replaced). `requireSignedVersion: true`: a signature must
   carry the version it was made for (Tauri CLI ≥ 2.12 writes it), so a tampered manifest can't pair
   a new version number with an older signed bundle.
3. **Channels / endpoints** (`updater.rs`, pure):
   - stable (default): `https://github.com/armendzekjiri/brainitive-desktop/releases/latest/download/latest.json`
     — GitHub's "latest" is the newest published release that is **not** a pre-release or draft.
   - beta: `https://github.com/armendzekjiri/brainitive-desktop/releases/download/updater-beta/beta.json`
     — a rolling pre-release `updater-beta` whose asset CI replaces when a pre-release is published.
     Beta clients check **both** manifests and take the higher version, so a stable release
     newer than the last beta reaches them even if `beta.json` is stale.
   - https only; redirects ≤ 5, each https and on `github.com`, `objects.githubusercontent.com`,
     `release-assets.githubusercontent.com` (GitHub's asset CDN). Applied with `configure_client`
     to the manifest and the bundle download.
4. **Schedule** (pure): first check 30 s after start, then every 6 h (wall clock, so sleep counts),
   1 h after a failed check; automatic checks can be turned off; "Check for Updates…" always works.
   Nothing is checked once an update is downloaded and waiting.
5. **Flow**: check → download in the background (signature verified by the plugin) → bundle
   staged in `<app cache>/update/` → notification "Update ready — restart to install", menu / tray
   "Restart to Update (x.y.z)". Only that click installs (signature verified again) and restarts.
6. **UI**: app menu (macOS app menu, Help on Windows / Linux) "Check for Updates…", which becomes
   "Restart to Update (x.y.z)"; the tray gets the same items; Settings → Updates: automatic checks,
   "Get beta updates", last check, current version, Check now / Restart to Update. The "Please
   update" screen (minimum version gate) gets "Check for update now".
7. **CI**: tag builds export `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)` and add a
   `bundle.createUpdaterArtifacts: true` overlay only when the secret exists (otherwise no updater
   artifacts, no failure); the release job writes `latest.json` (`scripts/updater-manifest.mjs`)
   into the draft release; publishing a pre-release copies its manifest to `updater-beta/beta.json`.
8. **Tests**: channel / URL building, redirect policy, placeholder guard (+ the committed config),
   schedule, and the plugin itself with a throwaway minisign key: bad signature / older / equal
   version are refused, via `restore_update` + `verify` and via a loopback HTTP manifest + download.
9. **Docs**: RELEASING (keys, beta vs stable, rollback = fix forward), SECURITY (threat model),
   ARCHITECTURE, CHANGELOG.

## Result

Built as planned on `main` (no push, no remote).

- `src-tauri/src/updater.rs` (pure part + runtime), wired in `lib.rs`, `menu.rs` (item
  `check-for-updates`), `tray.rs` (`add_update_entry`), `prefs.rs` (Settings commands),
  `commands.rs` (`check_for_update` / `restart_to_update` with a main-screen caller check),
  `settings.rs` (`auto_update_check`, `update_channel` — lenient — and `last_update_check`),
  `build.rs` + `capabilities/local.json` / `settings.json`; UI in `settings.html` / `settings.ts`
  and the "Please update" screen (`index.html` / `main.ts`).
- `tauri.conf.json`: `plugins.updater` with the **placeholder** public key, `requireSignedVersion`,
  passive Windows install; `createUpdaterArtifacts` stays off (CI overlay).
- CI: updater overlay + key only in the `tauri build` step when the secret exists and the key is
  real; signature checked against the committed key by the ignored test
  `ci_signed_bundle_verifies_with_the_committed_key`; `scripts/updater-manifest.mjs` → `latest.json`;
  `v*-*` tags → pre-release drafts; `beta-manifest` job on a published pre-release.
- Verified by hand with a throwaway key made by `npx tauri signer generate` (scratch dir, deleted):
  Tauri CLI 2.12.4 writes `version:` into the trusted comment, `key_status` accepts its public key,
  the CI check passes for the signed file and fails for a tampered one. Config restored afterwards.
- Tests: channels / URLs, lenient channel, https + host allow-list + redirect limit, the https-only
  client, placeholder / invalid / real / dev-build guard, the committed config (placeholder or real
  key, `requireSignedVersion`, no dangerous flags, no `createUpdaterArtifacts`), versions + newest
  pick, schedule (start delay, 6 h, 1 h retry, sleep, clock going back), auto-check conditions, menu
  labels, tray line, settings defaults; the plugin itself with throwaway minisign keys
  (`restore_update` + `verify`: good, tampered bytes, foreign key, signed-version mismatch, garbage,
  equal / older versions) and over loopback HTTP (`check` + `download`: good vs. foreign signature,
  equal version, beta picks the newest of two manifests and survives one failing, none answering =
  error); `tests/capabilities.rs`: no `updater:*` grant, update commands only in Settings / main.

Waits for the owner: the key pair (`npx tauri signer generate`), the public key in
`tauri.conf.json`, the secrets `TAURI_SIGNING_PRIVATE_KEY` / `_PASSWORD`, public release assets on
`armendzekjiri/brainitive-desktop`, and a first real tag build to see the bundles on all three OSes
(the Linux `.deb` update path uses `pkexec`; the macOS install may ask for an admin password when
the app is in a folder the user can't write).
