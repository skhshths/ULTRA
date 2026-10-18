//! "Check for updates" in settings. Every build knows the commit it was built from; the
//! repository's rolling "latest" release says which commit it was built from. If they differ,
//! there's an update. On Windows it is installed in place: the new exe is downloaded next to
//! the running one, checked, and swapped in by renaming (Windows lets a running exe be renamed,
//! not overwritten), then ULTRA restarts from the same path, so taskbar pins and shortcuts keep
//! working. On a Mac the new app bundle is downloaded next to the running one, unpacked with
//! ditto and swapped in by renaming the same way, then ULTRA relaunches.
//!
//! The experimental build lives in a private repository, which GitHub only shows to someone
//! signed in. The tester connects once by pasting a read-only GitHub token: on Windows it's
//! kept encrypted for this Windows user (DPAPI), on a Mac in a file only this user can read. It
//! is only ever sent to api.github.com, through curl's standard input (never on a command line
//! other programs can see).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The commit this build comes from (stamped by build.rs; empty if git wasn't available).
pub const COMMIT: &str = match option_env!("ULTRA_COMMIT") {
    Some(c) => c,
    None => "",
};
/// The release this build is (named by CI, e.g. "ULTRA 1 · Apple"; empty for local builds).
pub const RELEASE: &str = match option_env!("ULTRA_RELEASE") {
    Some(r) => r,
    None => "",
};

/// How this build introduces itself in settings: its release name, or else its commit.
pub fn version() -> String {
    if !RELEASE.is_empty() {
        RELEASE.to_string()
    } else if !COMMIT.is_empty() {
        format!("build {}", short(COMMIT))
    } else {
        String::new()
    }
}

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
    /// The GitHub token for a private repository's download.
    token: Option<String>,
    /// The file's SHA-256 as GitHub published it (hex), checked after downloading.
    sha256: Option<String>,
}

/// Does the downloaded file match the checksum GitHub published for it? (No published checksum:
/// older releases; the size and file-type checks still apply.)
fn checksum_ok(path: &Path, expected: Option<&str>) -> bool {
    use sha2::Digest;
    let Some(want) = expected else { return true };
    let Ok(bytes) = std::fs::read(path) else { return false };
    let got: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    got.eq_ignore_ascii_case(want)
}

/// The saved GitHub token, encrypted for this Windows user.
#[cfg(windows)]
const TOKEN_FILE: &str = "github-token";

/// A GitHub personal access token: fine-grained (github_pat_…) or classic (ghp_…).
pub fn valid_token(t: &str) -> bool {
    (t.starts_with("github_pat_") || t.starts_with("ghp_"))
        && (30..=255).contains(&t.len())
        && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(windows)]
pub fn load_token(dir: &Path) -> Option<String> {
    let raw = std::fs::read(dir.join(TOKEN_FILE)).ok()?;
    let t = String::from_utf8(crate::identity::unprotect(&raw)?).ok()?;
    valid_token(&t).then_some(t)
}

/// Keep the token for next time; refuses (false) unless Windows can encrypt it.
#[cfg(windows)]
pub fn save_token(dir: &Path, t: &str) -> bool {
    let (data, sealed) = crate::identity::protect(t.as_bytes());
    sealed && crate::library::write_atomic(&dir.join(TOKEN_FILE), &data).is_ok()
}

#[cfg(windows)]
pub fn forget_token(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(TOKEN_FILE));
}

#[cfg(not(windows))]
const TOKEN_FILE: &str = "github-token";

#[cfg(not(windows))]
pub fn load_token(dir: &Path) -> Option<String> {
    let t = std::fs::read_to_string(dir.join(TOKEN_FILE)).ok()?.trim().to_string();
    valid_token(&t).then_some(t)
}

/// Keep the token for next time, readable by this user only.
#[cfg(not(windows))]
pub fn save_token(dir: &Path, t: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(TOKEN_FILE);
    crate::library::write_atomic(&path, t).is_ok() && std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).is_ok()
}

#[cfg(not(windows))]
pub fn forget_token(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(TOKEN_FILE));
}

/// GitHub's "new token" page, filled in: read-only access to code and releases, for the owner
/// of the repository (the tester still picks the repository itself).
pub fn token_page(repo: &str) -> String {
    let owner = repo.split('/').next().unwrap_or("");
    format!(
        "https://github.com/settings/personal-access-tokens/new?name=ULTRA%20Experimental%20updates\
         &description=Lets%20ULTRA%20Experimental%20download%20its%20own%20updates\
         &target_name={owner}&expires_in=none&contents=read"
    )
}

