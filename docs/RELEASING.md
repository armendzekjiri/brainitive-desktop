# Releasing Brainitive Desktop (phase 10)

Everything in the repo is ready; what is missing are **accounts and secrets**. Until they exist the
CI (`.github/workflows/build.yml`) builds **unsigned** installers (Gatekeeper / SmartScreen warn).
Nothing here is wired to the auto-updater yet (BRAI-373).

## What is ready vs. what waits for you

| Part | State |
| --- | --- |
| CI matrix (macOS universal, Windows x64 NSIS, Linux AppImage + .deb), tests, clippy, draft release | ready; runs once the repo is on GitHub |
| macOS entitlements, hardened runtime, min. macOS 14 | ready (`src-tauri/entitlements.plist`, `bundle.macOS`) |
| macOS signing + notarization | **waits for an Apple Developer account** and 5 secrets |
| Windows signing | **waits for Azure Trusted Signing (or a certificate)** and 6 secrets |
| NSIS per-user install, Start menu, `brainitive://`, WebView2 bootstrapper; Linux `.desktop` MimeType, `.deb` deps | ready (`bundle.*` in `tauri.conf.json`) |
| Download buttons on the website | **waits for the first published release** (3 env values) |

The local token needs the `workflow` scope to push workflow files: `gh auth refresh -s workflow`.

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

`tauri.conf.json` keeps `signingIdentity: null`; Tauri reads the `APPLE_*` variables the workflow
exports. Hardened runtime is on; the only entitlements are `audio-input` (voice capture, without it
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

CI then installs `trusted-signing-cli` and passes a config overlay with
`bundle.windows.signCommand = "trusted-signing-cli -e … -a … -c … -d Brainitive %1"`
(Tauri calls it for every binary and the installer). The overlay is written at build time, so
`tauri.conf.json` itself never needs Azure.

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

## 4. Cut a release

1. Pick the version `X.Y.Z` (Desktop has its own numbers; CHANGELOG "Unreleased" → `## X.Y.Z`).
2. Set it in **all three**: `package.json` (and `package-lock.json`: `npm install --package-lock-only`),
   `src-tauri/Cargo.toml` (then `cargo check` to refresh `Cargo.lock`), `src-tauri/tauri.conf.json`.
3. Commit "chore(release): vX.Y.Z", `git tag vX.Y.Z`, push both.
4. The **Build** workflow runs on the tag: tests, three installers, a **draft** release with
   `SHA256SUMS`. Check each installer on a clean machine (install, open, `brainitive://` link,
   microphone), then publish the draft. Manual runs (Actions → Build → Run workflow) only upload
   workflow artifacts.

## 5. Download buttons on the website

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
