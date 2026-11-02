# Apple signing: steps for the Developer account holder

This is the checklist for the person who has the **Apple Developer Program** membership. All the
code, the entitlements and the CI workflow are ready. Only the Apple steps below are missing.
Background and the Windows / updater parts are in [`RELEASING.md`](RELEASING.md). An AI agent
doing this on the account holder's Mac starts with [`SIGNING-AGENT-BRIEF.md`](SIGNING-AGENT-BRIEF.md).

Time needed: about 30 minutes, plus Apple's wait for a new membership.

## Status and handover (2026-11-02)

**The team.** Brainitive is signed with Burim Bilali's Apple Developer team (Team ID
`65NCF3DHKS`). It is an **individual** membership. That has three consequences:
- **Only Burim can use developer.apple.com** (certificates, App IDs, profiles). Other people,
  including Admins in App Store Connect, see no team there. Team members on the developer site
  exist only for organization memberships.
- **Users see "Burim Bilali"** as the developer: Gatekeeper's "Open?" dialog, the signature, and
  later the App Store seller. To show a company name instead, enroll an **organization**
  (it needs a D-U-N-S number) and sign with that team. Updates keep working after such a switch;
  macOS asks once more for microphone access.
- A certificate belongs to the team, not to one app, so the same one can sign other apps of the
  team. Whoever holds the `.p12` can sign in Burim's name: share it only through a password manager.

**What exists.** A **Developer ID Application** certificate (created 2026-05-09) has been
exported. It was issued under Apple's previous intermediate (G1), so it **expires on
2027-02-01** instead of after 5 years. It is fine for the first test builds, but before the
public launch create a new one with **G2 Sub-CA** (step 2), valid for 5 years. Apps already
downloaded keep working after a certificate expires (the signature is timestamped).

An **Apple Development** certificate was exported too. It is only for testing on registered
devices and **can't** be used for distribution: signed with it, the app is blocked on other Macs
and isn't accepted by the App Store.

**Done in this repo:**
- the CI workflow with signing and notarization;
- the entitlements and hardened runtime;
- the actions pinned to commit SHAs;
- this checklist.

**Burim's steps, in order:**
1. **Developer ID Application certificate, G2 Sub-CA** (step 2). Export it as a `.p12` with a
   **new** password. The earlier export's password was shared in a chat, so don't reuse it.
2. **Notarization credentials** (step 3): an App Store Connect **team** API key (B), or an
   app-specific password for his Apple ID (A).
3. **GitHub secrets** (step 4). The repo `armendzekjiri/brainitive-desktop` is a personal repo, and
   on a personal repo only the owner (Armend) can set secrets. So, one of these:
   - Burim shares the values with Armend through a password manager, and Armend runs the
     `gh secret set` commands;
   - or the repo moves to a GitHub organization where Burim gets the admin role.
4. **Test run:** Actions → **Build** → Run workflow on `main`. Check the result (step 6).
5. **Later, for the Mac App Store** (optional; see "Mac App Store" below).

## Mac App Store (later)

The App Store needs its own items from developer.apple.com, so again only Burim can create them:
- an **App ID** `app.brainitive.desktop`;
- an **Apple Distribution** certificate and a **Mac Installer Distribution** certificate (1 year each);
- a **Mac App Store provisioning profile** for the App ID.

In App Store Connect, an Admin can do the rest: create the app record, write the metadata, add
screenshots, manage TestFlight, upload builds and submit them for review. The App Store build
itself (sandbox entitlements, no self-updater, review risks such as hiding billing and Sign in
with Apple) is not built yet. It will be its own task.

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
