//! Local history, bookmarks and command-palette history. Private-workspace pages
//! never reach this module. Everything lives in %LOCALAPPDATA%\Ultra as JSON.

use std::{cell::Cell, collections::HashMap, fs, path::Path};

use serde::{Deserialize, Serialize};

const MAX_HISTORY: usize = 5000;
const MAX_COMMANDS: usize = 50;
/// Tiles under the new-tab search box.
const MAX_SHORTCUTS: usize = 12;
/// Most-visited sites shown until the user adds shortcuts of their own.
const AUTO_SHORTCUTS: usize = 6;

#[derive(Serialize, Deserialize, Clone)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub visits: u32,
    /// Unix seconds of the last visit.
    pub last: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    pub added: u64,
}

/// A tile under the new-tab search box.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Shortcut {
    pub url: String,
    pub title: String,
}

/// Today's browsing at a glance (local data only, reset each calendar day).
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Digest {
    pub day: String,
    /// Active seconds per site.
    pub secs: HashMap<String, u64>,
    pub pages: u32,
    pub blocked: u64,
}

/// What the home screen shows.
#[derive(Serialize)]
pub struct DigestView {
    pub secs: u64,
    pub pages: u32,
    pub blocked: u64,
    pub top: Vec<(String, u64)>,
}

#[derive(Default)]
pub struct Library {
    pub history: Vec<Visit>,
    pub bookmarks: Vec<Bookmark>,
    pub commands: Vec<String>,
    pub digest: Digest,
    pub shortcuts: Vec<Shortcut>,
    dirty: Cell<bool>,
}

/// Local calendar date, e.g. "2026-10-16".
pub fn today() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
}

/// "https://www.github.com/x" -> "github.com"
pub fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let h = rest.split(['/', '?', '#']).next()?.split('@').last()?.split(':').next()?;
    let h = h.strip_prefix("www.").unwrap_or(h).to_lowercase();
    (!h.is_empty()).then_some(h)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Save a file so that a crash or power cut mid-write can never leave it half written: the
/// data goes to a temporary file, is flushed to disk, and then replaces the old file in one step.
pub fn write_atomic(path: &Path, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data.as_ref())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

