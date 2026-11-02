# BRAI-366 — offline mode inside the desktop webviews

Card: board `desktop-app`, BRAI-366 (id 646). Built on `main`, small commits.

## Finding first: does the web's own offline mode run in the webview?

The web app has an opt-in offline mode (BRAI-82): `public/sw.js` serves a cached `/offline` shell
(Cache API) when a navigation fails, the shell renders My Day, recently opened lists / issues and
quick capture from IndexedDB, and `lib/offline/sync.ts` sends its outbox to `POST /offline/sync`
with the web session.

Probe (2026-11-02, macOS 26, a Swift WKWebView with `WKWebsiteDataStore(forIdentifier:)` — the
store type the desktop uses): on `https://www.brainitive.app` **and** `http://localhost:8000`,
`navigator.serviceWorker` exists, `/sw.js` registers and activates, a reload is controlled by it,
the Cache API stores and IndexedDB exists. So on macOS the web's offline mode works inside the
desktop app **when the user turned it on in that webview** ("Work offline on this device" is per
browser profile = per desktop data store), and "Open web app offline" keeps using it.

What it does not cover, and what this card adds natively:
- users who never turned the web's offline mode on (the default), and a first start without network;
- WebKitGTK builds without service workers (untested), and the main window's bundled screens;
- anything outside the webview: the tray, notifications, an indicator, "Sync now".

## Plan

