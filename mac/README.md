# ULTRA for macOS

The macOS build of ULTRA, a minimalist, tiling, memory-frugal browser. It is the same app as
[`../windows`](../windows): same chrome UI, layout engine, workspaces, settings, history,
bookmarks, private workspace, themes and backgrounds. The difference is underneath: it runs on
Apple's **WKWebView** (WebKit, the engine Safari uses) instead of Microsoft's WebView2, through
the same Rust libraries ([wry](https://github.com/tauri-apps/wry) and [tao](https://github.com/tauri-apps/tao)).

> **Status:** this port compiles cleanly for both Apple silicon (`aarch64-apple-darwin`) and
> Intel (`x86_64-apple-darwin`), and the UI was run in a headless browser with no errors. It was
> written without access to a Mac, so it has **not been launched on real macOS yet**. Run
> through the checklist at the bottom on your first build.

---

## Requirements

| What | Version | Why |
|---|---|---|
| macOS | 11 Big Sur or newer | WKWebView page zoom APIs |
| macOS (optional) | 14.2 Sonoma or newer | Audio-reactive visuals and audio bars (Core Audio process taps). On older systems everything else works and those visuals stay still. |
| Rust | stable, 1.80+ | Install from <https://rustup.rs> |
| Xcode Command Line Tools | any recent | Linker and `codesign`: `xcode-select --install` |

There is nothing else to install: WebKit ships with macOS.

## Build and run

```sh
cd mac
cargo run --release          # quick run from the terminal
```

To get a real app you can keep in `/Applications` and the Dock:

```sh
tools/bundle.sh              # builds target/ULTRA.app for this Mac
tools/bundle.sh universal    # one app for both Apple silicon and Intel
open target/ULTRA.app
cp -R target/ULTRA.app /Applications/
```

Use the `.app` for everyday use. macOS ties permission prompts (camera, microphone, audio
visuals) to an app's bundle and `Info.plist`; a bare `cargo run` binary borrows your terminal's
permissions instead.

`../tools/make_icon.py` regenerates `assets/ultra.icns`, `ultra-inv.icns` and `ultra.png` for both
platforms (needs Pillow: `pip3 install pillow`). `tools/bundle.sh` picks the one matching `src/edition.rs`.

### Benchmark mode

```sh
target/release/ultra --bench
```

Builds webviews with the window hidden and writes timings to
`~/Library/Application Support/Ultra/bench.txt`. It also checks that private tabs are
non-persistent and share one store, and counts the WebKit processes ULTRA owns.

## Where things are stored

| What | Where |
|---|---|
| Settings, session, history, bookmarks, layouts, background image | `~/Library/Application Support/Ultra/` |
| Cookies, sign-ins, site data, cache | WebKit's own per-app store (`~/Library/WebKit/` and `~/Library/Caches/`) |
| Private workspace | Memory only. One in-memory WebKit store shared by the private tabs, thrown away when they close or the workspace is wiped. |

The files in `Application Support/Ultra` use the same format as on Windows, so you can copy
`settings.json`, `layouts.json`, `history.json` and `bookmarks.json` between the two.

## Keyboard

Every shortcut is the Windows one with **⌘ in place of ctrl** and **option in place of alt**.
The help text inside the app (tutorial, settings, tooltips) uses the Mac names.

| Action | Windows | Mac |
|---|---|---|
| New tab / close tab | ctrl+t / ctrl+w | ⌘T / ⌘W |
| Jump to tab 1‥9 | ctrl+1‥9 | ⌘1‥9 |
| Cycle tabs | ctrl+tab | **ctrl+tab** (⌘tab is the macOS app switcher) |
| Reopen closed tab | ctrl+shift+t | ⌘⇧T |
| Split the pane under the cursor | ctrl+shift+e | ⌘⇧E |
| Pull tab N into the pane under the cursor | ctrl+shift+1‥9 | ⌘⇧1‥9 (use **ctrl+shift+3/4/5**: macOS keeps ⌘⇧3/4/5 for screenshots) |
| Zoom pane / close pane under cursor | ctrl+shift+f / ctrl+shift+w | ⌘⇧F / ⌘⇧W |
| Focus pane left/right/up/down | ctrl+alt+arrows | ⌘⌥arrows |
| Workspace 1‥9 / private | ctrl+alt+1‥9 / ctrl+alt+0 | ⌘⌥1‥9 / ⌘⌥0 |
| Send tab to workspace | ctrl+alt+shift+1‥9 | ⌘⌥⇧1‥9 |
| Lock private workspace | ctrl+alt+l | ⌘⌥L |
| Address bar / all tabs / palette | ctrl+l / ctrl+k / ctrl+shift+p | ⌘L / ⌘K / ⌘⇧P |
| Bookmark / settings | ctrl+d / ctrl+, | ⌘D / ⌘, |
| Back / forward | alt+← / alt+→ | ⌘[ / ⌘], mouse buttons 4/5, two-finger swipe |
| Reload | ctrl+r, F5 | ⌘R, F5 |
| Search or go to a site | ctrl+l / ctrl+f | ⌘L / ⌘F |
| Downloads panel | ctrl+j | ⌘J |
| Page zoom | ctrl+= / ctrl+- / ctrl+0 | ⌘= / ⌘- / ⌘0, or pinch |
| Print / developer tools | ctrl+p / F12 | ⌘P / ⌘⌥I or F12 |
| Copy, paste, undo, select all | ctrl+c/v/z/a | ⌘C/V/Z/A |
| Quit | the ✕ button | ⌘Q, the red window button, or Quit from the Dock |
| Glance (preview a link) | ctrl+shift+click | ⌘⇧click; esc closes, ⌘enter tiles it |
| Reader view | F9 | F9 or ⌘⇧R (as in Safari) |
| Picture-in-picture | ctrl+alt+p | ⌘⌥P |

Holding **ctrl** instead of ⌘ also works for any shortcut that includes shift or option. A
plain ctrl+letter is left alone, because every Mac text field uses those for editing
(ctrl+a start of line, ctrl+k delete to end of line, and so on).

ULTRA catches its shortcuts natively, before the page sees them, so they work in every frame:
video players, ad iframes, PDFs, and sites that swallow keys. On Windows that is WebView2's
`AcceleratorKeyPressed`; on the Mac it is one `NSEvent` local monitor (`src/macos.rs`).

## How each Windows feature maps to macOS

| Feature | Windows (WebView2) | macOS (WebKit) |
|---|---|---|
| Page engine | WebView2 (Chromium/Edge) | WKWebView (WebKit/Safari) |
| Window | Borderless, ULTRA draws ─ □ ✕ and resize grips | Native window: traffic-light buttons sit over the left of ULTRA's top bar; resize from the edges; rounded corners, shadow, full screen |
| Shortcuts in every frame | `AcceleratorKeyPressed` per webview | One `NSEvent` local monitor for the window |
| Browser keys inside pages (reload, back, zoom, print, devtools) | Built into WebView2 | Routed to the pane by ULTRA |
| Copy/paste | Built in | App menu bar with an Edit menu (WKWebView has no copy/paste without one) |
| Sleeping hidden tabs | `TrySuspend` + memory level "low" | WebKit suspends hidden pages by itself (`inactiveSchedulingPolicy = suspend`); ULTRA's warm-tab budget and 10-minute sleep work the same |
| Tab playing audio (never put to sleep) | `IsDocumentPlayingAudio` | WebKit's `_isPlayingAudio` flag, the one Safari's tab speaker uses |
| Ad & tracker blocking | Per-domain request filters, empty 403 | The same domain list compiled into a WebKit content-rule list. Pages and frames themselves are never blocked; the blocked counter is fed by WebKit's rule notifications. |
| Pop-up sign-ins (OAuth) | Real popup window keeping `window.opener` | Same: wry opens a real window for sized `window.open` calls |
| Private workspace | WebView2 InPrivate profile | One shared non-persistent `WKWebsiteDataStore` |
| Hide private tabs from screen capture | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` | `NSWindow.sharingType = none` (see limitations) |
| Idle lock | `GetLastInputInfo` | `CGEventSourceSecondsSinceLastEventType` |
| Pane stats | Process list from WebView2, memory/CPU from Win32; per pane: JS heap | WebKit helper processes found through macOS's "responsible process" attribution (what Activity Monitor uses), memory/CPU from `proc_pid_rusage`; per pane: that page's process memory **and** CPU |
| Audio-reactive visuals | Windows mixer peak meter (no permission needed) | Core Audio process tap on ULTRA's own processes (macOS asks once for **System Audio Recording**) |
| Audio bars | WASAPI process loopback + FFT | The same tap + the same FFT |
| Crash recovery | Renderer crash → reload, twice → sleep; engine crash → relaunch | Page process crash → reload, twice → sleep; WebKit restarts its shared processes itself, so no relaunch is needed |
| Downloads | Saved to Downloads; ULTRA's downloads panel with live progress, pause, resume and cancel | Saved to ~/Downloads, numbered instead of overwriting; the same downloads panel, without byte progress or pause (wry reports only start and finish) |
| Slideshow folder picker | Windows folder dialog | Native folder picker as a sheet on the window |
| Phishing protection | SmartScreen | WebKit's fraudulent-website warnings (Safe Browsing) |
| Separate logins per workspace | A separate WebView2 profile per workspace | A separate persistent WebKit data store per workspace (`WKWebsiteDataStore(forIdentifier:)`, **macOS 14+**; on older systems the setting says so and stays off) |
| Glance, reader view, cookie-banner hiding, zap, memory cap | Same code on both (the page scripts `content.js` / `reader.js` are shared) | Same |
| Picture-in-picture | Chromium's `requestPictureInPicture` | WebKit's `requestPictureInPicture`, with Safari's `webkitSetPresentationMode` as fallback |
| App icon | `.ico` embedded by `build.rs` | `.icns` in the app bundle; set at runtime for `cargo run` |

## Features beyond browsing (same as Windows)

- **Glance**: ⌘⇧click a link to preview it floating over the layout; esc closes, ⌘enter or
  "⧉ split" tiles it, "keep as tab" keeps it in the background.
- **Reader view**: F9 / ⌘⇧R / `reader` in the palette.
- **Picture-in-picture**: ⌘⌥P with the page focused, or `pip` in the palette.
- **Hide distractions**: cookie banners from common consent tools hidden automatically (settings);
  `zap` in the palette hides any element on a site for good, `unzap` undoes it (`zaps.json`).
- **Memory cap**: settings → performance (1/2/4/8 GB); least-recently-used hidden tabs sleep past it.
- **Separate logins per workspace**: settings → privacy or `isolate` in the palette (macOS 14+).
- Notices show in the top bar, never under a page.

## Support prompt

On the fifth launch only, a slim "buy us a coffee" strip offers a way to support ULTRA. Dismiss it and it never comes back.

(Maintainers: where the button points is set in `SUPPORT_URL`; see [BUILDING.md](../BUILDING.md#support-prompt-maintainers).)

## Permissions you may see

* **System Audio Recording**: the first time a tab plays sound with "audio reactive" or
  "audio bars" on (macOS 14.2+). ULTRA taps only its own processes, keeps samples in memory
  and records nothing. Refuse it and the visuals simply stay still. You can change it later in
  System Settings → Privacy & Security → Screen & System Audio Recording.
* **Camera, microphone, location**: only when a website asks, as in Safari.

## Known differences and limitations

* **Engine differences.** Pages render with WebKit, so a site that only tests on Chrome can
  look or behave slightly differently, as it would in Safari. Chrome extensions don't exist in
  either build.
* **Screen-capture hiding** uses `NSWindow.sharingType`. Screenshots, the system screen
  recorder and most capture tools respect it, but some newer apps built on ScreenCaptureKit
  can still capture the window. Treat it as a privacy curtain, not a guarantee.
* **Pane stats** show the memory of each page's own WebKit process. WebKit shares its
  Networking and GPU processes between pages; those count toward the total in the top bar.
* `_isPlayingAudio`, `_webProcessIdentifier` and the content-rule notification are WebKit
  private interfaces (as is the developer-tools switch wry uses). Every call checks that the
  interface exists first; on a WebKit without them the related feature degrades quietly
  (no audio-aware sleeping, no per-pane stats, blocked count stays 0) instead of crashing.
  Private interfaces are fine for an app you distribute yourself but would not pass Mac App
  Store review.
* Distributing to other people: an ad-hoc signed app shows Gatekeeper's "unidentified
  developer" warning on other Macs. Sign with a Developer ID certificate and notarize for that.

## First-run checklist

Because this port has not been run on real hardware yet, please check these on your first
build and report anything that is off:

1. Window opens without a white flash; the traffic lights sit centred in the top bar (if
   not, adjust `TRAFFIC_LIGHTS` in `src/main.rs`).
2. Drag the window by an empty part of the top bar; double-click it to zoom.
3. ⌘T, ⌘W, ⌘1, ⌘⇧E, ⌘⌥2 with the focus inside a web page *and* in the address bar.
4. ⌘C / ⌘V in a page's text field.
5. Download a file: the ↓ appears in the top bar; ⌘J lists it with open / show in Finder.
6. Play a YouTube video with "audio reactive" on: accept the permission prompt and the
   background should pulse.
7. Settings → pane stats on: each pane header should show MB and CPU.
8. Open a site with ads: the "blocked" counter in the top bar should rise.
9. ⌘Q, reopen: the session comes back.

## Code map

| File | What it does |
|---|---|
| `src/main.rs` | The app: tabs, tiling, workspaces, sleeping, session, settings, commands. Same structure as the Windows `main.rs`; platform calls swapped. |
| `src/macos.rs` | macOS glue: key/mouse monitor and shortcut mapping, menu bar, audio and process probes, page setup, zoom, Dock icon. |
| `src/blocker.rs` | Ad/tracker domain list → WebKit content-rule list; blocked-request counter. |
| `src/audio.rs` | Core Audio process tap (level meter + samples for the bars). |
| `src/spectrum.rs` | FFT and band smoothing for the audio bars (no platform code). |
| `src/stats.rs` | Finds ULTRA's WebKit processes; memory and CPU per process. |
| `src/privacy.rs` | Password hashing, idle time, screen-capture hiding. |
| `src/slideshow.rs` | Folder picker sheet; loading images for the slideshow background. |
| `src/layout.rs`, `src/library.rs`, `src/downloads.rs` | Tiling tree; history, bookmarks, daily digest; the downloads list (shared with Windows). |
| `src/ui.html`, `src/content.js` | The chrome UI and the script injected into pages (Mac labels). |
| `src/reader.js` | Reader view (shared with Windows). |
| `bundle/Info.plist`, `tools/bundle.sh` | App bundle metadata and the script that builds `ULTRA.app`. |

## Checking the build from Linux

The project can be type-checked without a Mac. One dependency compiles a tiny Objective-C
file, so point it at clang:

```sh
rustup target add aarch64-apple-darwin
CC_aarch64_apple_darwin=clang cargo check --target aarch64-apple-darwin
```

Tests need a Mac: `cargo test`.