fn read<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Only real web pages are remembered.
fn rememberable(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

impl Library {
    pub fn load(dir: &Path) -> Self {
        Library {
            history: read(&dir.join("history.json")),
            bookmarks: read(&dir.join("bookmarks.json")),
            commands: read(&dir.join("commands.json")),
            digest: read(&dir.join("digest.json")),
            shortcuts: read(&dir.join("shortcuts.json")),
            dirty: Cell::new(false),
        }
    }

    /// Write whatever changed since the last save.
    pub fn save(&self, dir: &Path) {
        if !self.dirty.replace(false) {
            return;
        }
        let write = |name: &str, v: &dyn erased::Json| {
            if let Some(j) = v.json() {
                let _ = write_atomic(&dir.join(name), j);
            }
        };
        write("history.json", &self.history);
        write("bookmarks.json", &self.bookmarks);
        write("commands.json", &self.commands);
        write("digest.json", &self.digest);
        write("shortcuts.json", &self.shortcuts);
    }

    pub fn visit(&mut self, url: &str, title: &str) {
        if !rememberable(url) {
            return;
        }
        self.roll_day();
        self.digest.pages += 1;
        let t = now();
        match self.history.iter_mut().find(|v| v.url == url) {
            Some(v) => {
                v.visits = v.visits.saturating_add(1);
                v.last = t;
                if !title.is_empty() {
                    v.title = title.to_string();
                }
            }
            None => {
                self.history.push(Visit { url: url.into(), title: title.into(), visits: 1, last: t });
                if self.history.len() > MAX_HISTORY {
                    // Drop the least recently visited.
                    if let Some(i) = self.history.iter().enumerate().min_by_key(|(_, v)| v.last).map(|(i, _)| i) {
                        self.history.swap_remove(i);
                    }
                }
            }
        }
        self.dirty.set(true);
    }

    pub fn retitle(&mut self, url: &str, title: &str) {
        if let Some(v) = self.history.iter_mut().find(|v| v.url == url) {
            if !title.is_empty() && v.title != title {
                v.title = title.to_string();
                self.dirty.set(true);
            }
        }
    }

    /// Pin a site to the new-tab page. False if it isn't a web page, is already there, or the
    /// page is full.
    pub fn add_shortcut(&mut self, url: &str, title: &str) -> bool {
        let url = url.trim();
        if !rememberable(url) || self.shortcuts.iter().any(|s| s.url == url) || self.shortcuts.len() >= MAX_SHORTCUTS {
            return false;
        }
        let title = if title.trim().is_empty() { host(url).unwrap_or_default() } else { title.trim().chars().take(60).collect() };
        self.shortcuts.push(Shortcut { url: url.to_string(), title });
        self.dirty.set(true);
        true
    }

    pub fn remove_shortcut(&mut self, url: &str) {
        let n = self.shortcuts.len();
        self.shortcuts.retain(|s| s.url != url);
        if self.shortcuts.len() != n {
            self.dirty.set(true);
        }
    }

    /// What the new-tab page shows: the user's shortcuts, or until they add some, the sites
    /// they visit most (one tile per site). The flag says which.
    pub fn shortcuts_view(&self) -> (Vec<Shortcut>, bool) {
        if !self.shortcuts.is_empty() {
            return (self.shortcuts.clone(), false);
        }
        let mut sites: Vec<(String, String, u64)> = Vec::new();
        for v in &self.history {
            let Some(h) = host(&v.url) else { continue };
            let score = v.visits as u64;
            match sites.iter_mut().find(|s| s.0 == h) {
                Some(s) => s.2 += score,
                None => {
                    let origin = v.url.split_once("://").map(|(sch, rest)| format!("{sch}://{}/", rest.split(['/', '?', '#']).next().unwrap_or(""))).unwrap_or_default();
                    sites.push((h, origin, score));
                }
            }
        }
        sites.sort_by(|a, b| b.2.cmp(&a.2));
        (sites.into_iter().take(AUTO_SHORTCUTS).map(|(h, url, _)| Shortcut { url, title: h }).collect(), true)
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|b| b.url == url)
    }

    /// Add or remove a bookmark; returns whether it is now bookmarked.
    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        self.dirty.set(true);
        if let Some(i) = self.bookmarks.iter().position(|b| b.url == url) {
            self.bookmarks.remove(i);
            false
        } else {
            self.bookmarks.push(Bookmark { url: url.into(), title: title.into(), added: now() });
            true
        }
    }

    /// Start a fresh digest when the calendar day changes.
    fn roll_day(&mut self) {
        let d = today();
        if self.digest.day != d {
            self.digest = Digest { day: d, ..Default::default() };
            self.dirty.set(true);
        }
    }

    /// Credit active time on a page to its site.
    pub fn add_time(&mut self, url: &str, secs: u64) {
        let Some(h) = host(url).filter(|_| rememberable(url) && secs > 0) else { return };
        self.roll_day();
        *self.digest.secs.entry(h).or_default() += secs;
        self.dirty.set(true);
    }

    pub fn add_blocked(&mut self, n: u64) {
        if n > 0 {
            self.roll_day();
            self.digest.blocked += n;
            self.dirty.set(true);
        }
    }

    pub fn digest_view(&self) -> DigestView {
        let fresh = self.digest.day == today();
        let mut top: Vec<(String, u64)> = if fresh { self.digest.secs.iter().map(|(h, s)| (h.clone(), *s)).collect() } else { Vec::new() };
        top.sort_by(|a, b| b.1.cmp(&a.1));
        top.truncate(5);
        DigestView {
            secs: if fresh { self.digest.secs.values().sum() } else { 0 },
            pages: if fresh { self.digest.pages } else { 0 },
            blocked: if fresh { self.digest.blocked } else { 0 },
            top,
        }
    }

    /// Onboarding: copy bookmarks from Google Chrome's most recently used profile (bookmarks
    /// bar first, then the rest). Chrome may stay open; its Bookmarks file is plain JSON.
    /// Returns how many were added (duplicates are skipped) and the profile's name.
    /// Copy everything useful from another browser into ULTRA: bookmarks, new-tab shortcuts and
    /// history (new ones only), plus its open tabs and search engine for the caller to apply.
    /// The other browser is only read, never changed. Saved passwords and sign-ins are not
    /// touched.
    pub fn import_from(&mut self, id: &str) -> Result<Imported, String> {
        let name = browser_name(id);
        let base = browser_dir(id).ok_or_else(|| format!("couldn't find {name} on this computer"))?;
        let found = if id == "firefox" { firefox_profile(&base, name)? } else { chromium_profile(&base, name, id)? };
        let t = now();
        let mut bookmarks = 0;
        for (title, url) in found.bookmarks {
            if rememberable(&url) && !self.is_bookmarked(&url) {
                self.bookmarks.push(Bookmark { url, title, added: t });
                bookmarks += 1;
            }
        }
        let shortcuts = found.shortcuts.iter().filter(|(title, url)| self.add_shortcut(url, title)).count();
        let history = self.merge_history(found.history);
        if bookmarks > 0 || history > 0 {
            self.dirty.set(true);
        }
        Ok(Imported { bookmarks, shortcuts, history, tabs: found.tabs, search: found.search, label: found.label })
    }

    /// Fold visits from another browser into ULTRA's history: new pages are added, known ones
    /// keep the higher visit count and the latest visit. Returns how many pages were new.
    fn merge_history(&mut self, items: Vec<(String, String, u32, u64)>) -> usize {
        let mut known: HashMap<String, usize> = self.history.iter().enumerate().map(|(i, v)| (v.url.clone(), i)).collect();
        let mut added = 0;
        for (url, title, visits, last) in items {
            if !rememberable(&url) {
                continue;
            }
            match known.get(&url) {
                Some(&i) => {
                    let v = &mut self.history[i];
                    v.visits = v.visits.max(visits);
                    v.last = v.last.max(last);
                    if v.title.is_empty() {
                        v.title = title;
                    }
                }
                None => {
                    known.insert(url.clone(), self.history.len());
                    self.history.push(Visit { url, title, visits: visits.max(1), last });
                    added += 1;
                }
            }
        }
        if self.history.len() > MAX_HISTORY {
            // Keep the most recently visited.
            self.history.sort_by(|a, b| b.last.cmp(&a.last));
            self.history.truncate(MAX_HISTORY);
        }
        added
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.digest = Digest { day: today(), ..Default::default() };
        self.dirty.set(true);
    }

    pub fn push_command(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return;
        }
        self.commands.retain(|c| c != cmd);
        self.commands.push(cmd.chars().take(200).collect());
        if self.commands.len() > MAX_COMMANDS {
            self.commands.remove(0);
        }
        self.dirty.set(true);
    }

    /// Ranked matches for what's typed in the address bar: every word must appear in the
    /// url or title; bookmarks, frequently and recently visited pages, and host-prefix
    /// matches rank higher.
    pub fn suggest(&self, query: &str, limit: usize) -> Vec<(String, String, bool)> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let words: Vec<&str> = q.split_whitespace().collect();
        let t = now();
        let strip = |u: &str| {
            let u = u.split_once("://").map(|(_, r)| r).unwrap_or(u).to_string();
            u.strip_prefix("www.").map(str::to_string).unwrap_or(u)
        };
        let score = |url: &str, title: &str, visits: u32, last: u64, bm: bool| -> Option<f64> {
            let (lu, lt) = (url.to_lowercase(), title.to_lowercase());
            if !words.iter().all(|w| lu.contains(w) || lt.contains(w)) {
                return None;
            }
            let hours = t.saturating_sub(last) as f64 / 3600.0;
            let mut s = (visits as f64 + 1.0).ln() * 6.0 + 20.0 / (1.0 + hours / 24.0);
            if strip(&lu).starts_with(&q) {
                s += 40.0;
            }
            if bm {
                s += 30.0;
            }
            // Among equals, prefer shorter (root) addresses, as other browsers do.
            Some(s - lu.len() as f64 * 0.05)
        };
        let mut out: Vec<(f64, String, String, bool)> = Vec::new();
        for v in &self.history {
            let bm = self.is_bookmarked(&v.url);
            if let Some(s) = score(&v.url, &v.title, v.visits, v.last, bm) {
                out.push((s, v.url.clone(), v.title.clone(), bm));
            }
        }
        for b in &self.bookmarks {
            if !out.iter().any(|o| o.1 == b.url) {
                if let Some(s) = score(&b.url, &b.title, 0, b.added, true) {
                    out.push((s, b.url.clone(), b.title.clone(), true));
                }
            }
        }
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        out.into_iter().take(limit).map(|(_, u, t, b)| (u, t, b)).collect()
    }
}

