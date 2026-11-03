# UX review — desktop shell and web app (2026-11-03)

What was changed in this repo (branch `feat/tabs`), what is still open here, and what the web app
(app repo) should change: most of what the user sees inside a tab — page transitions, the
document editor — is the web app's, not the desktop shell's.

## Done in the desktop shell (`feat/tabs`)

| Area | Before | Now |
|---|---|---|
| Window frame | System title bar + classic white Win32 menu bar (white in dark mode) | Frameless window, Windows 11 Notepad layout: tabs in the title-bar row, menus in a row below that the active tab flows into; light / dark follow Windows |
| Tabs | Separate windows (File → New Window) | Tabs: Ctrl+T / +, Ctrl+W, Ctrl+Shift+T (reopen closed), Ctrl+Tab / Ctrl+Shift+Tab, middle-click close, drag to reorder, right-click menu (New, Duplicate, Reload, Close, Close others), icons per page kind, titles from the page, restored per server after restart |
| Menus | Win32 menu bar | File / Edit / View / Go / Server / Help buttons opening native Windows 11 menus; one ☰ button when the window is narrow |
| Shortcuts | Worked only with the system menu bar | `keys.rs`: the menu's shortcuts work in every tab and in the bar (WebView2 child webviews don't forward them otherwise) |
| White flash | A new tab painted white before the page (dark mode) | Tabs paint the web app's background (#18212a dark / #fff light), the bar its own |
| Start-up | "Connecting…" card with a spinner, then the page | Splash: the logo on the page colour, a thin progress line — the page takes over without a jump |
| Motion | None (everything appears instantly) | `motion.css` / `motion.ts`: popups rise in when shown (180–280 ms, ease-out), screens slide in, switcher results and panel tabs cascade, controls fade on hover and press down slightly, new tabs slide open; all off with Windows' "reduce motion" |
| Session race | A restored tab loaded before the session cookie existed → `/login` → a second sign-in round trip (and, twice within 30 s, the sign-in screen) | Restored tab loads once the first tab has its session |

Not verified by eye yet (the screen was locked): the motion timings, the splash, and the shortcuts
on a real keyboard. Gate: `npm run build`, `cargo test` (355 + 16 capability tests),
`cargo clippy --all-targets -D warnings` — all green.

## Still open in the desktop shell

1. **Tab switch is instant** (native webviews can't cross-fade). Acceptable — browsers do the same —
   but a 120 ms fade of the bar's active-tab highlight already softens it (160 ms).
2. **Mica / acrylic backdrop** behind the title-bar row (Windows 11). Needs a transparent window;
   on Windows 10 that renders black, so it must be feature-detected. Medium effort.
3. **Tab overflow**: with many tabs they shrink to 64 px; Notepad adds scroll arrows / a tab list
   dropdown. Add a "⌄ all tabs" button when tabs are below ~100 px.
4. **Close-tab animation** (tabs vanish instantly). (Reopen closed tab, Ctrl+Shift+T / File menu, is done: the last 10 per session, pages of the current server only.)
5. **Theme switch while running**: the bar follows instantly (CSS), the webviews' paint colour only
   for new tabs.
6. **Popups' palette** (bluish greys, `native.css`) differs from the bar's neutral Windows greys and
   the web app's navy. Pick one family (the web app's) for all bundled pages.
7. **Settings window** is long and plain; split into a sidebar with sections (General, Shortcuts,
   Servers, Voice, Updates) like Windows 11 Settings.

## Web app (app repo) — where most of the "feels like Windows 95" comes from

### Motion

- **Page changes**: wrap Inertia / router visits in the **View Transitions API**
  (`document.startViewTransition(() => swap())`, supported by WebView2 / Chromium ≥ 111, Safari 18).
  Default cross-fade 150–200 ms, the sidebar and header kept still (`view-transition-name`).
- **Panels, dialogs, popovers, dropdowns**: enter 160–200 ms (`opacity` + `translateY(4px)` /
  `scale(.98)`, ease-out `cubic-bezier(.16,1,.3,1)`), exit 120 ms. Today they pop in.
- **Lists and boards**: new cards / rows fade in; moved cards animate to their place (FLIP, e.g.
  `@formkit/auto-animate`); drag uses a lifted shadow and slight scale.
- **Loading**: skeletons that match the final layout instead of spinners; keep the previous page
  until the next one is ready (no blank frame).
- Respect `prefers-reduced-motion`. Keep durations ≤ 250 ms: motion should explain, not slow down.

### Writing documents

From the doc page as it looks today (read view with an **Edit** button, "Published · v2"):

1. **No mode switch**: click into the text and type (draft autosaves; "Publish" stays explicit).
   The read / Edit toggle is the biggest "old" feeling.
2. **Slash menu** (`/` → headings, lists, to-do, table, code, callout, image, embed, mention) and
   **Markdown shortcuts** (`#`, `-`, `1.`, `[]`, `>`, ``` ``` ```, `**bold**`).
3. **Floating toolbar on selection** (bold, italic, link, code, comment, AI) instead of a fixed
   toolbar.
4. **Block handles**: hover a block → `⋮⋮` to drag, `+` to insert; drop indicator line.
5. **Typography**: body 16 px / 1.65, content width ~720 px (the current page runs ~1200 px wide,
   long lines are tiring), headings with more space above than below, calmer H1 (the emoji +
   40 px H1 + coloured section icons compete).
6. **Autosave status** next to the title ("Saved" / "Saving…"), and presence avatars when someone
   else edits.
7. **Never show your own presence**: the live-editing name label must not appear on your own
   caret. Filter the awareness states by **user id** (not only by Yjs `clientID`): the same user
   in a second tab / window / device is a different client and today shows up with their own
   name where they type. Show others' carets and names only; at most a small "also open in
   another tab" hint for your own other sessions.
8. **Focus mode** (hide sidebar + header, Ctrl+.) and a sticky outline that highlights the current
   section (the "On this page" list does not track scroll).
9. **Smooth caret and selection** (`caret-color` accent; WebView2 supports
   `caret-animation`), and paste that keeps structure (lists, tables, links) from Word / Docs.

Most of this is what TipTap / ProseMirror extensions give out of the box (slash command,
bubble menu, drag handle, placeholder, typography); worth checking what the current editor is
built on before rebuilding anything.
