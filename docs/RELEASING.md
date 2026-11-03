# Releasing Brainitive Desktop (phase 10)

Release builds run from protected `main` through the reviewer-protected GitHub `release`
environment. macOS signing and notarization are required; a build fails if the necessary Apple
secrets are absent. The auto-updater remains off until its own signing key exists (section 4).

## What is ready vs. what waits for you

| Part | State |
| --- | --- |
| CI matrix (macOS universal, Windows x64 NSIS, Linux AppImage + .deb), tests, clippy, draft release | ready; runs once the repo is on GitHub |
| macOS entitlements, hardened runtime, min. macOS 14 | ready (`src-tauri/entitlements.plist`, `bundle.macOS`) |
| macOS signing + notarization | **waits for the certificate + notarization secrets** from the team holder (Burim Bilali, individual team `65NCF3DHKS`); handover in [`APPLE-SIGNING.md`](APPLE-SIGNING.md) |
| Windows signing | the **Microsoft Store** package is signed by the Store (no secret; CI builds it, section 2); the NSIS installer on GitHub stays unsigned unless Azure Trusted Signing (or a certificate) and 6 secrets are added |
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

GitHub environment secrets (Settings → Environments → release → Environment secrets):

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of the `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` password |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Name (TEAMID)` |
| A: `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Apple ID e-mail, app-specific password, Team ID |
| B: `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` | Key ID, Issuer ID, full text of the `.p8` |

**The build never sees these secrets** (codex review 11). `tauri.conf.json` keeps
`signingIdentity: null`, and `tauri build` runs with no `APPLE_*` variable, so it makes an
**unsigned** `Brainitive.app` and `.dmg` (npm, cargo and every crate's build script run in that
step). The next step, **"macOS signing + notarization"**, is the only one with the Apple secrets
(step `env:`, never `$GITHUB_ENV`). It runs [`scripts/macos-sign.sh`](../scripts/macos-sign.sh),
which uses Apple's tools only:

1. imports `APPLE_CERTIFICATE` (+ password) into a throwaway keychain in the runner's temp folder;
2. `codesign --force --deep --options runtime --timestamp --entitlements src-tauri/entitlements.plist
   --sign "$APPLE_SIGNING_IDENTITY"` on the `.app`, then `codesign --verify --deep --strict`;
3. replaces Tauri's `.dmg` (it holds the unsigned app) with a new one made by `hdiutil` from the
   signed app plus an `Applications` link, same file name, and signs it;
4. `xcrun notarytool submit <dmg> --wait` (option A: Apple ID; option B: the API key, its `.p8`
   written to a 0600 temp file), fails unless the status is `Accepted`, then
   `xcrun stapler staple` on the `.dmg` **and** the `.app`, `stapler validate`, `spctl --assess`.

The keychain and the `.p8` are deleted when the script exits. The release workflow rejects missing
Apple signing or notarization credentials before building. The script also supports local signing
without notarization credentials, which produces a signed but unnotarized app. Hardened runtime is on; the only entitlements are `audio-input` (voice capture, without it
a signed build never gets the microphone) and `network.client`. No JIT entitlement: WKWebView runs
JavaScript in WebKit's own processes. If a signed build ever crashes on launch with a code-signing
or JIT error, add the entitlement named in the crash log, not before.

The macOS build is `--target universal-apple-darwin` (arm64 + x86_64 compiled on one runner,
whisper.cpp with Metal). Not yet run in CI: if the x86_64 half of whisper.cpp fails to cross-compile,
split into two matrix entries (`--target aarch64-apple-darwin` / `x86_64-apple-darwin`).

## 2. Windows: signing

**The signed path is the Microsoft Store (BRAI-442).** The `msstore` job of `build.yml` builds an
unsigned `Brainitive_<X.Y.Z>_x64.msix` for stable versions during release builds and manual runs,
and uploads it as a workflow artifact. You upload it in Partner Center → Brainitive → Packages; the
Store signs it with Microsoft's certificate for our reserved identity, so Store installs show no
SmartScreen warning and update through the Store. No certificate, no Azure account, no secret.
Building, sideload testing, WACK and the submission steps: [`MICROSOFT-STORE.md`](MICROSOFT-STORE.md).
Each submission needs a **higher** version than the last one (the MSIX version is
`tauri.conf.json`'s `X.Y.Z` + `.0`), so bump it before the release build.

**The NSIS installer on GitHub stays unsigned** (SmartScreen warns on first run) until someone pays
for code signing; it keeps its own auto-updater (section 4). Optional, for that installer only:

**Azure Trusted Signing** (about 10 USD/month, no hardware token, builds SmartScreen
reputation; identity validation needed, individuals in some regions only, check availability).

1. Azure: create a **Trusted Signing account**, complete identity validation, create a
   **certificate profile** (Public Trust).
2. Create an app registration (service principal) with a client secret and give it the role
   **Trusted Signing Certificate Profile Signer** on the account.
3. Secrets: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`,
   `AZURE_TRUSTED_SIGNING_ENDPOINT` (e.g. `https://weu.codesigning.azure.net`, the region's
   endpoint), `AZURE_TRUSTED_SIGNING_ACCOUNT`, `AZURE_TRUSTED_SIGNING_PROFILE`.