/// Where Chrome keeps its profiles on this OS.
/// Browsers ULTRA can copy bookmarks from: (id, name). All but Firefox are built on Chromium
/// and keep bookmarks the way Chrome does.
pub const BROWSERS: &[(&str, &str)] = &[
    ("chrome", "Chrome"),
    ("edge", "Edge"),
    ("brave", "Brave"),
    ("vivaldi", "Vivaldi"),
    ("opera", "Opera"),
    ("firefox", "Firefox"),
];

pub fn browser_name(id: &str) -> &'static str {
    BROWSERS.iter().find(|(i, _)| *i == id).map_or("that browser", |(_, n)| n)
}

/// The browsers that have data on this computer (for the import buttons).
pub fn installed_browsers() -> Vec<(&'static str, &'static str)> {
    BROWSERS.iter().copied().filter(|(id, _)| browser_dir(id).is_some()).collect()
}

/// Where a browser keeps its profiles on this computer, if it's installed.
fn browser_dir(id: &str) -> Option<std::path::PathBuf> {
    let var = |v: &str| std::env::var_os(v).map(std::path::PathBuf::from);
    let p = if cfg!(target_os = "macos") {
        let s = var("HOME")?.join("Library/Application Support");
        match id {
            "chrome" => s.join("Google/Chrome"),
            "edge" => s.join("Microsoft Edge"),
            "brave" => s.join("BraveSoftware/Brave-Browser"),
            "vivaldi" => s.join("Vivaldi"),
            "opera" => s.join("com.operasoftware.Opera"),
            "firefox" => s.join("Firefox/Profiles"),
            _ => return None,
        }
    } else {
        let (local, roaming) = (var("LOCALAPPDATA")?, var("APPDATA")?);
        match id {
            "chrome" => local.join("Google/Chrome/User Data"),
            "edge" => local.join("Microsoft/Edge/User Data"),
            "brave" => local.join("BraveSoftware/Brave-Browser/User Data"),
            "vivaldi" => local.join("Vivaldi/User Data"),
            "opera" => roaming.join("Opera Software/Opera Stable"),
            "firefox" => roaming.join("Mozilla/Firefox/Profiles"),
            _ => return None,
        }
    };
    p.is_dir().then_some(p)
}

/// What an import found in another browser's profile.
struct Found {
    bookmarks: Vec<(String, String)>,
    shortcuts: Vec<(String, String)>,
    /// (url, title, visit count, last visit in unix seconds)
    history: Vec<(String, String, u32, u64)>,
    tabs: Vec<(String, String)>,
    search: Option<&'static str>,
    label: String,
}

/// The result of an import, for the caller: counts of what was added, plus the other browser's
/// open tabs and search engine (ULTRA's own names) to apply.
pub struct Imported {
    pub bookmarks: usize,
    pub shortcuts: usize,
    pub history: usize,
    pub tabs: Vec<(String, String)>,
    pub search: Option<&'static str>,
    pub label: String,
}

