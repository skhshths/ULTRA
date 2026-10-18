//! Crash log. ULTRA is built with panic = "abort", so a Rust panic ends the process on the
//! spot and Windows only records an exit code. The hook below runs first and appends what
//! happened (message, file and line, thread, version, backtrace) to crash.log in the data
//! folder, plus a marker so the next launch can say ULTRA closed unexpectedly. Nothing is sent
//! anywhere: the file stays on this computer.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const LOG: &str = "crash.log";
const MARKER: &str = "crashed";
/// The log keeps roughly the newest this-many bytes.
const KEEP: usize = 256 * 1024;

/// Log every panic (on any thread) before the process goes down.
pub fn install(dir: PathBuf, version: String) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("worker").to_string();
        let trace = std::backtrace::Backtrace::force_capture();
        let entry = format!("=== {} · {version}\nthread: {thread}\n{info}\n\nbacktrace:\n{trace}\n{}\n", stamp(now()), offsets());
        write(&dir, &entry);
        // Only the main thread's panic takes ULTRA down (workers just end): flag that one.
        if thread == "main" {
            let _ = fs::write(dir.join(MARKER), b"");
        }
        previous(info);
    }));
}

fn write(dir: &Path, entry: &str) {
    let path = dir.join(LOG);
    let mut old = fs::read(&path).unwrap_or_default();
    if old.len() + entry.len() > KEEP {
        // Keep the newest part, starting at an entry boundary.
        let cut = old.len() + entry.len() - KEEP;
        let start = old[cut.min(old.len())..].windows(4).position(|w| w == b"=== ").map(|p| p + cut).unwrap_or(old.len());
        old.drain(..start);
    }
    if let Ok(mut f) = fs::File::create(&path) {
        let _ = f.write_all(&old);
        let _ = f.write_all(entry.as_bytes());
        let _ = f.sync_all();
    }
}

/// The same stack as offsets into ULTRA's exe ("ultra.exe+0x1a2b3c"). A downloaded build has no
/// .pdb beside it, so the backtrace above can't name ULTRA's own functions; these offsets can be
/// matched against that release's .pdb later.
#[cfg(windows)]
fn offsets() -> String {
    use windows::Win32::System::{Diagnostics::Debug::RtlCaptureStackBackTrace, LibraryLoader::GetModuleHandleW};
    let mut frames = [std::ptr::null_mut(); 64];
    let n = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
    let base = unsafe { GetModuleHandleW(None) }.map(|h| h.0 as usize).unwrap_or(0);
    let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string())).unwrap_or_default();
    let mut out = String::from("frames in the exe:\n");
    for &f in &frames[..n] {
        let a = f as usize;
        // Only addresses inside the exe image (a generous 64 MB window above its base).
        if base != 0 && a >= base && a - base < 64 << 20 {
            out += &format!("  {exe}+{:#x}\n", a - base);
        }
    }
    out
}

#[cfg(not(windows))]
fn offsets() -> String {
    String::new()
}

/// Whether the last run crashed; clears the flag.
pub fn take_notice(dir: &Path) -> bool {
    fs::remove_file(dir.join(MARKER)).is_ok()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "2026-10-17 19:48:28 UTC" from Unix seconds.
fn stamp(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_and_trimming() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(stamp(1_792_266_508), "2026-10-17 19:48:28 UTC");
        let dir = std::env::temp_dir().join(format!("ultra-crash-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let big = "x".repeat(100 * 1024);
        for i in 0..5 {
            write(&dir, &format!("=== entry {i}\n{big}\n"));
        }
        let log = fs::read_to_string(dir.join(LOG)).unwrap();
        assert!(log.len() <= KEEP && log.starts_with("=== ") && log.contains("entry 4"));
        assert!(!take_notice(&dir));
        fs::write(dir.join(MARKER), b"").unwrap();
        assert!(take_notice(&dir) && !take_notice(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
