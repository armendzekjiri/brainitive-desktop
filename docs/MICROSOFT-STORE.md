# Microsoft Store package (MSIX, BRAI-442)

Brainitive for Windows ships two ways:

- **Microsoft Store** (this document): an x64 **MSIX** that CI builds unsigned and Partner Center
  signs. Installs without a SmartScreen warning and is updated by the Store.
- **NSIS installer on GitHub** (`Brainitive_<ver>_x64-setup.exe`): unsigned, with the app's own
  auto-updater ([RELEASING.md](RELEASING.md) § 2 and § 4).

The listing texts (description, screenshots, age rating, certification notes) are in
[MICROSOFT-STORE-LISTING.md](MICROSOFT-STORE-LISTING.md). Package identity, reserved in Partner
Center; the manifest must match it exactly (`src-tauri/tests/msix_manifest.rs` and
`scripts/msix-manifest.test.mjs` check it):

| Field | Value |
| --- | --- |
| Name | `Ndize.al.Brainitive` |
| Publisher | `CN=B7545416-53F6-4408-AB46-55AD313FFC15` |
| PublisherDisplayName | `Ndize.al` |
| Package family name | `Ndize.al.Brainitive_x9qbckpnnsjh8` |
| Store ID | `9N08DHPCF7WB` |
| AUMID (toasts) | `Ndize.al.Brainitive_x9qbckpnnsjh8!Brainitive` |

## What differs from the NSIS build

The Store build is the same app compiled with `--features msstore` and the config overlay
`src-tauri/tauri.msstore.conf.json`. Without the feature, nothing below is active.