/// How many history entries an import reads (the most recent).
const IMPORT_HISTORY: usize = 5000;

/// ULTRA's name for a search engine, from a browser's description of it (a keyword, name, url
/// or id), when it's one ULTRA offers.
fn engine_of(desc: &str) -> Option<&'static str> {
    let d = desc.to_lowercase();
    if d.contains("duckduckgo") || d.starts_with("ddg") {
        Some("duckduckgo")
    } else if d.contains("google") {
        Some("google")
    } else if d.contains("bing") {
        Some("bing")
    } else if d.contains("brave") {
        Some("brave")
    } else {
        None
    }
}

/// Open a copy of another browser's SQLite database (the original may be locked while that
/// browser runs; its write-ahead log is copied along so recent visits are included).
fn open_copy(db: &Path, tmp: &Path) -> Option<rusqlite::Connection> {
    let _ = fs::create_dir_all(tmp);
    let name = db.file_name()?.to_string_lossy().to_string();
    fs::copy(db, tmp.join(&name)).ok()?;
    for ext in ["-wal", "-journal"] {
        let side = db.with_file_name(format!("{name}{ext}"));
        if side.exists() {
            let _ = fs::copy(&side, tmp.join(format!("{name}{ext}")));
        }
    }
    rusqlite::Connection::open_with_flags(tmp.join(&name), rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).ok()
}

