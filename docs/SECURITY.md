# Security model and threat model

Brainitive Desktop shows a **remote** web app (the user's Brainitive server) inside a native
shell that can touch the OS. The main risk is remote content getting native powers it should
not have. Cards: BRAI-351 (remote IPC, CSP, navigation guard), BRAI-428 (app API security),
phase 05: BRAI-359 / 360 / 361 / 362 / 363 / 364 / 401 (native windows, global shortcuts, tray,
downloads, autostart), the menu-bar panel, BRAI-366 (offline copy and outbox), BRAI-439 (voice capture: microphone,
model download, no audio retention), BRAI-373 (auto-updates: signed bundles, channels).

## Assets

| Asset | Where it lives |
|---|---|
| Refresh token `brr_…` (60 days, single use, rotates) | OS keychain only — macOS Keychain / Windows Credential Manager / Secret Service; service `app.brainitive.desktop`, account = server origin. One entry per server. |
| Access token `bra_…` (15 min) | Rust process memory only (`session.rs`). Never on disk, never in the webview. |
| PKCE verifier + `state` of a sign-in in progress | Rust memory only, 15 min, consumed by the first matching callback. |
| Web session cookie of the server | The webview's data store **for that server** (macOS 14+: `WKWebsiteDataStore(forIdentifier:)`; Windows / Linux: a data directory per server). |
| Settings (current server, ≤ 20 saved servers with their names / order / last use, preferences: shortcuts, tray, downloads, zoom) | `settings.json` in the app config dir. No secrets. |
| Quick captures waiting to be sent (text, link, type, server origin, request id) | `capture-queue.json` in the app's local data dir, **plain text** (like the webview's own cache), mode 0600 in a 0700 folder on macOS / Linux (`private_fs`, BRAI-377), only while offline; deleted per server on sign-out / forget. |
| Offline copy of the user's own data (BRAI-366): open work assigned to them, today's My Day, the kept task lists with their tasks, the private capture inbox, the account id and name, the sync cursor | `offline/<store id>/cache.json` in the app's local data dir, **plain JSON**, mode 0600, per server; no token, no cookie. Deleted on sign-out, forget and when another account signs in on that server. |
| Offline changes waiting to be sent, and changes the server refused (with the user's text) | `offline/<store id>/outbox.json` next to it, same rules; refused ones until the user dismisses them. |
| Downloads waiting for the Save dialog | `<app cache>/downloads/<uuid>/`, deleted on cancel and at start. |
| Voice capture audio (BRAI-439) | **Process memory only**, while recording and transcribing (≤ 2 min, mono f32), zeroed and freed afterwards. Never written to disk, never sent anywhere. |
| Voice transcript | Memory, then the quick-capture window's text field (like typed text); saved / queued only when the user saves the capture. Never logged. |
| Speech model (`ggml-small-q5_1.bin`, 190 MB) | `<app local data>/voice/`, a public file (no user data); removed with "Remove voice model". |
| Update signing key (minisign / Ed25519, BRAI-373) | **Private key**: offline with the owner (backed up) and in the GitHub secrets `TAURI_SIGNING_PRIVATE_KEY` / `_PASSWORD`, used only by the separate **Updater signing** step after `tauri build` of a signing build (BRAI-377: the build itself never sees it). **Public key**: compiled into the app (`tauri.conf.json` → `plugins.updater.pubkey`). |
| A downloaded update waiting for "Restart to Update" | `<app cache>/update/pending.update`, mode 0600, deleted at start, on a channel change and after a failed install. |
| Update preferences (automatic checks, channel, last check time) | `settings.json`. No secrets. |
| Login item (launch at login) | macOS LaunchAgent plist / Windows Run registry key / Linux autostart `.desktop` file, written only when the user turns it on. |

## Who can do what

**Remote content (the server's pages in the main webview)** can:
- run in the main webview only while the main frame is on the **current server's origin**
  (exact scheme + host + port);
- call exactly four commands — `brainitive_notify`, `brainitive_set_badge`,
  `brainitive_open_external`, `brainitive_sign_out` — through a runtime capability for that origin
  (`shell::allow_bridge_for`, `remote.urls = ["<origin>/*"]`, windows `main` + `app-*`,
  `local: false`). Each command checks again that the caller is an **app window** (`main` or
  `app-<n>`) on the current origin, because Tauri can add but not remove a capability at runtime
  (after "Switch server" the old origin's capability still exists, the check rejects it).
  Notify / badge from an `app-<n>` window are accepted but ignored (the main window already
  reports them);
- open `http`, `https` and `mailto` links in the system browser / mail client (validated in Rust,
  ≤ 10 per minute, BRAI-377).

Remote content **cannot**: call any other app command (an app ACL manifest in `build.rs` makes
every command need an explicit permission; the bundled-screen commands are granted to local
content only), use any plugin command (no fs, shell, dialog, opener, notification JS APIs are
granted to it), read tokens (they never enter the webview), call `/api/v1` with the app's token,
or navigate the main frame to another site.

**Other origins** (iframes like YouTube / Figma embeds, a site the user clicked) get no IPC at all:
no capability matches them.

**Extra app windows** (`app-<n>`, BRAI-362) are the main window's twins: same server data store,
same navigation and page-load guard, same downloads handler, same bridge capability. They only
open on a page of the current server while signed in and with the main window on that server's
store, and close when the server is switched, the user signs out or the server is forgotten.

**The bundled screens** (`index.html`, served from `tauri://localhost` on macOS / Linux and
`http://tauri.localhost` on Windows — only this platform's one origin counts as bundled,
`links::bundled_origin`, BRAI-377)
may call the local commands in `capabilities/local.json`. They never render server-provided text
as HTML (`textContent` only) and run under a strict CSP:
`default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'`.
The CSP applies to bundled assets only; the server's pages keep the server's own headers.

## The bundled native windows (quick capture, quick switcher, settings)

Threat: a native window with powerful commands (post captures, search, copy to the clipboard,
start timers, change shortcuts, register a login item) reachable from the wrong content.

- They load **bundled pages only** (`capture.html`, `switcher.html`, `settings.html`) under the same
  CSP as the connect screens. Their navigation guard (`links::decide_local_navigation`) allows the
  bundled origin only; http(s) / mailto links open in the system browser; `file:`, `javascript:`,
  `brainitive:` and everything else is blocked; `window.open` is denied.
- **One capability file per window** (`capabilities/<label>.json`): `local: true`, no `remote`,
  `windows: [<label>]`, and only that window's `allow-<command>` permissions. The server's pages,
  the main window's bundled screens and the other popups cannot call them. Every command also
  checks in Rust that the caller is that window's label on a bundled origin
  (`popup::ensure_caller` → `links::is_local_caller`). `tests/capabilities.rs` guards the files.
- They never see a token: every server call is a Rust command that uses `Session::request` (the
  stored app token, refresh + one retry on 401). `ApiRequest::url` only builds `/api/v1/…` paths on
  the current origin (no `..`, query or fragment smuggled in a path; ids that go into paths are
  checked: `issue:<digits>` / `task:<digits>`, numeric snippet / issue ids) and never follows
  redirects.
- Server text is rendered with `textContent` only. The switcher drops result links that are not on
  the current origin; opening one re-validates it in Rust (`links::resolve_on_instance`).
- No plugin's JavaScript API is granted to any webview (global-shortcut, autostart, dialog,
  clipboard, fs, opener, notification): the plugins are used from Rust only.

## The offline window and the offline copy (BRAI-366)

Threat: the offline copy leaks (local data at rest), reaches the wrong account, or the offline
window becomes a way to call the API.

- **Window**: `offline.html`, label `offline`, its own `capabilities/offline.json` (`local: true`, no
  `remote`, `windows: ["offline"]`, only `allow-offline-*` + listening for events). Every command
  checks the caller (`popup::ensure_caller`). It renders with `textContent` only; it has no links that
  navigate, and the same navigation guard as the other bundled windows. It only gets the event
  `offline://changed`, emitted to that window.
- **No token** reaches it: it reads the view model and hands over fixed changes (`offline_set_done
  {work_id, done}`, `offline_my_day_add`, `offline_add_task`, `offline_update_task`, `offline_add_capture`,
  `offline_sync_now`, `offline_dismiss_problem`, `offline_open_app`). Rust checks them against the cache
  (work ids `issue:<digits>` / `task:<digits>`, list and task ids that are in the cache, titles ≤ 500
  characters without control characters, captures as quick capture) and queues them; only
  `POST /api/v1/sync` and `GET /api/v1/sync/changes` / `GET /api/v1/me` are called, from Rust, with
  `Session::request` (`/api/v1/` paths only, no redirects). The server applies the same policies as
  the web (forbidden / deleted come back as results and are shown as such).
- **Local data at rest**: the cache and outbox are plain JSON (0600 on macOS / Linux; on Windows the
  user's profile ACLs), readable by other processes of the same OS user — the same exposure as the
  webview's own cache and IndexedDB of the server's pages, and the capture queue. They contain the
  user's own work titles, task titles, My Day and private captures, never tokens, cookies or other
  users' content (the feed only sends the user's own records and those of `project_ids`). Disk
  encryption (FileVault / BitLocker / LUKS) is what protects them on a lost laptop; the app does not
  encrypt them itself (a key would sit in the same keychain session anyway).
- **Whose data**: kept per server (folder = the server's store id) and per account: `GET /me` is
  asked once per session before anything is sent or shown as current; another account's cache and
  outbox are deleted, never sent as the new account. Sign-out (menu, the web app) and Forget delete
  the folder; a sync still running then writes nothing back.
- Lost access: records of projects that are no longer in `project_ids` are dropped on the next sync,
  deleted things by their tombstones.
- No outbox entry is retried forever on a 4xx; refused changes stay listed with the user's text until
  dismissed (so nothing typed offline is silently lost).

## The menu-bar panel

Threat: a window with the account's work and notifications and commands that change them (tick
done, mark read, start / stop / discard the timer) reachable from the wrong content, handed ids or
URLs it should not act on, or showing another account's data.

- **Window**: `panel.html`, label `panel`, its own `capabilities/panel.json` (`local: true`, no
  `remote`, `windows: ["panel"]`, only `allow-panel-*` + listening to `panel://changed`, which is
  sent to that window only). Every command starts with `popup::ensure_caller(Panel)`
  (`tests/capabilities.rs` checks both). Same CSP and navigation guard as the other bundled
  windows; server text via `textContent` only; colours only as `#rgb` / `#rrggbb`.
- **No token, no URL in the page**: every call is a Rust command with `Session::request`
  (`/api/v1/…` paths only, no redirects). The page gets `has_link` flags; a click sends a reference
  (`{kind: work|event|notification, id}` or a fixed page), Rust takes the URL from **its own copy**
  of the server's answer and opens it with `links::resolve_deep_link_target` (this server, never an
  auth page); anything else just focuses the app. A notification's link is the row's server-built
  `url` (absolute; null = no link), else the payload's on an older server — the same check either
  way, so a foreign host is never opened.
- **Ids**: commands act only on ids in the panel's kept answers of the current server + sign-in
  (work `issue:<n>` / `task:<n>`, notification uuids — hex and dashes —, the timer's own activity
  ids; the timer id is never taken from the page; start candidates = open issues the server listed
  in My Day / assigned work, or the offline copy's when nothing live was loaded). Offline ticks are
  checked against the offline copy as in the offline window. Offline timer / read changes go to the
  same per-server outbox with ids from those answers (an activity for a refused stop only from that
  result's own `activities`); the timer description is one line, ≤ 255 characters. The server
  enforces its own policies anyway; nothing new is trusted.
- **Whose data**: the kept answers belong to one origin + `account_epoch`; another server, a
  sign-out or another sign-in drops them, answers that arrive after a switch are dropped
  (`PollTicket`), and the window is destroyed on sign-out (`Popup::holds_account_data`).
- **Glass**: the window is transparent (macOS `NSVisualEffectView` popover material). Tauri 2.12.5
  enables the private transparent-window API in every build (`macos-private-api` is a no-op since
  2.12.1; nothing extra set in `tauri.conf.json`). Fine for direct distribution (Developer ID); an
  App Store build would need a review of private API use.
- The native tray menu stays on right click (and ⌃-click); ⌃ is read from `NSEvent.modifierFlags`
  at click time only.

## The New task window

Threat: a window that creates issues with the app token reachable from the wrong content, handed
a board / project the user didn't pick from the server's list, leaking a token or URL to the page,
duplicating issues on retries, or showing another account's projects.

- **Window**: `task.html`, label `task`, its own `capabilities/task.json` (`local: true`, no
  `remote`, `windows: ["task"]`, only the nine `allow-task-*` commands, no events). Every command
  starts with `caller(&app, &webview)?` = `popup::ensure_caller(Task)` (`tests/capabilities.rs`
  checks the file and the first line of each command). Same CSP and navigation guard as the
  other bundled windows; server text via `textContent` only; colours only as `#rgb…` hex.
- **No token, no URL in the page**: `GET /capture-destinations`, `GET /projects/{id}/config` and
  `POST /boards/{id}/items` run in Rust with `Session::request` (`/api/v1/…` only, no redirects).
  The page gets ids, names and the issue key; the created issue's `url` stays in Rust and
  `task_open` opens it through `shell::open_page` → `links::resolve_deep_link_target` (this server,
  never an auth page).
- **Ids**: `task_config` / `task_create` accept only projects, boards, types and priorities of the
  kept answers of the current origin + `account_epoch` (`model::draft`); the server checks
  `BoardPolicy::createItems` anyway. Title ≤ 255 and description ≤ 100 000 characters are checked
  before sending; the window height is clamped. An offline create (newer servers) is the same
  checked draft, queued as `issue.create` in this server's outbox (wiped with the sign-in).
- **Idempotency**: every create carries a `request_id` (uuid v4). It is reused only for the same
  board + body after a call that got no answer, so a retry returns the first issue (200) and never
  makes a second; any answer or other input gets a new one (the server would answer 409 for the
  same id with other input).
- **Error bodies**: `ApiRequest::error_body()` (opt-in, this window only) returns a 4xx answer's
  JSON (field errors, the plan limit) instead of a bare error; 401 still refreshes / signs out.
  Messages shown are the server's "safe to show" `message` / field errors, capped at 300 chars.
- **Whose data**: the kept projects / configs / created URL belong to one origin + sign-in; another
  one drops them, `flows::clear_account_traces` clears them, and the window is destroyed on
  sign-out (`Popup::holds_account_data`). The last project / board / type per server in
  `settings.json` are ids only (no names) and are removed on Forget.
- **Inbox fallback**: `task_save_to_inbox` uses the quick-capture path (`capture::save`: the queue
  is per origin, 0600, deleted on sign-out / forget).

## Global shortcuts

Threat: a global shortcut steals keys from other apps, or something else changes / triggers it.

- **The Brainitive key** (BRAI-440, ⌃⌘K / Ctrl+Alt+K) is one registered combination; the letter
  after it is read by the overlay window (`leader.html`, label `leader`) only while that window
  has the focus — no letter is registered globally and no key is read from other apps. The
  overlay has its own `capabilities/leader.json` (`local: true`, no `remote`,
  `windows: ["leader"]`, only `allow-leader-view` / `-press` / `-release` / `-close`, no events);
  every command checks the caller (`popup::ensure_caller`). The page only reports a letter A–Z;
  Rust decides the action from the preferences, and the actions are the same ones the menu and
  tray offer (open quick capture / switcher / the main window, start voice capture **only while
  voice capture is turned on**). It holds no account data. Voice's key-up in the indicator is
  its own command `voice_release` (voice capability), which only ends a recording started by a
  held letter.
- Letters and the key are changed only through Settings' commands (`settings_set_shortcut`,
  `settings_set_leader_letter`: A–Z, unique); a broken letters entry in `settings.json` falls back
  to the defaults. The letter **T** (New task) only opens the New task window.
- **New task** ⌃⌘T / Ctrl+Alt+T (`HotkeyAction::Task`) only opens (or focuses) its window; nothing is
  sent without the user's Create. Ctrl+Alt+T is refused on **Linux only** (the terminal on
  Ubuntu / GNOME; New task has no default there); on Windows it is free, but on a layout where
  AltGr+T (= Ctrl+Alt+T) types a character, that character can't be typed while the shortcut is
  on — QA 4C checks common layouts; Settings can change or turn it off.

- Registered from Rust only; changed only through the Settings window's command (its capability),
  validated by `shortcuts.rs`: at least two modifiers incl. Ctrl / Alt / Cmd (F-keys: one), no
  system combination (Spotlight, screenshots, input sources, lock, force quit, Ctrl+Alt+Del, Alt+F4
  …), no duplicates. If the OS refuses one (taken by another app) the previous ones stay.
- A shortcut only **opens a bundled window** (voice capture: also starts the microphone, see
  below); it never sends anything by itself. Nothing is read
  from other apps (no key logging: the OS delivers only the registered combination).
- While Settings records new keys the current shortcuts are unregistered and restored when the
  recording ends or the window closes.

## Voice capture (BRAI-439)

Threats: the microphone records when the user doesn't expect it; audio or transcripts leak (disk,
network, logs, another webview); a tampered speech model is loaded; the model download is
redirected somewhere else.

