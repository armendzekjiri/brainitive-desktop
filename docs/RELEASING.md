# Releasing Brainitive Desktop (phase 10)

Everything in the repo is ready; what is missing are **accounts and secrets**. Until they exist the
CI (`.github/workflows/build.yml`) builds **unsigned** installers (Gatekeeper / SmartScreen warn),
and the auto-updater (BRAI-373) stays **off** in every build until its key exists (section 4).

## What is ready vs. what waits for you

| Part | State |
| --- | --- |
| CI matrix (macOS universal, Windows x64 NSIS, Linux AppImage + .deb), tests, clippy, draft release | ready; runs once the repo is on GitHub |
| macOS entitlements, hardened runtime, min. macOS 14 | ready (`src-tauri/entitlements.plist`, `bundle.macOS`) |
| macOS signing + notarization | **waits for the certificate + notarization secrets** from the team holder (Burim Bilali, individual team `65NCF3DHKS`); handover in [`APPLE-SIGNING.md`](APPLE-SIGNING.md) |
| Windows signing | **waits for Azure Trusted Signing (or a certificate)** and 6 secrets |
| NSIS per-user install, Start menu, `brainitive://`, WebView2 bootstrapper; Linux `.desktop` MimeType, `.deb` deps | ready (`bundle.*` in `tauri.conf.json`) |
| Download buttons on the website | **waits for the first published release** (3 env values) |
| Auto-updater: plugin, stable / beta channels, signed updater bundles, `latest.json`, beta manifest | code + CI ready; **waits for the update key pair** (public key in `tauri.conf.json`, 2 secrets) and **public release assets** |

The repo owner's `gh` token needs the `workflow` scope to push workflow files (`gh auth refresh -s workflow`).
The step-by-step list for the person with the Apple Developer account is [`APPLE-SIGNING.md`](APPLE-SIGNING.md).

## 1. macOS: Apple Developer ID + notarization

1. Join the **Apple Developer Program** (99 USD/year).
2. In Certificates, Identifiers & Profiles create a **Developer ID Application** certificate
   (CSR from Keychain Access). Download it, import it into Keychain Access, export it with its
   private key as a `.p12` (set a password).
3. `base64 -i cert.p12 | pbcopy` gives the value for `APPLE_CERTIFICATE`.
4. Signing identity string: `security find-identity -v -p codesigning` →
   `Developer ID Application: Your Name (TEAMID)`.
5. Notarization credentials, **one** of:
   - **A. Apple ID**: an app-specific password (appleid.apple.com → Sign-In and Security) plus the
     Team ID (developer.apple.com → Membership).
   - **B. App Store Connect API key** (better for CI): App Store Connect → Users and Access →
     Integrations → Team keys → create a key (role Developer); note the Key ID and Issuer ID,
     download the `.p8` once.

GitHub secrets (Settings → Secrets and variables → Actions):

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of the `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` password |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Name (TEAMID)` |
| A: `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Apple ID e-mail, app-specific password, Team ID |
| B: `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` | Key ID, Issuer ID, full text of the `.p8` |

`tauri.conf.json` keeps `signingIdentity: null`; Tauri reads the `APPLE_*` variables, which the
workflow passes **only to the `tauri build` step** of the macOS runner (step `env:`, never
`$GITHUB_ENV`; BRAI-377). Option B's `.p8` is written to the runner's temp folder (0600) by its own
step and deleted right after the build. Hardened runtime is on; the only entitlements are `audio-input` (voice capture, without it
a signed build never gets the microphone) and `network.client`. No JIT entitlement: WKWebView runs
JavaScript in WebKit's own processes. If a signed build ever crashes on launch with a code-signing
or JIT error, add the entitlement named in the crash log, not before.

The macOS build is `--target universal-apple-darwin` (arm64 + x86_64 compiled on one runner,
whisper.cpp with Metal). Not yet run in CI: if the x86_64 half of whisper.cpp fails to cross-compile,
split into two matrix entries (`--target aarch64-apple-darwin` / `x86_64-apple-darwin`).