/// A scratch folder for database copies, removed when dropped.
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new() -> Self {
        Scratch(std::env::temp_dir().join(format!("ultra-import-{}-{}", std::process::id(), now())))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Recent history of a Chromium-based browser (times count microseconds from 1601).
fn chromium_history(dir: &Path) -> Vec<(String, String, u32, u64)> {
    let scratch = Scratch::new();
    let Some(db) = open_copy(&dir.join("History"), &scratch.0) else { return Vec::new() };
    let Ok(mut q) = db.prepare("SELECT url, title, visit_count, last_visit_time FROM urls WHERE hidden = 0 ORDER BY last_visit_time DESC LIMIT ?1") else {
        return Vec::new();
    };
    let rows = q.query_map([IMPORT_HISTORY as i64], |r| {
        let t: i64 = r.get(3)?;
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, i64>(2)?.max(0) as u32, (t / 1_000_000 - 11_644_473_600).max(0) as u64))
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// Recent history of a Firefox profile (times count microseconds from 1970).
fn firefox_history(dir: &Path) -> Vec<(String, String, u32, u64)> {
    let scratch = Scratch::new();
    let Some(db) = open_copy(&dir.join("places.sqlite"), &scratch.0) else { return Vec::new() };
    let Ok(mut q) = db.prepare(
        "SELECT url, title, visit_count, last_visit_date FROM moz_places WHERE visit_count > 0 AND hidden = 0 AND last_visit_date IS NOT NULL ORDER BY last_visit_date DESC LIMIT ?1",
    ) else {
        return Vec::new();
    };
    let rows = q.query_map([IMPORT_HISTORY as i64], |r| {
        let t: i64 = r.get(3)?;
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), r.get::<_, i64>(2)?.max(0) as u32, (t / 1_000_000).max(0) as u64))
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// Open tabs from a Chromium session file ("SNSS": a log of commands). Each tab's navigations
/// are recorded with "update tab navigation" (6), the one showing with "set selected
/// navigation index" (7); "tab closed" (16) drops a tab. Only web pages are kept.
fn chromium_session_tabs(data: &[u8]) -> Vec<(String, String)> {
    if data.get(..4) != Some(b"SNSS") {
        return Vec::new();
    }
    let i32_at = |b: &[u8], o: usize| b.get(o..o + 4).map(|x| i32::from_le_bytes([x[0], x[1], x[2], x[3]]));
    struct Tab {
        navs: Vec<(i32, String, String)>,
        selected: Option<i32>,
        closed: bool,
    }
    let mut tabs: Vec<(i32, Tab)> = Vec::new();
    let tab = |tabs: &mut Vec<(i32, Tab)>, id: i32| -> usize {
        match tabs.iter().position(|(t, _)| *t == id) {
            Some(p) => p,
            None => {
                tabs.push((id, Tab { navs: Vec::new(), selected: None, closed: false }));
                tabs.len() - 1
            }
        }
    };
    let mut i = 8;
    while i + 2 <= data.len() {
        let size = u16::from_le_bytes([data[i], data[i + 1]]) as usize;
        i += 2;
        if size == 0 || i + size > data.len() {
            break;
        }
        let (cmd, p) = (data[i], &data[i + 1..i + size]);
        i += size;
        match cmd {
            // Pickle: payload size, tab id, navigation index, url (length + bytes, padded to 4),
            // title (UTF-16 length + units, padded to 4), ...
            6 => {
                let (Some(id), Some(index), Some(len)) = (i32_at(p, 4), i32_at(p, 8), i32_at(p, 12)) else { continue };
                let len = len.max(0) as usize;
                let Some(url) = p.get(16..16 + len).map(|b| String::from_utf8_lossy(b).to_string()) else { continue };
                let t = 16 + len.div_ceil(4) * 4;
                let title = i32_at(p, t)
                    .and_then(|n| p.get(t + 4..t + 4 + n.max(0) as usize * 2))
                    .map(|b| String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()))
                    .unwrap_or_default();
                let k = tab(&mut tabs, id);
                let navs = &mut tabs[k].1.navs;
                navs.retain(|n| n.0 != index);
                navs.push((index, url, title));
            }
            7 => {
                if let (Some(id), Some(index)) = (i32_at(p, 0), i32_at(p, 4)) {
                    let k = tab(&mut tabs, id);
                    tabs[k].1.selected = Some(index);
                }
            }
            16 => {
                if let Some(id) = i32_at(p, 0) {
                    let k = tab(&mut tabs, id);
                    tabs[k].1.closed = true;
                }
            }
            _ => {}
        }
    }
    tabs.into_iter()
        .filter(|(_, t)| !t.closed)
        .filter_map(|(_, t)| {
            let n = t.selected.and_then(|s| t.navs.iter().find(|n| n.0 == s)).or_else(|| t.navs.iter().max_by_key(|n| n.0))?;
            (n.1.starts_with("http://") || n.1.starts_with("https://")).then(|| (n.2.clone(), n.1.clone()))
        })
        .collect()
}

/// The newest session file of a Chromium profile (Sessions/Session_*, or the older
/// "Current Session").
fn chromium_tabs(dir: &Path) -> Vec<(String, String)> {
    let mut files: Vec<std::path::PathBuf> = fs::read_dir(dir.join("Sessions"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Session_")))
        .collect();
    files.push(dir.join("Current Session"));
    let newest = files.into_iter().filter_map(|p| Some((fs::metadata(&p).ok()?.modified().ok()?, p))).max_by_key(|(t, _)| *t);
    newest.and_then(|(_, p)| fs::read(p).ok()).map(|d| chromium_session_tabs(&d)).unwrap_or_default()
}

/// Open tabs from Firefox's session store (recovery.jsonlz4 while it runs, sessionstore.jsonlz4
/// after it closed): each tab's entries, with `index` (1-based) the one showing.
fn firefox_tabs(dir: &Path) -> Vec<(String, String)> {
    let candidates = [dir.join("sessionstore-backups").join("recovery.jsonlz4"), dir.join("sessionstore.jsonlz4")];
    let newest = candidates.iter().filter_map(|p| Some((fs::metadata(p).ok()?.modified().ok()?, p))).max_by_key(|(t, _)| *t);
    let Some(json) = newest.and_then(|(_, p)| fs::read(p).ok()).and_then(|d| mozlz4(&d)).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for w in json["windows"].as_array().into_iter().flatten() {
        for t in w["tabs"].as_array().into_iter().flatten() {
            let entries = t["entries"].as_array().cloned().unwrap_or_default();
            let at = t["index"].as_u64().unwrap_or(entries.len() as u64).max(1) as usize - 1;
            if let Some(e) = entries.get(at.min(entries.len().saturating_sub(1))) {
                let url = e["url"].as_str().unwrap_or("");
                if url.starts_with("http://") || url.starts_with("https://") {
                    out.push((e["title"].as_str().unwrap_or("").to_string(), url.to_string()));
                }
            }
        }
    }
    out
}

/// The profile last used in a Chromium-based browser: its bookmarks, its new-tab shortcuts
/// (Chrome keeps the ones you added in Preferences, as "custom_links") and its name.
fn chromium_profile(base: &Path, name: &str, id: &str) -> Result<Found, String> {
    let state: serde_json::Value = fs::read_to_string(base.join("Local State"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let profile = state["profile"]["last_used"].as_str().unwrap_or("Default").to_string();
    let label = state["profile"]["info_cache"][&profile]["name"].as_str().unwrap_or(&profile).to_string();
    // Opera keeps a single profile right in its folder.
    // (Opera keeps a single profile right in its folder.) Any one of these files is enough:
    // a profile without bookmarks still has history, tabs and settings to bring over.
    let dir = [base.join(&profile), base.join("Default"), base.to_path_buf()]
        .into_iter()
        .find(|d| ["Bookmarks", "History", "Preferences"].iter().any(|f| d.join(f).is_file()))
        .ok_or_else(|| format!("{name} has nothing to import yet"))?;
    let json: serde_json::Value = fs::read_to_string(dir.join("Bookmarks")).ok().and_then(|f| serde_json::from_str(&f).ok()).unwrap_or_default();
    let mut bookmarks = Vec::new();
    for root in ["bookmark_bar", "other", "synced"] {
        walk(&json["roots"][root], &mut bookmarks);
    }
    let prefs: serde_json::Value = fs::read_to_string(dir.join("Preferences"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let shortcuts = prefs["custom_links"]["list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| !l["isMostVisited"].as_bool().unwrap_or(false))
        .filter_map(|l| Some((l["title"].as_str().unwrap_or("").to_string(), l["url"].as_str()?.to_string())))
        .collect();
    // The search engine: the one the user picked, else the browser's own default.
    let picked = &prefs["default_search_provider_data"]["template_url_data"];
    let search = [picked["keyword"].as_str(), picked["short_name"].as_str(), picked["url"].as_str()]
        .into_iter()
        .flatten()
        .find_map(engine_of)
        .or(match (picked.is_null(), id) {
            (true, "chrome" | "opera") => Some("google"),
            (true, "edge") => Some("bing"),
            (true, "brave") => Some("brave"),
            _ => None,
        });
    Ok(Found { bookmarks, shortcuts, history: chromium_history(&dir), tabs: chromium_tabs(&dir), search, label })
}

/// A Firefox profile: bookmarks and history from its places database (read from a copy, as
/// Firefox locks it while running; its daily compressed bookmark backup is the fallback), the
/// sites pinned on its new-tab page, its open tabs and its search engine. The profile in use is
/// the one whose database changed last.
fn firefox_profile(profiles: &Path, name: &str) -> Result<Found, String> {
    let dir = fs::read_dir(profiles)
        .map_err(|_| format!("couldn't find {name}'s profiles"))?
        .flatten()
        .map(|e| e.path())
        .filter_map(|d| Some((fs::metadata(d.join("places.sqlite")).ok()?.modified().ok()?, d)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, d)| d)
        .ok_or_else(|| format!("{name} has nothing to import yet"))?;
    // Profile folders are named "<random>.<profile name>".
    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let label = folder.split_once('.').map_or(folder.clone(), |(_, b)| b.to_string());
    let mut bookmarks = firefox_bookmarks_db(&dir);
    if bookmarks.is_empty() {
        let newest = fs::read_dir(dir.join("bookmarkbackups"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|f| f.file_name().to_string_lossy().ends_with(".jsonlz4"))
            .filter_map(|f| Some((f.metadata().ok()?.modified().ok()?, f.path())))
            .max_by_key(|(t, _)| *t);
        if let Some(json) = newest.and_then(|(_, p)| fs::read(p).ok()).and_then(|d| mozlz4(&d)).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
            walk_firefox(&json, &mut bookmarks);
        }
    }
    let shortcuts = fs::read_to_string(dir.join("prefs.js")).map(|p| firefox_pinned(&p)).unwrap_or_default();
    // The default search engine, from Firefox's compressed search settings.
    let search = fs::read(dir.join("search.json.mozlz4"))
        .ok()
        .and_then(|d| mozlz4(&d))
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|j| ["defaultEngineId", "current"].iter().find_map(|k| j["metaData"][k].as_str().and_then(engine_of)));
    Ok(Found {
        bookmarks,
        shortcuts,
        history: firefox_history(&dir),
        tabs: firefox_tabs(&dir),
        search,
        label: if label.is_empty() { "default".into() } else { label },
    })
}

/// Firefox's bookmarks from its places database, folder by folder in Firefox's order
/// (tags also appear as bookmarks there; duplicates are skipped when importing).
fn firefox_bookmarks_db(dir: &Path) -> Vec<(String, String)> {
    let scratch = Scratch::new();
    let Some(db) = open_copy(&dir.join("places.sqlite"), &scratch.0) else { return Vec::new() };
    let Ok(mut q) = db.prepare(
        "SELECT b.title, p.url FROM moz_bookmarks b JOIN moz_places p ON p.id = b.fk WHERE b.type = 1 ORDER BY b.parent, b.position",
    ) else {
        return Vec::new();
    };
    let rows = q.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, String>(1)?)));
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// Sites pinned on Firefox's new-tab page: prefs.js holds them as a JSON string,
/// user_pref("browser.newtabpage.pinned", "[{\"url\":…,\"label\":…},null,…]");
fn firefox_pinned(prefs: &str) -> Vec<(String, String)> {
    let Some(line) = prefs.lines().find(|l| l.starts_with("user_pref(\"browser.newtabpage.pinned\"")) else { return Vec::new() };
    let Some(raw) = line.split_once(", \"").and_then(|(_, r)| r.rsplit_once("\");")).map(|(v, _)| v) else { return Vec::new() };
    let unescaped = raw.replace("\\\"", "\"").replace("\\\\", "\\");
    serde_json::from_str::<Vec<serde_json::Value>>(&unescaped)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| {
            let url = v["url"].as_str()?.to_string();
            let label = v["label"].as_str().or(v["title"].as_str()).unwrap_or("").to_string();
            Some((label, url))
        })
        .collect()
}

/// Collect (title, url) from a Firefox bookmark backup, in Firefox's order.
fn walk_firefox(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
    if node["type"] == "text/x-moz-place" {
        if let Some(u) = node["uri"].as_str() {
            out.push((node["title"].as_str().unwrap_or("").to_string(), u.to_string()));
        }
    }
    for c in node["children"].as_array().into_iter().flatten() {
        walk_firefox(c, out);
    }
}

/// Firefox's "mozLz40" files: an 8-byte magic, the decompressed size, then one LZ4 block.
fn mozlz4(data: &[u8]) -> Option<Vec<u8>> {
    let body = data.strip_prefix(b"mozLz40\0")?;
    let size = u32::from_le_bytes(body.get(..4)?.try_into().ok()?) as usize;
    lz4_block(&body[4..], size)
}

/// Decompress one LZ4 block of known size (sequences of literals plus back-references).
fn lz4_block(src: &[u8], size: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(size.min(64 << 20));
    let mut i = 0;
    let more = |i: &mut usize, mut n: usize| -> Option<usize> {
        loop {
            let b = *src.get(*i)?;
            *i += 1;
            n += b as usize;
            if b != 255 {
                return Some(n);
            }
        }
    };
    while i < src.len() {
        let token = src[i];
        i += 1;
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            lit = more(&mut i, lit)?;
        }
        out.extend_from_slice(src.get(i..i + lit)?);
        i += lit;
        if i >= src.len() {
            break; // the last sequence is literals only
        }
        let off = u16::from_le_bytes([*src.get(i)?, *src.get(i + 1)?]) as usize;
        i += 2;
        let mut len = (token & 15) as usize;
        if len == 15 {
            len = more(&mut i, len)?;
        }
        len += 4;
        if off == 0 || off > out.len() || out.len() + len > size {
            return None;
        }
        let start = out.len() - off;
        for k in 0..len {
            out.push(out[start + k]); // may overlap what it's copying: byte by byte
        }
    }
    (out.len() == size).then_some(out)
}

/// Collect (title, url) from a Chrome bookmark folder, depth first, in Chrome's order.
fn walk(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
    match node["type"].as_str() {
        Some("url") => {
            if let Some(u) = node["url"].as_str() {
                out.push((node["name"].as_str().unwrap_or("").to_string(), u.to_string()));
            }
        }
        _ => {
            for c in node["children"].as_array().into_iter().flatten() {
                walk(c, out);
            }
        }
    }
}

/// Lets `save` write differently typed lists through one closure.
mod erased {
    pub trait Json {
        fn json(&self) -> Option<String>;
    }
    impl<T: serde::Serialize> Json for T {
        fn json(&self) -> Option<String> {
            serde_json::to_string(self).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("ultra-atomic-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let f = dir.join("session.json");
        write_atomic(&f, "one").unwrap();
        write_atomic(&f, "two").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "two");
        assert!(!dir.join("session.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn suggestions_rank_and_filter() {
        let mut l = Library::default();
        l.visit("https://github.com/rust-lang/rust", "rust-lang/rust");
        l.visit("https://news.ycombinator.com/", "Hacker News");
        l.visit("https://www.github.com/", "GitHub");
        l.visit("ultra:home", "");
        assert_eq!(l.history.len(), 3, "non-web urls are not remembered");
        let s = l.suggest("git", 8);
        assert_eq!(s[0].0, "https://www.github.com/", "host prefix ranks first");
        assert!(l.suggest("rust lang", 8).iter().all(|x| x.0.contains("rust")));
        assert!(l.toggle_bookmark("https://news.ycombinator.com/", "HN"));
        assert!(l.suggest("news", 8)[0].2);
        assert!(!l.toggle_bookmark("https://news.ycombinator.com/", "HN"));
    }

    #[test]
    fn chrome_bookmark_tree_flattens_in_order() {
        let j: serde_json::Value = serde_json::from_str(r#"{"type":"folder","children":[
            {"type":"url","name":"GitHub","url":"https://github.com/"},
            {"type":"folder","name":"dev","children":[{"type":"url","name":"Docs","url":"https://docs.rs/"}]},
            {"type":"url","name":"HN","url":"https://news.ycombinator.com/"}]}"#).unwrap();
        let mut out = Vec::new();
        walk(&j, &mut out);
        let urls: Vec<&str> = out.iter().map(|x| x.1.as_str()).collect();
        assert_eq!(urls, ["https://github.com/", "https://docs.rs/", "https://news.ycombinator.com/"]);
    }

    #[test]
    fn firefox_backup_decompresses_and_flattens() {
        // "abcabcabcabc" + "XYZ": 3 literals, a 9-byte back-reference, 3 closing literals.
        let mut f = b"mozLz40\0".to_vec();
        f.extend(15u32.to_le_bytes());
        f.extend([0x35, b'a', b'b', b'c', 3, 0, 0x30, b'X', b'Y', b'Z']);
        assert_eq!(mozlz4(&f).as_deref(), Some(&b"abcabcabcabcXYZ"[..]));
        assert_eq!(mozlz4(b"notmozlz"), None);
        let j: serde_json::Value = serde_json::from_str(r#"{"type":"text/x-moz-place-container","children":[
            {"type":"text/x-moz-place-container","title":"toolbar","children":[
                {"type":"text/x-moz-place","title":"MDN","uri":"https://developer.mozilla.org/"},
                {"type":"text/x-moz-place-separator"}]},
            {"type":"text/x-moz-place","title":"Rust","uri":"https://www.rust-lang.org/"}]}"#).unwrap();
        let mut out = Vec::new();
        walk_firefox(&j, &mut out);
        assert_eq!(out, [("MDN".to_string(), "https://developer.mozilla.org/".to_string()), ("Rust".to_string(), "https://www.rust-lang.org/".to_string())]);
        assert_eq!(browser_name("edge"), "Edge");
    }

    #[test]
    fn shortcuts_add_remove_and_fall_back_to_most_visited() {
        let mut l = Library::default();
        l.visit("https://github.com/a", "A");
        l.visit("https://github.com/b", "B");
        l.visit("https://docs.rs/x", "X");
        let (auto, is_auto) = l.shortcuts_view();
        assert!(is_auto);
        assert_eq!(auto[0], Shortcut { url: "https://github.com/".into(), title: "github.com".into() });
        assert!(l.add_shortcut("https://news.ycombinator.com/", ""));
        assert!(!l.add_shortcut("https://news.ycombinator.com/", "again"), "no duplicates");
        assert!(!l.add_shortcut("javascript:alert(1)", "x"), "web pages only");
        let (mine, is_auto) = l.shortcuts_view();
        assert!(!is_auto);
        assert_eq!(mine[0].title, "news.ycombinator.com");
        l.remove_shortcut("https://news.ycombinator.com/");
        assert!(l.shortcuts.is_empty());
    }

    #[test]
    fn firefox_pinned_sites_parse() {
        let prefs = "user_pref(\"a.b\", 1);\nuser_pref(\"browser.newtabpage.pinned\", \"[{\\\"url\\\":\\\"https://www.youtube.com/\\\",\\\"label\\\":\\\"YouTube\\\"},null,{\\\"url\\\":\\\"https://example.org/\\\"}]\");\n";
        assert_eq!(firefox_pinned(prefs), [("YouTube".to_string(), "https://www.youtube.com/".to_string()), (String::new(), "https://example.org/".to_string())]);
        assert!(firefox_pinned("nothing here").is_empty());
    }

    #[test]
    fn chrome_session_tabs_follow_navigation_and_closes() {
        fn pickle_nav(tab: i32, index: i32, url: &str, title: &str) -> Vec<u8> {
            let mut b = Vec::new();
            b.extend(0u32.to_le_bytes());
            b.extend(tab.to_le_bytes());
            b.extend(index.to_le_bytes());
            b.extend((url.len() as i32).to_le_bytes());
            b.extend(url.as_bytes());
            while b.len() % 4 != 0 {
                b.push(0);
            }
            let t: Vec<u16> = title.encode_utf16().collect();
            b.extend((t.len() as i32).to_le_bytes());
            for u in t {
                b.extend(u.to_le_bytes());
            }
            b
        }
        let mut f = b"SNSS".to_vec();
        f.extend(3i32.to_le_bytes());
        let mut cmd = |id: u8, p: Vec<u8>| {
            f.extend(((p.len() + 1) as u16).to_le_bytes());
            f.push(id);
            f.extend(p);
        };
        cmd(6, pickle_nav(1, 0, "https://a.example/", "A"));
        cmd(6, pickle_nav(1, 1, "https://a.example/next", "A next"));
        cmd(7, [1i32.to_le_bytes(), 0i32.to_le_bytes()].concat()); // tab 1 went back to its first page
        cmd(6, pickle_nav(2, 0, "https://b.example/", "B"));
        cmd(6, pickle_nav(3, 0, "chrome://settings/", "Settings"));
        cmd(6, pickle_nav(4, 0, "https://closed.example/", "Gone"));
        cmd(16, [4i32.to_le_bytes().to_vec(), 0i64.to_le_bytes().to_vec()].concat());
        assert_eq!(chromium_session_tabs(&f), [("A".to_string(), "https://a.example/".to_string()), ("B".to_string(), "https://b.example/".to_string())]);
        assert!(chromium_session_tabs(b"nope").is_empty());
    }

    #[test]
    fn history_merges_and_engines_map() {
        let mut l = Library::default();
        l.visit("https://github.com/", "GitHub");
        let added = l.merge_history(vec![
            ("https://github.com/".into(), "".into(), 40, 5),
            ("https://docs.rs/".into(), "Docs".into(), 3, 7),
            ("file:///c:/secret.txt".into(), "x".into(), 9, 9),
        ]);
        assert_eq!(added, 1);
        assert_eq!(l.history.iter().find(|v| v.url == "https://github.com/").unwrap().visits, 40);
        assert_eq!(engine_of("google.com"), Some("google"));
        assert_eq!(engine_of("ddg@search.mozilla.org"), Some("duckduckgo"));
        assert_eq!(engine_of("Yahoo"), None);
    }

    /// Reads the real browsers on this computer and prints only counts (run by hand:
    /// cargo test --release real_browsers -- --ignored --nocapture).
    #[test]
    #[ignore]
    fn real_browsers() {
        for (id, name) in installed_browsers() {
            let base = browser_dir(id).unwrap();
            match if id == "firefox" { firefox_profile(&base, name) } else { chromium_profile(&base, name, id) } {
                Ok(f) => println!("{name}: {} bookmarks, {} shortcuts, {} history, {} open tabs, search {:?}", f.bookmarks.len(), f.shortcuts.len(), f.history.len(), f.tabs.len(), f.search),
                Err(e) => println!("{name}: {e}"),
            }
        }
    }

    #[test]
    fn digest_counts_today() {
        let mut l = Library::default();
        assert_eq!(host("https://www.GitHub.com:443/a?b").as_deref(), Some("github.com"));
        l.visit("https://github.com/", "GitHub");
        l.add_time("https://github.com/x", 90);
        l.add_time("https://docs.rs/", 30);
        l.add_time("ultra:home", 500);
        l.add_blocked(7);
        let d = l.digest_view();
        assert_eq!((d.secs, d.pages, d.blocked), (120, 1, 7));
        assert_eq!(d.top[0], ("github.com".to_string(), 90));
    }
}
