//! Slideshow background: a folder of images, shown one at a time. Images are read and
//! encoded on a background thread so even large PNGs never stall the UI.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel, NSWindow};
use objc2_foundation::NSString;

/// Largest image shown (bigger files are skipped).
const MAX_BYTES: u64 = 12 * 1024 * 1024;

/// The standard macOS folder picker, as a sheet on ULTRA's window. `done` gets the folder,
/// or None if cancelled; it runs on the main thread once the sheet closes.
pub fn pick_folder(window: &NSWindow, done: impl FnOnce(Option<String>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setAllowsMultipleSelection(false);
    panel.setCanCreateDirectories(false);
    panel.setMessage(Some(&NSString::from_str("choose a folder of background images")));
    panel.setPrompt(Some(&NSString::from_str("Choose")));
    let done = Cell::new(Some(done));
    let p = panel.clone();
    let handler = RcBlock::new(move |resp: NSModalResponse| {
        let dir = (resp == NSModalResponseOK)
            .then(|| p.URL())
            .flatten()
            .and_then(|u| u.path())
            .map(|s| s.to_string());
        if let Some(f) = done.take() {
            f(dir);
        }
    });
    panel.beginSheetModalForWindow_completionHandler(window, &handler);
}

fn mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Images directly inside `dir`, in name order.
pub fn list(dir: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| mime(p).is_some() && p.metadata().is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Read an image as a data URL.
pub fn load(path: &Path) -> Option<String> {
    let m = mime(path)?;
    let bytes = std::fs::read(path).ok()?;
    Some(format!("data:{m};base64,{}", base64(&bytes)))
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
