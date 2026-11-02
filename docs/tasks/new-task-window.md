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

## Result

**Built**

- **Shortcut** `HotkeyAction::Task`: macOS ⌃⌘T, Windows Ctrl+Alt+T, Linux Off
  (`shortcuts::DEFAULT_TASK` / `default_task_for`); Ctrl+Alt+T is now refused on Linux only
  (`parse_on`). Migration `shortcut_scheme` 1 → 2: the default unless the Brainitive key or a
  direct shortcut already uses those keys (then Off); a scheme-0 file runs both steps.
- **Letter T** (`LeaderAction::Task`, customizable); a file whose T belongs to another action gives
  New task N, else the first free letter (the user's letters are kept). Overlay grew to five rows.
- **Settings → Shortcuts**: letter row "New task" and the direct row (Change / Default / Off;
  Default = Off on Linux, its tooltip says why).
- **Tray** "New Task…" under Quick Capture with its key; **panel footer** "New Task · Capture ·
  Open Brainitive · Settings" (measured: fits 360 pt). Not added to the app's File menu (⌘N there
  is the web's New Issue dialog).
- **Window** `task` (`task.html`, `src/task.ts`, `src/task.css`, `src-tauri/src/new_task/`,
  `capabilities/task.json`, 9 commands): searchable project select grouped by workspace, board
  only for several boards, title focused, collapsible Markdown description, priority chips (None +
  the project's), type in the footer when the project has types; ⌘↩ / Ctrl+↩ creates, Esc closes
  (closes only the project list when it is open), the shortcut while open focuses it. Created
  state with **Open** (Rust-held URL → main window) and **New another** (chosen over a
  notification: no extra round trip, Enter opens it). Inline 422 per field, 402 the server's
  message, 403 / 404 (list reloads), offline / 5xx / 429 with **Save to Inbox instead** (capture
  path, `intended_type: issue`, title + description). Signed out / no server screens. Height
  follows the page (180…620). Task tab only (the Page tab was optional and is not built).
- Memory per server in `Preferences.new_task` (ids only, ≤ 10, lenient, removed on Forget);
  `request_id` reused only for a retry of the same input after no answer; state per origin +
  `account_epoch`, cleared on sign-out (`flows::clear_account_traces`), window destroyed.
- `ApiRequest::error_body()` (opt-in) returns 4xx bodies (not 401) for the field errors / limit.
- `capture::save` extracted from `capture_submit` and shared with the inbox fallback.

**Tests**: `cargo test`: 287 unit tests (18 new: defaults per OS + Linux-only reservation,
migration 1 → 2 and 0 → 2, Task hotkey conflicts, letter T / N fallback, memory per server and
forget, destinations / config parsing, safe colours, payload mapping, id checks, request id
reuse, created answer, 422 / 402 / 403 / 404 / 409 / 429 / offline / signed-out mapping, inbox
text, height clamp, error-body opt-in, tray entry; updated: popup, leader, hotkeys) + 12
capability tests (2 new: only `task.json` grants `allow-task-*` and exactly its nine commands;
every task command checks its caller first). `cargo clippy --all-targets -D warnings`, `cargo fmt
--check`, `npm run build`, `npx tauri build --debug --bundles app` green
(`src-tauri/target/debug/bundle/macos/Brainitive.app`).

**Commits**: `99de671` plan · `46797d2` Rust (shortcut, letter, migration, commands, tray, panel,
capability) · `4b5d3c3` page, Settings rows, overlay, panel footer · docs (this one).

**Checked visually**: the built `task.html` in headless Chrome with mocked commands, light and
dark: form, open project list, description open, offline error with the inbox button, created
state, signed out; the panel footer width.

**Not tested**

- The running app was not launched (as asked): no real create against a server, no shortcut
  registration, focus / blur behaviour, `task_resize` of the real window, Open into the main
  window, inbox fallback through the queue — QA-CHECKLIST 4C.
- Windows / Linux not built or run here; Ctrl+Alt+T vs. AltGr characters on some Windows layouts
  (QA 4C.21); Wayland has no global shortcuts (tray / panel / ⌃⌘K T still work).
- Native `<select>` popups (board, type) inside a window that hides on blur: fine in quick capture
  so far, unverified here.

## Open questions (API)

1. ~~**Offline issues**: `issue.create` has no `description` and no `type_id`~~ **Answered**
   (server v1.49.0 client gaps): it takes `description` and `item_type_id`; on such a server an
   offline create goes to the outbox ([api-client-gaps.md](api-client-gaps.md)). Older servers keep
   "Save to Inbox instead".
2. **Recent destinations**: there is no server list of recently used projects / boards
   (`/me/navigation`'s `recent` only resolves what the device sends); the window remembers its own
   last choice per server. A shared "last created in" (web + apps) would keep them in step. (Still
   open.)
3. ~~`GET /capture-destinations` has no ETag~~ **Answered**: it has one; the window asks with
   `If-None-Match` on every open. The 1-minute throttle stays only for servers without ETags.
4. `POST /boards/{id}/items` 402 `limit.billing_url` is not shown (the window shows the message
   only); say if an "Upgrade" button that opens it from Rust is wanted. (Still open.)
