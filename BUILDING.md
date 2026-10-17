# Building ULTRA

ULTRA is written in Rust. The repository holds two complete, independent apps that share the
same chrome UI, tiling engine, settings format and features:

| Folder | Platform | Web engine | Build | Output |
|---|---|---|---|---|
| [`windows/`](windows) | Windows 10/11 | Microsoft WebView2 (Chromium) | `cd windows && cargo build --release` | `windows/target/release/ultra.exe` |
| [`mac/`](mac) | macOS 11+ | Apple WKWebView (WebKit) | `cd mac && tools/bundle.sh universal` | `mac/target/ULTRA.app` |

Each folder has its own README covering requirements, architecture and platform details:
[`windows/README.md`](windows/README.md), [`mac/README.md`](mac/README.md).

## Requirements

- **Rust** (stable) from <https://rustup.rs>.
- **Windows:** the MSVC toolchain (Visual Studio Build Tools, "Desktop development with C++").
  The WebView2 runtime ships with Windows 11.
- **macOS:** Xcode Command Line Tools (`xcode-select --install`).

Run the tests with `cargo test --release` in either folder.

## Shared files

`layout.rs`, `library.rs`, `downloads.rs`, `ui.html`, `content.js` and `reader.js` exist in both `windows/src`
and `mac/src`. When you change one, port the change to the other so the two builds keep
behaving the same. The UI shows platform key names itself (`ctrl`/`alt` on Windows,
`cmd`/`option` on macOS).

## Releases

[`.github/workflows/release.yml`](.github/workflows/release.yml) runs on every push to `main`
(and can be started by hand from the Actions tab). It:

1. builds `ultra.exe` on a Windows runner;
2. builds a universal (Apple silicon + Intel) `ULTRA.app` on a macOS runner and zips it;
3. replaces the release tagged **`latest`** with the new files:
   `ULTRA-windows-x64.exe` and `ULTRA-macos.zip`.

The download links in the README use `releases/latest/download/<file>`, so they always serve the
newest build. Neither build is code-signed: Windows SmartScreen and macOS Gatekeeper show a
warning on first launch, which the README explains how to get past.

## Icons

`tools/make_icon.py` (at the top of the repo) regenerates both platforms' icons, normal and inverted
(needs Python with Pillow). The inverted one is used by the experimental edition.
`windows/build.rs` embeds `windows/assets/ultra.ico` into the exe; `mac/tools/bundle.sh` copies
`mac/assets/ultra.icns` into the app bundle.

## Developer tools

`ultra.exe --bench` (Windows) opens an invisible window, times webview creation, checks
InPrivate and audio capture, and writes the results to `%LOCALAPPDATA%\Ultra\bench.txt`.
