# New task window

A global **New task** shortcut and a small floating window that creates an issue on a board, like
the web app's Create dialog (Task tab), from any app.

## API (read from `../docs/ai/API.md` + `../docs/api/openapi.json`, not changed)

- **Where to**: `GET /api/v1/capture-destinations` — per workspace the projects the user may write
  to, each with its boards (`{id, name, type, columns}`). One call for every workspace, so it is the
  project list (projects without a board are left out). There is **no server-side "recent
  destinations"**: `GET /me/navigation?recent[]=` only resolves entries the device keeps.
- **Types / priorities**: `GET /api/v1/projects/{id}/config` → `types` (enabled, `is_default`),
  `priorities` (the project's, usually Highest … Lowest).
- **Create**: `POST /api/v1/boards/{id}/items` `{title, description? (Markdown), type_id?,
  priority_id?, request_id?}` → 201 `{data: issue}` (`id`, `key`, `url`, …); the same `request_id`
  with the same input → 200 with the first issue, with other input → 409. Errors: 422
  `validation_failed` + `errors`, 402 `limit_reached` + `limit`, 403 no `can.create_items`, 404
  board gone.
- **Offline**: `POST /api/v1/sync` has `issue.create {board_id, title, column_id?, priority_id?,
  assigned_to?, due_date?}` — **no `description`, no `type_id`**. A queued issue would lose them,
  so the window offers **Save to Inbox instead** (the existing capture queue, `intended_type:
  issue`, title + description as the text) and keeps the form. Open question below.

## Decisions

- **Shortcut** `HotkeyAction::Task`, `Preferences.task_shortcut`: macOS **⌃⌘T**, Windows
  **Ctrl+Alt+T**, Linux **Off** (Ctrl+Alt+T opens the terminal on Ubuntu / GNOME). Ctrl+Alt+T is no
  longer refused on every OS, only on Linux (`shortcuts::is_reserved`). Pressing it while the
  window is open focuses it (no toggle: the text is not thrown away).
- **Migration** (`shortcut_scheme` 1 → 2): files without the field get the default, unless one of
  the other shortcuts (Brainitive key, capture, switcher, voice) already uses those keys → Off.
- **Brainitive-key letter T** (`LeaderAction::Task`), customizable like the others. A file whose
  user already gave T to another action: the new action gets N, else the first free letter (instead
  of resetting every letter).
- **Settings → Shortcuts**: the letter row "New task" and a direct-shortcut row (Change / Default /
  Off; Default = Off on Linux).
- **Tray** "New Task…" under Quick Capture (its key as the accelerator); **panel footer** "New
  Task" (Quick Capture becomes "Capture" so four buttons fit 360 pt).
- **Window** `task` (`task.html`, `src/task.ts`, `src/task.css`, `capabilities/task.json`):
  floating, centred, hides on blur (the text stays), holds account data (destroyed on sign-out).
  Grows with its content (`task_resize`, the top stays put). Task only (no Page tab).
  - Project: searchable select (type to filter, ↑↓ ↩), grouped by workspace when there are
    several. Pre-filled with the last project of this server, else the first.
  - Board: only when the project has more than one; the last-used board is remembered.
  - Title (focused), Description (collapsed, plain Markdown), Priority chips (None + the
    project's), Type (when the project has types; last one, else the default).
  - ⌘↩ / Ctrl+↩ creates, Esc closes. After creating: "Created KEY" with **Open** (the issue in the
    main window, URL held by Rust) and **New another** (keeps project / board / type).
  - Inline errors: 422 per field, 402 the server's message, 403 / 404, offline (+ Save to Inbox
    instead), signed out ("Sign in to Brainitive" + open the main window), no server.
- **Memory** per server in `Preferences.new_task` (`origin → {project_id, board_id, type_id}`,
  ≤ 10, lenient load, removed on Forget).
- **request_id**: one per create; a retry of the same input (timeout, offline answer lost) reuses
  it, so the server answers with the first issue instead of a second; changed input or "New
  another" → a new one.
- **Security**: every call in Rust via `Session::request`; the page gets ids, names and keys, never
  a token or a URL; commands accept only project / board / type / priority ids of the last answers
  for this server and sign-in; `ensure_caller(Task)` first in every command; no events.
- To show 422 / 402 details, `ApiRequest::error_body()` returns a 4xx answer (not 401) with its
  body instead of the bare error (opt-in, used only here).

## Steps

1. Plan (this file) → commit.
2. Rust: `shortcuts` / `settings` / `hotkeys` / `leader` / `prefs` (shortcut, letter, migration,
   memory) → commit.
3. Rust: `new_task/` (model pure + commands), `Popup::Task`, `api` error body, capture refactor,
   tray / panel entries, capability + build.rs + tauri.conf + vite → commit.
4. Page `task.html`, `task.ts`, `task.css`; Settings rows; overlay letter; panel footer → commit.
5. Docs: ARCHITECTURE, SECURITY, CHANGELOG, README, QA-CHECKLIST → commit.
6. Gate: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`,
   `npm run build`, `npx tauri build --debug --bundles app`.

## Tests (Rust)

Defaults per OS + Ctrl+Alt+T only reserved on Linux; migration (default kept / Off on clash / runs
once); letters (T default, N when T is taken); memory per server (pick, fall back, cap, forget,
lenient load); destinations / config parsing; payload mapping (trimmed title, empty description
left out, ids only from the answers); request id reuse; error mapping (422 fields, 402, 403, 404,
offline, signed out); window size clamp; capability rules (own commands only, caller check first).
