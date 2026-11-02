# Brainitive Desktop: manual QA checklist (BRAI-376)

Covers macOS, Windows and Linux. Tick a box when the step does what the Expected column says. Write
the failing step, the OS and the version in the bug report. `n/a` means the step does not apply on
that OS. Automated tests (`cargo test`, `npm run build`) are not repeated here.

## Before you start

**Build.** Until CI artifacts exist (`.github/workflows/build.yml`: macOS universal `.dmg`, Windows
x64 NSIS installer, Linux `.deb` and AppImage on `v*` tags or a manual run), build from the repo:

- macOS: `npx tauri build --debug --bundles app`, then open
  `src-tauri/target/debug/bundle/macos/Brainitive.app`. Only a bundled app registers the
  `brainitive://` scheme, so do not test deep links with `tauri dev` on macOS.
- Windows and Linux: `npm run tauri dev`. The scheme is registered at runtime.
- Release-only steps (the release profile, the Local and Pre-release buttons that are debug only) say so.
  Use a release build for section 18.

**Test account.** A test account on the server, not a personal one. Section 2 also needs a second
account (for the account switch) and a viewer-role account in a workspace (for "Add task" hidden).

**Servers.**

- Local: `composer run dev` in the app repo, then `http://localhost:8000`. Debug builds show the
  **Local (localhost:8000)** button on the connect screen.
- Pre-release: `https://dev.brainitive.app` (debug builds show **Pre-release (dev.brainitive.app)**).
- Cloud: `https://brainitive.app` (the one-click **Brainitive Cloud** choice).
- Offline checks (section 12) need the server to have run `php artisan migrate` (adds
  `sync_tombstones`) and to be v1.49.0 or later.

**Clean state.** For section 1, start without a `settings.json` in the app data folder
(`app.brainitive.desktop`: macOS `~/Library/Application Support/`, Windows `%APPDATA%`, Linux
`~/.local/share/`), or use a second OS user. Remove the keychain entry for the server when you need
a fresh sign-in.

**Shortcuts.** The table writes the macOS shortcut first. Windows and Linux use Ctrl where macOS uses
⌘, and Ctrl+Alt where macOS uses ⌥. Quick capture is ⇧⌘Space (Ctrl+Shift+Space); the switcher is
⌥⌘K (Ctrl+Alt+K); voice is ⇧⌘V (Ctrl+Shift+V) and only exists while voice capture is on.

**Record.** Note the app version (Help → Copy Diagnostics, section 16), the OS version and the
webview version for each run.

---

## 1. First run: choosing Cloud or self-hosted

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 1.1 Start the app with no `settings.json`. | The connect screen shows **Brainitive Cloud** and **Self-hosted server**. No main window yet. | [ ] | [ ] | [ ] |
| 1.2 Click **Brainitive Cloud**. | The app asks `https://brainitive.app/api/v1/instance` and shows the server name and version before any sign-in button leads anywhere. | [ ] | [ ] | [ ] |
| 1.3 Type `localhost:8000` (debug build) and continue. | Address becomes `http://localhost:8000`; the name and version of the local server are shown. | [ ] | [ ] | [ ] |
| 1.4 Type `brainitive.example.com` (any host you control or a non-existing one). | `https://` is added in the shown address; the check runs against that address. | [ ] | [ ] | [ ] |
| 1.5 Type `https://brainitive.example.com/app/?x=1/` | Path, query and trailing slash are dropped; the address shown is the bare origin. | [ ] | [ ] | [ ] |
| 1.6 Type `http://brainitive.example.com` and continue. | Refused with a message that `http://` is only allowed for localhost and 127.0.0.1. Nothing is saved. | [ ] | [ ] | [ ] |
| 1.7 Type `https://self-signed.badssl.com`. | "Certificate" error with a readable reason (self-signed). No "continue anyway" button; the server is not saved. | [ ] | [ ] | [ ] |
| 1.8 Type `https://expired.badssl.com`, then `https://wrong.host.badssl.com`. | Each shows its own certificate reason (expired, wrong host). Refused each time. | [ ] | [ ] | [ ] |
| 1.9 Type `https://brainitive.app` where the site redirects to `http://` (or enter any https address that redirects to plain http). | Refused: the app never follows a redirect from https to http, including to localhost. | [ ] | [ ] | [ ] |
| 1.10 Enter an address that answers at a different address than typed (a redirect to another https host). | Screen says "You entered X, but the server answers at Y" and asks before using Y. Choosing "use Y" saves Y; cancel saves nothing. | [ ] | [ ] | [ ] |
| 1.11 Enter a server older than v1.49.0 (Cloud before the upgrade, or a self-hosted older build). | "Server too old" style message (the instance endpoint answers 404). Not saved as a working server. | [ ] | [ ] | [ ] |
| 1.12 Enter `https://example.com` (a site that is not Brainitive). | "Not a Brainitive server" with a suggested fix. | [ ] | [ ] | [ ] |
| 1.13 Connect to the local server (`http://localhost:8000`). | The check shows name and version, the server is saved as current and listed under recent servers, and the sign-in screen appears. | [ ] | [ ] | [ ] |
| 1.14 Connect to the Cloud (one click). | Same as 1.13 for Cloud; window title reads "Brainitive — Cloud". | [ ] | [ ] | [ ] |
| 1.15 Connect to a self-hosted host. | Window title reads "Brainitive — <host>" (no scheme, no path). | [ ] | [ ] | [ ] |
| 1.16 Debug build: look at the connect screen. | **Local (localhost:8000)** and **Pre-release (dev.brainitive.app)** buttons are shown. | [ ] | [ ] | [ ] |
| 1.17 Release build: look at the connect screen. | The Local and Pre-release buttons are not shown. | [ ] | [ ] | [ ] |
| 1.18 Add a sixth server. | Recent list keeps the last 5 servers; the sixth replaces the oldest. | [ ] | [ ] | [ ] |
| 1.19 Restart the app. | The last server is remembered and its check runs again on start. | [ ] | [ ] | [ ] |

## 2. Sign-in, sign-out, account switch, forget server

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 2.1 On the sign-in screen click **Sign in**. | The system's default browser opens the server's sign-in page. The app window stays on the sign-in screen. | [ ] | [ ] | [ ] |
| 2.2 Complete sign-in in the browser. | The browser hands back to Brainitive (`brainitive://auth/callback`); the app comes to the front and loads the web app in the main window, signed in. | [ ] | [ ] | [ ] |
| 2.3 Close the browser tab without signing in, then click **Sign in** again. | No error loop; the app waits and a second sign-in works. | [ ] | [ ] | [ ] |
| 2.4 Check where the refresh token lives. | The server has one keychain entry (Keychain Access / Credential Manager / Secret Service). `settings.json` in the app data folder contains no token (search for "refresh" and "bra_": no hit). | [ ] | [ ] | [ ] |
| 2.5 Restart the app while signed in. | Opens straight into the web app, no sign-in again. | [ ] | [ ] | [ ] |
| 2.6 Sign out from the web app's account menu. | Back on the sign-in screen. Keychain entry for that server is gone. Quick capture, switcher and offline windows are closed or empty when reopened. | [ ] | [ ] | [ ] |
| 2.7 Sign in as account A, sign out, sign in as account B. | Account B sees its own My Day and captures; A's offline copy (section 12) is deleted. | [ ] | [ ] | [ ] |
| 2.8 Signed in on server 1. Server → **Switch Server…** → server 2 → sign in there. Switch back to server 1 from the recent list. | Server 1 comes back signed in without a sign-in prompt. Each server keeps its own sign-in. | [ ] | [ ] | [ ] |
| 2.9 Switch Server while extra app windows are open (section 5). | Extra windows close when the server changes. | [ ] | [ ] | [ ] |
| 2.10 Click **Forget** next to a recent server that is not current. | That server disappears from the list; its keychain entry is removed; re-adding it asks for sign-in. | [ ] | [ ] | [ ] |
| 2.11 Forget the server you are signed in to. | Signed out there, data deleted, back on the connect screen. | [ ] | [ ] | [ ] |
| 2.12 Revoke the app's session on the server (log the session out from the web's session list), then wait or reload. | App returns to the sign-in screen. No blank page, no endless refresh spinner. | [ ] | [ ] | [ ] |
| 2.13 Cancel the sign-in in the browser before it completes, or wait past the code's lifetime and sign in again. | The app recovers and signs in on the next attempt. | [ ] | [ ] | [ ] |

## 3. Deep links (`brainitive://`)

