# wry 0.57.0, patched for ULTRA

An unchanged copy of wry 0.57.0 from crates.io (license: MIT or Apache-2.0, see LICENSE-*),
with one change in `src/webview2/mod.rs` and `src/wkwebview/mod.rs`: the `window.ipc` object
wry gives every page is defined with `configurable: true`.

Why: wry defined it as a locked (non-configurable) global, so any page whose own code declares
a top-level `ipc` (Google Docs does) crashed with "Identifier 'ipc' has already been declared"
and never loaded. ULTRA's scripts always call `window.ipc.postMessage(...)` explicitly, so a
page's own `ipc` doesn't affect them.

Used through `[patch.crates-io]` in windows/Cargo.toml and mac/Cargo.toml. When upgrading wry,
re-apply this change (or drop the patch if upstream has fixed it).
