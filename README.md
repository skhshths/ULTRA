# ULTRA

A minimalist, tiling, memory-frugal browser written in Rust.

- **Tiling like Hyprland**: split the pane under your cursor, pull any tab into it, zoom it,
  close it, drag the gaps to resize.
- **Workspaces**, plus a password-locked **private workspace** that never touches disk.
- **Light by design**: only visible tabs and a few warm ones stay loaded; hidden tabs freeze,
  then sleep.
- **Command palette**, tab overview, type-anywhere search, history, bookmarks, saved layouts.
- **Glance** link previews, **reader view**, **picture-in-picture**, cookie-banner hiding and a
  "zap" tool for anything else, a **memory cap**, and **separate logins per workspace**.
- **Ad & tracker blocking**, address-bar spoofing protection, phishing protection.
- **Looks**: animated backgrounds, audio-reactive visuals, 12 themes and a colour picker.
  **ULTRASPEED** turns every effect off.

## Pick your platform

| Folder | Platform | Engine | Build |
|---|---|---|---|
| [`windows/`](windows) | Windows 10/11 | Microsoft WebView2 (Chromium) | `cd windows && cargo build --release` → `target/release/ultra.exe` |
| [`mac/`](mac) | macOS 11+ (Apple silicon and Intel) | Apple WKWebView (WebKit) | `cd mac && tools/bundle.sh` → `target/ULTRA.app` |

Each folder is a complete, independent Rust project with its own README covering
requirements, building, where data is stored, and how it works. The two share the same chrome
UI, tiling engine, settings format and features; the macOS README has a table of how each
Windows-specific piece is done on the Mac and which shortcuts change (⌘ in place of ctrl).

Shared files (`layout.rs`, `library.rs`, `ui.html`, `content.js`, `reader.js`) exist in both folders. When
you change one, port the change to the other so the two builds keep behaving the same.