## 2. Windows: signing

**Preferred: Azure Trusted Signing** (about 10 USD/month, no hardware token, builds SmartScreen
reputation; identity validation needed, individuals in some regions only, check availability).

1. Azure: create a **Trusted Signing account**, complete identity validation, create a
   **certificate profile** (Public Trust).
2. Create an app registration (service principal) with a client secret and give it the role
   **Trusted Signing Certificate Profile Signer** on the account.
3. Secrets: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`,
   `AZURE_TRUSTED_SIGNING_ENDPOINT` (e.g. `https://weu.codesigning.azure.net`, the region's
   endpoint), `AZURE_TRUSTED_SIGNING_ACCOUNT`, `AZURE_TRUSTED_SIGNING_PROFILE`.

CI then installs `trusted-signing-cli` **0.11.0** (pinned: `cargo install --version 0.11.0
--locked`; check that `trusted-signing-cli --help` still has `-e -a -c -d` before bumping) and
passes a config overlay with
`bundle.windows.signCommand = "trusted-signing-cli -e … -a … -c … -d Brainitive %1"`
(Tauri calls it for every binary and the installer). The overlay is written at build time, so
`tauri.conf.json` itself never needs Azure. `AZURE_TENANT_ID` / `_CLIENT_ID` / `_CLIENT_SECRET` go
to the Windows `tauri build` step only (step `env:`).

**Alternative: a `.pfx` (OV/EV) certificate.** Not wired in CI. EV certificates live on hardware
tokens / cloud HSMs and usually need the vendor's signing tool; with an exportable OV `.pfx`:
base64 it into a secret, import it on the runner (`Import-PfxCertificate`), and set
`bundle.windows.certificateThumbprint`, `digestAlgorithm: "sha256"`, `timestampUrl` in an
overlay. OV certificates still show SmartScreen warnings until reputation builds.

NSIS settings (`bundle.windows`): per-user install (`currentUser`, no admin prompt), Start menu
folder "Brainitive", WebView2 **download bootstrapper** (silent; needs internet on first install;
use `embedBootstrapper` or `offlineInstaller` for offline installs, bigger file). The
`brainitive://` scheme is registered by the installer (HKCU) from `plugins.deep-link`; no extra
config. MSI is not built (NSIS only).

## 3. Linux

`.deb` (Tauri adds webkit2gtk-4.1, GTK and the tray library; we add `libasound2 | libasound2t64`
for the microphone) and **AppImage** (built on Ubuntu 22.04 so it runs on glibc 2.35 and newer).
Both get the `.desktop` entry with `MimeType=x-scheme-handler/brainitive` from the deep-link
config, and the icons from `src-tauri/icons`. Not signed. Wayland has no global shortcuts (known).

## 4. Auto-updates: the update signing key (BRAI-373)

The app installs an update only when the bundle's **minisign (Ed25519) signature** matches the
public key compiled into it (`src-tauri/tauri.conf.json` → `plugins.updater.pubkey`) and the
signature names the version the manifest announces (`requireSignedVersion`). That key is
independent of the Apple / Azure code signing. The repo carries a **placeholder**: while it is
there, the app logs `updater: off (PlaceholderKey)`, greys out "Check for Updates…" and never asks
the network for an update, and CI makes no updater bundles.

### Create the key pair (once, on your own machine)

```bash
npx tauri signer generate -w ~/.tauri/brainitive-desktop-updater.key
```

- Set a **password** when asked (it protects the key file; CI needs it too).
- It writes `~/.tauri/brainitive-desktop-updater.key` (**private**) and `….key.pub` (public).
- Keep the private key **offline and backed up** (password manager entry or an encrypted USB stick,
  plus a second copy). Never commit it, never paste it into an issue or chat.
