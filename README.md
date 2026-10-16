# ULTRA

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
- Built-in tutorial (can be re-run from settings, `ctrl+,`).

## Build

```
cargo build --release
```

The exe lands in `target/release/ultra.exe`. Requires the Microsoft Edge WebView2 Runtime
(preinstalled on Windows 11). Settings, session and sign-ins live in `%LOCALAPPDATA%\Ultra`.

`tools/make_icon.py` regenerates `assets/ultra.ico` (needs Pillow); `build.rs` embeds it.