- **Microphone only on purpose**: opened when the user holds / presses the voice shortcut or the
  Brainitive key's voice letter (held, in the focused overlay), picks
  Voice Capture in the menu or tray, or clicks the microphone in quick capture — and only after
  voice capture was turned on (off by default; while off the shortcut is not registered and the menu, the tray and the
  microphone button only explain it and offer "Turn on").
  The always-on-top indicator shows a red dot, the level and the timer for the whole recording;
  the stream is closed on release / Done / Esc / the 2-minute cut-off / an error. Nothing listens
  in the background (no wake word, no buffering before the press).
- **OS permission**: macOS asks once with `NSMicrophoneUsageDescription` (`src-tauri/Info.plist`);
  denied / restricted → nothing is opened, the indicator explains how to allow it and opens the
  privacy page (a **fixed** OS URL, not one a page can choose). Windows: the per-app microphone
  switch (an error when opening). Linux: no per-app permission.
- **Audio stays in this process's memory**: `cpal` → a `Vec<f32>` → resampled copy → whisper.cpp,
  all in Rust / C++ in-process; the buffers are zeroed after use and on cancel. No file, no network
  call, no webview ever sees audio (the indicator gets a level 0–1 and the elapsed time only).
  The OS could still page memory to the swap file (encrypted on macOS; FileVault / BitLocker / LUKS
  protect it at rest), like any app memory.