| | NSIS build | Store build |
| --- | --- | --- |
| Updates | own updater (GitHub, minisign) | **none**: the plugin isn't registered and the overlay removes `plugins.updater`. "Check for Updates…" (menu, tray), Settings → Updates ("Open Microsoft Store") and the "Please update" screen open `ms-windows-store://pdp/?productid=9N08DHPCF7WB` |
| Signing | unsigned (SmartScreen warns) | signed by the Store |
| Install | per user, `%LOCALAPPDATA%\Programs\Brainitive` | per user, `C:\Program Files\WindowsApps\…` (read-only) |
| Open at login | registry Run key (autostart plugin) | the manifest's startup task `BrainitiveStartup`, switched with the WinRT `StartupTask` API; listed in Task Manager → Startup apps. If the user disabled it there, the app says so and can't turn it back on (Windows rule) |
| `brainitive://` | registered by the installer (HKCU) | declared in the manifest (`windows.protocol`); Windows starts `Brainitive.exe "<uri>"`, the single-instance plugin hands it to the running app |
| Notifications | the plugin, AUMID `app.brainitive.desktop` (the installer's Start-menu shortcut) | toasts from the package identity (AUMID above); a click while on screen works as before, a click in the notification centre starts / focuses the app |
| Data | `%APPDATA%` / `%LOCALAPPDATA%\app.brainitive.desktop` | the same paths as the app sees them; Windows stores new files in `%LOCALAPPDATA%\Packages\Ndize.al.Brainitive_x9qbckpnnsjh8\LocalCache\{Local,Roaming}\…` and deletes them on uninstall. Help → Open Logs Folder opens that copy |
| Sign-in tokens | Credential Manager | Credential Manager (not virtualized: **stays after uninstall** unless the user signed out) |
| User agent | ` BrainitiveDesktop/<ver>` | ` BrainitiveDesktop/<ver> (MSStore)`; API client `(windows; MSStore)` |
| WebView2 | bootstrapper in the installer | not bundled (no bootstrapper in an MSIX): part of Windows 11, installed with Edge on Windows 10 |
| Architectures | x64 | x64 (arm64 later) |

Unchanged: global shortcuts, tray and panel, voice capture (the package declares `microphone`;
the model downloads into the package's data copy), downloads (the folder the user picks or
Downloads), the offline copy, `private_fs` (the user profile's ACLs cover the package copy).

## How the package is built

**In CI** (`.github/workflows/build.yml`, job **`msstore`** "Microsoft Store (MSIX)",
`windows-latest`): on every `v*` tag without a pre-release part (`v1.0.0`, not `v1.0.0-beta.1`) and
on manual runs (Actions → Build → Run workflow). No secrets.

1. `npm ci`, `npm run build`, `node --test scripts/` (manifest + logo tests),
   `cargo test --features msstore`, `cargo clippy --all-targets --features msstore -- -D warnings`.
2. `scripts/build-msix.ps1 -SignForTest` (below).
3. WACK, if the runner image has it (below).
4. Artifacts:

   | Artifact | Content | Goes to |
   | --- | --- | --- |
   | `installers-msstore` | `Brainitive_<X.Y.Z>_x64.msix` (**unsigned**) | Partner Center; also attached to the draft release (with `SHA256SUMS`) |
   | `msstore-sideload-test` | `Brainitive_<X.Y.Z>_x64_sideload-test.msix` + `Brainitive_sideload-test.cer` | your test PC only |
   | `msstore-wack-report` | `wack-report.xml` (when WACK ran) | reading |

   The draft release waits for this job; on a pre-release tag it is skipped and the release goes
   ahead without an `.msix`.

**`scripts/build-msix.ps1`** (Windows with the Windows 10/11 SDK, Node 20, Rust):

```powershell
pwsh scripts/build-msix.ps1                # build + package, unsigned
pwsh scripts/build-msix.ps1 -SignForTest   # + a self-signed sideload copy and its .cer
pwsh scripts/build-msix.ps1 -SkipBuild     # package the exe a previous msstore build left
```

It runs `npx tauri build --no-bundle --config src-tauri/tauri.msstore.conf.json --features msstore`,
then lays out `src-tauri/target/msix/package/`:

- `Brainitive.exe`: the release exe (the frontend is embedded; the VC++ runtime is linked
  statically by tauri-build, WebView2's loader statically by webview2-com, so there are no DLLs);
- `AppxManifest.xml`: `src-tauri/msix/AppxManifest.xml.tmpl` rendered by
  `node scripts/msix-manifest.mjs` (version = `tauri.conf.json`'s `X.Y.Z` + `.0`; a pre-release
  version is refused);
- `Assets\`: `node scripts/msix-assets.mjs src-tauri/icons/icon.png`: StoreLogo, Square44x44 (scale
  100/200/400, targetsize 16/24/32/48/256 plain and `altform-unplated`), Square71x71,
  Square150x150, Wide310x150, Square310x310, transparent backgrounds;
- `resources.pri`: `makepri createconfig` + `makepri new`, so Windows picks the right size and the
  unplated taskbar icons for the manifest's plain `Assets\Square44x44Logo.png`;

and packs it with `makeappx pack /d … /p src-tauri/target/msix/Brainitive_<X.Y.Z>_x64.msix`.
`makeappx` validates the manifest against the schema and fails on any error.

With `-SignForTest` it creates a code-signing certificate on the fly whose subject is the Store
publisher (`CN=B7545416-…`; Windows only installs a package signed by its `Identity/@Publisher`),
signs a **copy** (`…_sideload-test.msix`) with `signtool`, exports the public `.cer`, and deletes
the certificate and its private key from the store. Nothing of it is committed or reused.

The manifest: `Windows.Desktop` 10.0.17763.0 (1809, the WebView2 baseline) to 10.0.26100.0
(24H2), `EntryPoint="Windows.FullTrustApplication"` with `runFullTrust`, `internetClient`,
`microphone`, the `brainitive` protocol (`Parameters="%1"`) and the startup task (declared off). No
package dependency: WebView2 is not a framework package and VC++ is static.

## Sideload test

On a test PC (Windows 10 1809+ or 11), with the `msstore-sideload-test` artifact:

1. Trust the throwaway certificate (admin PowerShell):
   ```powershell
   Import-Certificate -FilePath .\Brainitive_sideload-test.cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople
   ```
2. Install: double-click `Brainitive_<X.Y.Z>_x64_sideload-test.msix` (App Installer), or
   `Add-AppxPackage .\Brainitive_<X.Y.Z>_x64_sideload-test.msix`.
3. Run `docs/QA-CHECKLIST.md` § 20.
4. Clean up: Settings → Apps → Brainitive → Uninstall (or `Get-AppxPackage Ndize.al.Brainitive |
   Remove-AppxPackage`); remove the certificate again:
   ```powershell
   Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object Subject -eq 'CN=B7545416-53F6-4408-AB46-55AD313FFC15' | Remove-Item
   ```
   Credential Manager → Windows Credentials keeps the `app.brainitive.desktop` sign-ins if you
   didn't sign out: delete them there.

Without any certificate (developer machine): turn on Settings → System → For developers →
Developer Mode, build with `pwsh scripts/build-msix.ps1`, and register the loose folder:
`Add-AppxPackage -Register src-tauri\target\msix\package\AppxManifest.xml`. It runs with the
package identity (startup task, toasts, protocol) like an installed package.

The sideload copy and the Store version share the identity: uninstall the sideload copy before
installing from the Store.

## WACK (Windows App Certification Kit)

The Store runs the same checks in certification; run them before each submission.

- **CI**: the `msstore` job runs `appcert.exe` on the sideload copy when the runner image has the
  kit (`C:\Program Files (x86)\Windows Kits\10\App Certification Kit\appcert.exe`). The step is
  informational (it can't fail the job); the overall result is printed and the report uploaded as
  `msstore-wack-report`. If the image has no kit, the step says so: run it by hand.
- **By hand** (admin PowerShell, Windows SDK installed, the sideload copy installed or not):
  ```powershell
  $wack = "${env:ProgramFiles(x86)}\Windows Kits\10\App Certification Kit\appcert.exe"
  & $wack reset
  & $wack test -appxpackagepath .\Brainitive_<X.Y.Z>_x64_sideload-test.msix -reportoutputpath .\wack-report.xml
  ```
  Or the GUI: Start → "Windows App Cert Kit" → Validate Store App → pick the package. Expect PASS;
  `runFullTrust` and the protocol are normal for a desktop app. Fix FAILs; note warnings you keep
  in the submission notes.

## Submitting in Partner Center

Partner Center → Apps and games → **Brainitive** (Store ID `9N08DHPCF7WB`):

1. **Start your submission** (first time) or **Update** on the last published submission.
2. **Packages**: drag in the **unsigned** `Brainitive_<X.Y.Z>_x64.msix` from the `installers-msstore`
   artifact (or the draft release). Never the `_sideload-test` copy: its signature is ours, not
   Microsoft's, and the Store rejects it. Partner Center checks the identity (Name, Publisher,
   version); a mismatch is shown right there.
   - Device family availability: **Windows 10/11 Desktop** only.
   - Remove the previous package only if you want to stop offering it; the Store serves the highest
     version per device anyway.
3. **Submission options → Notes for certification**: the text in
   MICROSOFT-STORE-LISTING.md § 6 (test account, `runFullTrust` justification). Partner Center asks
   for the restricted-capability reason in the same section.
4. Check Pricing, Properties, Age ratings and the Store listing (MICROSOFT-STORE-LISTING.md § 1–5;
   unchanged between versions unless "What's new" changes).
5. **Submit to the Store**. Certification usually takes up to three business days; Partner Center
   mails the result. Publishing happens as set in Pricing and availability.

## Versions

- Every submission needs a **higher** package version than the one before; `X.Y.Z.0` comes from
  `tauri.conf.json`, so bump with `node scripts/bump-version.mjs X.Y.Z` as for any release
  (RELEASING.md § 5). The fourth part stays `0` (the Store reserves it).
- Pre-release tags (`vX.Y.Z-beta.N`) build no MSIX: an MSIX version has no pre-release part. Store
  betas, if ever needed, are a Partner Center **package flight** of a normal version.
- The NSIS and Store builds of the same tag have the same app version; a PC should have only one
  of them (uninstall the NSIS build before installing from the Store: both read the same data
  folders and Credential Manager entries, so sign-ins carry over; on Windows 10 1903+ the Store
  build keeps reading files the NSIS build left in AppData).

## Troubleshooting

- **`makeappx` error `0x80080204` / schema errors**: the message names the line in the rendered
  `src-tauri/target/msix/package/AppxManifest.xml`. Check the template; locally
  `node scripts/msix-manifest.mjs out.xml && xmllint --noout out.xml` catches XML mistakes, and
  `node --test scripts/` the identity.
- **`makepri` / `makeappx` / `signtool` not found**: they come with the Windows 10/11 SDK
  (`Windows Kits\10\bin\<ver>\x64`); the script takes the newest version folder that has the tool.
- **"App didn't start" on a Windows 10 PC without WebView2**: install the WebView2 Runtime
  (Evergreen) from Microsoft; Store installs on current Windows 10 already have it via Edge.
- **Toasts missing**: Settings → System → Notifications → Brainitive must be on; the log line
  `notification failed: …` names the WinRT error.
- **Open at login doesn't stick**: Task Manager → Startup apps decides; the log has `startup task
  BrainitiveStartup: …` on errors.
