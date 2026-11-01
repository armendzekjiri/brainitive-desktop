# Security model and threat model

Brainitive Desktop shows a **remote** web app (the user's Brainitive server) inside a native
shell that can touch the OS. The main risk is remote content getting native powers it should
not have. Cards: BRAI-351 (remote IPC, CSP, navigation guard), BRAI-428 (app API security).

## Assets

| Asset | Where it lives |
|---|---|
| Refresh token `brr_…` (60 days, single use, rotates) | OS keychain only — macOS Keychain / Windows Credential Manager / Secret Service; service `app.brainitive.desktop`, account = server origin. One entry per server. |
| Access token `bra_…` (15 min) | Rust process memory only (`session.rs`). Never on disk, never in the webview. |
| PKCE verifier + `state` of a sign-in in progress | Rust memory only, 15 min, consumed by the first matching callback. |
| Web session cookie of the server | The webview's data store **for that server** (macOS 14+: `WKWebsiteDataStore(forIdentifier:)`; Windows / Linux: a data directory per server). |
| Settings (current server, ≤ 5 recent servers) | `settings.json` in the app config dir. No secrets. |

## Who can do what

**Remote content (the server's pages in the main webview)** can:
- run in the main webview only while the main frame is on the **current server's origin**
  (exact scheme + host + port);
- call exactly three commands — `brainitive_notify`, `brainitive_set_badge`,
  `brainitive_open_external` — through a runtime capability for that origin
  (`shell::allow_bridge_for`, `remote.urls = ["<origin>/*"]`). Each command checks again that the
  caller is the main webview on the current origin, because Tauri can add but not remove a
  capability at runtime (after "Switch server" the old origin's capability still exists, the
  check rejects it);
- open `http`, `https` and `mailto` links in the system browser / mail client (validated in Rust).

Remote content **cannot**: call any other app command (an app ACL manifest in `build.rs` makes
every command need an explicit permission; the bundled-screen commands are granted to local
content only), use any plugin command (no fs, shell, dialog, opener, notification JS APIs are
granted to it), read tokens (they never enter the webview), call `/api/v1` with the app's token,
or navigate the main frame to another site.

**Other origins** (iframes like YouTube / Figma embeds, a site the user clicked) get no IPC at all:
no capability matches them.

**The bundled screens** (`index.html`, served from `tauri://localhost` / `http://tauri.localhost`)
may call the local commands in `capabilities/local.json`. They never render server-provided text
as HTML (`textContent` only) and run under a strict CSP:
`default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; frame-ancestors 'none'`.
The CSP applies to bundled assets only; the server's pages keep the server's own headers.

## Navigation guard (`links.rs`, `shell.rs`)

| Main webview goes to | Result |
|---|---|
| bundled screens, `about:blank`, `data:`/`blob:` (frames) | allowed |
| current server, any path | allowed — except `/login` → a fresh web session from the stored sign-in (no password form in the webview, no social login inside a webview), and `/` → `/dashboard` |
| another http(s) site, top level | cancelled (`window.stop()` on load start, navigate back if it got through) and opened in the system browser |
| another http(s) site in an iframe | allowed, but no IPC (see above) |
| `target=_blank` / `window.open` | denied; server pages navigate the main window, other http(s)/mailto open outside |
| `mailto:` | mail client |
| `brainitive://` | handled as a deep link |
| `file:`, `javascript:`, unknown schemes | blocked |

macOS / Linux report iframe navigations to the handler too, hence the page-load step for
top-level loads; Windows (WebView2) only reports top-level ones and opens them outside directly.

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

Sign out (menu, or the web app's own "Log out"): `POST /api/v1/auth/logout`, delete the keychain
entry, clear that server's webview data. "Forget this server": the same, plus its webview data
store is deleted and it leaves the recent list.

## TLS policy

- `https` only; `http` only for `localhost`, `127.0.0.1`, `[::1]` (local development). Enforced
  when an address is typed (`server.rs`), when settings are loaded, and on every redirect hop.
- Certificates are verified against the **OS trust store** (rustls-platform-verifier): a company CA
  installed on the machine works; self-signed, expired or wrong-host certificates are shown as an
  error and are **never** auto-trusted. There is no "continue anyway".
- The instance check follows at most 5 redirects, each to an allowed origin (no http hop), and
  shows the final origin to the user before it is used.

## Known gaps / accepted risks (v0.1)

- **Crash between rotation and keychain write**: if the app dies after the server rotated the
  refresh token but before the keychain write, the next start presents the old token → the server
  treats it as reuse and revokes the device; the user signs in again (fail-safe, not fail-open).
- **Dev builds and the keychain**: unsigned debug builds change their code signature on every
  rebuild, so macOS may ask for keychain access again. Signed builds (phase 05) don't.
- **Web logout detection** is heuristic (`/` followed by `/login` within 10 s). A dedicated bridge
  command would be cleaner (see the open contract items in the README).
- **macOS 13 and older** have no per-server data stores → the app requires macOS 14
  (`bundle.macOS.minimumSystemVersion`).
- Remote content can still show notifications (≤ 20/min, text clipped) and a badge; those are the
  intended powers.
