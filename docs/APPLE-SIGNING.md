# Apple signing: steps for the Developer account holder

This is the checklist for the person who has the **Apple Developer Program** membership. All the
code, the entitlements and the CI workflow are ready. Only the Apple steps below are missing.
Background and the Windows / updater parts are in [`RELEASING.md`](RELEASING.md).

Time needed: about 30 minutes, plus Apple's wait for a new membership.

## What you hand back

You return five values: either the five secrets from step 4, or a signed build (step 5). Never
send them by e-mail or chat. Use a password-manager share (1Password, Bitwarden) or set them in
GitHub yourself.

## 1. Membership

- Join the Apple Developer Program (99 USD/year). Either an individual or an organization works.
  An organization account shows the company name in macOS' "Open?" dialog; an individual account
  shows the person's name.
- Note the **Team ID**: developer.apple.com → Account → Membership details (10 characters).

## 2. Developer ID Application certificate

1. On your Mac open **Keychain Access → Certificate Assistant → Request a Certificate From a
   Certificate Authority…**. Enter your e-mail, choose "Saved to disk", and save the `.certSigningRequest`.
2. developer.apple.com → Certificates, Identifiers & Profiles → Certificates → **+** →
   **Developer ID Application** (G2 Sub-CA). Upload the CSR and download the `.cer`.
3. Double-click the `.cer` so it lands in your login keychain next to its private key.
4. Check it:
   ```bash
   security find-identity -v -p codesigning
   # → "Developer ID Application: <Name> (<TEAMID>)"   ← this whole string is the signing identity
   ```
5. In Keychain Access, find the certificate under **My Certificates**, right-click → **Export…** → `.p12`,
   and set a strong password. The `.p12` contains the private key: keep it in a password manager.

## 3. Notarization credentials (pick ONE)

- **B (recommended for CI): App Store Connect API key.**
  1. Open App Store Connect → Users and Access → Integrations → **Team Keys**.
  2. Click **+** and choose the role **Developer**.
  3. Note the **Key ID** and the **Issuer ID**.
  4. Download the `.p8`. Apple lets you download it only once.
- **A: Apple ID.**
  1. Open appleid.apple.com → Sign-In and Security → **App-Specific Passwords**.
  2. Create one named "Brainitive notarize".
  3. You'll also need your Apple ID e-mail and the Team ID.

## 4. GitHub secrets

The repo is `armendzekjiri/brainitive-desktop`. Its secrets live under Settings → Secrets and variables
→ Actions → **New repository secret**. On a personal repo only the owner can open Settings.
So you either send the values to the owner, who runs the commands below, or you get admin
access.

| Secret | Value | How to get it |
| --- | --- | --- |
| `APPLE_CERTIFICATE` | the `.p12` as base64 | `base64 -i cert.p12 \| pbcopy` |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` password | |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <Name> (<TEAMID>)` | step 2.4 |
| B: `APPLE_API_KEY` | Key ID | step 3 B |
| B: `APPLE_API_ISSUER` | Issuer ID | step 3 B |
| B: `APPLE_API_KEY_P8` | full text of the `.p8` file | `pbcopy < AuthKey_XXXX.p8` |
| A: `APPLE_ID` | Apple ID e-mail | step 3 A |
| A: `APPLE_PASSWORD` | app-specific password | step 3 A |
| A: `APPLE_TEAM_ID` | Team ID | step 1 |

With the GitHub CLI (the owner, or anyone with admin access), each one reads from stdin, so nothing lands in the
shell history:

```bash
gh secret set APPLE_CERTIFICATE -R armendzekjiri/brainitive-desktop < <(base64 -i cert.p12)
gh secret set APPLE_CERTIFICATE_PASSWORD -R armendzekjiri/brainitive-desktop   # paste, then Ctrl+D
gh secret set APPLE_SIGNING_IDENTITY -R armendzekjiri/brainitive-desktop
gh secret set APPLE_API_KEY -R armendzekjiri/brainitive-desktop
gh secret set APPLE_API_ISSUER -R armendzekjiri/brainitive-desktop
gh secret set APPLE_API_KEY_P8 -R armendzekjiri/brainitive-desktop < AuthKey_XXXX.p8
```

Then test it: GitHub → Actions → **Build** → **Run workflow** on `main`. Signing runs only for a `v*`
tag or a manual run on `main`. The macOS job should log "signing" and "notarizing". The
draft release then holds a signed `.dmg`.

## 5. Or: a signed build on your own Mac

Use this if you'd rather not hand out the certificate.

```bash
# once
xcode-select --install
brew install node cmake           # cmake: voice capture (whisper.cpp)
curl https://sh.rustup.rs -sSf | sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin

git clone git@github.com:armendzekjiri/brainitive-desktop.git && cd brainitive-desktop
npm ci

# each build (option B shown; for A set APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID instead)
export APPLE_SIGNING_IDENTITY="Developer ID Application: <Name> (<TEAMID>)"
export APPLE_API_KEY=<Key ID> APPLE_API_ISSUER=<Issuer ID>
export APPLE_API_KEY_PATH=$HOME/keys/AuthKey_<Key ID>.p8
npx tauri build --target universal-apple-darwin
```

The output is `src-tauri/target/universal-apple-darwin/release/bundle/dmg/Brainitive_<version>_universal.dmg`.

## 6. Check the result

```bash
APP="src-tauri/target/universal-apple-darwin/release/bundle/macos/Brainitive.app"
codesign --verify --deep --strict --verbose=2 "$APP"     # valid on disk, satisfies its requirement
codesign -d --entitlements - "$APP"                      # only audio-input + network.client
spctl -a -vv "$APP"                                      # accepted, source=Notarized Developer ID
xcrun stapler validate "$APP"                            # The validate action worked!
```

Finally, open the `.dmg` on **another** Mac (or after `xattr -w com.apple.quarantine "0081;0;Safari;" <dmg>`).
It must open without "unidentified developer", and the microphone prompt must appear the first time
voice capture is used.

## If something fails

- **"errSecInternalComponent" while signing:** the keychain is locked, or the certificate has no
  private key. Re-export the `.p12` from **My Certificates**, not from Certificates.
- **Notarization "Invalid":** run `xcrun notarytool log <submission-id> --key … --key-id … --issuer …`.
  The log names the binary and the problem.
- **The x86_64 half fails to compile** (whisper.cpp): build the two architectures separately, as
  described in `RELEASING.md` § 1.
- **The signed app crashes at launch with a code-signing or JIT error:** add only the entitlement
  that the crash log names to `src-tauri/entitlements.plist`, then tell the owner.
