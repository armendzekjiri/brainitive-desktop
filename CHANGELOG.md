# Changelog

Brainitive Desktop has its own version numbers (0.x until Desktop 1.0). It needs a Brainitive
server with the App API v1 (Brainitive v1.49.0 or later).

## 0.1.0 (unreleased)

- Tauri 2 shell: the bundled connect / sign-in screens, then the server's web app in the main window.
- Connect **Brainitive Cloud** or a **self-hosted server** (address normalised, `https` only except
  localhost), checked with `GET /api/v1/instance` (name, version, minimum app version, TLS errors).
- **Switch Server…** in the menu, the last 5 servers for one-click switching, **Forget** per server;
  one keychain entry and one webview data store per server; the current server in the window title.
- Sign-in in the system browser (PKCE S256 + state) with the `brainitive://auth/callback` deep link;
  access token in memory, refresh token in the OS keychain, serialized refreshes; web session for
  the webview; sign out.
- Security model: the server's origin gets IPC for three bridge commands only (notify, badge, open
  external), navigation guard (other sites open in the browser), CSP for the bundled screens.
- Deep links `brainitive://open?url=…` and single instance.
- Native notifications (click opens the page), dock badge, downloads to the Downloads folder.
