# App API client gaps (server v1.49.0 `feat/app-api-client-gaps`)

The server answered the open questions of [tray-panel.md](tray-panel.md) and
[new-task-window.md](new-task-window.md). This change makes the panel and the New task window use
it, and falls back to the current behaviour on an older self-hosted server.

## What the server added (contract: `../docs/ai/API.md`)

- `ETag` / 304 on `GET /me/work`, `GET /notifications` (first page, no cursor),
  `GET /capture-destinations`.
- Notification rows: `summary` (plain text, ≤ 200) and `url` (absolute, or null when the user can't
  open it now).
- `GET /me/timer`: `today_logged_seconds` (finished logs only) and `timer.description`;
  `POST /me/timer/start` takes `description?`.
- `POST /api/v1/sync`: `timer.start {issue_id, description?, started_at}`, `timer.stop {timer_id,
  stopped_at, activity_id?}`, `timer.discard {timer_id}`, `notification.read {notification_id}`,
  `notifications.read_all {before}`; `issue.create` also takes `description` + `item_type_id`.
  New status `gone` (410), record types `timer` / `notification` / `unread_count`, `activities` +
  `default_activity_id` on an `invalid` `timer.stop`.

## Old servers

`/api/v1/instance`'s `version` can't tell a pre-feature v1.49.0 build from a current one, so
support is learned from the answers themselves:

- **ETag**: always send `If-None-Match` when one is kept; a server without ETags answers 200 and
  nothing is kept (the New task window keeps its 1-minute throttle only when no ETag came).
- **Notifications**: `summary` / `url` per row when the row has them; otherwise the client wording
  and the payload link (as today).
- **Timer / offline sync types**: `today_logged_seconds` in `GET /me/timer` (tray poll or panel)
  marks the server as supporting the new sync types. The flag is kept in the offline cache
  (`cache.json`, per server + account, wiped with it), so it is known while offline. Unknown /
  missing → today's behaviour: `/work-logs` for today's total, timer + mark-read disabled
  offline, New task offers only "Save to Inbox instead".

## Plan

1. **ETag reuse** — a pure `api::revalidate` (cached tag + body, answer → use cached / store /
   ask again) shared by `tray::get_cached` (now takes a request with its query, key = path +
   query) and the New task window (destinations, its own ETag in `TaskState`).
   Panel: `/me/work?status=open&limit=100` and `/notifications?limit=25` through `get_cached`.
2. **Notifications** — `view::describe` prefers `summary`; `find_notification` / `has_link` use the
   row's `url` when the row has the key (null = no link), else the payload link. Same-origin check
   and opening in the main window from Rust stay.
3. **Timer** — `TimerView.today_seconds` from `today_logged_seconds` (page adds the running
   elapsed time); `/work-logs` only when the field is missing. `RunningTimer.description`; Start
   has an optional "What are you doing?" field (only when supported), sent as `description`.
4. **Offline (outbox)** — new `outbox::Change` variants `TimerStart`, `TimerStop`, `TimerDiscard`,
   `NotificationRead`, `NotificationsReadAll`, `IssueCreate`; their payloads; folding (read twice =
   once, a later read-all moves `before` forward, discarding an unsent start removes it).
   Results: `gone` → dropped quietly (log only); `timer.start` `conflict` → dropped with a problem
   ("A timer is already running …") and its `entity` (the running timer) shown; `timer.stop`
   `invalid` with `activities` → kept as `Outbox.activity_needed`, the panel shows the inline
   picker next time and a pick re-queues the stop with `activity_id` (same `stopped_at`);
   `timer` / `notification` / `unread_count` entities update the tray, badge and panel.
   Panel: optimistic overlay with a "Waiting to sync" badge (started / stopped / discarded timer,
   read rows, unread count); `read_all.before` = the newest `created_at` the panel showed. The
   "offline" hints go away for these actions on supporting servers.
5. **New task offline** — on a supporting server an offline create goes to the outbox as
   `issue.create` (`board_id`, `title`, `description`, `item_type_id`, `priority_id`) and the
   window shows "Saved — created when you're back online"; old server or a full outbox → "Save to
   Inbox instead" as today.
6. **Tests** (Rust): ETag reuse; summary / url fallback; each new change's payload; result
   handling (ok / conflict / invalid + activities / gone); old-server detection; `before`.
7. **Docs**: ARCHITECTURE, CHANGELOG (1.0.0 draft), QA-CHECKLIST, the two task files' open
   questions, this file's result.

Security stays: every call in Rust, the page gets ids / flags only (no URL, no token), commands
take ids from the kept answers, `ensure_caller` first in every command; no new command (the
existing ones get optional arguments), no new crate.