- **Losing the private key or its password means installed apps can never update again**: every
  user would have to download a new build (with a new key) by hand. There is no recovery.

### Put it in place

1. Public key → `src-tauri/tauri.conf.json`, `plugins.updater.pubkey`: replace the whole
   `PLACEHOLDER-…` string with the **content** of `….key.pub` (one base64 line, as printed). Commit
   it — it is public. `cargo test` checks it is a real minisign key (or still the placeholder).
2. GitHub secrets (Settings → Secrets and variables → Actions):

   | Secret | Value |
   | --- | --- |
   | `TAURI_SIGNING_PRIVATE_KEY` | the **content** of `~/.tauri/brainitive-desktop-updater.key` |
   | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its password |

3. The release assets must be **public** (public repo, or at least public releases): the app reads
   them without any token. While the repo is private the updater finds nothing (it treats that as
   a failed check and tries again in an hour).

Then signing builds (see "Who gets the secrets" below) build **without** the key (the committed
config keeps `createUpdaterArtifacts` off and nothing switches it on), and a separate **Updater
signing** step after `tauri build` — the only step that gets `TAURI_SIGNING_PRIVATE_KEY` — packs
`Brainitive.app.tar.gz` from the signed, notarized `.app` (`COPYFILE_DISABLE=1 tar -czf … -C
bundle/macos Brainitive.app`: the app at the archive's top level, the layout the updater unpacks)
and signs the updater bundles with `npx tauri signer sign --app-version <tauri.conf.json version>`
(`Brainitive.app.tar.gz` for macOS, the NSIS `-setup.exe`, the `.AppImage` and `.deb`;
`--app-version` puts the version into the signed trusted comment that `requireSignedVersion`
checks, as `tauri build` would). CI then **checks the
signature against the committed public key** with the updater plugin itself (a mismatched pair fails
the build instead of shipping a release nobody can update to), and the draft release gets a
`latest.json` (`scripts/updater-manifest.mjs`; the `.sig` files go into it, not into the release).
Without the secret (or with the placeholder still in the config) the updater bundles are skipped
and nothing fails.

Rotating the key (only if it leaked): ship one last release signed with the **old** key whose
config contains the **new** public key; after users have it, sign with the new key only. Revoke by
deleting the old secrets.

### Channels

| Channel | Who | The app reads |
| --- | --- | --- |
| **stable** (default) | everyone | `https://github.com/armendzekjiri/brainitive-desktop/releases/latest/download/latest.json` — GitHub's "latest" = the newest **published, non-pre-release** release |
| **beta** (Settings → "Get beta updates") | testers | `…/releases/download/updater-beta/beta.json` **and** the stable `latest.json`; the higher version wins |

`updater-beta` is a rolling **pre-release** that only holds `beta.json`. CI creates it the first time a
pre-release is published and replaces `beta.json` each time; it stays marked pre-release so it never
becomes "latest". Don't delete it (beta users would fall back to stable updates only, which is safe).

The app checks 30 s after start and every 6 hours (1 hour after a failure; "Check for Updates…" in
the app menu / Help menu / tray / Settings any time), downloads in the background, shows "Update
ready — restart to install" and installs only when the user clicks **Restart to Update**.

### Who gets the secrets (BRAI-377)

- **Signing only runs for a `v*` tag push, or a manual run (Actions → Build → Run workflow) on the
  default branch** (`SIGNING_ALLOWED` in `build.yml`). Pull requests, other branches and
  `release` events never reach a signing step.
- Every secret is passed through `env:` of the one step that uses it; nothing is written to
  `$GITHUB_ENV` (which would hand it to every later step, `npm`, `cargo` and their build scripts
  included). `tauri build` gets the Apple / Azure credentials of its own OS only; the updater key
  never reaches `tauri build` at all.

### Actions are pinned to commit SHAs

`build.yml` and `ci.yml` reference every action by a full commit SHA
(`uses: owner/action@<40 hex> # vX.Y.Z`), so a moved tag can't change what runs next to the
signing secrets (done 2026-11-02, resolved with `git ls-remote`). To update one:

```bash
git ls-remote https://github.com/<owner>/<action> "refs/tags/<tag>*"   # annotated: use the ^{} line
```

`dtolnay/rust-toolchain` is pinned to the commit its `1.90.0` branch pointed to. Dependabot
(`package-ecosystem: github-actions`) can keep SHA pins and their comments current.

## 5. Cut a release

1. Pick the version `X.Y.Z` (Desktop has its own numbers; CHANGELOG "Unreleased" → `## X.Y.Z`).
2. Set it in **all three**: `package.json` (and `package-lock.json`: `npm install --package-lock-only`),
   `src-tauri/Cargo.toml` (then `cargo check` to refresh `Cargo.lock`), `src-tauri/tauri.conf.json`.
3. Commit "chore(release): vX.Y.Z", `git tag vX.Y.Z`, push both.
4. The **Build** workflow runs on the tag: tests, three installers, a **draft** release with
   `SHA256SUMS` (and `latest.json` + updater bundles once the update key exists). Check each
   installer on a clean machine (install, open, `brainitive://` link, microphone), then publish the
   draft. Manual runs (Actions → Build → Run workflow) only upload workflow artifacts; they are
   signed only when started on the default branch.

**Stable vs. beta.** A plain tag `vX.Y.Z` makes a normal release: publishing it moves GitHub's
"latest", so every stable user gets it (and beta users too, when it is newer than their beta). A tag
with a pre-release part, **`vX.Y.Z-beta.N`** (semver: `1.2.0-beta.1 < 1.2.0-beta.2 < 1.2.0`), makes a
**pre-release** draft; publishing it updates `updater-beta/beta.json` (the "Beta channel manifest" job),
so only beta users get it. Stable users never see betas. A beta user who turns beta off keeps the beta
until a stable version above it is out (no downgrade). Promote a beta by tagging the final `vX.Y.Z`.

**Rollback = fix forward.** The updater only installs a **higher** version, and each signature is bound
to its version, so republishing an older release (or pointing a manifest at an old bundle) never
downgrades anyone. To undo a bad release:
1. Stop the spread: edit the bad release on GitHub — for stable mark an older good release as
   "latest" (or turn the bad one back into a draft / pre-release), for beta re-publish (or re-run the
   job of) the previous good pre-release so `beta.json` points at it again. Users who haven't
   updated stop being offered the bad version; those who have keep it.
2. Ship the fix as a **new, higher** version: revert the change, bump `X.Y.(Z+1)`, tag, publish.
   Everyone on the bad version moves to it on their next check (within 6 hours).
3. Only if the bad version can't update at all (broken updater): users need a manual download;
   say so in the release notes and on the download page.

## 6. Download buttons on the website

The app repo reads `config/desktop.php` → env values; empty means "Coming soon". After publishing
set on the server (`.env`, then reload PHP / clear config cache if one is used):

```
DESKTOP_DOWNLOAD_MACOS_URL=https://github.com/<owner>/<repo>/releases/download/vX.Y.Z/Brainitive_X.Y.Z_universal.dmg
DESKTOP_DOWNLOAD_WINDOWS_URL=https://github.com/<owner>/<repo>/releases/download/vX.Y.Z/Brainitive_X.Y.Z_x64-setup.exe
DESKTOP_DOWNLOAD_LINUX_URL=https://github.com/<owner>/<repo>/releases/download/vX.Y.Z/Brainitive_X.Y.Z_amd64.AppImage
```

(File names as the draft release shows them.) The repo is currently private/unpublished: for
public links the release assets must be public, or the files must be hosted elsewhere
(brainitive-website, R2/S3). A `releases/latest/download/<fixed name>` link would avoid editing the
env per release but needs fixed file names.