Open a link with the OS: macOS Terminal `open "<link>"`; Windows Run dialog or
`start "" "<link>"` in cmd; Linux `xdg-open "<link>"`. Quote the link.

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 3.1 Signed in, open `brainitive://open?url=/my-work`. | The app comes to the front and shows My Work. | [ ] | [ ] | [ ] |
| 3.2 Open `brainitive://open?url=/app/session/x`. | Nothing happens. The log has "deep link to another origin or an auth page ignored" (section 16). | [ ] | [ ] | [ ] |
| 3.3 Open `brainitive://open?url=/%61pp/session/x` (percent-encoded). | Same as 3.2. | [ ] | [ ] | [ ] |
| 3.4 Open `brainitive://open?url=/%2561pp/session/x` (double-encoded). | Same as 3.2. | [ ] | [ ] | [ ] |
| 3.5 Open `brainitive://open?url=/login/google` and `…?url=/logout`, `…?url=/password`, `…?url=/oauth/x`. | None opens; each is ignored. | [ ] | [ ] | [ ] |
| 3.6 Open `brainitive://open?url=//evil.example` and `…?url=https://evil.example/`. | Refused (other origin); the main window does not change. | [ ] | [ ] | [ ] |
| 3.7 Open `brainitive://open?url=/apps` and `…?url=/authors`. | These ordinary pages open (the deny list matches whole path segments). | [ ] | [ ] | [ ] |
| 3.8 Open a deep link while the app is not running. | The app starts; the page opens after the server check and sign-in. | [ ] | [ ] | [ ] |
| 3.9 Open a deep link while the app is running. | No second app window or process; the running app handles the link (single instance). | [ ] | [ ] | [ ] |
| 3.10 Type `brainitive://auth/callback?code=abc&state=wrong` by hand. | Rejected: no sign-in happens, the session is unchanged. | [ ] | [ ] | [ ] |
| 3.11 Windows / Linux, debug build with `tauri dev`: open the first link in 3.1. | The scheme works without installing (registered at runtime). | n/a | [ ] | [ ] |
| 3.12 macOS: open the first link with `tauri dev`, not the bundled app. | Expected to fail (scheme only for the bundled app). Recorded only so nobody reports it as a bug. | [ ] | n/a | n/a |

## 4. Menus and shortcuts

Use **File** on Windows and Linux where macOS shows the app menu (Settings, Quit). Ctrl stands for ⌘.

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 4.1 Open Settings (⌘, / Ctrl+,). | Settings window opens; a second press focuses the same window. | [ ] | [ ] | [ ] |
| 4.2 Press ⌘N (Ctrl+N) on a signed-in page. | The web's new-issue dialog opens. If the web has no hook for it, the web's command palette (⌘.) opens. No blank or error page. | [ ] | [ ] | [ ] |
| 4.3 Press ⌥⌘N (Ctrl+Alt+N). | New doc, same rule as 4.2. | [ ] | [ ] | [ ] |
| 4.4 Press ⇧⌘N (Ctrl+Shift+N). | A new app window with the same page, already signed in (no second sign-in). | [ ] | [ ] | [ ] |
| 4.5 Press ⌘W (Ctrl+W) in the main window. | The window closes; with "keep running in the tray" on the app keeps running. | [ ] | [ ] | [ ] |
| 4.6 Navigate two pages, then press ⌘[ and ⌘] (Ctrl). | Back and Forward work; a menu item greys out when there is nothing to go to. | [ ] | [ ] | [ ] |
| 4.7 Press ⌘R (Ctrl+R). | The current page reloads. | [ ] | [ ] | [ ] |
| 4.8 Press ⌘= , ⌘- and ⌘0 (Ctrl). | Zoom in, out, reset. The level is remembered after restart. | [ ] | [ ] | [ ] |
| 4.9 Press ⌘K (Ctrl+K) on a page. | The web's search opens. On Windows with WebView2 focused, the accelerator must still reach the app (known risk). | [ ] | [ ] | [ ] |
| 4.10 Go menu: Search, Dashboard, My Work, Captures, Notifications, Account Settings. | Each item opens the matching page in the main window. | [ ] | [ ] | [ ] |
| 4.11 Edit menu: Copy, Paste, Select All in a text field on a page. | Standard editing works (⌘C / ⌘V). Clipboard access from the page works. | [ ] | [ ] | [ ] |
| 4.12 Rebind quick capture in Settings to another ⌘/Ctrl+Shift+letter combo. | The new combo opens quick capture; the old one no longer does. | [ ] | [ ] | [ ] |
| 4.13 Try to bind a reserved combination (for example Ctrl+C or ⌘Q). | Refused with a message; the old binding stays. | [ ] | [ ] | [ ] |
| 4.14 Settings → Shortcuts → Direct shortcuts: turn on the quick switcher (**Default**), then bind quick capture to the same combo. | Refused, naming the other one ("Quick switcher already uses …"): the shortcuts must differ. | [ ] | [ ] | [ ] |
| 4.15 Set quick capture to **off** in Settings. | The shortcut no longer opens quick capture; the menu item still does. | [ ] | [ ] | [ ] |
| 4.16 Wayland session: press the quick capture shortcut. | No global shortcut is registered (known gap). Menu, tray and the quick capture item still work. | n/a | n/a | [ ] |
| 4.17 Help menu: Copy Diagnostics, Open Logs Folder, Check for Updates (Help on Windows and Linux; app menu on macOS). | Items exist; Copy Diagnostics and Open Logs Folder work (section 16); Check for Updates is greyed out in this build (section 17). | [ ] | [ ] | [ ] |

## 4A. Brainitive key (BRAI-440)

