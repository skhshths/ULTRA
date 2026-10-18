# Sign-out sheet

Handoff notes for the next agent working on ULTRA. **Read all of this before you change
anything.** When you finish a session, add your own entry to the [log](#sign-out-log) at the
bottom (newest first) and update anything above that is no longer true.

---

## 1. What ULTRA is

ULTRA is a fast, minimal **tiling web browser** written in Rust, owned by **oliverbarnet**
(GitHub `skhshths`). It runs on two engines and is built as two separate apps:

- **Windows:** `windows/`, using wry 0.57 + tao 0.37 on **WebView2**
- **macOS:** `mac/`, using **WKWebView**

Both apps share a single-file chrome UI (`ui.html`), the tiling engine (`layout.rs`), the
library (`library.rs`) and the settings format. Selling points: panes like a tiling window
manager, dozens of tabs without slowdown (hidden tabs are suspended, then put to sleep), and
built-in ad/tracker blocking.

## 2. The two versions

| | **Public** | **Experimental** (private) |
|---|---|---|
| GitHub | `skhshths/ULTRA` | `skhshths/ULTRA-experimental` (invite only) |
| Local worktree | `C:\Users\olive\code\ultra`, branch `main` | `C:\Users\olive\code\ultra-experimental`, branch `experimental` |
| Push with | `git push origin main` | `git push experimental experimental:main` |
| Name / data folder | ULTRA / `%LOCALAPPDATA%\Ultra` | ULTRA Experimental / `%LOCALAPPDATA%\Ultra Experimental` |
| Windows exe | `ultra.exe` | `ultra-exp.exe` (`[[bin]]` in `windows/Cargo.toml`) |
| Look | normal icon | same colors as public; only the app icon is inverted, plus an `EXP` tag |
| Monetization | **none**: no coffee strip, no `SUPPORT_URL` | coffee strip on 5th launch (`SUPPORT_URL` empty for now) |
| DevTools test switch | off (`TEST_SWITCHES = false`) | on: `ULTRA_DEVTOOLS_PORT=9333` |
| Release on push | new **food-named** release (`v2-banana`, "ULTRA 2 · Banana") | food-named too ("ULTRA Experimental 1 · Apple"); Mac build only for `[mac]` commits, otherwise the newest Mac zip is carried over |
| Updates | in place from public releases | in place too, after a one-time "connect GitHub" in settings (read-only token, DPAPI-encrypted `github-token` in the data folder, sent only to api.github.com via curl stdin) |
| Experimental-only files | none | `BUGS.md`, `SIGNOUT.md` (this file) |

The difference between editions lives in `windows/src/edition.rs` and `mac/src/edition.rs`.
Those files are protected by `.gitattributes` (`merge=ours`), so each branch keeps its own copy
across merges. In a new clone, run this once:

```bash
git config merge.ours.driver true
```

## 3. Standing rules from the owner (do not break these)

1. **New features go to experimental first.** Public only gets work that has been tested in
   experimental, and only when the owner asks for a public release.
2. **Every push to public `main` is a release.** The workflow names it after the next food in
   alphabetical order: Apple (1), Banana (2), so **Cherry (3)** is next, then Date, Elderberry,
   Fig, Grape…  (note: the workflow's list is Apple Banana Cherry Dumpling Eclair Fig Gelato…)
   - Releases so far: ULTRA 1 · Apple, ULTRA 2 · Banana and ULTRA 3 · Cherry (2026-10-17).
   - Don't push trivial commits to public. Batch them.
3. **Mac builds in experimental cost about 10× the Actions minutes.** Put the literal marker
   (square brackets around the word `mac`) in a commit message **only** after a big batch of
   changes.
   - Never write that marker in a commit message by accident, not even when describing the
     rule.
   - Otherwise the experimental release keeps its previous Mac download.
4. **Public keeps its identity:** the name ULTRA, data folder `Ultra`, the dark icon, no coffee
   strip and no monetization of any kind, the exe named `ultra.exe`, and `TEST_SWITCHES = false`.
5. **Never put private details into the public repo:** the experimental repo, `BUGS.md` or this
   file.
6. **Keep the two platforms in step.** A change to a shared file
   (`layout.rs`, `library.rs`, `downloads.rs`, `ui.html`, `content.js`, `reader.js`, and usually
   `main.rs`) must be ported to both `windows/src` and `mac/src`.
7. **Keep the user's local copies current.** After pushing, rebuild the release exe in each
   worktree you touched. `C:\Users\olive\code\ULTRA.lnk` and `ULTRA Experimental.lnk` point at
   those builds.
8. **Commit only when asked or when it's part of the task the owner gave.** Never on public
   without a release request. End commit messages with the `Co-Authored-By` line your harness
   gives you.

## 4. Hard limits (safety)

- **Never** build or describe importing other browsers' **saved passwords, cookies or sign-in
  sessions**. A safety classifier blocked this once. Import covers bookmarks, new-tab
  shortcuts, history, open tabs and search engine only.
- **Don't disturb the owner's real browser profiles or data.** For tests:
  - Run ULTRA with `LOCALAPPDATA` pointed at a temp folder, which gives a fresh, isolated
    profile.
  - Run Chromium browsers with `--user-data-dir`.
  - **Arc** ignores `--user-data-dir` for its sidebar. It always opens the owner's real spaces
    (synced to their account), so close every tab you open in it.
- **Drive apps through the DevTools protocol, not simulated keyboard or mouse input.** The
  owner is often using the computer while you test.
- Security work done on 2026-10-17 (keep it intact):
  - a strict CSP in `ui.html` (`script-src 'nonce-ultra-chrome'`, `connect-src 'none'`); every
    new `<script>` needs that nonce
  - pages may only use the page protocol, never ULTRA commands
  - pop-up limit of 4 per 10 s, web URLs only, on-screen pages only
  - Argon2id passwords
  - the updater only downloads `https://github.com/{REPO}/releases/download/…`
  - atomic saves through `library::write_atomic`

## 5. Build, run, test

```bash
cd windows && cargo build --release
cd windows && cargo test --release
```

The Windows output is `windows/target/release/ultra.exe` (public) or `ultra-exp.exe`
(experimental). For macOS, use `cd mac && sh tools/bundle.sh universal`; it can only be built
on CI from this Windows machine. Run `cargo check` for mac on Windows when possible, but expect
platform errors. Mac code is checked by the release workflow.

- **Before every push:**
  - all tests pass (23 at the time of writing)
  - no compiler warnings
  - the `ui.html` script still parses: extract the `<script nonce="ultra-chrome">` block and
    run `node --check` on it
- **Live testing (experimental only):**
  1. Start with `ULTRA_DEVTOOLS_PORT=9333` and an isolated `LOCALAPPDATA`.
  2. Connect to `http://127.0.0.1:9333/json`, pick the target titled `ULTRA`, and evaluate:

     ```js
     const r = ultra.render; ultra.render = (st) => { window.__S = st; return r(st); };
     window.ipc.postMessage(JSON.stringify({ cmd: 'newurl', arg: 'https://example.com' }));
     ```

  3. Read state back from `window.__S`:
     - `tabs[]` has `{id, t, u, s, w, p, v, g}`, where `s` is home, load, live or sleep
     - `cfg`, `sc`, `me`, `version`
  4. The commands are the `match cmd` arms in `App::command` and `App::ipc` in `main.rs`.
- **Benchmark harness:** see `tools/bench/` (ULTRA vs Arc vs Neo). There is also
  `ultra.exe --bench` for engine micro-timings.
- **Releases:** check that the run went green, that
  `https://api.github.com/repos/skhshths/ULTRA/releases/latest` names the new food, and that
  both `releases/latest/download/ULTRA-windows-x64.exe` and `…/ULTRA-macos.zip` return 200.

## 6. Shipping experimental → public (checklist)

1. Make sure experimental is committed, pushed and green.
2. In `C:\Users\olive\code\ultra`, run `git fetch experimental`, then
   `git merge experimental/main`.
3. Resolve conflicts so that **public keeps:**
   - `.github/workflows/release.yml` (the food-release workflow)
   - its own `edition.rs` (add any new constant with the public value, e.g.
     `TEST_SWITCHES = false`)
   - the README download links to `skhshths/ULTRA`
   - no coffee strip or `SUPPORT_URL` code in `main.rs` / `ui.html`, and no "Support prompt"
     section in `BUILDING.md`
   - no `ultra-exp` `[[bin]]` in `Cargo.toml`
4. Delete `BUGS.md`, `SIGNOUT.md` and `tools/bench/` from public if the merge brought them
   in. Grep for the private repo's name (`ULTRA-experimental`): it must not appear in public.
5. Build, test, and launch once to check that the public build ignores `ULTRA_DEVTOOLS_PORT`.
6. Push `origin main`. The workflow creates "ULTRA N · Food". Verify it as in §5.

## 7. Architecture cheat sheet

- **`main.rs`:** the `App` struct and event loop.
  - Native shortcuts arrive as `UserEvent::Key` on Windows (`on_native` on mac).
  - Messages from pages are `UserEvent::Content`, handled by `page_msg(slot, msg, native)`.
  - Commands from the UI go through `App::command` / `ipc`.
  - `flush()` sends state to the UI's `ultra.render(state)`.
- **Tabs:**
  - Up to `DEFAULT_WARM` (3) hidden tabs stay live, plus the visible panes. Hidden live tabs
    are suspended after 20 s and slept after 10 min, and least-recently-used tabs are slept
    beyond the warm count.
  - `ULTRA_WARM_TABS` (env) sets the default.
  - Restored tabs start asleep.
- **`layout.rs`:** the pane tree, splitters and minimum pane size (about 60 px).
- **`library.rs`:** bookmarks, history, new-tab shortcuts and import.
  - Import reads copies of History/places.sqlite with rusqlite, parses Chromium SNSS session
    files, and decodes Firefox mozLz4.
  - Stale `ultra-import-*` temp folders are cleared at startup.
- **`identity.rs`:** ULTRA ID. An ed25519 key pair, an ID code
  `ULTRA-XXXX-XXXX-XXXX-XXXX` from SHA-256, and the private key encrypted with DPAPI (Windows)
  or stored in a 0600 file (mac). The UI never sees the key.
- **`privacy.rs`:** private-workspace password (Argon2id; the old SHA-256 format is upgraded on
  unlock), lock after 5 min idle, wipe after 5 wrong tries.
- **`updater.rs`:**
  - `build.rs` stamps `ULTRA_COMMIT` and `ULTRA_RELEASE`.
  - Downloads use the System32 `curl`.
  - The running exe is swapped by renaming, then ULTRA relaunches with `ULTRA_WAIT_PID`.
- **`instance.rs`:** single instance through a named pipe derived from the data folder.
- **`audio.rs` / `spectrum.rs`:** audio bars. Capture runs in a helper process (it used to
  corrupt the heap in-process).
- **`blocker.rs`:** ad/tracker filters; `content.js` is injected into pages (declutter,
  page protocol).

## 8. Open threads and ideas

- **ULTRA ID → social features (planned):** friends, chat, then collaborative workspaces where
  different people control different tabs. Each ID already has a signing key (`identity::sign`)
  for this.
  - A Raspberry Pi 5 was discussed as a short-term relay server. It's fine for a few hundred to
    a few thousand idle WebSocket connections. Collaborative tab streaming would need a lot
    more.
  - No server exists yet.
- **"Apps in panes" (asked 2026-10-17):** embedding native Windows apps with `SetParent` is
  possible but fragile:
  - input, focus and DPI problems
  - elevated, UWP and anti-cheat apps refuse it
  - mac can't do it at all

  Web apps and PWAs in panes are the practical route.
- **Accepted low risks (same-user only):** someone squatting the named pipe; history kept in
  plain JSON like every browser; the mac identity file uses 0600 permissions, not the Keychain.
- **Mac** is less tested than Windows. Anything Windows-only (default-browser button, DPAPI,
  in-place updates) has a simpler mac fallback.

## 9. To-do (from the owner, 2026-10-17): do these first

1. **Ad blocker, next phases.** Phase 1 shipped (Windows): Brave's `adblock` engine with
   EasyList, EasyPrivacy, uBlock filters and privacy, and Peter Lowe's lists (about 151k rules).
   - The lists live in `windows/assets/filters/` and are packed into the exe by `build.rs`. The
     engine is cached as `adblock.dat` in the data folder.
   - Coverage: every subresource, frames, each site's own hide rules, and the ◆ shield with a
     per-site off switch.
   - Still to do:
     - generic element hiding (class/id rules)
     - scriptlets / YouTube ads
     - weekly list updates
   - Mac (2026-10-18): the same lists become WebKit content rules at build time
     (`mac/build_rules.rs`, about 143k rules in 4 lists of up to 45k; every list ends with all
     the exceptions). Shield and per-site switch work like Windows. Type-checked only: the owner
     should try it on a real Mac.
   - Don't set Edge tracking prevention to Strict: it blocks before ULTRA's handler, so the
     shield can't count those.
   - Note: requests started by DevTools `Runtime.evaluate` bypass WebResourceRequested. Test
     with real page loads (`tools/bench/sites.mjs`).
2. **The privacy checks aren't working either** (owner report: tracker/privacy test sites, e.g.
   coveryourtracks.eff.org, show no protection). Probably the same root cause as #1, plus no
   fingerprinting or third-party-cookie protection. Ask the owner which checks they ran if
   unclear.
3. **Find today's crashes.** Every panic is now written to `crash.log` in the data folder
   (palette: **crash log**); the next launch says so once.
   - `crash.log` has file:line in local builds, which have the .pdb, plus `exe+offset` frames
     for downloaded builds.
   - One real crash was caught while testing: tao `event_loop/runner.rs:246` "event handler is
     re-entrant", at startup, under a test harness. Something re-enters the event loop from
     inside a handler (a modal dialog? synchronous WebView2 creation?). It is a strong
     candidate for the owner's 0xc0000409 crashes.
4. **Faster waking of sleeping tabs.** The pool of 2 spare webviews only refills after 1.2 s of
   quiet (`SPARES`, `SPARE_IDLE` in `main.rs`). Back-to-back wakes then cost about 0.5–0.65 s
   instead of about 80 ms. Opening many tabs at once is also ~1.7× slower than Neo, for the
   same reason (webview creation per tab).

---

## Sign-out log

Newest first. Copy the template for your entry.

```
### YYYY-MM-DD — <agent / model>
**Did:** what changed, where (experimental / public), commits.
**Verified:** what you tested and how; what you could NOT verify.
**Left off:** the exact next step, anything half-done, open questions for the owner.
```

### 2026-10-18 — Claude (Opus 5.5)
**Did:** the real ad blocker (phase 1, Windows) and hard numbers.
- On 5 ad-heavy sites, 3 cold loads each, ULTRA let 2 of 33 requests to known ad/tracker
  servers through. Edge (Chrome's engine) let 668 of 753 through; Neo let 168 of 293 through.
- ULTRA made half the requests of Edge and loaded the five sites in 4.6 s in total, against
  9.1 s for Edge and 12.4 s for Neo.
- Memory at 50 tabs: ULTRA ~320 MB, Edge ~1.34 GB, Neo ~3.09 GB.
- Chrome itself won't start on the owner's PC (blocked), so Edge stands in for it.

### 2026-10-17 (late night) — Claude (Opus 5.5)
**Did:**
- Fixed typing randomly losing focus: wry gives new web views focus by default, so spares built
  in the background stole it. Now `.with_focused(false)` in `build_view`. Reproduced before and
  after with a DevTools test.
- Shipped everything to public as **ULTRA 3 · Cherry** (merge `fa654cd`).
- Repo-name text in the GitHub-connect UI is now generic, so public never names the private
  repo.

**Left off:** §9 to-dos.

### 2026-10-17 (night) — Claude (Opus 5.5)
**Did (experimental):**
- Crash log with a next-launch notice and a `crash log` palette command; release builds keep
  line tables in the .pdb.
- Experimental releases are food-named.
- "Disconnect GitHub" is a real button.
- The owner fixed Ctrl+Shift+E and cleared the Arc sidebar themselves.

**Verified:**
- A deliberate crash (`test_panic`, testing builds only) logged file:line.
- The flag showed once on relaunch.
- Tests pass.

**Note:** the owner's experimental settings still have a light-pink `bgcolor` (`#ead4d4`), picked
while the UI was inverted. They were told to set it back to black in settings.

### 2026-10-17 (evening) — Claude (Opus 5.5)
**Did (experimental):**
- Ctrl+R reloads the tab from inside pages too.
- The experimental updater installs straight from the private repo once a read-only GitHub
  token is connected in settings.
- New app icon: a bold mint U on a dark tile (inverted for experimental). Regenerate with
  `tools/make_icon.py`.
- Earlier the same evening: the UI is no longer inverted (only the icon is), suggested
  shortcut tiles were removed, and ctrl shortcuts are no longer dropped (see that commit).

**Verified:**
- Tests pass; the UI script parses.
- Live: private-repo detection, the connect row, bad-token rejection, the pre-filled GitHub
  token page, a fake token getting a 401 (so the header is sent), and the stored token being
  an encrypted DPAPI blob.
- Not verified: a real token download end to end (the owner has to paste theirs).

**Left off:** §9 to-dos; public hasn't received any of tonight's changes.

### 2026-10-17 — Claude (Opus 5.5)
**Did:**
- Experimental:
  - Security pass (CSP, pop-up limits, Argon2id, updater URL check, DevTools switch gated,
    stale import cleanup, wipe confirmation)
  - customizable new-tab shortcuts (right-click to rename or remove; settings switch)
  - ULTRA ID
  - fuller import from Chrome, Edge, Brave, Vivaldi, Opera and Firefox
  - updater with food release names; default-browser button fix
  - every item in `BUGS.md` fixed
- Public: shipped as **ULTRA 1 · Apple**, then **ULTRA 2 · Banana** (merge `a0fd016`).
- Ran a stress test of ULTRA against Arc and Neo at 1/10/25/50 tabs; the harness is in
  `tools/bench/`. Memory is private working set across each browser's processes; tabs are
  local "realistic" pages, each its own site.

  Steady state, 2 minutes after the last tab loaded:

  | | 1 tab | 10 tabs | 25 tabs | 50 tabs |
  |---|---|---|---|---|
  | ULTRA (default; hidden tabs sleep) | ~270 MB | ~300 MB | ~305 MB | ~320 MB |
  | ULTRA, all tabs awake | ~262 MB | ~415 MB | ~840 MB | ~1.29 GB |
  | Neo | ~392 MB | ~815 MB | ~1.49 GB | ~3.09 GB, plus ~10% CPU (about 2.3 cores) that never settled |
  | Arc (30 s snapshot only) | ~636 MB | ~611 MB | ~1.0 GB | crashed |

  - Time until all tabs loaded at 10/25/50 tabs:
    - ULTRA: 1.1 / 3.1 / 6.6 s
    - Neo: 0.9 / 1.7 / 4.2 s
    - Arc: 0.5 / 1.1 s, then crashed at 50
  - First page: ULTRA ~70 ms, Neo 120–390 ms, Arc 80–250 ms.
  - Switching to a live tab: ULTRA-awake 15–80 ms, Neo 14–185 ms, Arc ~35–50 ms. Arc ignored
    many of the scripted switches.
  - Idle CPU settles near 0.1% in ULTRA. Expect a ~15 s burst about 20 s after tabs hide, when
    they are suspended and compacted. Neo's on-device AI embedder adds a burst after loading.

**Verified:**
- 23 unit tests pass.
- Live DevTools-driven checks of every feature above.
- The Banana release exists with both downloads returning 200.
- Mac changes compile on CI but were not run on a real Mac.

**Left off:** see §9 To-do (ad blocker and privacy checks first). Later:
- the ULTRA ID friend/chat server design
- a code-signing certificate, so SmartScreen and Gatekeeper stop warning
- a real-Mac test pass
