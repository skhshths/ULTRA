//! Slideshow background: a folder of images, shown one at a time. Images are read and
//! encoded on a background thread so even large PNGs never stall the UI.

use std::path::{Path, PathBuf};

use windows::Win32::{
    Foundation::HWND,
    System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER},
    UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH},
};
use windows_core::HSTRING;

/// Largest image shown (bigger files are skipped).
const MAX_BYTES: u64 = 12 * 1024 * 1024;

/// The standard Windows folder picker. None if cancelled.
pub fn pick_folder(owner: isize) -> Option<String> {
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let _ = dlg.SetTitle(&HSTRING::from("choose a folder of background images"));
        dlg.Show(Some(HWND(owner as *mut _))).ok()?;
        let p = dlg.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
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