1. **`offline_cache.rs` (pure)** — the cache of one server + user: records keyed `type:id`
   (`work`, `task_list`, `task`, `capture`, `my_day`), cursor, kept task lists, `project_ids`.
   `GET /api/v1/sync/changes`: full run (no cursor; `scopes=work,task_lists,my_day,captures`,
   `task_lists=` the lists of the user's assigned list tasks + the lists already kept, ≤ 50) →
   pages while `has_more` → drop what the full run didn't send; delta runs with the kept cursor.
   Tombstones (`op: delete`) drop; upserts apply by `updated_at`; records of projects not in
   `project_ids` drop; only the newest My Day is kept. New lists to keep → a new full run. The
   view model (My Day, open work, lists + tasks, inbox captures) with the outbox laid over it.
2. **`outbox.rs` (pure)** — waiting changes per server (`offline/<store>/outbox.json`, atomic
   writes, ≤ 1000): `work.done`, `my_day.add`, `list_task.create`, `list_task.update` (with
   `base.fields`), `capture.create`, each with its own `request_id`. Unsent changes coalesce
   (tick + untick = nothing; an edit of an unsent new task changes the create). Batches of ≤ 50
   in order to `POST /api/v1/sync`; per result: applied / conflict → done (a conflict becomes a
   problem with mine / theirs), deleted / forbidden / invalid → dropped as a problem with the
   user's text, waiting → kept (dropped after 5 tries), rate_limited / error → kept, stop.
   A 4xx for a whole batch → one by one, a 4xx for one change → dropped: never retried forever.
3. **`offline.rs` (glue)** — files in `<app local data>/offline/<store id>/` (`cache.json`,
   `outbox.json`), loaded for the current server; `GET /me` once per session: another account →
   the cache and outbox are deleted. `sync` = check user → send the outbox → pull changes; run
   by the poller (every poll while in the app), "Sync now" (tray + window), after each write.
   Notification when queued changes were sent / could not be applied. Deleted on sign-out,
   forget, account switch.
4. **The offline window** (`offline.html`, label `offline`, `capabilities/offline.json`, its own
   `offline_*` commands, `popup::ensure_caller`): banner "Offline — changes will sync" / synced,
   Sync now; My Day (tick), My Work (tick, add to My Day), task lists (add, tick, rename),
   captures (add, inbox), problems (copy my text, dismiss). Opened by "Open offline" on the
   unreachable screen and automatically once per outage when a cache exists; "Open web app
   offline" keeps the old behaviour (the web's service worker).
5. **Status** — tray line "Offline — N changes will sync", "Open Offline View…", "Sync Now";
   window title "Brainitive — <server> (offline)".
6. Tests: cursor / full + delta runs / reset / tombstones / project_ids / my_day / list choice,
   overlay, outbox coalescing / order / persistence, result handling per status, batch errors,
   clearing (sign-out / account switch), capability file.
7. Docs: ARCHITECTURE, SECURITY (local data at rest), CHANGELOG, this file's result.

## Result

**Built** (commits `5dfb107`, `3c40107`, `3f6716f`, `91619d7`, + docs):
- `offline_cache.rs`, `outbox.rs` (pure, unit tested), `offline.rs` (files, sync, commands).
- Offline window `offline.html` / `src/offline.ts` / `src/offline.css`: status banner (offline /
  syncing / up to date / N waiting, "synced 3 min ago"), Sync now, Open Brainitive; tabs My Day,
  My Work (+ My Day), Task lists (add, tick, rename by double-click or "Rename"), Captures (add,
  inbox), "Needs a look" (problems with the user's text, Copy my text, Dismiss / Dismiss all).
- `capabilities/offline.json` + 9 `offline_*` commands (build.rs), `tests/capabilities.rs` covers it
  (and that no other window gets them). Main window: `open_offline` now opens the offline window,
  `open_web_offline` = the old behaviour ("Use the web app offline").
- Tray: offline line, Open Offline View…, Sync Now, tooltip; title "(offline)"; notifications
  (sent / need a look → click opens the window). Poller: sync after every successful poll.
- Sign-out, forget, account switch delete `offline/<store id>/`.

**Tests**: 144 lib tests (+26: cache runs / reset / tombstones / project_ids / My Day / list choice /
overlay / applied patching / persistence; outbox mutations JSON / folding / bases / order / bound /
verdicts / per-change results / waiting give-up / batch errors / persistence; files per server and
write order; clearing on sign-out and account switch; tray offline entries) + 4 capability tests.
`cargo clippy --all-targets -D warnings` clean, `npm run build` ok.

**Not verified**
- No end-to-end sync: the local dev database has no `sync_tombstones` table yet (the routes answer
  401 unauthenticated, so they exist). **Manual check after `php artisan migrate`**: sign in, wait a
  minute (full run), stop the server, Open offline, tick / add / rename / capture, start the server,
  Sync now → the notification, the web shows the changes; edit a task title on the web while
  offline here → a conflict with mine / theirs.
- The offline window was not clicked through in a running app: the user's debug Brainitive.app was
  running (single instance), and it was not quit.
- Windows / Linux: nothing run. 0600 is Unix only (Windows relies on the profile ACL); WebView2 /
  WebKitGTK service workers, the window, tray labels and notification clicks are untested.

**API gaps / questions** (not changed here)
1. No way to ask which task lists a user works in: the feed needs explicit `task_lists` ids. The app
   keeps the lists of the user's assigned open list tasks (+ those already kept). Pins are per
   workspace (`/me/navigation`, one call each); a "my lists" set (pinned / recently opened, all
   workspaces) would help.
2. Changing `task_lists` restarts the run for **all** scopes (full download). An additive list
   change, or a cursor per scope, would be cheaper.
3. The `entity` of `work.done` / `list_task.*` results is the web engine's snapshot, not the API's
   Task / Work item resource, so it can't replace a cached record; the app patches done / title and
   waits for the next pull.
4. The feed's `task_list` records carry no `can` (manage_items): the offline view can't hide "Add
   task" for viewers; they learn it as `forbidden` on sync.
5. Does a `work.done` change re-send the `my_day` record (its items' `done`)? Unclear from API.md;
   the app patches its My Day copy locally.
6. `captures` sends every state; the offline view only needs the inbox — a state filter would keep
   the cache small.

## Result: sync v2 (BRAI-441, app commit 4a11d787)

All six gaps above are closed on the server and adopted here:
1. `task_lists=mine` replaces the guessed ids (`wanted_lists` removed); the answer's `task_lists` is
   stored.
2. Changing the lists is additive (`reset: false`); `start_run` no longer restarts anything.
3. Results carry `entity` + `record_type`: the cached record is replaced (`Cache::apply_result`);
   `entity: null` keeps the local patch. The task-title / done patch is only the fallback now.
4. `can.manage_items` on `task_list` records: "Add task" is hidden (and `offline_add_task` refuses).
5. My Day is re-sent after done changes; the local patch stays as the optimistic update.
6. `captures=inbox`; capture deletes drop captures that left the inbox (the view's state filter is
   gone). A `task_list` delete drops the list and its tasks.
Old stored cursors keep working (the filters are sent with the cursor), no client migration.
