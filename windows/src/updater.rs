//! "Check for updates" in settings. Every build knows the commit it was built from; the
//! repository's rolling "latest" release says which commit it was built from. If they differ,
//! there's an update. On Windows it is installed in place: the new exe is downloaded next to
//! the running one, checked, and swapped in by renaming (Windows lets a running exe be renamed,
//! not overwritten), then ULTRA restarts from the same path, so taskbar pins and shortcuts keep
//! working. On a Mac the download page opens instead (an app bundle is replaced by hand).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The commit this build comes from (stamped by build.rs; empty if git wasn't available).
pub const COMMIT: &str = match option_env!("ULTRA_COMMIT") {
    Some(c) => c,
    None => "",
};
/// The release file this platform updates from.
#[cfg(windows)]
const ASSET: &str = "ULTRA-windows-x64.exe";
#[cfg(not(windows))]
const ASSET: &str = "ULTRA-macos.zip";

/// A newer build that can be installed.
#[derive(Clone)]
pub struct Update {
    pub url: String,
    pub size: u64,
}

/// Short form of a commit, as shown in settings.
pub fn short(c: &str) -> &str {
    &c[..c.len().min(7)]
}

pub fn releases_page(repo: &str) -> String {
    format!("https://github.com/{repo}/releases/latest")
}

/// The system's own curl (Windows 10+ and macOS ship one), by full path.
fn curl() -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let mut c = std::process::Command::new(sys.join(r"System32\curl.exe"));
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("/usr/bin/curl")
    }
}

/// Ask GitHub about the latest release. Blocking: run it off the UI thread. Returns the state
/// for the settings UI, and the update to install if there is one.
pub fn check(repo: &str) -> (Value, Option<Update>) {
    let out = curl()
        .args(["-sS", "-L", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-H", "User-Agent: ULTRA-updater", "-w", "\n%{http_code}"])
        .arg(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .output();
    let Ok(out) = out else { return (json!({ "s": "failed", "m": "couldn't run curl to reach GitHub" }), None) };
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or((&text, ""));
    match code.trim() {
        "200" => {}
        // A private repository (the experimental build) can't be read without signing in.
        "404" => return (json!({ "s": "private", "m": "updates for this build are on its private GitHub page", "page": releases_page(repo) }), None),
        "" | "000" => return (json!({ "s": "failed", "m": "couldn't reach GitHub · check your connection" }), None),
        c => return (json!({ "s": "failed", "m": format!("GitHub answered {c} · try again later") }), None),
    }
    let Ok(rel) = serde_json::from_str::<Value>(body) else { return (json!({ "s": "failed", "m": "GitHub sent something unexpected" }), None) };
    let latest = rel["target_commitish"].as_str().unwrap_or("").to_string();
    let asset = rel["assets"].as_array().into_iter().flatten().find(|a| a["name"] == ASSET);
    let (Some(asset), false) = (asset, latest.is_empty()) else {
        return (json!({ "s": "failed", "m": "the latest release has no download for this computer" }), None);
    };
    if !COMMIT.is_empty() && latest == COMMIT {
        return (json!({ "s": "uptodate", "m": format!("you have the latest build ({})", short(COMMIT)) }), None);
    }
    let up = Update {
        url: asset["browser_download_url"].as_str().unwrap_or("").to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
    };
    let when = rel["published_at"].as_str().unwrap_or("").split('T').next().unwrap_or("").to_string();
    (json!({ "s": "available", "m": format!("a new build is out ({}, {when})", short(&latest)), "page": releases_page(repo) }), Some(up))
}

/// Where the exe sits while it's being replaced.
fn sibling(exe: &Path, tag: &str) -> PathBuf {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "ultra".into());
    exe.with_file_name(format!("{stem}.{tag}.exe"))
}

/// Download the update and swap it in for the running exe. Blocking. Returns the exe path to
/// restart from (the same path as before).
#[cfg(windows)]
pub fn install(up: &Update) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|_| "couldn't find where ULTRA is running from".to_string())?;
    let (new, old) = (sibling(&exe, "new"), sibling(&exe, "old"));
    let _ = std::fs::remove_file(&new);
    let ok = curl()
        .args(["-sS", "-L", "-f", "--max-time", "300", "-H", "User-Agent: ULTRA-updater", "-o"])
        .arg(&new)
        .arg(&up.url)
        .status()
        .is_ok_and(|s| s.success());
    let fail = |m: &str| {
        let _ = std::fs::remove_file(&new);
        Err(m.to_string())
    };
    if !ok {
        return fail("the download didn't finish · try again");
    }
    // A whole, real Windows program: the size GitHub promised, starting with "MZ".
    let head = std::fs::read(&new).map(|b| (b.len() as u64, b.get(..2) == Some(b"MZ"))).unwrap_or((0, false));
    if head.0 == 0 || (up.size > 0 && head.0 != up.size) || !head.1 {
        return fail("the download looks damaged · nothing was changed");
    }
    // Swap: the running exe steps aside (allowed while running), the new one takes its name.
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| {
        let _ = std::fs::remove_file(&new);
        format!("couldn't replace ULTRA ({e}) · is it in a folder you can't write to?")
    })?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe); // put the old one back
        let _ = std::fs::remove_file(&new);
        return Err(format!("couldn't put the new version in place ({e})"));
    }
    Ok(exe)
}

/// Start the updated ULTRA; it waits for this one to close before taking over.
#[cfg(windows)]
pub fn restart(exe: &Path) -> bool {
    std::process::Command::new(exe).env("ULTRA_WAIT_PID", std::process::id().to_string()).spawn().is_ok()
}

/// A freshly updated ULTRA: wait (briefly) for the old one to finish closing, so it hands
/// over the session instead of being mistaken for an ULTRA that's still running.
#[cfg(windows)]
pub fn wait_for_previous() {
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    let Some(pid) = std::env::var("ULTRA_WAIT_PID").ok().and_then(|p| p.parse::<u32>().ok()) else { return };
    std::env::remove_var("ULTRA_WAIT_PID");
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, 15_000);
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
    }
}

/// Remove the previous exe left beside this one by the last update (best effort).
#[cfg(windows)]
pub fn clean_up() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(sibling(&exe, "old"));
        let _ = std::fs::remove_file(sibling(&exe, "new"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn names_beside_the_exe() {
        let exe = Path::new(r"C:\Apps\ULTRA\ultra-exp.exe");
        assert_eq!(sibling(exe, "old"), Path::new(r"C:\Apps\ULTRA\ultra-exp.old.exe"));
        assert_eq!(short("0123456789abcdef"), "0123456");
        assert_eq!(short("ab"), "ab");
    }
}
