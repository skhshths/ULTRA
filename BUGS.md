# Bug list (deep check, 2026-10-16)

Found by reading through every feature and by driving a real, isolated copy of ULTRA
Experimental from test scripts (DevTools protocol, no keyboard or mouse simulation).
Not fixed yet.

## Serious (all fixed 2026-10-16, in both versions)


1. ~~FIXED~~ **The public ULTRA still has the audio-bars crash.** The fix (capture in a helper process)
   is only in experimental. Anyone on the public download with audio bars on can crash.
2. ~~FIXED~~ **Any website can control ULTRA.** Page scripts can send ULTRA's shortcut commands
   (`window.ipc.postMessage('new')`, `'close'`, `'ws:3'`, `'lock'`, `'palette'`, …) while the
   page is on screen. Verified: a test page opened a tab in ULTRA.
3. ~~FIXED~~ **Saving isn't crash-safe.** session.json, settings, history, bookmarks and downloads
   are overwritten in place; a crash or power cut mid-write can wipe them.
4. ~~FIXED~~ **Pages that freeze for a few seconds get force-reloaded.** When WebView2 reports a page
   as unresponsive, ULTRA reloads it right away, losing anything typed, instead of waiting.
5. ~~FIXED~~ **Public version: opening ULTRA twice runs two copies on the same profile.** Whichever
   closes last overwrites the other's tabs. (Fixed in experimental by the single-instance work.)

## Medium (all fixed 2026-10-16; 7–10 in both versions, 11 is experimental-only)

6. ~~FIXED~~ **Palette "split right/left/up/down" has no minimum size.** It made 15 panes, the
   smallest 1px wide. (ctrl+shift+e respects the minimum; the palette doesn't.)
7. ~~FIXED~~ **Address bar treats some searches as websites.** `note:meeting` → `https://note:meeting`,
   `3.14` → `https://3.14`: anything without spaces that has a colon or a dot.
8. ~~FIXED~~ **Window size, position and maximized state aren't remembered.** Always 1280×800.
9. ~~FIXED~~ **Sending a private tab to another workspace silently does nothing** (by design it can't
   move, but there's no message).
10. ~~FIXED~~ **Links opened from other apps while the private workspace is locked** land in workspace 1
    in the background, with no sign they arrived.
11. ~~FIXED~~ **Experimental: the coffee strip still shows on the 5th launch, but its button goes
    nowhere** (SUPPORT_URL is empty). The experimental profile also copies the regular launch
    counter, so it can appear on experimental's very first launch.

17. ~~FIXED~~ **Found while fixing these: reopening ULTRA right after closing it could crash.** The
    web engine was still shutting down and startup gave up hard (`.expect`); it now waits for it.

## Minor (all fixed 2026-10-17; 15 and 16 partly experimental-only)

12. ~~FIXED~~ Every settings change, including each step of dragging a slider, writes settings.json and
    re-lays out every pane.
13. ~~FIXED~~ With a glance open, one extra hidden tab can be put to sleep (the glance counts against
    the warm-tabs budget).
14. ~~FIXED~~ Private downloads stay in the downloads list after the private workspace is wiped (until
    restart).
15. ~~FIXED~~ If an old "Zenith" data folder exists and no regular one, the experimental build would take
    it over instead of the regular build.
16. ~~FIXED~~ A doc comment ended up above the wrong function (`open_external` in main.rs). Cosmetic.

## Not verified

- **Mac:** none of the Mac code has run on a real Mac, and the private repo's build status
  isn't visible from here, so whether the latest experimental Mac build compiled is unknown.
- **Real keypresses:** shortcuts arriving from the OS (the native accelerator path) weren't
  exercised; everything else was driven through ULTRA's own command channel and real mouse and
  key events inside the UI.

## Checked and working

Address bar navigation, back/forward (keys and ‹ › buttons), new/close/reopen tab (reopen
returns to the exact pane, side and size), zoom/unzoom, focus between panes, divider dragging
(no text selection), ctrl+shift+e on a full board, workspaces (switch, keep layout, send tab),
private workspace (password gate, lock hides titles, wrong/right password), bookmarks (ctrl+d,
bookmarks bar click), history suggestions, downloads (progress, speed, pause, resume, cancel,
clear), command palette (split, theme, ws, plain text = search), glance (open, keep as tab),
reader view, overview (filter + enter), tab bar (click, middle-click close, right-click sleep),
new-tab search box, type-to-search from a page, settings switches and esc, bookmarks-bar toggle
moving panes, ULTRASPEED, pane stats, layouts (save, load, delete), session restore after a
normal close (private tab correctly not restored), and no script errors in the UI throughout.