/// Short form of a commit, as shown in settings.
pub fn short(c: &str) -> &str {
    &c[..c.len().min(7)]
}

/// Every platform can sign in to a private repository and install from it.
const CAN_AUTH: bool = true;

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

/// Run curl, handing it the token (if any) as a header through its standard input.
fn run(mut c: std::process::Command, token: Option<&str>) -> std::io::Result<std::process::Output> {
    use std::io::Write;
    use std::process::Stdio;
    if token.is_some() {
        c.args(["-K", "-"]);
    }
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    if let (Some(t), Some(mut stdin)) = (token, child.stdin.take()) {
        let _ = writeln!(stdin, "header = \"Authorization: Bearer {t}\"");
    }
    child.wait_with_output()
}

/// Ask GitHub about the latest release. Blocking: run it off the UI thread. Returns the state
/// for the settings UI, and the update to install if there is one.
pub fn check(repo: &str, token: Option<String>) -> (Value, Option<Update>) {
    let linked = token.is_some();
    let mut c = curl();
    c.args(["-sS", "-L", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-H", "User-Agent: ULTRA-updater", "-w", "\n%{http_code}"])
        .arg(format!("https://api.github.com/repos/{repo}/releases/latest"));
    let out = run(c, token.as_deref());
    let Ok(out) = out else { return (json!({ "s": "failed", "m": "couldn't run curl to reach GitHub" }), None) };
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or((&text, ""));
    match code.trim() {
        "200" => {}
        // A private repository (the experimental build) can't be read without signing in.
        "401" if linked => return (json!({ "s": "private", "linked": true, "canauth": CAN_AUTH, "m": "GitHub didn't accept the saved token (expired?) · connect again below" }), None),
        "404" if linked => {
            let name = repo.split('/').nth(1).unwrap_or(repo);
            return (json!({ "s": "private", "linked": true, "canauth": CAN_AUTH, "m": format!("the saved token can't see this repository · make one with {name} picked under repository access") }), None);
        }
        "404" => {
            let m = if CAN_AUTH { "this build updates from a private GitHub repository · connect once below and updates install by themselves" } else { "updates for this build are on its private GitHub page" };
            return (json!({ "s": "private", "canauth": CAN_AUTH, "m": m, "page": releases_page(repo) }), None);
        }
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
        return (json!({ "s": "uptodate", "linked": linked, "m": "you have the latest version" }), None);
    }
    // A private repository's file comes through the API (signed in); a public one from its page.
    let url = match (&token, asset["id"].as_u64()) {
        (Some(_), Some(id)) => format!("https://api.github.com/repos/{repo}/releases/assets/{id}"),
        _ => asset["browser_download_url"].as_str().unwrap_or("").to_string(),
    };
    let sha256 = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_string);
    let up = Update { url, size: asset["size"].as_u64().unwrap_or(0), token, sha256 };
    // Only ever download from this repository's own releases, over HTTPS.
    let ours = [format!("https://github.com/{repo}/releases/download/"), format!("https://api.github.com/repos/{repo}/releases/assets/")];
    if !ours.iter().any(|p| up.url.starts_with(p.as_str())) || up.size == 0 {
        return (json!({ "s": "failed", "m": "the latest release's download doesn't look right · nothing was changed" }), None);
    }
    let when = rel["published_at"].as_str().unwrap_or("").split('T').next().unwrap_or("").to_string();
    let name = rel["name"].as_str().filter(|n| !n.is_empty() && !n.contains("latest build")).map(str::to_string).unwrap_or_else(|| format!("a new build ({})", short(&latest)));
    (json!({ "s": "available", "linked": linked, "m": format!("{name} is out ({when})"), "page": releases_page(repo) }), Some(up))
}

/// Where the exe sits while it's being replaced.
#[cfg(windows)]
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
    // (curl never forwards the token to the storage server GitHub redirects the download to.)
    let mut c = curl();
    c.args(["-sS", "-L", "-f", "--max-time", "300", "-H", "User-Agent: ULTRA-updater", "-H", "Accept: application/octet-stream", "-o"])
        .arg(&new)
        .arg(&up.url);
    let ok = run(c, up.token.as_deref()).is_ok_and(|o| o.status.success());
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
    if !checksum_ok(&new, up.sha256.as_deref()) {
        return fail("the download doesn't match GitHub's checksum · nothing was changed");
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

/// The app bundle ULTRA is running from on a Mac (…/ULTRA.app).
#[cfg(not(windows))]
fn bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|_| "couldn't find where ULTRA is running from".to_string())?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf).ok_or_else(|| "ULTRA isn't running from an app bundle".into())
}