- **Transcripts**: handed only to the bundled quick-capture window (`capture_context.prefill`,
  taken once), where the user edits and saves them like typed text (the usual capture path, its
  queue rules). Logs contain durations only (`voice: 4.8 s recorded, … transcription 210 ms`),
  never audio or text; whisper.cpp's own log lines go through the redacting logger at warn+.
- **Indicator window**: `voice.html`, label `voice`, its own `capabilities/voice.json`
  (`local: true`, no `remote`, `windows: ["voice"]`, only `allow-voice-*` + listening to events
  sent to it); every command checks the caller (`popup::ensure_caller`). It cannot start a
  recording by itself without the user (Start / Try again are buttons in that bundled window; no
  remote page or other window can call them). Quick capture's microphone button is its own
  command `capture_voice` (capture capability); Settings' language and "Remove voice model" are
  Settings commands.
- **Model download** — new network host, used for this file only: `https://huggingface.co`
  (`ggerganov/whisper.cpp`, whisper.cpp's official model repository), URL **pinned to a commit**
  in code (`voice::model::MODEL`), redirected by Hugging Face to its CDN (`*.hf.co` /
  `*.huggingface.co`). Rust only: https only, ≤ 5 redirects and each one must be a Hugging Face
  host over https (`model::allowed_url`), no cookies or tokens sent (a plain client, not the API
  client), OS trust store. The file is written to `.part` and accepted only with the exact size and
  the **pinned SHA-256**; anything else is deleted. It is downloaded only after the user turned
  voice capture on (and again only after "Remove voice model"). The request carries the app's user
  agent; no account or server data.
- **Loading the model**: only the verified file is ever renamed into place; its size is checked
  again before each recording. A same-user process could swap the file afterwards — it could
  equally change the app itself (accepted).

## Auto-updates (BRAI-373)

Threats: someone ships code to every user through the updater — a tampered bundle, a manifest
on a host we don't control, a downgrade to an old vulnerable release, a mix-up between channels —
or an update replaces the app while the user works.

- **Only signed bundles install.** `tauri-plugin-updater` (`=2.14.0`) verifies the bundle's minisign
  (Ed25519) signature against the public key **compiled into the app** before it returns the
  download, and again on install (the staged file is re-read and re-verified, so swapping it on disk
  does not help). Neither the manifest nor the server can supply another key.
- **Signature bound to the version.** `requireSignedVersion: true`: the signed trusted comment
  carries the version the bundle was built as (Tauri CLI ≥ 2.12), and it must equal the version the
  manifest announces. The manifest itself is not signed; this is what stops a tampered manifest from
  pairing "9.9.9" with a validly signed **old** bundle.
- **No downgrades.** Only a strictly higher version is an update (our comparator, semver:
  `1.0.0-beta.2 < 1.0.0`); `allowDowngrades` stays off (`cargo test` checks the config).
- **Placeholder guard.** While `plugins.updater.pubkey` is the repo's placeholder or anything that is
  not a minisign Ed25519 public key, and in every development build, the updater does not run at
  all: no request, nothing downloaded or installed, "Check for Updates…" greyed out, Settings and the
  log say why (`updater::availability`). CI makes no updater bundles while the placeholder is in.
- **Network.** New hosts, used for updates only: `github.com` (manifests and release assets of
  `armendzekjiri/brainitive-desktop`) and GitHub's asset CDN `objects.githubusercontent.com` /
  `release-assets.githubusercontent.com`. The client is https only (`https_only`), follows ≤ 5
  redirects and **each hop must be https on one of those hosts** (`updater::allowed_url`, standard
  port, no user info); the bundle URL from the manifest must be on them too. OS trust store (rustls),
  a plain client: no cookies, no tokens, no account or server data; the request carries the plugin's
  user agent only (our manifest URLs are fixed: no version / OS / arch placeholders in them). Manifest request timeout 30 s.
- **Channels.** Stable = GitHub's "latest" release, which is never a pre-release or draft; beta =
  `beta.json` of the rolling `updater-beta` pre-release **plus** stable, the higher version wins.
  Switching channel never downgrades; a waiting update of the other channel is dropped, and a check
  or download of the old channel still running is discarded when it ends (generation counter,
  BRAI-377).
- **The user decides when.** Checks and downloads run in the background; installing happens only on
  a click on "Restart to Update" (app menu, tray, Settings, the "Please update" screen). Nothing
  restarts by itself. Windows runs the NSIS installer in passive mode (progress bar) and restarts the
  app; macOS may ask for an admin password when the app lives in a folder the user can't write.
- **Who can trigger it.** The commands are local only: `check_for_update` / `restart_to_update` (main
  window's bundled screens, `capabilities/local.json`, plus a check that the caller is the main
  window on a bundled origin) and `settings_set_update_channel` / `settings_check_for_updates` /
  `settings_restart_to_update` (Settings window). The plugin's own JavaScript commands are granted
  to no webview (`tests/capabilities.rs`); remote pages can neither check nor install.
- **Logs** name versions and failure reasons, never more than a URL on GitHub.

Accepted risks: the manifest is not signed, so whoever controls the GitHub repo (or TLS to
github.com) can **withhold** updates (freeze users on the current version) or offer a version that
doesn't exist (the download then fails verification) — but cannot install anything without the
private key. **A stolen private key plus control of the releases is a full compromise** of every
installed app: the key is kept offline, only GitHub Actions has a copy, and only the "Updater
signing" step of a `v*` tag build (or a manual run on the default branch) sees it — after
`tauri build`, which never gets it; Apple / Azure secrets reach only the `tauri build` step of their
OS, never `$GITHUB_ENV` (BRAI-377). Rotating it needs one release signed with the old key that carries the
new public key (docs/RELEASING.md). The repo must publish its releases publicly for updates to work.

## Tray, polling, clipboard

- The tray menu is native (no webview). Its actions call fixed endpoints with validated ids.
- Polling uses the app token from Rust, about once a minute while signed in, backing off on errors;
  nothing is polled while signed out. An answer is written (badge, timer, My Day, ETag cache) only
  while the server and the sign-in it was asked for are still current (`tray::PollTicket`,
  BRAI-377): a sign-out or switch during the request drops it.
- The switcher writes a snippet's content to the clipboard only on an explicit ⌘C / Ctrl+C.

## Downloads (BRAI-363)

- A download's destination is chosen by the app: a private staging folder (then the user's choice
  in the native Save dialog) or the Downloads folder. The server's suggested name is reduced to its
  last path component without reserved characters (`downloads::clean_name`), so it can't escape the
  folder or overwrite a file (`unique_path`). BRAI-377: the name also loses Unicode bidi / format
  characters (no `invoice_\u202Efdp.exe` that reads as `.pdf`), trailing dots and spaces, and never
  is a Windows device name (`CON`, `NUL.txt`, `COM1` → `_CON` …).
