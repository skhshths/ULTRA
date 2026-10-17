# ULTRA

**A fast, minimal, tiling web browser.** Split your screen into panes like a tiling window
manager, keep dozens of tabs open without your computer slowing down, and browse ad- and
tracker-free.

## ⬇ Download

| | |
|---|---|
| **Windows 10 / 11** | **[Download ULTRA for Windows (.exe)](https://github.com/skhshths/ULTRA/releases/latest/download/ULTRA-windows-x64.exe)** |
| **macOS 11+** (Apple silicon & Intel) — *early preview* | **[Download ULTRA for Mac (.zip)](https://github.com/skhshths/ULTRA/releases/latest/download/ULTRA-macos.zip)** |

Always the newest build: these links point at the [latest release](https://github.com/skhshths/ULTRA/releases/latest),
which is rebuilt automatically every time ULTRA is updated.

### Windows: first launch

1. Download `ULTRA-windows-x64.exe` and double-click it. There's nothing to install. You can
   move the file anywhere you like (your Desktop, or `C:\Program Files\ULTRA`) and pin it to
   the taskbar.
2. If Windows shows **"Windows protected your PC"**, click **More info → Run anyway**. ULTRA
   isn't code-signed yet, so Windows doesn't recognise it.
3. ULTRA uses Microsoft's WebView2 engine, which comes with Windows 11. On an older Windows 10
   that doesn't have it, install the free
   [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/#download-section).

### Mac: first launch (early preview)

1. Download `ULTRA-macos.zip`, open it, and drag **ULTRA.app** into your **Applications** folder.
2. The first time, **right-click ULTRA → Open → Open**. ULTRA isn't notarised by Apple yet, so a
   normal double-click is blocked once.
3. If macOS says the app **"is damaged"**, open Terminal and run
   `xattr -dr com.apple.quarantine /Applications/ULTRA.app`, then open it again.

The Mac version is new and less tested than the Windows one. If something doesn't work,
please [open an issue](https://github.com/skhshths/ULTRA/issues).

## Coming from Chrome?

Bring your bookmarks with you in one click:

- The welcome tour offers it right after you first open ULTRA, **or**
- open **Settings** (`ctrl+,`) → **Import from Chrome** → **import bookmarks**, **or**
- open the command palette (`ctrl+shift+p`) and type `import chrome`.

Your bookmarks show up in the bookmarks bar under the address bar. Chrome can stay open, and
nothing in Chrome is changed.

## Getting started

When ULTRA opens, a short tour walks you through everything. You can replay it any time from
**Settings → Help → tutorial**. The basics:

- **Search or type an address** in the box at the top, or just start typing on any page.
- **Split the screen:** point at a side of the window and press `ctrl+shift+e`. The pane under
  your mouse splits toward that side. Do it again on any pane to split further.
- **Move a tab into a pane:** point at a pane and press `ctrl+shift+` and the tab's number.
- **Switch which pane is active:** click it, or press `ctrl+alt+arrow keys`.
- **Close the pane under your mouse:** `ctrl+shift+w`.
- **Every shortcut** is listed in **Settings → Keyboard shortcuts** (or type `shortcuts` in the
  command palette). On a Mac, use `cmd` instead of `ctrl` and `option` instead of `alt`.

### Most useful shortcuts

| Shortcut | What it does |
|---|---|
| `ctrl+t` / `ctrl+w` | New tab / close tab |
| `ctrl+l` or `ctrl+f` | Go to the address bar: search, or type a site to go there |
| `ctrl+j` | Downloads |
| `ctrl+d` | Bookmark the page |
| `ctrl+k` | See all your tabs |
| `ctrl+shift+p` | Command palette: type what you want to do |
| `ctrl+shift+e` | New pane where your mouse is |
| `ctrl+shift+f` | Zoom the pane under your mouse to full size (press again to go back) |
| `ctrl+shift+r` | Turn the pane under your mouse back into a blank tab |
| `ctrl+alt+1…9` | Switch workspace (each one is its own set of panes) |
| `ctrl+alt+0` | Private workspace |
| `ctrl+,` | Settings |

## What you get

- **Tiling panes and workspaces.** See several sites side by side, keep work and personal
  browsing on separate workspaces, and save layouts to reopen later (`layout save` / `layout load`
  in the command palette).
- **Lots of tabs, little memory.** Only the tabs you can see stay fully loaded. Hidden tabs
  pause, then sleep, and wake instantly when you come back. Music keeps playing.
- **Ad and tracker blocking** built in, plus automatic cookie-banner hiding.
- **A private workspace** (`ctrl+alt+0`) that saves nothing at all, is hidden from screenshots and
  screen sharing, and locks with a password when you step away.
- **Separate logins per workspace**, so you can be signed into two accounts at once.
- **A clean downloads panel** (`ctrl+j`, or the ↓ in the top bar): progress and speed for each
  download, pause or cancel, open the file or show it in its folder.
- **Handy extras:** link previews (`ctrl+shift+click` a link), reader view, picture-in-picture for
  videos, history and bookmark suggestions as you type, and a "today" summary on new tabs.
- **Make it yours:** 12 colour themes plus a colour picker, animated backgrounds (or your own
  picture or a slideshow of a folder), audio-reactive visuals and music bars.
- **ULTRASPEED** in Settings turns every visual effect off for maximum speed.

## Your data and privacy

Everything ULTRA remembers (settings, open tabs, history, bookmarks) stays on your computer:
in `%LOCALAPPDATA%\Ultra` on Windows, or `~/Library/Application Support/Ultra` on a Mac. Nothing
is sent anywhere. Settings → Privacy → **clear history** forgets your history at any time.

## Help and feedback

Found a bug or have an idea? [Open an issue](https://github.com/skhshths/ULTRA/issues).

Want to build ULTRA yourself or change it? See [BUILDING.md](BUILDING.md).