One global shortcut, then one letter. Defaults: **⌃⌘K** (macOS), **Ctrl+Alt+K** (Windows, Linux);
letters **C** quick capture, **T** new task (section 4C), **K** quick switcher, **V** voice capture (hold), **O** open Brainitive.
Settings → Shortcuts holds the key, the letters and the optional direct shortcuts.

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 4A.1 Fresh install: open Settings → Shortcuts. | Brainitive key ⌃⌘K / Ctrl+Alt+K; letters C, T, K, V, O; Direct shortcuts: quick capture ⇧⌘Space / Ctrl+Shift+Space, New task ⌃⌘T / Ctrl+Alt+T (Linux **Off**), quick switcher **Off**, voice capture **Off**. | [ ] | [ ] | [ ] |
| 4A.2 In another app press the Brainitive key. | A small centred panel takes the focus and lists the five letters with their actions (none cut off); the key is shown in its title. | [ ] | [ ] | [ ] |
| 4A.3 Press the key, then **C**. | Quick capture opens with the text field focused; the panel is gone. Esc in quick capture returns to the app you came from. | [ ] | [ ] | [ ] |
| 4A.4 Press the key, then **K**. Then press the key and **K** while still holding ⌃⌘ (Ctrl+Alt). | Both open the quick switcher (the key pressed again counts as its letter K). | [ ] | [ ] | [ ] |
| 4A.5 Main window hidden or behind another app: press the key, then **O**. | The main Brainitive window comes to the front. | [ ] | [ ] | [ ] |
| 4A.6 Press the key, then wait. | The panel stays open; Esc or a click elsewhere closes it and the focus goes back to the app you were in. | [ ] | [ ] | [ ] |
| 4A.7 Press the key, then Esc; again, then click another app. | Esc closes it; clicking elsewhere closes it. Nothing runs. | [ ] | [ ] | [ ] |
| 4A.8 Press the key, then a letter that has no action (for example **X**). | The panel says "X does nothing here." and stays a moment; nothing runs. | [ ] | [ ] | [ ] |
| 4A.9 Voice capture **off**: press the key. | **V** is greyed out with "Off. Turn it on in Settings → Voice capture."; pressing V shows that message, the microphone stays closed. | [ ] | [ ] | [ ] |
| 4A.10 Voice capture on (model downloaded): press the key, then **hold V**, speak, let go. | Key-down starts the recording (indicator with meter and timer, hint "Let go of V to finish"), the release stops it, the text opens in quick capture. | [ ] | [ ] | [ ] |
| 4A.11 Same as 4A.10, but tap V (under half a second). | Toggle mode: the recording goes on, the hint says ↩ to finish; ↩ / Done ends it. | [ ] | [ ] | [ ] |
| 4A.12 First voice use after an app start (the indicator window is built then): hold V for 2 s, let go. | The release is still seen (by the panel or the indicator) and the recording stops. If it is missed, record it (fallback: Done / ↩ / 2-minute cut-off). | [ ] | [ ] | [ ] |
| 4A.13 Settings → Shortcuts → Brainitive key → **Change**, press ⌃⌥⌘B (Ctrl+Alt+Shift+B). | The new key opens the panel, the old one no longer does; File menu and tray show the new key next to **Brainitive Key…**. | [ ] | [ ] | [ ] |
| 4A.14 Record the Brainitive key on quick capture's keys (⇧⌘Space / Ctrl+Shift+Space). | Refused inline: "Quick capture already uses ⇧⌘Space."; the old key stays. | [ ] | [ ] | [ ] |
| 4A.15 Record the Brainitive key on keys another app has registered globally (for example another launcher's shortcut). | Refused inline with "… is already used by another app or the system"; the previous key keeps working. | [ ] | [ ] | [ ] |
| 4A.16 Start another app that registers ⌃⌘K / Ctrl+Alt+K first, then start Brainitive. | Settings shows the refusal at the top of Shortcuts; quick capture's direct shortcut still works (one refused key no longer blocks the others). | [ ] | [ ] | [ ] |
| 4A.17 Letters: change quick capture's letter to **n**; then try **v**, **1**, **ä**. | N is taken (shown upper case, the panel lists it); "V is already used for Voice capture."; "Use one letter, A–Z." for 1 and ä; leaving the field shows the letter in use. **Default** brings back C. | [ ] | [ ] | [ ] |
| 4A.18 Brainitive key **Off**. | The key does nothing in other apps; the letters are greyed in Settings; File → **Brainitive Key…** and the tray item still open the panel (the tray item is hidden while the key is off). | [ ] | [ ] | [ ] |
| 4A.19 Direct shortcuts: turn on the quick switcher (**Default**: ⌥⌘K / Ctrl+Alt+Shift+K) and voice capture (⇧⌘V / Ctrl+Shift+V). | Both work as before alongside the Brainitive key; the File menu and tray show them. | [ ] | [ ] | [ ] |
| 4A.20 Upgrade from a build before BRAI-440 with untouched shortcuts. | After the update: Brainitive key on; quick switcher and voice direct shortcuts **Off**; quick capture unchanged. | [ ] | [ ] | [ ] |
| 4A.21 Upgrade from a build before BRAI-440 where you had **recorded** a quick switcher or voice shortcut (or set one Off). | Your recorded keys and your Off stay as they were. If you had recorded ⌃⌘K / Ctrl+Alt+K yourself, the Brainitive key starts Off (no clash). | [ ] | [ ] | [ ] |
| 4A.22 German / Swiss / French / Polish layout: type `@`, `€`, `{`, `[` and `µ` with AltGr in another app while Brainitive runs. | All characters type normally. Czech / Slovak / Hungarian layouts: AltGr+K types `ł` there, so Ctrl+Alt+K is taken; record it and change the key in Settings. | n/a | [ ] | [ ] |
| 4A.23 Wayland session: press the Brainitive key. | No global shortcut (known gap, like 4.16). File → **Brainitive Key…** and the tray still open the panel. | n/a | n/a | [ ] |

## 4B. Title bars (macOS 26)

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 4B.1 Settings: scroll down. | "GENERAL" and everything else stays below the title bar; the title bar is the page's colour (white / dark), no text behind "Settings". | [ ] | n/a | n/a |
| 4B.2 Offline view: scroll the lists. | Nothing scrolls under the title bar; the strip matches the status bar (light and dark). | [ ] | n/a | n/a |
| 4B.3 Switch light ↔ dark with Settings / Offline open. | The title bar strip follows the page. | [ ] | n/a | n/a |
| 4B.4 Main window and New Window: scroll a long web page. | The page starts below the title bar; nothing shows behind the window title. Fullscreen still works. | [ ] | n/a | n/a |
| 4B.5 Settings / Offline / main window on Windows and Linux. | Normal system title bars, unchanged. | n/a | [ ] | [ ] |

## 4C. New task

Defaults: **⌃⌘T** (macOS), **Ctrl+Alt+T** (Windows), **Off** (Linux); Brainitive key + **T**; tray
**New Task…**; panel footer **New Task**. Use an account with two workspaces, a project with two
boards and one with a single board.

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 4C.1 In another app press ⌃⌘T / Ctrl+Alt+T (Linux: set a key in Settings first). | A centred window "New task in <server>" opens with the title field focused; project and board are filled in. | [ ] | [ ] | [ ] |
| 4C.2 Press the Brainitive key, then **T**; tray → **New Task…**; panel footer → **New Task**. | Each opens the same window. | [ ] | [ ] | [ ] |
| 4C.3 With the window open and text typed, press the shortcut again; click another app, press it again. | The window comes to the front with the text still there (no toggle). | [ ] | [ ] | [ ] |
| 4C.4 Project field: click it, type part of a name or a key, ↑/↓, ↩. | The list filters, grouped by workspace when there are several; ↩ picks; Esc closes only the list. Projects without boards are not listed. | [ ] | [ ] | [ ] |
| 4C.5 Pick a project with one board, then one with two. | One board: no Board field; two: a Board select (the last-used board preselected). | [ ] | [ ] | [ ] |
| 4C.6 Description: click **Description**, type Markdown (`- item`, `**bold**`). | The field opens and the window grows; the issue shows the formatting on the web. | [ ] | [ ] | [ ] |
| 4C.7 Priority chips and Type. | None + the project's priorities (arrow keys move); Type only for projects with types, the last used (else the default) preselected. | [ ] | [ ] | [ ] |
| 4C.8 Type a title, ⌘↩ / Ctrl+↩. | "WEB-12 created" with **Open** and **New another**; the issue is on the chosen board with the priority and type. | [ ] | [ ] | [ ] |
| 4C.9 **Open**. | The issue opens in the main window; the New task window closes. | [ ] | [ ] | [ ] |
| 4C.10 **New another**; close and reopen the window; restart the app and reopen. | A fresh form with the same project / board / type; after the restart still the last project of this server. Another server remembers its own. | [ ] | [ ] | [ ] |
| 4C.11 Empty title, ⌘↩. | "Give the task a title." under the title; nothing is sent. | [ ] | [ ] | [ ] |
| 4C.12 A board where you are a viewer; a board deleted on the web while the window is open; a workspace at its storage limit. | "You can't create issues on this board." / "This board is gone …" (the list reloads) / the server's limit message — inline. | [ ] | [ ] | [ ] |
| 4C.13 Network off, title + description, ⌘↩. | "You're offline …" with **Save to Inbox instead**; clicking it says the capture is kept and sent later; it appears in the web inbox (title, blank line, description) when back online. | [ ] | [ ] | [ ] |
| 4C.14 Slow network: ⌘↩ twice quickly; or turn the network off right after ⌘↩ and retry the same text when back. | One issue only (the retry reuses the request id). | [ ] | [ ] | [ ] |
| 4C.15 Signed out / no server chosen. | "Sign in to Brainitive" / "Connect to Brainitive" with **Open Brainitive**. | [ ] | [ ] | [ ] |
| 4C.16 Sign out while the window holds text; sign in as another account. | The window is gone; reopened it shows only the new account's projects. | [ ] | [ ] | [ ] |
| 4C.17 Light and dark mode. | Fields, chips, list and the created state follow the system. | [ ] | [ ] | [ ] |
| 4C.18 Settings → Shortcuts → New task: **Change** to another key, **Default**, **Off**; letter T → N. | The new key works, the tray shows it; Default gives ⌃⌘T / Ctrl+Alt+T (Linux: Off); Off frees the key; ⌃⌘K N opens New task. | [ ] | [ ] | [ ] |
| 4C.19 Linux: record Ctrl+Alt+T. | Refused ("That shortcut belongs to the system."); the terminal still opens with Ctrl+Alt+T. | n/a | n/a | [ ] |
| 4C.20 Update from a build without New task, where a shortcut of yours already used ⌃⌘T / Ctrl+Alt+T (or the letter T). | That shortcut keeps its keys, New task starts **Off**; your letter stays and New task gets **N**. Without a clash New task gets ⌃⌘T / Ctrl+Alt+T and T. | [ ] | [ ] | [ ] |
| 4C.21 Windows, Polish / Romanian / Czech layouts: type the AltGr+T character (if the layout has one) in another app. | Record whether Ctrl+Alt+T takes it; if so, change New task's key in Settings. | n/a | [ ] | n/a |

## 5. Extra windows

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 5.1 Open a page with a list, then ⇧⌘N (Ctrl+Shift+N). | A second window shows the same page; the two windows are independent. | [ ] | [ ] | [ ] |
| 5.2 In the extra window, sign-in state. | Signed in already; no sign-in prompt. | [ ] | [ ] | [ ] |
| 5.3 In a page, click a link that opens a new tab on the same server (for example an attachment). | Opens in an app window, not in the browser. | [ ] | [ ] | [ ] |
| 5.4 In a page, click a link to another website that opens a new tab. | Opens in the default browser. The app windows do not change. | [ ] | [ ] | [ ] |
| 5.5 Change zoom in one window (⌘=). | Every app window zooms. | [ ] | [ ] | [ ] |
| 5.6 Move and resize the main window and an extra window, quit, start again. | Both come back at their last positions and sizes. | [ ] | [ ] | [ ] |
| 5.7 Close the main window while an extra window is open. | Record what happens to the extra window (stays or closes). Reopen from the tray or the Dock/taskbar and check the windows are still usable. | [ ] | [ ] | [ ] |
| 5.8 Sign out with extra windows open. | Extra windows close. | [ ] | [ ] | [ ] |
| 5.9 Start a second copy of the app. | Only one process; the existing window comes to the front. | [ ] | [ ] | [ ] |

## 6. Quick capture (incl. offline queue)

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 6.1 Press ⇧⌘Space (Ctrl+Shift+Space) while another app is focused. | A small window appears on top, without a title bar; the text field has focus. | [ ] | [ ] | [ ] |
| 6.2 Type "QA capture 1" and press ⌘↩ (Ctrl+Enter). | The window hides. In the web app's Captures the item appears in the inbox, with source "Brainitive Desktop". | [ ] | [ ] | [ ] |
| 6.3 Open quick capture, type text, press Esc. | Window hides; nothing is saved. Reopen: the text is still there (until sign-out). | [ ] | [ ] | [ ] |
| 6.4 Open quick capture, type text, click another app. | Window hides on blur. | [ ] | [ ] | [ ] |
| 6.5 Fill the link field with `https://example.com` and a title, save. | The capture in the web inbox holds the text and the link. | [ ] | [ ] | [ ] |
| 6.6 Change the type in the type selector and save. | The capture arrives with the chosen type. | [ ] | [ ] | [ ] |
| 6.7 Click save with an empty text field. | Nothing is sent; the field or button signals it is empty. | [ ] | [ ] | [ ] |
| 6.8 Turn the network off (Wi-Fi off, cable out, or block the server host). Capture "QA offline 1". | Capture reports it is waiting on this computer; it is not lost. | [ ] | [ ] | [ ] |
| 6.9 While offline, capture 2 more items. Then restore the network. | The waiting captures are sent in order, each once, without waiting for a restart. The web inbox shows each item exactly once (check the count). | [ ] | [ ] | [ ] |
| 6.10 While the queue holds items, check the queue file in the app data folder (`capture-queue.json`). | File exists; its permissions are 0600 (macOS/Linux: `ls -l`). After the queue empties the file is gone. Windows: check only that it exists and is in the profile folder. | [ ] | [ ] | [ ] |
| 6.11 Quit the app while captures wait, start it again with the network on. | The waiting captures are sent after start, once. | [ ] | [ ] | [ ] |
| 6.12 Quick capture is open with text; sign out (section 2). | Text is cleared; the window is closed or empty when reopened. | [ ] | [ ] | [ ] |
| 6.13 Quick capture mic button (only when voice is on, section 13). | Starts a voice recording, same as the voice shortcut. | [ ] | [ ] | [ ] |
| 6.14 Quick capture when the server is reachable but refuses a capture (for example a rejected field, if you can produce one). | The item is dropped with a notification, not retried forever. Skip if no rejection can be produced; record "not reproduced". | [ ] | [ ] | [ ] |

## 7. Quick switcher

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 7.1 Press ⌃⌘K, then K (Ctrl+Alt+K, then K); with the direct shortcut turned on (4A.19) also ⌥⌘K (Ctrl+Alt+Shift+K). | A Spotlight-like window opens with the search field focused. | [ ] | [ ] | [ ] |
| 7.2 Type two letters of an issue title. | Results appear after a short pause (debounced), at most 12. | [ ] | [ ] | [ ] |
| 7.3 Use the arrow keys, press ↩ on an issue. | The main window comes to the front and opens that issue. The switcher hides. | [ ] | [ ] | [ ] |
| 7.4 Open a doc or plan from the results and press ↩. | Same as 7.3 for the doc or plan. | [ ] | [ ] | [ ] |
| 7.5 Select a snippet and press ⌘C (Ctrl+C), or use its copy button. | Snippet content is on the clipboard: paste in another app to check. | [ ] | [ ] | [ ] |
| 7.6 Select an issue and press ⌘T (Ctrl+T). | A timer starts on that issue; the tray shows the running timer (section 8). | [ ] | [ ] | [ ] |
| 7.7 Press Esc, or click another app. | Switcher hides. | [ ] | [ ] | [ ] |
| 7.8 Sign out, then open the switcher (⌃⌘K K). | No results from the previous account (sign-out clears them). | [ ] | [ ] | [ ] |
| 7.9 Switcher with the network off. | Shows a clear message; does not hang or show stale results as live. | [ ] | [ ] | [ ] |

## 8. Tray / menu bar

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 8.1 Look for the icon after start. | macOS: menu bar; Windows: notification area (may sit under the ^ overflow, pin it); Linux: system tray (needs libayatana-appindicator). | [ ] | [ ] | [ ] |
| 8.2 Right-click the icon (macOS also ⌃-click). | Server and account line; Brainitive Key… (with the key); Quick Capture; Quick Switcher; My Day; running timer entry; unread notifications; Open Brainitive; Settings; Check for Updates (greyed); Quit. | [ ] | [ ] | [ ] |
| 8.3 Linux: click the icon. | Shows the menu (AppIndicator has no click events); its first entry under the server line is **Open Panel…** (section 8A). | n/a | n/a | [ ] |
| 8.4 My Day: tick an item in the tray menu. | The item is done in the web My Day within a minute. The tray entry shows the change. | [ ] | [ ] | [ ] |
| 8.5 Start a timer in the web on an issue. | Tray shows the running timer with its time, updating. | [ ] | [ ] | [ ] |
| 8.6 Timer in tray: choose **Stop and log**. | Timer stops; the web shows the logged time. | [ ] | [ ] | [ ] |
| 8.7 Start a timer, choose **Discard** in the tray. | Timer stops; nothing is logged in the web. | [ ] | [ ] | [ ] |
| 8.8 Unread notifications: create one unread notification on the server. | Tray count rises within about a minute. macOS also shows the count next to the menu-bar icon. | [ ] | [ ] | [ ] |
| 8.9 Click the unread entry in the tray. | The Notifications page opens in the main window. | [ ] | [ ] | [ ] |
| 8.10 Disconnect the network. | Tray line "Offline — changes will sync" when changes wait; macOS title shows "(offline)"; the tooltip changes. | [ ] | [ ] | [ ] |
| 8.11 With changes waiting offline, tray → **Sync Now**. | Sync runs; the tray line updates once the changes are sent. | [ ] | [ ] | [ ] |
| 8.12 Tray → **Open Offline View…**. | The offline window opens (section 12). | [ ] | [ ] | [ ] |
| 8.13 Close the main window with the window's close button. | The window hides; the app keeps running in the tray. Clicking the tray item or the Dock icon brings the window back. | [ ] | [ ] | [ ] |
| 8.14 Turn "keep running in the tray" off in Settings, then close the main window. | The app quits (close behaves as a normal quit). | [ ] | [ ] | [ ] |
| 8.15 Tray → **Quit**. | App exits; the tray or menu-bar icon disappears. | [ ] | [ ] | [ ] |
| 8.16 Linux without libayatana-appindicator installed. | App still starts and the main window works; no tray icon (known gap). Record the log line. | n/a | n/a | [ ] |
| 8.17 Change the system theme (macOS light and dark menu bar). | The menu-bar icon stays readable. | [ ] | n/a | n/a |

## 8A. Menu-bar panel

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 8A.1 Left-click the tray / menu-bar icon. | A panel (about 360 pt wide) opens right under the icon (macOS, frosted glass, rounded corners) / just above or beside the tray icon (Windows), fully on screen; height fits the content, long lists scroll. | [ ] | [ ] | n/a |
| 8A.2 Linux: tray menu → **Open Panel…**. | The panel opens in the top-right corner (solid background). | n/a | n/a | [ ] |
| 8A.3 Click the icon again; open it and press Esc; open it and click another app. | Each closes the panel; clicking the icon while it is open closes it (no flicker back open). | [ ] | [ ] | [ ] |
| 8A.4 Right-click the icon (macOS: also ⌃-click) while the panel is open. | The panel closes; the native menu opens (Quit, Settings, … as in 8.2). | [ ] | [ ] | n/a |
| 8A.5 Second monitor / Dock or taskbar on the left or right / icon at the far right of the menu bar. | The panel opens on the icon's screen, next to the bar, never cut off. | [ ] | [ ] | [ ] |
| 8A.6 Header. | The server's host and a dot: green up to date, blue pulsing syncing, amber changes waiting, red offline. | [ ] | [ ] | [ ] |
| 8A.7 **Today** tab. | Next events (all-day first, "Now" on the running one, ended ones gone, "in 42 min") and the work due today / overdue (red "Overdue"). Click an event or item → it opens in the main window, the panel closes. | [ ] | [ ] | [ ] |
| 8A.8 **My Day** tab: tick an item, untick it. | The circle fills, the web's My Day shows it within a minute; the progress bar and "2 of 5" follow; the tray's My Day too. | [ ] | [ ] | [ ] |
| 8A.9 **Assigned** tab. | Open work assigned to you, grouped by project with its colour; tick = done (it stays struck through until the next refresh); click opens it; "Show all in My Work" when there are more than 100. | [ ] | [ ] | [ ] |
| 8A.10 **Inbox** tab. | Latest 25 notifications worded like the web bell, unread in bold with a blue dot; the tab shows the unread badge. Click → opens the page and marks it read (badge and Dock count drop). **Mark all read** → badge gone. | [ ] | [ ] | [ ] |
| 8A.11 **Timer** tab with a timer running. | The clock ticks every second from the real start time; **Stop & log** logs it ("Time logged."), or, where the workspace needs an activity and has no default, shows the activities inline; picking one logs it. **Discard** asks for a second click. | [ ] | [ ] | [ ] |
| 8A.12 **Timer** tab without a timer. | "Start a timer" with a search over your My Day and assigned issues; Enter / click starts it; today's entries and the total below. | [ ] | [ ] | [ ] |
| 8A.13 Keyboard. | ⌘1–⌘5 (Ctrl+1–5) switch tabs; ←/→ on the tab bar; ↓/↑ move through rows; Enter opens; Tab reaches the circles, Space ticks. | [ ] | [ ] | [ ] |
| 8A.14 Footer. | **New Task** opens New task, **Capture** quick capture, **Open Brainitive** the main window, **Settings** the Settings window; the panel closes; the four buttons fit on one line. | [ ] | [ ] | [ ] |
| 8A.15 Close and reopen. | The last tab is open again (also after a restart). | [ ] | [ ] | [ ] |
| 8A.16 Network off, open the panel. | Red dot, "Offline — showing what was loaded …"; ticks still work and show "Waiting to sync" (sent when back, see 12); Stop / Start / Discard / Mark all read are greyed with the offline hint. Today without events says they show when back online. | [ ] | [ ] | [ ] |
| 8A.17 Network off right after a cold start (no answer yet). | My Day, Assigned and Today come from the offline copy. | [ ] | [ ] | [ ] |
| 8A.18 Sign out, then click the icon. | "Sign in to Brainitive" with **Open Brainitive**; nothing of the previous account. No server chosen: "Connect to Brainitive". | [ ] | [ ] | [ ] |
| 8A.19 Light and dark mode (switch while the panel is open). | Text, separators and the glass follow; macOS looks like a system menu-bar extra; Windows / Linux solid panel with a border. | [ ] | [ ] | [ ] |
| 8A.20 First open with slow network. | A skeleton shows, then the data; a failed refresh shows a red line with **Try again** above the last data. | [ ] | [ ] | [ ] |

## 9. Notifications and badge

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 9.1 Create a notification for the signed-in user on the server (mention, assignment). | A native notification appears within about a minute, with the app's name. | [ ] | [ ] | [ ] |
| 9.2 Click the notification. | The app comes to the front and opens the page the notification points to. Windows: the click is only reported while the toast is still on screen (known gap); record if a click after it disappears does nothing. | [ ] | [ ] | [ ] |
| 9.3 A notification whose link points to an auth page (`/login`, `/app/session/...`) if you can produce one. | The auth page is not opened. The main window stays where it is. Skip and mark n/a if no such link can be produced. | [ ] | [ ] | [ ] |
| 9.4 Unread count on the Dock icon. | macOS: the Dock badge shows the count. | [ ] | n/a | n/a |
| 9.5 Unread count on the taskbar. | Windows: the taskbar icon has an overlay with the count. | n/a | [ ] | n/a |
| 9.6 Unread count on the launcher. | Linux: the count shows only on a Unity-style launcher (Unity launcher API); on other desktops nothing is shown, which is expected. | n/a | n/a | [ ] |
| 9.7 Close the main window, then create a notification on the server. | The badge (Dock, overlay, launcher) updates while the window is closed. | [ ] | [ ] | [ ] |
| 9.8 Read all notifications in the web. | Badge and tray count go to zero within about a minute. | [ ] | [ ] | [ ] |
| 9.9 Turn notifications off for Brainitive in the OS settings, then trigger one. | No notification appears; no crash; the badge still updates. | [ ] | [ ] | [ ] |
| 9.10 Finish a download to the Downloads folder (section 10) and click its notification. | The file is revealed in Finder / Explorer. Linux: opens the containing folder in the file manager (untested). | [ ] | [ ] | [ ] |
| 9.11 Offline changes are sent after the network returns (section 12). | One notification "changes sent"; a problem gives a second notification, and clicking it opens the offline window. | [ ] | [ ] | [ ] |

## 10. Downloads

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 10.1 Settings → Downloads → turn on "ask where to save". Download an attachment from a page. | The native Save dialog opens, with the file name suggested. | [ ] | [ ] | [ ] |
| 10.2 Choose a folder in the Save dialog. | The file is saved there with that name; it opens correctly. | [ ] | [ ] | [ ] |
| 10.3 Start a download and press Cancel in the Save dialog. | No file is left in the folder; the staging folder holds no leftover file. | [ ] | [ ] | [ ] |
| 10.4 Turn "ask where to save" off. Download the same attachment twice. | Both files go to the Downloads folder; the second gets a unique name, nothing is overwritten. | [ ] | [ ] | [ ] |
| 10.5 macOS: `xattr -p com.apple.quarantine <file>` on a downloaded file. | Prints a value starting with `0081;` and contains `Brainitive`. | [ ] | n/a | n/a |
| 10.6 macOS: open the downloaded file (for example a PDF). | Gatekeeper's "downloaded from the Internet" prompt appears for an app or installer, not for an ordinary document; behaviour is as for a browser download. | [ ] | n/a | n/a |
| 10.7 Windows: `Get-Content <file> -Stream Zone.Identifier` in PowerShell. | Shows `ZoneId=3` (Mark of the Web). | n/a | [ ] | n/a |
| 10.8 Windows: open a downloaded `.exe` or `.msi` test file. | SmartScreen or the "downloaded from the internet" prompt appears. | n/a | [ ] | n/a |
| 10.9 Linux: download a file and open it. | Saved normally, no quarantine prompt (no quarantine flag on Linux, expected). | n/a | n/a | [ ] |
| 10.10 Upload a test file whose name holds a right-to-left override character, ends with a dot, or is a Windows device name (`CON`, `NUL`) and download it again. | The saved name has the RTL character removed, no trailing dot, and device names get an `_` prefix. Same rules on every OS. | [ ] | [ ] | [ ] |
| 10.11 Download a file that a page makes in the browser (an export that uses a `blob:` URL). | The file saves with the right content and size. Known risk (blob and data downloads): record the result. | [ ] | [ ] | [ ] |
| 10.12 Download while the Downloads folder is read-only. | A clear error, no crash, no partial file left. | [ ] | [ ] | [ ] |

## 11. Drag & drop uploads

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 11.1 Drag one file from Finder / Explorer / the file manager onto an issue's attachment area. | The file uploads and appears in the list. | [ ] | [ ] | [ ] |
| 11.2 Drag three files at once onto the same area. | All three upload. | [ ] | [ ] | [ ] |
| 11.3 Drop a file on the page outside any drop zone. | The app does not navigate to the file; the page stays. | [ ] | [ ] | [ ] |
| 11.4 Drag a board card between two columns. | The card moves; the web app's drag works (HTML5 drag reaches the page). | [ ] | [ ] | [ ] |
| 11.5 Drag a Gantt bar. | The bar moves; known risk on WebKitGTK and WebView2: record the result. | [ ] | [ ] | [ ] |
| 11.6 Use the file picker (`<input type=file>`, for example "Attach file"). | The native file chooser opens; the chosen file uploads. | [ ] | [ ] | [ ] |
| 11.7 Drop a text link from the browser onto the page. | The app does not navigate away from the page. | [ ] | [ ] | [ ] |
| 11.8 Drag a file from outside the app while the main window is hidden (close-to-tray). | Expected: nothing happens (window hidden). Bring the window back, then repeat 11.1. | [ ] | [ ] | [ ] |

## 12. Offline mode

Before this section: on the server run `php artisan migrate`, and use an account that works on
desktop. The first sync is a full run; wait about a minute after sign-in before going offline.

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 12.1 Sign in online and wait about a minute. | The offline copy is written under `offline/<server id>/` in the app data folder (`cache.json`). macOS/Linux: permissions 0600. | [ ] | [ ] | [ ] |
| 12.2 Stop the local server (or disconnect the network) and wait for the reconnect screen (section 15). | The unreachable screen shows **Open offline**. | [ ] | [ ] | [ ] |
| 12.3 Click **Open offline**. | The offline window shows My Day, open work, your task lists and your capture inbox. A banner reads "Offline — changes will sync". | [ ] | [ ] | [ ] |
| 12.4 Restart the app with the server down and a copy present. | The offline window opens by itself once for this outage (not again on every retry). | [ ] | [ ] | [ ] |
| 12.5 In My Day tick an item done; untick it again. | The change shows at once; after untick the item is back to open. | [ ] | [ ] | [ ] |
| 12.6 Add an item to My Day from open work. | It appears in My Day. | [ ] | [ ] | [ ] |
| 12.7 In a task list, add a task, tick it, rename it (double-click or Rename). | Each change shows at once in the list. | [ ] | [ ] | [ ] |
| 12.8 Add a capture in the offline window's Captures tab. | It appears in the inbox list. | [ ] | [ ] | [ ] |
| 12.9 Log in as the viewer-role account and open a task list offline. | "Add task" is not shown (sync v2 `can.manage_items`). | [ ] | [ ] | [ ] |
| 12.10 While offline, check the tray and the window title. | Tray: "Offline — N changes will sync". Window title: "Brainitive — <server> (offline)". | [ ] | [ ] | [ ] |
| 12.11 Start the server, then choose **Sync Now** (tray or window). | Changes are sent in order; a notification "changes sent" appears; the web app shows the changes. | [ ] | [ ] | [ ] |
| 12.12 Check the web app for the captures from 12.8. | Each capture is there exactly once (no duplicates after the sync). | [ ] | [ ] | [ ] |
| 12.13 Offline: tick a task done. Then, still offline, untick it. Sync. | Final state on the server is "not done" (changes fold, nothing wrong is sent). | [ ] | [ ] | [ ] |
| 12.14 Conflict: go offline, rename a task in the desktop. Rename the same task in the web. Sync. | The offline window's "Needs a look" lists the task with "mine" and "theirs"; the web keeps the other user's title; **Copy my text** copies your text. Dismiss removes the entry. | [ ] | [ ] | [ ] |
| 12.15 Deleted on the web while offline: tick a task offline, delete it on the web, sync. | The change is listed with your text instead of vanishing. | [ ] | [ ] | [ ] |
| 12.16 Sign out, then sign in again. | The offline copy and the waiting changes are deleted at sign-out (folder `offline/<server id>/` gone). | [ ] | [ ] | [ ] |
| 12.17 Forget the server with waiting changes. | Its offline folder is deleted. | [ ] | [ ] | [ ] |
| 12.18 Sign in on a fresh server and try **Open offline** before any sync. | A message says there is no copy yet; no crash. | [ ] | [ ] | [ ] |
| 12.19 Turn on the web's own offline mode (Settings → work offline on this device) and use **Open web app offline** from the reconnect screen. | The web's offline shell opens. Verified on macOS (WKWebView); on Windows and Linux the service worker support is untested, so record the result. | [ ] | [ ] | [ ] |

## 13. Voice capture

Voice is off by default. Its direct shortcut is off by default too (BRAI-440: ⌃⌘K, then hold V, see
4A.10); turn it on in Settings → Shortcuts → Direct shortcuts to test the rows that name ⇧⌘V. A direct
shortcut is only registered while voice capture is on.

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 13.1 With voice off (or its direct shortcut off), press ⇧⌘V (Ctrl+Shift+V) in another app with a text field. | The key is not taken by Brainitive; it works as the app's own shortcut (for example "paste as plain text"). | [ ] | [ ] | [ ] |
| 13.2 With voice off, choose **File → Voice Capture…**. | An explanation with a **Turn on** button appears. | [ ] | [ ] | [ ] |
| 13.3 Turn voice on from each entry point in turn: Settings, File menu, tray, quick capture mic button. | Each one turns it on; ⌃⌘K V (and a direct shortcut, if set) then starts it. Test each entry at least once. | [ ] | [ ] | [ ] |
| 13.4 Turn it on and click **Turn on**: the speech model download starts (190 MB). | Progress is shown; a checksum check runs; the download finishes; the model file is in the app data `voice/` folder. | [ ] | [ ] | [ ] |
| 13.5 Interrupt the download (disconnect the network midway). | The partial file is removed or resumed cleanly; a retry works; no broken model is used. | [ ] | [ ] | [ ] |
| 13.6 Second use after the model exists. | No new download. | [ ] | [ ] | [ ] |
| 13.7 macOS: the first recording asks for microphone permission. Allow it. | The system prompt appears once; after allowing, recording works. | [ ] | n/a | n/a |
| 13.8 Windows: Settings → Privacy → Microphone blocked for desktop apps, then record. | Indicator shows a "denied" message with a button to the privacy settings. Mapping untested on Windows (known gap). | n/a | [ ] | n/a |
| 13.9 macOS: deny microphone permission, then record. | Indicator explains how to allow it and the button opens Privacy → Microphone. | [ ] | n/a | n/a |
| 13.10 Linux: no working input device (or ALSA device missing). Record. | A clear error in the indicator; no crash. | n/a | n/a | [ ] |
| 13.11 Hold ⇧⌘V (Ctrl+Shift+V), say a short sentence, release. | Indicator with level meter and timer while held; after release the text opens in quick capture. Check it and save. | [ ] | [ ] | [ ] |
| 13.12 Tap the shortcut once (under half a second), speak, tap again. | Recording continues after the tap (toggle mode); the second tap ends it and the text opens in quick capture. | [ ] | [ ] | [ ] |
| 13.13 Start a recording, then press ↩ instead of the shortcut. | Recording stops and the text opens. | [ ] | [ ] | [ ] |
| 13.14 Start a recording, press Esc. | Recording cancels; no text is inserted. No late transcript shows up afterwards. | [ ] | [ ] | [ ] |
| 13.15 Press Esc while the transcription is running (a few seconds after a long clip). | Cancelled; nothing is inserted. | [ ] | [ ] | [ ] |
| 13.16 Hold the shortcut for longer than 2 minutes. | Recording stops at 2:00 by itself; the text of the first 2 minutes opens in quick capture. | [ ] | [ ] | [ ] |
| 13.17 Press and release without speaking. | Nothing is inserted. No invented text such as "Thank you." | [ ] | [ ] | [ ] |
| 13.18 Quick capture already has text; record a sentence. | The spoken text is appended to the existing text; nothing already typed is lost. | [ ] | [ ] | [ ] |
| 13.19 Settings → Voice capture → language: German; speak German. | German text; repeat with English and with Albanian (Albanian accuracy untested: record it, a native speaker judges). | [ ] | [ ] | [ ] |
| 13.20 Language set to automatic; speak German. | German text (auto-detect). | [ ] | [ ] | [ ] |
| 13.21 Turn the network off; record. | Transcription still works (on this computer). | [ ] | [ ] | [ ] |
| 13.22 After several recordings, search the app data folder and the temp folder for new audio files (`*.wav`, `*.pcm`, `*.ogg`, `*.mp3`). | None: audio is kept in memory only. | [ ] | [ ] | [ ] |
| 13.23 Windows: hold the shortcut, release it. | Release is detected by polling; recording stops within about half a second. If release is missed, the 2-minute cut-off is the fallback: record it. | n/a | [ ] | n/a |
| 13.24 Linux on X11: hold the shortcut, release it. | Release is detected through detectable auto-repeat: recording stops on release. | n/a | n/a | [ ] |
| 13.25 Linux on Wayland: the voice shortcut. | No global shortcut (known gap). Start the recording from the tray, File menu or quick capture mic button instead. | n/a | n/a | [ ] |
| 13.26 Settings → Shortcuts → Direct shortcuts → rebind the voice shortcut, then use the new one. | Works; the old one stops working. | [ ] | [ ] | [ ] |
| 13.27 Settings → Voice capture → **Remove voice model**. | The model file is gone from the `voice/` folder; the next use asks to download it again. | [ ] | [ ] | [ ] |
| 13.28 Turn voice off in Settings. | Shortcut no longer registered: ⇧⌘V (Ctrl+Shift+V) works in other apps again (13.1). | [ ] | [ ] | [ ] |
| 13.29 Signed release build only: check the microphone prompt. | Requires the `audio-input` entitlement in a signed build (not configured yet). Unsigned debug builds may not show the prompt; record what happens. | [ ] | [ ] | [ ] |

## 14. Launch at login and start hidden

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 14.1 Settings → turn on **Open at login**. | The toggle is on. macOS: a LaunchAgent exists (`~/Library/LaunchAgents/`), and the app may appear under System Settings → Login Items. Windows: a Run key for Brainitive in `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Linux: an autostart `.desktop` file in `~/.config/autostart/`. | [ ] | [ ] | [ ] |
| 14.2 Log out and back in (or reboot). | The app starts by itself. | [ ] | [ ] | [ ] |
| 14.3 Turn on **Start hidden** and log in again. | The app starts with no window: tray or menu-bar icon only. Open it from the tray. | [ ] | [ ] | [ ] |
| 14.4 Turn on **Open at login**, then remove the entry by hand (Login Items, Run key or autostart file). Open Settings. | The toggle shows off: the OS state is the truth. | [ ] | [ ] | [ ] |
| 14.5 Turn **Open at login** off. | The entry is gone. | [ ] | [ ] | [ ] |
| 14.6 Start at login while the server is unreachable. | The app starts and shows the reconnect screen with retry; no crash. | [ ] | [ ] | [ ] |
| 14.7 Start at login on a Mac or PC where the app was never run before (first run). | The first-run choice (section 1) appears, not a blank window. | [ ] | [ ] | [ ] |

## 15. Reconnect and unreachable screens

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 15.1 Stop the local server while signed in. Wait. | The reconnect screen names the problem (connection refused) and a suggested fix. | [ ] | [ ] | [ ] |
| 15.2 Watch the countdown between retries. | Waits grow 5 s, 10 s, 20 s, 40 s and cap at 60 s. "Retrying…" shows during an attempt. | [ ] | [ ] | [ ] |
| 15.3 Start the local server again. | Reconnects by itself within the current wait (or at once when the network check sees it). | [ ] | [ ] | [ ] |
| 15.4 Click **Retry** on the screen during a long wait. | An attempt runs at once. | [ ] | [ ] | [ ] |
| 15.5 Turn the network off (Wi-Fi, cable). | Message says there is no network. Turn it on: the app retries at once. | [ ] | [ ] | [ ] |
| 15.6 Connect to a host that does not exist (for example `nosuch.brainitive.invalid`). | "Address not found" (DNS) with a fix. | [ ] | [ ] | [ ] |
| 15.7 Connect to a non-routable address (for example `https://10.255.255.1`). | "No answer" (timeout), with a fix. Waits, then retries. | [ ] | [ ] | [ ] |
| 15.8 Connect to a stub server that answers 500. | "Server error (HTTP 5xx)" with a fix. | [ ] | [ ] | [ ] |
| 15.9 Connect to a site that is not Brainitive. | "Not a Brainitive server" (section 1.12). | [ ] | [ ] | [ ] |
| 15.10 Put the computer to sleep for a minute, then wake it while the server is down. | Retry starts at once after wake. | [ ] | [ ] | [ ] |
| 15.11 Server needs a newer desktop app (its minimum desktop version is above this build). Connect. | "Please update Brainitive Desktop" screen with the server name, both versions and a download link. The web app is not loaded. | [ ] | [ ] | [ ] |
| 15.12 On that screen press **Check for update now**. | In this build: says updates are not available (placeholder signing key, section 17). Does not crash. | [ ] | [ ] | [ ] |
| 15.13 With the update-required screen showing, click **Open offline**. | Refused while the server is known to need a newer app. | [ ] | [ ] | [ ] |
| 15.14 Confirm the reconnect screen keeps working after Open offline has been used once. | Open offline still works from the reconnect screen when a copy exists. | [ ] | [ ] | [ ] |

## 16. Diagnostics and logs

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 16.1 Help → **Copy Diagnostics**. Paste into a text editor. | Shows app version, OS, webview version, server host, signed in yes or no, and the last 50 log lines. | [ ] | [ ] | [ ] |
| 16.2 Search the pasted text for `bra_`, `brr_`, `Bearer`, `eyJ` (JWT), `token=`, `code=`, `state=`, `password=`, `Cookie`, `?`. | None of them appears. | [ ] | [ ] | [ ] |
| 16.3 Help → **Open Logs Folder**. | The folder opens and holds `brainitive.log`. macOS: `~/Library/Logs/app.brainitive.desktop/`. | [ ] | [ ] | [ ] |
| 16.4 Search `brainitive.log` for `bra_`, `brr_`, `code=`, `state=`, `access_token`. | No hits; URLs lose query and fragment. | [ ] | [ ] | [ ] |
| 16.5 Start a sign-in, then read the log lines for it. | Log shows the step, not the code, state or token. | [ ] | [ ] | [ ] |
| 16.6 Log size: after heavy use, check the log folder. | At most 5 files, each about 2 MB (`brainitive.log` plus rotated copies). | [ ] | [ ] | [ ] |
| 16.7 Windows: the diagnostics show the WebView2 version; Linux: the WebKitGTK version. | A version number is shown (untested on Windows and Linux, record). | [ ] | [ ] | [ ] |
| 16.8 Copy Diagnostics while offline and while signed out. | Works; the signed-in line says no. | [ ] | [ ] | [ ] |
| 16.9 Copy Diagnostics with the network blocked by a firewall (or Little Snitch / Windows Firewall logs). | No network request is made by the copy action. | [ ] | [ ] | [ ] |
| 16.10 Windows and Linux: Open Logs Folder and the log path. | The folder opens and holds the log (untested on both, record the path). | n/a | [ ] | [ ] |

## 17. The updater

Only the rows marked "now" apply today. The app ships with a placeholder public key
(`plugins.updater.pubkey` in `tauri.conf.json`), so update checks are off. The rest apply once the
owner has generated the key pair, put the public key in the config and added the CI secrets.

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 17.1 (now) Open the update entry in the app menu (macOS) or Help (Windows, Linux) and in the tray. | "Check for Updates…" is greyed out. | [ ] | [ ] | [ ] |
| 17.2 (now) Open Settings → Updates. | Says why updates are off (no signing key in this build). Version is shown. | [ ] | [ ] | [ ] |
| 17.3 (now) Start a development (debug) build and wait 30 seconds. | No update check happens (log has no update check line). | [ ] | [ ] | [ ] |
| 17.4 (after key) Settings → Updates: "Get beta updates" off (stable is the default). Press **Check now**. | Shows the last check time; reports up to date when the latest stable is the installed version. | [ ] | [ ] | [ ] |
| 17.5 (after key) Turn on "Get beta updates". Check. | The beta manifest is used; the higher of stable and beta is offered. Turn it back off: stable again. | [ ] | [ ] | [ ] |
| 17.6 (after key) Automatic checks: first check about 30 s after start, then every 6 h. Turn automatic checks off. | With the setting off no check runs; "Check for Updates…" still works by hand. | [ ] | [ ] | [ ] |
| 17.7 (after key) Start a download of a newer version, switch the channel during the download. | The download is discarded; no "ready" notice for the old one. | [ ] | [ ] | [ ] |
| 17.8 (after key) When the download is ready. | Notification "Update ready — restart to install"; the menu and tray show "Restart to Update (x.y.z)". Nothing is installed yet. | [ ] | [ ] | [ ] |
| 17.9 (after key) Choose **Restart to Update**. | The app restarts on the new version; Settings shows the new version. | [ ] | [ ] | [ ] |
| 17.10 (after key) macOS: app in a folder you cannot write to. | Install asks for an administrator password. | [ ] | n/a | n/a |
| 17.11 (after key) Windows: the passive NSIS installer runs and the app restarts. | Progress is shown, no UAC loop, app starts on the new version. | n/a | [ ] | n/a |
| 17.12 (after key) Linux `.deb`: update install. | Uses `pkexec` (password prompt). Untested; record. | n/a | n/a | [ ] |
| 17.13 (after key) Linux AppImage: update install. | The AppImage is replaced in place and the app restarts. | n/a | n/a | [ ] |
| 17.14 (after key) Offer an update whose manifest is older than or equal to the installed version (test server). | Never installed. | [ ] | [ ] | [ ] |
| 17.15 (after key) Tamper with the downloaded bundle or serve a bad signature (test server). | Refused; the app keeps the current version. | [ ] | [ ] | [ ] |

## 18. Performance

Procedure in `docs/PERFORMANCE.md`. Use a release build, a quiet machine and a single running copy.
Enter the numbers in the table there (not in this file).

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 18.1 Quit all copies. Make it cold: reboot, or `sudo purge` on macOS. Start the release build. | Only one process starts. | [ ] | [ ] | [ ] |
| 18.2 Read `brainitive.log`: the line `startup: bundled shell loaded +N ms`. | Target: under 2000 ms. Enter the number in docs/PERFORMANCE.md (cold start, bundled shell). | [ ] | [ ] | [ ] |
| 18.3 Same log: `startup: remote page loaded +N ms`. | Enter the number (cold start, remote page). | [ ] | [ ] | [ ] |
| 18.4 Warm start: quit, start again. | Second number recorded in the notes column or a second row, labelled warm. | [ ] | [ ] | [ ] |
| 18.5 On the connect screen, wait 30 s, then measure memory (macOS: `ps -axo pid,rss,comm` filtered to brainitive and com.apple.WebKit, or Activity Monitor; Windows: Task Manager → Details, msedgewebview2 children; Linux: `ps --ppid` for WebKitWebProcess). | Enter "Idle RSS, Choose screen". Budget: 120 MB total. | [ ] | [ ] | [ ] |
| 18.6 Sign in, open the dashboard, wait 30 s, measure again with the same method. | Enter "Idle RSS, signed in on the dashboard". Budgets: macOS 350 MB, Windows 400 MB, Linux 350 MB. | [ ] | [ ] | [ ] |
| 18.7 Note the number of helper processes (WebContent, Networking, GPU). | Counts recorded with the RSS numbers. | [ ] | [ ] | [ ] |
| 18.8 Large page: open a board with many cards, then measure again. | Record the number; no target yet (web app side). | [ ] | [ ] | [ ] |

## 19. Security spot checks

Use a debug build for the devtools steps (19.1 and 19.5). Use a real signed-in session.

| Step | Expected | macOS | Windows | Linux |
|---|---|---|---|---|
| 19.1 Debug build: on a server page, open devtools and run `window.__TAURI_INTERNALS__.invoke("brainitive_notify", {})` with empty arguments, then try a non-bridge command (for example a settings or sign-out command). | Only the three bridge commands (notify, badge, open external) are reachable from the server origin. Other native commands are refused. | [ ] | [ ] | [ ] |
| 19.2 Foreign page in the main window: from a page, click a link to another website (not a new tab). | The main window does not stay on that site: it returns to the last server page at once; the site opens in the default browser. macOS and Linux: the page is reverted on start; Windows: blocked by the navigation guard before it loads. | [ ] | [ ] | [ ] |
| 19.3 An embedded frame of another site on a server page (for example a video embed). | Loads inside the page. Frames are not reverted. | [ ] | [ ] | [ ] |
| 19.4 Sign out with Quick Capture holding text, the switcher showing results, New task holding text, and a recording running. | All four are cleared or closed. | [ ] | [ ] | [ ] |
| 19.5 Debug build: call `brainitive_open_external` from a server page more than 10 times within a minute. | The 11th call is refused (10 per minute). | [ ] | [ ] | [ ] |
| 19.6 Link to another site opened inside the app window (macOS, Linux). | The browser opens the link; the app stays on its page. | [ ] | [ ] | [ ] |
| 19.7 Refresh token and access token are not in `settings.json`, the offline files, the capture queue or the logs. | Search the app data folder and logs for `bra_` and `brr_`: none. | [ ] | [ ] | [ ] |
| 19.8 macOS and Linux: permissions on app data files (`capture-queue.json`, `offline/…`, `voice/…`). | 0600 for files, 0700 for folders. Windows: profile ACL only (not checked here). | [ ] | n/a | [ ] |
| 19.9 Server redirect from https to http (section 1.9) and deep links to auth paths (section 3). | Refused as listed. Repeated here only as a reminder that these are security rules. | [ ] | [ ] | [ ] |

---

## Known gaps per OS

Taken from the task files (`docs/tasks/*.md`, `docs/SPIKE.md`, CHANGELOG "Unreleased"). Each one is
a row above or must be recorded as a result.

**Cross-OS**

- No real update has been installed: the updater ships with a placeholder key (section 17).
- Release builds are unsigned: Gatekeeper and SmartScreen warn. Signing secrets are not set.
- Release blocker: the six GitHub Actions in CI are not pinned to commit SHAs (`docs/RELEASING.md`).
- Offline sync is not clicked through end to end: needs `php artisan migrate` on the server.
- Voice capture: the microphone, the permission prompt, the indicator and hold-to-talk were not run in
  a running app on macOS either; Albanian accuracy is untested (no Albanian voice to test with);
  Intel Macs untested.
- Cold start and idle memory were not measured yet (section 18).
- New task (4C): built and checked as a static page with mocked commands only; not run in the app
  against a real server (create, Open, inbox fallback, shortcut registration on each OS).
- Minimum-version check across restarts and the download URL are open questions in
  `docs/tasks/phase-09-copilot.md` (`https://www.brainitive.app/download` is a guess).
- Errors from remote pages and popups are not written to the log (only the main bundled window).
- The `https://brainitive.app` redirect should go straight to `https://www`: the nginx fix is pending
  (`docs/SPIKE.md`).

**macOS**

- Native offline view and sync not run through in a running app; the web's offline mode was probed
  in WKWebView only.
- Quick capture, switcher, tray, menus, notifications and launch at login were compiled but not
  clicked through in a running app (`docs/SPIKE.md`, phase 05 and BRAI-439 notes).
- Dock badge and menu-bar count: written, not exercised in a running app.
- Signed builds need the microphone entitlement (`com.apple.security.device.audio-input`).
- Unsigned debug builds can make macOS ask for keychain access after every rebuild.
- Minimum macOS 14; Intel not tested.

**Windows**

- Nothing run yet: the CI build (`ci.yml`) is the first step; then the manual pass.
- WebView2 drag & drop (Tauri's drop handler is disabled so HTML5 drops reach the page): untested.
- Accelerators while WebView2 has focus: untested.
- User agent: a fixed Chrome version is spelled out (wry can only replace the UA): check the web app
  does not sniff it wrongly.
- Overlay icon for the badge: written, not run. No dock badge (not applicable).
- Voice: MSVC build of whisper.cpp, WASAPI capture, the "denied" error mapping and key-up by polling
  are untested.
- Launch at login through the Run key: untested.
- Notification clicks only reported while the toast is on screen.
- Diagnostics: WebView2 version, log path and Open Logs Folder untested; DNS and refused error strings
  are string-matched and untested.
- Tray icon may sit in the overflow area.
- Mark of the Web and SmartScreen behaviour on downloads: not run.

**Linux**

- Nothing run yet: CI (`ci.yml` / `build.yml`) builds `.deb` and AppImage; the manual pass follows.
- WebKitGTK version differences: collaborative editor, whiteboard and canvas performance are open
  risks; service worker support depends on the version, so the native offline view is the fallback.
- Global shortcuts: X11 only. No global shortcuts on Wayland (menu, tray and mic button still work).
- Tray needs libayatana-appindicator; left click shows the menu.
- Quick capture and switcher focus and always-on-top behaviour vary per window manager.
- Dock/launcher badge: Unity launcher API only.
- Save dialog: xdg portal or GTK dialog, untested.
- Voice: needs `libasound2-dev` to build and ALSA at runtime (`.deb` depends on ALSA); key-up
  detection depends on X11 auto-repeat.
- `.deb` update uses `pkexec`; autostart uses a `.desktop` file; neither has run.
- Reveal-in-file-manager for downloads and the notification click: untested.
- Quarantine flag does not exist on Linux (expected, section 10.9).