- Every saved file is marked as downloaded (`downloads::mark_downloaded`): `com.apple.quarantine`
  (`0081;<hex time>;Brainitive;<UUID>`, one WebKit set is kept) on macOS, so Gatekeeper checks an
  app or script opened from it; `Zone.Identifier` (zone 3, Mark of the Web) on Windows.
  `LSFileQuarantineEnabled` is **not** set: it would quarantine every file the app writes (settings,
  logs, the offline copy, the voice model and the updater's freshly unpacked app bundle).
- The dialog is opened from Rust (`tauri-plugin-dialog`); no webview can open file dialogs or read
  the chosen path.

## Launch at login (BRAI-364)

- Off by default; only the Settings window's command (its capability) can turn it on or off. The
  login item starts the app's own executable with `--autostart`, which only decides whether the
  main window starts hidden. It does not change the security model: the stored sign-in is
  restored the same way as on a manual start.

## Navigation guard (`links.rs`, `shell.rs`)

| Main webview goes to | Result |
|---|---|
| bundled screens, `about:blank`, `data:`/`blob:` (frames) | allowed |
| current server, any path | allowed — except `/login` → a fresh web session from the stored sign-in (no password form in the webview, no social login inside a webview), and `/` → `/dashboard` |
| another http(s) site, top level | Windows: cancelled in the navigation handler. macOS / Linux: reverted on page-load **start** (`window.stop()` + navigate back to the last good page, `links::decide_page_load`, BRAI-377); opened in the system browser |
| another http(s) site in an iframe | allowed, but no IPC (see above) |
| `target=_blank` / `window.open` | denied; server pages navigate the main window, other http(s)/mailto open outside |
| `mailto:` | mail client |
| `brainitive://` | handled as a deep link |
| `file:`, `javascript:`, unknown schemes | blocked |

macOS / Linux report iframe navigations to the handler too, hence the page-load step for
top-level loads; Windows (WebView2) only reports top-level ones and opens them outside directly.
wry 0.57.1 (tauri 2.12.5) passes the navigation handler the URL only — not
`WKNavigationAction.targetFrame.isMainFrame` (WebKit) or the frame of the WebKitGTK policy
decision — so a foreign main-frame load can't be refused up front there. Page-load events are
main-frame only (`didCommitNavigation` / `load-changed: committed`), so on `Started` the app
reverts at once instead of waiting for `Finished` (BRAI-377). The foreign page may run for the
few milliseconds until the navigation back; it never has IPC.

## Sign-in (OAuth 2.1 code + PKCE, first party)

1. Rust generates verifier (48 random bytes, base64url), S256 challenge and `state` (32 bytes).
2. The **system browser** opens `{server}{authorize_path}?client=desktop&code_challenge=…&code_challenge_method=S256&state=…&device_name=…`
   (Google blocks OAuth in embedded webviews; the browser also keeps the user's password manager
   and passkeys).
3. `brainitive://auth/callback?code=…&state=…` comes back via the OS. The state is compared in
   constant time; a wrong state does **not** cancel the real pending sign-in; the pending sign-in
   is single use and expires after 15 minutes (the server's code after 60 s).
4. Rust calls `POST /api/v1/auth/exchange` with the verifier. The redirect policy of token calls is
   "never follow" (a redirected POST could carry a code or token elsewhere).
5. The refresh token goes to the keychain **before** the access token is used. Refreshes are
   serialized behind one async mutex (the server revokes the device when a refresh token is used
   twice).
6. `POST /api/v1/auth/web-session` → a one-time `/app/session/<code>` URL **on the same origin**
   (checked) that the webview opens to get its cookie.

Deep links, web notification URLs, notification clicks and links the server hands the tray / menu /
switcher never open the auth endpoints (`links::resolve_deep_link_target`, BRAI-377): `/app/*`,
`/login*`, `/logout*`, `/register*`, `/password/*`, `/auth/*`, `/oauth/*`, `/api/*`,
`/forgot-password`, `/reset-password`, `/confirm-password`, `/verify-email`, `/github-app`,
`/github/app`, compared after percent-decoding (also nested), `\` → `/`, `//`, dot segments and
`;params` collapsed, and lower-casing. Without it, `brainitive://open?url=/app/session/<code>` with
an attacker's code would sign the webview into the attacker's account (login CSRF; the server also
asks "Continue as …?" when another user is signed in). Only step 6 opens `/app/session/<code>`.

Sign out (menu, or the web app's own "Log out"): `POST /api/v1/auth/logout`, delete the keychain
entry, clear that server's webview data, and (BRAI-377) cancel a running voice recording, drop a
transcript waiting for quick capture and destroy the quick-capture, switcher and offline windows (their
text and last results). "Forget this server": the same, plus its webview data
store is deleted and it leaves the saved list. Settings → Servers → Sign out of a server that is
not current: its device, keychain entry, queue and offline files, and its store (scheduled); it
stays in the list.

## Switching servers (several accounts)

Threats: a page (or another server's page) switches the app to a server the user never added, a
switch sends one server's data to another (outbox, captures, panel / New task answers, a late
poll), or the old server's account stays readable after the switch.

- **Ids, not URLs**: every switch command takes a saved server's **id** (`servers::server_id`, 16
  hex characters of its store id) and looks it up in `settings.json`; an unknown id is refused.
  Adding a server is only the connect flow (`check_server` → confirm → `use_server`, with its TLS /
  redirect / version checks). The switch's restore asks `/api/v1/instance` again and refuses a
  server that now redirects.
- **Who may switch**: `switch_to_server` (main window's bundled screens, `capabilities/local.json`,
  `ensure_main_screen`), `panel_switch_server` (`panel.json`, `ensure_caller(Panel)`), the
  `settings_*_server(s)` commands (`settings.json`, `ensure_caller(Settings)` first — tested), the
  native app menu and tray (Rust). No remote page can call any of them; the bridge still has its
  four commands only.
- **Menu shortcuts are local**: ⌃⌘1…9 / Alt+1…9 are menu accelerators of the app (only while it is
  in front), never registered globally; Settings refuses a global shortcut on those keys.
- **No data across servers**: tokens stay per origin in the keychain (a switch keeps them, so going
  back needs no sign-in); the capture queue, offline files and New-task memory are per origin;
  in memory the switch bumps `account_epoch` (every poll, panel load, New-task call answering for
  the old server is dropped by its ticket), clears the panel / New-task / tray answers and the
  ETags (now keyed by origin + path, so a 304 can't hand one server's body to another), unloads
  the offline data and destroys the windows that hold account data. The old server's outbox is
  sent to **that** origin only (bounded, before the switch); a sync still running stops at its
  next step because loading / unloading the offline data bumps its epoch (before, it could take
  the newly loaded server's outbox to the old origin). Captures queued for a server are only sent
  while it is current.
- `app-<n>` windows close on a switch: they share the main window's store and the guard / bridge
  check of the current origin.
- **Sign out** in Settings for a server that is not current revokes that device on the server,
  deletes its keychain entry, queue and offline files, and schedules its webview store for
  deletion; **Forget** does the same and removes it from the list.
- Names are the user's or the server's (`/api/v1/instance`), cleaned (no control or bidi
  characters, ≤ 60) and rendered with `textContent` / as native menu text only (`&` escaped for
  Windows / GTK mnemonics).

## Unreachable and "update required" screens (BRAI-367)

- Bundled screens only (`capabilities/local.json`, main window). `open_download_page` opens a
  **constant** (`version::DOWNLOAD_URL`) in the system browser: the page can't choose the address.
- A server whose `min_versions.desktop` is above this app's version is **never loaded**: `restore`
  asks `/api/v1/instance` before it enters the app, `open_offline` refuses while the last answer
  of the server (or the screen) says the app is too old. A broken minimum ("soon") locks nobody out.
- The problem text is built in Rust from fixed sentences (`problem.rs`); only the host name of the
  user's own server is inserted, and the screen renders it with `textContent`. The raw OS / TLS
  error chain is logged, never shown.
- Certificate problems are shown as what they are and **never retried by themselves** or bypassed.
  The automatic retry only runs for transient problems and sends the same `/api/v1/instance` and
  web-session requests as a manual retry (≥ 5 s apart, ≤ 1 per minute at the cap); the "is the
  network back?" probe is a local name lookup of the user's own server host, nothing else.

## Logs, redaction and diagnostics (BRAI-368)

- **No telemetry, no network sending.** Logs stay in the app log dir (files 0600 on Unix, rotation
  5 × 2 MB). Copy Diagnostics only writes to the user's clipboard, on their click.
- **Redaction (`redact.rs`) runs on every line before it is written**, and again on diagnostics:
  URLs lose userinfo, query, fragment and opaque path segments; `bra_`/`brr_` tokens, JWTs,
  `Bearer …` values, `token/password/secret/cookie/authorization/…=` pairs and opaque
  `code/state/session=` values are masked; the home directory becomes `~`. It is best-effort
  defence in depth: code still must not log secrets in the first place (tokens never leave `session`).
- Panic messages and backtraces are redacted the same way. Lines are flattened and length-bounded
  (no log injection through newlines).
- New local command `log_client_error` (main window only, `capabilities/local.json`): message ≤ 500
  and source ≤ 200 characters, ≤ 20 per minute, redacted. Remote pages cannot call it; errors in
  remote pages or popup windows are not logged.
- Start-up timing lines (`startup: <phase> +Nms`) contain phase names and milliseconds only.
- Diagnostics contain the server origin only, never user data or paths with query strings.

## TLS policy

- `https` only; `http` only for `localhost`, `127.0.0.1`, `[::1]` (local development). Enforced
  when an address is typed (`server.rs`), when settings are loaded, and on every redirect hop.
- Certificates are verified against the **OS trust store** (rustls-platform-verifier): a company CA
  installed on the machine works; self-signed, expired or wrong-host certificates are shown as an
  error and are **never** auto-trusted. There is no "continue anyway".
- The instance check follows at most 5 redirects, each to an allowed origin, and **never from
  https down to http, also not to localhost** (`api::redirect_refusal`, BRAI-377); http hops only
  while the whole chain is http (the user entered `http://localhost`). When the final origin
  differs from the one entered, the confirm screen says so before "Use this server", and
  `use_server` refuses an origin that changed since it was confirmed (a recent server too).

## Known gaps / accepted risks (v0.1)

- **Crash between rotation and keychain write**: if the app dies after the server rotated the
  refresh token but before the keychain write, the next start presents the old token → the server
  treats it as reuse and revokes the device; the user signs in again (fail-safe, not fail-open).
- **Dev builds and the keychain**: unsigned debug builds change their code signature on every
  rebuild, so macOS may ask for keychain access again. Signed builds (phase 05) don't.
- **macOS 13 and older** have no per-server data stores → the app requires macOS 14
  (`bundle.macOS.minimumSystemVersion`).
- Remote content can still show notifications (≤ 20/min, text clipped) and a badge; those are the
  intended powers.
- **Queued captures are plain text on disk** while offline (`capture-queue.json`), readable by
  other processes of the same OS user — the same exposure as the webview's cache of the server's
  pages. Accepted; deleted on sign-out / forget and as soon as they are sent.
- A captured text queued under a server is sent to whoever is signed in to that server when the
  connection is back (sign-out deletes the queue, so this is only the same user after a session
  expiry).
- **Voice capture's direct shortcut ⇧⌘V / Ctrl+Shift+V** (BRAI-439) takes the combination from
  other apps ("paste as plain text" in browsers, Slack, VS Code, and **paste in most Linux
  terminals**) only when the user sets it (off by default since BRAI-440: ⌃⌘K, then hold V) and
  voice capture is on.
- **The Brainitive key** ⌃⌘K / Ctrl+Alt+K (BRAI-440) is taken from other apps while it is on (the
  default). On Czech, Slovak and Hungarian keyboard layouts AltGr+K types `ł`, which Windows
  reports as Ctrl+Alt+K: those users need to change or turn off the key in Settings → Shortcuts.
- **Signed macOS builds** with the hardened runtime will need the
  `com.apple.security.device.audio-input` entitlement for the microphone (not set yet: signing is
  not configured; part of the beta checklist).
- **The offline copy is plain JSON on disk** (BRAI-366, see above): accepted like the webview cache;
  it lives until sign-out / forget / an account switch. Offline changes are only sent after `GET /me`
  confirmed the account they were made as (or, for changes made before the first sync, the account
  signed in at that time).

## Desktop security review (BRAI-377)

Findings of the review and what became of them (details in the sections above; plan and tests in
`docs/tasks/brai-377-desktop.md`).

| # | Finding | State |
| --- | --- | --- |
| 1 | Deep links / notification URLs could open `/app/session/<code>` and other auth pages (login CSRF) | **Resolved**: `links::resolve_deep_link_target` for deep links, web notifications, notification clicks, tray / menu / switcher links; normalised path deny list. Server side ("Continue as …?" when another user is signed in) is in the app repo. |
| 2 | CI: updater key in the `tauri build` env; Apple / Azure secrets in `$GITHUB_ENV`; signing on any ref; unpinned signing CLI and actions | **Resolved**, except the action SHAs: exact tags with `# TODO pin SHA`, a release blocker in `docs/RELEASING.md` (could not be verified offline). |
| 3 | macOS / Linux navigation guard reverted a foreign top-level page only on `Finished` | **Resolved** as far as wry allows: revert on `Started`. **Accepted**: no up-front refusal of foreign main-frame navigations on macOS / Linux — wry 0.57.1 gives the navigation handler no frame information; the page never gets IPC and is gone within milliseconds. |
| 4 | Downloads without quarantine | **Resolved**: `com.apple.quarantine` / `Zone.Identifier` on every saved file; `LSFileQuarantineEnabled` deliberately not set. |
| 5 | Both `tauri://localhost` and `http(s)://tauri.localhost` counted as bundled on every OS | **Resolved**: one origin per platform (`links::bundled_origin`, from tauri 2.12.5's `protocol::origin`). |
| 6 | Queue / outbox temp files created with the default mode, folders world-readable | **Resolved**: `private_fs` (0600 files from creation, 0700 folders) for queue, offline, update, voice. |
| 7 | Instance check allowed https → http://localhost; final origin used without a new confirm | **Resolved**: `api::redirect_refusal`, redirect notice on the confirm screen, `use_server` refuses an unconfirmed origin. |
| 8 | Changing the update channel during a download still installed the old channel's update | **Resolved**: generation counter, stale results discarded. |
| 9 | Download names kept bidi / format characters, Windows device names, trailing dots | **Resolved** in `downloads::clean_name`. |
| 10 | Tray poller wrote results after a sign-out / switch | **Resolved**: `tray::PollTicket` (server + account epoch) checked under the lock of each write. |
| 11 | `brainitive_open_external` had no rate limit | **Resolved**: 10 per minute (`bridge::allow_rate`, shared with notify's 20). |
| 12 | Sign-out left a voice transcript, the capture text, switcher results and a running recording | **Resolved**: `flows::clear_account_traces`. |
