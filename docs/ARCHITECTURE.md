# Architecture

Brainitive Desktop is a **Tauri 2** shell: one native window that shows either a few bundled
screens (connect a server, sign in) or the user's Brainitive server — the full web app, loaded
remotely. The native layer adds what a browser tab can't: system-browser sign-in with keychain
storage, deep links, native notifications and the dock badge. Server contract:
[`../../docs/ai/API.md`](../../docs/ai/API.md) in the app repo (when checked out at `brainitive/desktop/`).

```
desktop/
├── index.html, src/            bundled screens (Vite + TypeScript, no framework)
│   ├── main.ts                 renders UiState, forwards clicks to Rust commands
│   └── styles.css
├── public/logo.png
└── src-tauri/
    ├── tauri.conf.json         id app.brainitive.desktop, CSP, deep-link scheme, macOS ≥ 14
    ├── build.rs                app ACL manifest (every command needs a permission)
    ├── capabilities/local.json the bundled screens' commands (local origin only)
    ├── src/
    │   ├── lib.rs              plugins, commands, menu, deep-link + notification wiring, run loop
    │   ├── server.rs           ServerOrigin: typed address → origin, TLS rule, store ids   (pure)
    │   ├── links.rs            navigation decisions, deep-link parsing, URL validation      (pure)
    │   ├── pkce.rs             PKCE S256 + state                                             (pure)
    │   ├── version.rs          min_versions.desktop check                                    (pure)
    │   ├── settings.rs         settings.json: current + ≤ 5 recent servers, pending wipes    (pure)
    │   ├── api.rs              reqwest client for /api/v1 (instance, exchange, refresh, logout, web-session)
    │   ├── secrets.rs          refresh token per origin in the OS keychain (keyring)
    │   ├── session.rs          sign-in state, in-memory access tokens, serialized refresh
    │   ├── state.rs            AppState managed by Tauri (phase, notice, window store, …)
    │   ├── shell.rs            main window: per-server data store, guards, UA, capability, downloads
    │   ├── flows.rs            startup, connect, sign-in, deep links, sign-out, switch, forget
    │   ├── commands.rs         commands of the bundled screens
    │   └── bridge.rs           brainitive_notify / _set_badge / _open_external (remote origin)
    └── tests/live_network.rs   `--ignored` checks against real hosts (TLS errors, 404 detection)
```

## Servers: Cloud or self-hosted

A server is identified by its **origin** (`ServerOrigin`, `server.rs`). The connect screen offers
**Brainitive Cloud** (`https://www.brainitive.app`, one click) and **Self-hosted server** (a URL
field). Normalisation of the typed address:

- trimmed; no scheme → `https://` (`http://` when the host is `localhost` / `127.0.0.1` / `[::1]`);
- path, query, fragment and trailing slashes dropped; host lowercased, IDN → punycode, default port dropped;
- `http` rejected except for those loopback hosts; `user:pass@`, other schemes, bare words rejected.

Then Rust calls `GET {origin}/api/v1/instance` (redirects: ≤ 5, each to an allowed origin; the
final origin is what gets used). The screen shows the name + version before continuing, and
these errors: not a Brainitive server, TLS problem (expired / unknown issuer / wrong host —
never auto-trusted), **server too old** (404 on `/api/v1/instance` but `/site.webmanifest` has
Brainitive's theme colour), **app too old** (`min_versions.desktop`). Debug builds also offer
the pre-release (`dev.brainitive.app`) and local (`localhost:8000`) servers.

The host can be changed any time: **Server → Switch Server…** in the menu (and a "Switch server"
button on the sign-in / unreachable screens) returns to the choice. Up to **5 recent servers**
(name + URL) are remembered for one-click switching. Each server has **its own keychain entry**
(account = origin) and **its own webview data store**, so switching does not sign you out of the
other one. **Forget** (two clicks, recent list) signs that device out on the server, deletes its
keychain entry and its webview data, and removes it from the list. The window title and the
Server menu show the current server ("Brainitive — Cloud", "Brainitive — brainitive.example.com").

## Webview data per server

The main window is created with a data store derived from the origin
(`ServerOrigin::store_id`, 16 bytes of sha256): `data_store_identifier` on macOS 14+,
`data_directory` (`<app local data>/webview/<hex>`) on Windows / Linux. A WebView's store is
fixed at creation, so entering another server **rebuilds** the main window (`shell::open_main`:
destroy → `Destroyed` event → build; `ExitRequested` is prevented meanwhile; the window-state
plugin restores size/position by label). The bundled screens use whatever store the window has;
before any server is chosen a neutral "local" store is used. Forgotten stores are deleted once no
window uses them (`settings.pending_wipes`, retried at start-up).

## Flows (`flows.rs`)

- **Start**: current server + keychain token → `Connecting` → refresh → web-session → main window
  on `/app/session/<code>` → `/dashboard`. No token → sign-in screen. Network error → "can't be
  reached" with Retry / Open offline (the webview cache and service worker) / Switch server.
- **Sign in**: PKCE + state → system browser at `{origin}{authorize_path}` → `brainitive://auth/callback`
  → state check → exchange → refresh token to the keychain → web-session → app.
- **Session lost in the webview** (`/login` reached): a fresh web session from the stored sign-in,
  back to the last page (`?to=`). Twice within 30 s → sign-in screen. Right after `/` (the web
  app's "Log out" lands there) → native sign-out.
- **Deep links** (BRAI-356): `brainitive://open?url=<path or URL on the current origin>` focuses the
  app and navigates (signed out → after sign-in); `brainitive://auth/callback` finishes a sign-in.
  A second launch focuses the running window (single-instance; on Windows / Linux it also carries
  the link).
- **Sign out** (Server menu): `POST /auth/logout`, delete the keychain entry, clear this server's
  webview data.

## Remote content and the bridge

See [SECURITY.md](./SECURITY.md). In short: the current origin gets a runtime capability
(`Manager::add_capability`, `remote.urls = ["<origin>/*"]`) for exactly `brainitive_notify {title, body, url?}`,
`brainitive_set_badge {count}`, `brainitive_open_external {url}` (called by the web app's
`resources/js/lib/desktop.ts` via `window.__TAURI_INTERNALS__.invoke`). The navigation guard keeps
the main frame on the bundled screens and the current origin. The user agent ends with
` BrainitiveDesktop/<version>`.

## Tests

`cargo test` runs the pure logic (server URL rules, PKCE incl. the RFC 7636 vector, deep links,
navigation decisions, versions, settings, token / error parsing). `cargo test --test live_network -- --ignored`
checks discovery against real hosts (badssl.com certificate errors, Cloud 404 detection).
