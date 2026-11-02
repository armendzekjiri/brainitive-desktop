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

(filled in at the end)
