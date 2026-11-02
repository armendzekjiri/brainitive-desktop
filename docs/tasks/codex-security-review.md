# Codex security review of Brainitive Desktop (2026-11-02)

Branch `fix/security-review` (from `main` at `f29ace7`), merged into `main` locally; not pushed.
The review was source-only (no build, no runtime). Every finding was traced in the code first,
then reproduced with a test where the logic is testable; one commit per finding.

Test support added for this: `src/testing.rs` (a loopback fake of the App API whose requests can be
held at their `.await`, so a test can sign out in between) and `secrets::fake` (unit tests never
touch the real keychain; deletions can be made to fail).

| # | Sev. | Verified? | Fix | Tests |
| --- | --- | --- | --- | --- |
| 1 | High | **Yes.** `panel` / `new_task` fallbacks called `offline::queue` / `with_outbox` after their request, and those took the *current* origin: a switch (or another sign-in) during the request queued A's change into B's outbox. | `state::AccountTicket` (origin + `account_epoch`) taken when the command starts; `queue`, `with_outbox`, `resend_stop`, `queue_work_done` write only while it holds (`offline::owned_outbox`, under the state lock). `with_data` also checks the loaded origin. | `offline::tests::a_change_made_for_server_a_never_lands_in_server_b_s_outbox`, `…before_another_sign_in_on_the_same_server_is_dropped` |
| 2 | High | **Yes.** A completed sign-in only bumped the epoch; the offline window / panel read the cache without `/me`; queued captures had no owner. | A random sign-in id per server (`RecentServer::sign_in`, new on every sign-in, gone on sign-out); a completed sign-in drops the old account's memory (`drop_account_state`). Cache / outbox / captures carry the id; `OfflineData::load` holds files of another sign-in back (not shown, sent or written) until `adopt_user` confirms the account (taken over) or not (deleted). Captures: `queue::owner` → send / wait / remove unsent (+ notification). Offline window: "Checking your account". | `offline::tests::data_of_an_earlier_sign_in_is_not_shown_or_sent_before_me_confirms_it`, `…an_outbox_nobody_confirmed_is_never_adopted…`, `queue::tests::a_queued_capture_is_sent_only_as_the_account_it_was_made_for`, `capture::tests::captures_of_another_account_are_removed_unsent` |
| 3 | High | **Yes.** `sign_out` released the token lock before the logout; a refresh in between restored a token; an exchange already past the pending check stored its pair after the sign-out. | Session epoch in `Session` (one std mutex `Control`: pending sign-in, per-origin generation, `closed`). Sign-out bumps + closes first; refresh / exchange `claim` the generation before storing; closed origins get no token; a late exchange is revoked; a late refresh's token is used for the logout. | `session::tests::a_refresh_that_finishes_after_the_sign_out_began_stores_nothing`, `…a_sign_in_that_completes_after_the_sign_out_stores_nothing`, `…no_token_is_handed_out_while_the_sign_out_runs_and_a_new_sign_in_works` |
| 4 | Medium | **Yes.** Keychain, offline and webview errors were ignored; the notice always said "You are signed out." | `SignOutReport` (remote: revoked / already ended / not confirmed; keychain deleted), keychain delete and offline wipe retried, `flows::end_sign_in` + `sign_out_notice`, `pending_sign_outs` in settings: closed at start, retried. | `session::tests::a_refused_keychain_deletion_is_retried_reported_and_never_restored`, `…a_server_that_cannot_be_reached_is_reported_as_not_revoked`, `flows::tests::a_sign_out_says_what_is_left_instead_of_claiming_success`, `…an_unfinished_sign_out_stays_marked…` |
| 5 | Medium | **Yes.** `capture::save` pushed a failed send into the queue without any check; sign-out bumped the epoch only at its end. | Sign-out / Forget bump the epoch first; `capture::keep_for_retry` checks the ticket under the lock. | `capture::tests::a_send_that_fails_after_the_sign_out_does_not_recreate_the_queue` |
| 6 | Medium | **Yes.** `decide_navigation` allowed the bundled origin and routed `brainitive://` whoever navigated. | `links::Initiator` from the window's committed page (`Inner::committed`, set on every page load) or a one-shot native grant (`shell::grant_local`, 10 s); server content → bundled page / deep link = `Block` (logged). | `links::tests::server_content_cannot_open_the_bundled_screens_or_deep_links` + QA 19.10 |
| 7 | Medium | **Yes, still a platform limit.** wry 0.57.1 (tauri 2.12.5): `navigation_policy` passes the URL only; WebKitGTK's decide-policy also covers sub-frames; no pre-commit main-frame hook. Not fixable without an upstream change. | Kept as accepted (revert on `Started`). The bridge check is now a pure function (`bridge::is_bridge_caller`) and tested against foreign / look-alike / http / bundled URLs; the capability stays `<origin>/*`. | `bridge::tests::a_foreign_page_in_an_app_window_never_reaches_the_bridge` + QA 19.2 |
| 8 | Medium | **Yes.** 14 of 17 commands in `commands.rs` had no caller check. | `ensure_main_screen` first in each (`get_state` etc. now return `Result`); `settings_set_shortcut`'s check moved to the first line. | `tests/capabilities.rs::every_command_of_the_manifest_checks_its_caller_first` (fails on the old code) |
| 9 | Medium | **Yes.** `panel_set_done` updated the tray's My Day without the ticket; `apply_sync_states` took whoever was current. | `panel::mark_tray_done` (ticket checked), the outbox clears go through the request's ticket (finding 1), `apply_sync_states` takes the sync's own `AccountTicket`. | `panel::tests::a_done_tick_confirmed_after_a_switch_does_not_touch_the_new_account` + QA 19.14 |
| 10 | Medium | **Yes.** The 51st download evicted a running one (finished unmarked); `(None, true)` showed "saved" without marking; the webview-default path was untracked; mark failures only logged. | `downloads::admit` (≤ 50, one per URL, never evict), refuse when no folder of ours, `downloads::finish` marks every known path, `mark_downloaded` returns whether it worked and the notification warns. | `downloads::tests::every_successful_download_with_a_known_path_is_marked`, `…running_downloads_are_never_evicted`, `…a_file_that_cannot_be_marked_is_reported` + QA 19.15 |
| 11 | Medium | **Yes.** `APPLE_*` / `AZURE_*` went to the `tauri build` step. | Build unsigned; "macOS signing + notarization" (`scripts/macos-sign.sh`: throwaway keychain, `codesign --deep --options runtime --timestamp --entitlements`, new signed `.dmg`, `notarytool submit --wait` must be `Accepted`, staple `.dmg` + `.app`); Windows `--no-bundle` → sign exe → `tauri bundle --bundles nsis` → sign installer; tool installed without secrets. Docs: RELEASING, APPLE-SIGNING, SIGNING-AGENT-BRIEF. | `scripts/workflow-secrets.test.mjs` (fails on the old workflow); script dry-run with an ad-hoc identity; QA 19.16 |
| 12 | Low | **Confirmed.** Placeholder `pubkey` → `Availability::PlaceholderKey`, CI skips updater bundles. Fail-closed. | Nothing (owner step, RELEASING § 4). | existing `updater::tests::the_placeholder_key_switches_the_updater_off`, `…committed_config_has_the_placeholder_or_a_real_key…` |

## Not tested here (running app / CI only)

- Navigation blocking inside real WKWebView / WebView2 / WebKitGTK (QA 19.10); the macOS / Linux
  foreign-page commit window (QA 19.2).
- The signing workflow on GitHub (no secrets here): Apple keychain import, notarization, staple,
  `tauri bundle` on Windows, trusted-signing-cli — only the script's plumbing ran locally with an
  ad-hoc identity. The first real signing run (QA 19.16) is the check.
- Windows `Zone.Identifier` writing (no Windows machine); the offline-account flows end to end
  (QA 19.12 / 19.13).

## Residual risks (documented)

- Finding 7 (above).
- The signing steps share the runner with the build: a process a compromised build left running
  could watch them. A separate signing job would isolate them (RELEASING § 4).
- The NSIS uninstaller is no longer signed (signing it needs the credentials inside the bundler).
- Captures / offline changes that nobody can attribute after a new sign-in are removed, not sent:
  a safe loss, reported to the user.
