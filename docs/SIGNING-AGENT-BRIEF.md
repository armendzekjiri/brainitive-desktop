# Brief for the signing agent (Burim's Mac)

You are an AI coding agent on **Burim Bilali's Mac**. Burim holds the Apple Developer team
`65NCF3DHKS` (an individual membership). Your job: produce a **signed and notarized macOS build**
of Brainitive Desktop, and help Burim with the Apple steps only he can do. You are not here to
change the app.

Read first: [`APPLE-SIGNING.md`](APPLE-SIGNING.md) (the steps and the "Status and handover"
section), then [`RELEASING.md`](RELEASING.md) § 1 and § 4. Skim [`../AGENTS.md`](../AGENTS.md)
for the repo rules.

## Hard rules
- **Never commit or paste secrets:** no `.p12`, `.cer`, `.p8`, passwords, app-specific
  passwords, API keys or the updater private key. Not in the repo, not in issues or PRs, not in
  chat logs. Put them in environment variables for one shell session, or in Burim's keychain.
  `.gitignore` already blocks `.env*`. Before every commit, check `git status` shows no key files.
- **Don't change the app:** no code, no bundle identifier (`app.brainitive.desktop`), no version.
  - Change `src-tauri/entitlements.plist` only if a signed build crashes at launch, and then add
    only the entitlement the crash log names.
  - If something in the repo blocks signing, write it down for Armend instead of working around
    it.
- **Don't push to `main`.** The repo `armendzekjiri/brainitive-desktop` belongs to Armend.
  - If Burim is a collaborator: use a branch + pull request.
  - If not: send the change as a patch (`git format-patch`).
  - Only the repo owner can set GitHub Actions secrets.
- **Ask Burim before any action at Apple:** creating or revoking certificates, keys or profiles,
  or accepting agreements. Revoking a Developer ID certificate breaks every app signed with it.

## Step by step
1. **Check the Mac:**
   - Xcode command line tools (`xcode-select -p`);
   - Node 20+ (`node -v`);
   - Rust (`rustup show`), with `rustup target add aarch64-apple-darwin x86_64-apple-darwin`;
   - CMake (`brew install cmake`; whisper.cpp needs it for voice capture).
2. **Clone and build unsigned first,** so build problems and signing problems stay separate:
   ```bash
   git clone https://github.com/armendzekjiri/brainitive-desktop.git && cd brainitive-desktop
   npm ci
   (cd src-tauri && cargo test)
   npx tauri build --target universal-apple-darwin --bundles app,dmg
   ```
   If the x86_64 half fails (whisper.cpp), build `aarch64-apple-darwin` and `x86_64-apple-darwin`
   separately and report it to Armend (`RELEASING.md` § 1).
3. **The certificate.**
   - Run `security find-identity -v -p codesigning`. You need a line
     `Developer ID Application: Burim Bilali (65NCF3DHKS)`.
   - The one that exists today expires on **2027-02-01** (old G1 intermediate). For the public
     release, Burim creates a new one with **G2 Sub-CA** (`APPLE-SIGNING.md` step 2), valid 5 years.
   - "Apple Development" certificates can't be used; they only work on test devices.
4. **Notarization credentials.** Pick one (`APPLE-SIGNING.md` step 3):
   - an App Store Connect **team** API key (Key ID, Issuer ID, `.p8`), the recommended way;
   - or Burim's Apple ID with an app-specific password.
5. **The signed build** — build unsigned, then sign what was built (the build never gets the
   signing credentials; this is how CI does it too, `RELEASING.md` § 1):
   ```bash
   # 5a. unsigned build: no APPLE_* variable may be set in this shell
   unset APPLE_SIGNING_IDENTITY APPLE_CERTIFICATE APPLE_ID APPLE_PASSWORD APPLE_API_KEY APPLE_API_KEY_PATH
   npx tauri build --target universal-apple-darwin --bundles app,dmg
   # 5b. sign + notarize + staple
   export APPLE_SIGNING_IDENTITY="Developer ID Application: Burim Bilali (65NCF3DHKS)"
   # option B (API key):
   export APPLE_API_KEY=<Key ID> APPLE_API_ISSUER=<Issuer ID> APPLE_API_KEY_PATH=<path to .p8>
   # or option A (Apple ID):
   # export APPLE_ID=<e-mail> APPLE_PASSWORD=<app-specific password> APPLE_TEAM_ID=65NCF3DHKS
   B=src-tauri/target/universal-apple-darwin/release/bundle
   scripts/macos-sign.sh "$B/macos/Brainitive.app" "$B"/dmg/Brainitive_*_universal.dmg
   ```
   `scripts/macos-sign.sh` signs the `.app` with the hardened runtime and
   `src-tauri/entitlements.plist`, replaces the `.dmg` with a signed one made from the signed app,
   notarizes it (`notarytool --wait`; it stops unless Apple answers `Accepted`) and staples the
   `.dmg` and the `.app`. Leave `APPLE_CERTIFICATE` unset: the identity comes from Burim's login
   keychain (allow `codesign` when macOS asks). Don't edit the script; report problems to Armend.
6. **Verify** with the commands in `APPLE-SIGNING.md` step 6:
   - `codesign --verify --deep --strict`;
   - `codesign -d --entitlements -` (only `audio-input` + `network.client`);
   - `spctl -a -vv` (`source=Notarized Developer ID`);
   - `xcrun stapler validate`.
   Then open the `.dmg` on a second Mac, or after adding the quarantine flag. Turn on voice
   capture once (Settings → Voice) and check that the microphone prompt appears.
7. **Hand over:**
   - the `.dmg`: through a GitHub draft release if Burim has write access, otherwise a file share;
   - a short report for Armend: the certificate's expiry date, which notarization option, the
     verify output, anything that failed.
   - For automatic builds in GitHub Actions, Burim shares the secret values with Armend through a
     password manager. Only the owner can set them (`APPLE-SIGNING.md` step 4).

## Not your job
- **The auto-updater key** (`TAURI_SIGNING_PRIVATE_KEY`): Armend creates and holds it. Until its
  public key replaces the placeholder in `tauri.conf.json`, the updater stays off in every build.
  That's expected.
- **Windows signing and Linux:** separate (`RELEASING.md` § 2–3).
- **The Mac App Store build:** not ready yet. It will be its own task; see `APPLE-SIGNING.md`,
  "Mac App Store (later)".