CI then signs **after** the build, so `tauri build` and its build scripts never see Azure
credentials (codex review 11):

1. **Windows signing tool** (no secrets): `cargo install trusted-signing-cli --version 0.11.0
   --locked --root $RUNNER_TEMP/signing-tools` (pinned; check that `trusted-signing-cli --help`
   still has `-e -a -c -d` before bumping).
2. **tauri build** with `--no-bundle` (no secrets): `src-tauri/target/release/brainitive-desktop.exe`.
3. **Windows signing (app)**: `trusted-signing-cli -e <endpoint> -a <account> -c <profile> -d
   Brainitive brainitive-desktop.exe`.
4. **tauri bundle (Windows)** (no secrets): `npx tauri bundle --bundles nsis` packs the signed exe
   into the installer.
5. **Windows signing (installer)**: the same command on `bundle/nsis/*-setup.exe`.

Only steps 3 and 5 get `AZURE_TENANT_ID` / `_CLIENT_ID` / `_CLIENT_SECRET` and the three
`AZURE_TRUSTED_SIGNING_*` values (step `env:`); `tauri.conf.json` never needs Azure. The NSIS
uninstaller that the installer writes is not signed separately (Tauri's `signCommand` used to sign
it inside the build, which would hand the credentials to the build again).

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
2. GitHub environment secrets (Settings → Environments → release → Environment secrets):

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

- **Signing runs only from the default branch**, for a `build:` commit or a manual run (Actions →
  Build → Run workflow). The `release` environment restricts deployments to `main`, requires an
  Armend or Burim approval, and holds the signing secrets. Pull requests and other refs cannot
  reach that environment; the separate CI workflow checks pull requests without release secrets.
- Every secret is passed through `env:` of the one step that uses it; nothing is written to
  `$GITHUB_ENV` (which would hand it to every later step, `npm`, `cargo` and their build scripts
  included).
- **No signing secret reaches a build step** (codex review 11): `npm ci`, `npm run build`,
  `cargo test` / `clippy`, `tauri build` and `tauri bundle` run without any; the Apple secrets go
  to "macOS signing + notarization" only, the Azure ones to "Windows signing (app)" /
  "(installer)" only, the updater key to "Updater signing" only, and those steps build nothing.
  `scripts/workflow-secrets.test.mjs` (`node --test scripts/`) fails when a change breaks that.
- Residual: the signing steps run on the same runner after the build, so a process a compromised
  build left running could still watch them. Moving signing to its own job (fresh runner, the
  unsigned bundle as an artifact) would close that; not done yet to keep the pipeline simple.

### Actions are pinned to commit SHAs

`build.yml` and `ci.yml` reference every action by a full commit SHA
(`uses: owner/action@<40 hex> # vX.Y.Z`), so a moved tag can't change what runs next to the
signing secrets (done 2026-11-02, resolved with `git ls-remote`). To update one:

```bash
git ls-remote https://github.com/<owner>/<action> "refs/tags/<tag>*"   # annotated: use the ^{} line
```

Current pins (2026-11-02), all `runs.using: node24`, each the newest release at least two weeks old:
`actions/checkout` v7.0.1, `actions/setup-node` v7.1.0, `Swatinem/rust-cache` v2.9.2,
`actions/upload-artifact` v7.0.2, `actions/download-artifact` v8.0.2. Notes on the major bumps: checkout
v6+ keeps credentials in a separate file (we never push from CI), v7 refuses fork-PR checkouts in
`pull_request_target` / `workflow_run` (we use neither); upload/download-artifact keep our usage
(`name`, `path`, `pattern` + `merge-multiple`) unchanged.

`dtolnay/rust-toolchain` is pinned to the commit its `1.90.0` branch pointed to. Dependabot
(`package-ecosystem: github-actions`) can keep SHA pins and their comments current.

## 5. Cut a release

1. Bump the Desktop version with `node scripts/bump-version.mjs X.Y.Z`, update the changelog,
   run the repository gate, and open a reviewed pull request to `main`. Use a merge commit or
   squash title beginning with `build:` (or `build(scope):`) to start a release build.
2. Approve the protected `release` environment when GitHub requests it. The workflow builds
   from that exact `main` commit, signs and notarizes macOS, and creates a **draft** GitHub
   release for the version in `src-tauri/tauri.conf.json`. It refuses an existing tag or draft.
3. Check `SHA256SUMS`, the macOS notarization and each installer on a clean machine. Publish the
   draft only after review. The release tag targets the build commit.
   Do not push a release tag directly: tag pushes do not invoke the signing workflow.
4. A manual **Build → Run workflow** on `main` uses the same protected environment but only
   uploads workflow artifacts; it does not create or publish a release.

Stable versions become GitHub's latest release when published; prerelease versions update the
beta manifest. The updater only installs higher signed versions, so recover from a bad release by
shipping a new, higher version. Until its own updater key pair is configured, no updater manifest
is produced.

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
