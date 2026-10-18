//! "Check for updates" in settings. Every build knows the commit it was built from; the
//! repository's rolling "latest" release says which commit it was built from. If they differ,
//! there's an update. On Windows it is installed in place: the new exe is downloaded next to
//! the running one, checked, and swapped in by renaming (Windows lets a running exe be renamed,
//! not overwritten), then ULTRA restarts from the same path, so taskbar pins and shortcuts keep
//! working. On a Mac the download page opens instead (an app bundle is replaced by hand).
//!
//! The experimental build lives in a private repository, which GitHub only shows to someone
//! signed in. On Windows the tester connects once by pasting a read-only GitHub token; it is
//! kept encrypted for this Windows user (DPAPI) and only ever sent to api.github.com, through
//! curl's standard input (never on a command line other programs can see).

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

/// Whether this platform can sign in to a private repository and install from it.
const CAN_AUTH: bool = cfg!(windows);

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
        "404" if linked => return (json!({ "s": "private", "linked": true, "canauth": CAN_AUTH, "m": "the saved token can't see this repository · make one with ULTRA-experimental picked under repository access" }), None),
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
    let up = Update { url, size: asset["size"].as_u64().unwrap_or(0), token };
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

    #[test]
    fn tokens() {
        assert!(valid_token("github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyz"));
        assert!(valid_token("ghp_0123456789abcdefghijABCDEFGHIJ123456"));
        assert!(!valid_token("ghp_short"));
        assert!(!valid_token("github_pat_abc def\"ghi jklmnopqrstuvwxyz0123456789"), "no spaces or quotes");
        assert!(!valid_token("password1234567890123456789012345"));
        assert!(token_page("skhshths/ULTRA-experimental").contains("&target_name=skhshths&"));
    }
}