/// Download the update (a zipped app), unpack it beside the running app and swap it in by
/// renaming, like Windows. Blocking. Returns the new app's executable to restart from.
#[cfg(not(windows))]
pub fn install(up: &Update) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|_| "couldn't find where ULTRA is running from".to_string())?;
    let app = bundle()?;
    let dir = app.parent().ok_or("couldn't find ULTRA's folder")?.to_path_buf();
    let work = dir.join(".ultra-update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("couldn't write next to ULTRA ({e}) · move ULTRA into your Applications folder"))?;
    let fail = |m: String| {
        let _ = std::fs::remove_dir_all(&work);
        Err(m)
    };
    let zip = work.join("update.zip");
    let mut c = curl();
    c.args(["-sS", "-L", "-f", "--max-time", "300", "-H", "User-Agent: ULTRA-updater", "-H", "Accept: application/octet-stream", "-o"])
        .arg(&zip)
        .arg(&up.url);
    if !run(c, up.token.as_deref()).is_ok_and(|o| o.status.success()) {
        return fail("the download didn't finish · try again".into());
    }
    // A whole zip: the size GitHub promised, starting with "PK".
    let head = std::fs::read(&zip).map(|b| (b.len() as u64, b.get(..2) == Some(b"PK"))).unwrap_or((0, false));
    if head.0 == 0 || (up.size > 0 && head.0 != up.size) || !head.1 {
        return fail("the download looks damaged · nothing was changed".into());
    }
    if !checksum_ok(&zip, up.sha256.as_deref()) {
        return fail("the download doesn't match GitHub's checksum · nothing was changed".into());
    }
    let unpacked = std::process::Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&zip).arg(&work).status();
    if !unpacked.is_ok_and(|s| s.success()) {
        return fail("couldn't unpack the update · nothing was changed".into());
    }
    let Some(new_app) = std::fs::read_dir(&work).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "app")) else {
        return fail("the update had no app inside · nothing was changed".into());
    };
    // Swap: the running app steps aside (allowed while running), the new one takes its place.
    let old = app.with_extension("app-old");
    let _ = std::fs::remove_dir_all(&old);
    if let Err(e) = std::fs::rename(&app, &old) {
        return fail(format!("couldn't replace ULTRA ({e}) · is it in a folder you can't write to?"));
    }
    if let Err(e) = std::fs::rename(&new_app, &app) {
        let _ = std::fs::rename(&old, &app); // put the old one back
        return fail(format!("couldn't put the new version in place ({e})"));
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(app.join("Contents/MacOS").join(exe.file_name().unwrap_or_default()))
}

/// A freshly updated ULTRA: wait (briefly) for the old one to finish closing.
#[cfg(not(windows))]
pub fn wait_for_previous() {
    let Some(pid) = std::env::var("ULTRA_WAIT_PID").ok().and_then(|p| p.parse::<i32>().ok()) else { return };
    std::env::remove_var("ULTRA_WAIT_PID");
    for _ in 0..150 {
        // Signal 0 only asks whether the process still exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Remove the previous app left beside this one by the last update (best effort).
#[cfg(not(windows))]
pub fn clean_up() {
    if let Ok(app) = bundle() {
        let _ = std::fs::remove_dir_all(app.with_extension("app-old"));
        if let Some(dir) = app.parent() {
            let _ = std::fs::remove_dir_all(dir.join(".ultra-update"));
        }
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

    #[test]
    fn checksums() {
        let f = std::env::temp_dir().join(format!("ultra-sum-{}", std::process::id()));
        std::fs::write(&f, b"hello").unwrap();
        assert!(checksum_ok(&f, Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")));
        assert!(!checksum_ok(&f, Some("00f24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")));
        assert!(checksum_ok(&f, None));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn tokens() {
        assert!(valid_token("github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz"));
        assert!(valid_token("ghp_0123456789abcdefghijABCDEFGHIJ123456"));
        assert!(!valid_token("ghp_short"));
        assert!(!valid_token("github_pat_abc def\"ghi jklmnopqrstuvwxyz0123456789"), "no spaces or quotes");
        assert!(!valid_token("password1234567890123456789012345"));
        assert!(token_page("someone/some-repo").contains("&target_name=someone&"));
    }
}
