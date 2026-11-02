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

(filled in at the end)
