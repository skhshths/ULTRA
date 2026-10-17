# ULTRA for Windows

Just want to use it? [Download the latest ULTRA for Windows](https://github.com/skhshths/ULTRA/releases/latest/download/ULTRA-windows-x64.exe). The macOS version lives in [`../mac`](../mac).

A minimalist, tiling, memory-frugal browser for Windows, written in Rust on top of
WebView2 (via [wry](https://github.com/tauri-apps/wry) and [tao](https://github.com/tauri-apps/tao)).

- **Tiling like Hyprland**: split the pane under your cursor (`ctrl+shift+e`), pull any tab
  into it (`ctrl+shift+1‥9`), zoom it (`ctrl+shift+f`), close it (`ctrl+shift+w`), drag gaps to resize.
- **Workspaces**: `ctrl+alt+1‥9` to switch, `ctrl+alt+shift+1‥9` to send a tab.
- **Light by design**: only visible tabs plus a few warm ones stay loaded; hidden tabs freeze,
  then sleep. Pre-built spare webviews make waking a tab ~5 ms instead of ~80 ms.
- **Command palette** (`ctrl+shift+p`), tab overview (`ctrl+k`), type-anywhere search.
- **Ad & tracker blocking**, address-bar spoofing protection, SmartScreen on.
- **Looks**: animated ASCII swirl, solid colour or your own image as background; 12 themes
  plus a custom colour picker; optional scanlines. **ULTRASPEED** turns every effect off.
- **Glance**: `ctrl+shift+click` a link to preview it floating over your layout. `esc` closes it,
  `ctrl+enter` (or "⧉ split") tiles it next to the focused pane, "keep as tab" keeps it in the background.
- **Reader view** (`F9`, or `reader` in the palette): just the article, in ULTRA's colours. `F9` again or `esc` to leave.
- **Picture-in-picture** (`ctrl+alt+p` with the page focused, or `pip` in the palette): pops the video out.
- **Hide distractions**: cookie banners from the common consent tools are hidden automatically
  (settings → "hide cookie banners"). `zap` in the palette lets you click any element to hide it on
  that site for good; `unzap` brings everything back. Kept in `zaps.json`.
- **Memory cap** (settings → performance): past 1/2/4/8 GB, the tabs you used least long ago are put to
  sleep one at a time. Pages on screen, a glance and tabs playing sound are never touched.
- **Separate logins per workspace** (settings → privacy, or `isolate` in the palette): a lit workspace
  gets its own cookies and sign-ins (a separate WebView2 profile), e.g. work accounts in 1, personal in 2.
  Its button in the top bar gets an underline.
- Notices ("bookmarked", "downloaded"...) appear in the top bar, so a page can never cover them.
- Built-in tutorial (can be re-run from settings, `ctrl+,`).

## Build

From this `windows/` folder:

```
cargo build --release
```

The exe lands in `target/release/ultra.exe`. Requires the Microsoft Edge WebView2 Runtime
(preinstalled on Windows 11). Settings, session and sign-ins live in `%LOCALAPPDATA%\Ultra`.

`tools/make_icon.py` regenerates `assets/ultra.ico` (needs Pillow); `build.rs` embeds it.
