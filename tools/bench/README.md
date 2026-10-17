# Browser stress test: ULTRA vs Arc vs Neo

Measures start-up, opening N tabs, memory, idle CPU and tab switching, the same way in every
browser:

- **Test site:** `server.mjs` serves a realistic page (120 cards, inline SVG, a 400-item feed,
  some script, a ticking clock) from a separate loopback address per tab (`127.0.0.2…61`). Every
  tab is therefore its own site, as in real browsing.
- **Timing:** each page reports `load` and `visibilitychange` back to the server, so times are
  measured from the page's point of view, never from screenshots.
- **Driving:** the DevTools protocol, never simulated keyboard or mouse input:
  - Chromium browsers: `Target.createTarget` / `Page.bringToFront`
  - ULTRA: its own `newurl` / `tab` commands through the experimental `ULTRA_DEVTOOLS_PORT`
- **Memory:** `mem.ps1` adds up every process in the browser's process tree. It reports private
  working set (the "Memory" column in Task Manager) and private bytes, plus CPU over 10 s idle.

```bash
node bench.mjs ultra 1,10,25,50
node bench.mjs ultra-awake 1,10,25,50
node bench.mjs neo 1,10,25,50
node bench.mjs arc 1,10,25,50
```

`ultra` is the normal ULTRA. `ultra-awake` keeps every tab live (`ULTRA_WARM_TABS=999`) for an
engine-to-engine comparison. Results are appended to `results.jsonl`. Build
`windows/target/release/ultra-exp.exe` first.

Each run starts the browser fresh and opens the N tabs back to back. It times how long until
every tab has loaded. It then samples memory and idle CPU 30, 60, 90 and 120 s later, since
browsers do one-off background work shortly after tabs are hidden. Finally it switches to 8
tabs and times each one until the page is visible.

The first-run tour in ULTRA covers the panes, so pages count as hidden; the script closes it.
Occlusion detection is turned off in every browser, so a window sitting behind others doesn't
change the results.

**Profiles:**
- ULTRA gets a fresh temp `LOCALAPPDATA` on every run.
- Neo and Arc use `%TEMP%\ultra-bench-profiles`.
- **Arc keeps its sidebar outside the profile, so it opens your real spaces.** The script
  closes every tab it opened, but don't run it while you're using Arc.
