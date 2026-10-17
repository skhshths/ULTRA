//! The downloads list behind the downloads panel (ctrl+j): what was downloaded, where it went,
//! and live progress for the ones still coming in. Finished downloads are remembered in
//! downloads.json; private-workspace downloads are listed but never written anywhere.

use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_KEPT: usize = 200;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Active,
    Paused,
    Done,
    Failed,
    Cancelled,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Download {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub url: String,
    pub received: i64,
    /// Total size in bytes, 0 while unknown.
    pub total: i64,
    pub state: State,
    /// Unix seconds when it started.
    pub started: u64,
    #[serde(skip)]
    pub private: bool,
    /// Bytes per second, smoothed.
    #[serde(skip)]
    rate: f64,
    #[serde(skip)]
    sample: Option<(Instant, i64)>,
}

#[derive(Default)]
pub struct Downloads {
    pub list: Vec<Download>,
    next: u64,
    dirty: Cell<bool>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The file name part of a path, whichever separator it uses.
pub fn file_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

impl Downloads {
    pub fn load(dir: &Path) -> Self {
        let mut list: Vec<Download> =
            fs::read_to_string(dir.join("downloads.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        // Anything still running when ULTRA last closed was cut off.
        for d in &mut list {
            if matches!(d.state, State::Active | State::Paused) {
                d.state = State::Failed;
            }
        }
        let next = list.iter().map(|d| d.id).max().unwrap_or(0);
        Downloads { list, next, dirty: Cell::new(false) }
    }

    pub fn save(&self, dir: &Path) {
        if !self.dirty.replace(false) {
            return;
        }
        let kept: Vec<&Download> = self.list.iter().filter(|d| !d.private).collect();
        if let Ok(j) = serde_json::to_string(&kept) {
            let _ = fs::write(dir.join("downloads.json"), j);
        }
    }

    /// A new download; returns its id. Newest first.
    pub fn start(&mut self, url: &str, path: &str, total: i64, private: bool) -> u64 {
        self.next += 1;
        self.list.insert(
            0,
            Download {
                id: self.next,
                name: file_name(path),
                path: path.to_string(),
                url: url.to_string(),
                received: 0,
                total: total.max(0),
                state: State::Active,
                started: now(),
                private,
                rate: 0.0,
                sample: None,
            },
        );
        self.list.truncate(MAX_KEPT);
        self.dirty.set(true);
        self.next
    }

    fn get(&mut self, id: u64) -> Option<&mut Download> {
        self.list.iter_mut().find(|d| d.id == id)
    }

    /// New byte counts. True when the change is worth redrawing (about 4 times a second).
    pub fn progress(&mut self, id: u64, received: i64, total: i64) -> bool {
        let Some(d) = self.get(id) else { return false };
        d.received = received;
        if total > 0 {
            d.total = total;
        }
        let t = Instant::now();
        match d.sample {
            Some((at, bytes)) => {
                let dt = t.duration_since(at).as_secs_f64();
                if dt < 0.25 {
                    return false;
                }
                let r = (received - bytes).max(0) as f64 / dt;
                d.rate = if d.rate == 0.0 { r } else { d.rate * 0.7 + r * 0.3 };
                d.sample = Some((t, received));
                true
            }
            None => {
                d.sample = Some((t, received));
                true
            }
        }
    }

    pub fn set_state(&mut self, id: u64, state: State, path: Option<&str>) {
        if let Some(d) = self.get(id) {
            d.state = state;
            d.rate = 0.0;
            d.sample = None;
            if let Some(p) = path.filter(|p| !p.is_empty()) {
                d.path = p.to_string();
                d.name = file_name(p);
            }
            if state == State::Done && d.total > 0 {
                d.received = d.total;
            }
            self.dirty.set(true);
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.list.retain(|d| d.id != id);
        self.dirty.set(true);
    }

    /// Forget everything that isn't still downloading.
    pub fn clear(&mut self) {
        self.list.retain(|d| matches!(d.state, State::Active | State::Paused));
        self.dirty.set(true);
    }

    pub fn path_of(&self, id: u64) -> Option<String> {
        self.list.iter().find(|d| d.id == id).map(|d| d.path.clone())
    }

    pub fn active(&self) -> usize {
        self.list.iter().filter(|d| d.state == State::Active).count()
    }

    /// What the chrome shows: the list, plus whether each finished file is still on disk.
    pub fn view(&self, show_private: bool) -> Value {
        let items: Vec<Value> = self
            .list
            .iter()
            .filter(|d| show_private || !d.private)
            .map(|d| {
                let gone = d.state == State::Done && !Path::new(&d.path).exists();
                json!({ "id": d.id, "n": d.name, "u": d.url, "r": d.received, "t": d.total, "s": d.state,
                        "at": d.started, "bps": d.rate.round(), "gone": gone, "p": d.private })
            })
            .collect();
        let (got, all) = self
            .list
            .iter()
            .filter(|d| d.state == State::Active && d.total > 0)
            .fold((0i64, 0i64), |(g, a), d| (g + d.received, a + d.total));
        // ctl: whether running downloads can be paused and cancelled (WebView2 yes, WKWebView via wry no).
        json!({ "items": items, "active": self.active(), "pct": if all > 0 { got as f64 / all as f64 } else { -1.0 }, "ctl": cfg!(windows) })
    }
}

/// The user's Downloads folder.
pub fn folder() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.map(PathBuf::from).unwrap_or_default().join("Downloads")
}

/// Open a downloaded file with its usual app.
pub fn open(path: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// Show a file selected in Explorer / Finder (or just open the folder if it's gone).
pub fn reveal(path: &str) {
    let p = Path::new(path);
    if !p.exists() {
        open(&p.parent().map(Path::to_path_buf).unwrap_or_else(folder).to_string_lossy());
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{path}\"")).spawn();
    }
    #[cfg(not(windows))]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_and_private_downloads_stay_off_disk() {
        let dir = std::env::temp_dir().join(format!("ultra-dl-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let mut d = Downloads::default();
        let a = d.start("https://x.test/a.zip", r"C:\Users\u\Downloads\a.zip", 100, false);
        let b = d.start("https://x.test/b.pdf", "/Users/u/Downloads/b.pdf", 0, true);
        assert_eq!(d.list[0].id, b, "newest first");
        assert_eq!(d.list[1].name, "a.zip");
        assert_eq!(d.list[0].name, "b.pdf");
        assert!(d.progress(a, 10, 100));
        assert!(!d.progress(a, 20, 100), "redraws are throttled");
        assert_eq!(d.active(), 2);
        d.set_state(a, State::Done, None);
        assert_eq!(d.list[1].received, 100);
        d.save(&dir);
        let back = Downloads::load(&dir);
        assert_eq!(back.list.len(), 1, "private download not saved");
        assert_eq!(back.list[0].state, State::Done);
        // A download that was running when ULTRA closed comes back as failed.
        let mut c = Downloads::default();
        c.start("https://x.test/c", "c", 0, false);
        c.save(&dir);
        assert_eq!(Downloads::load(&dir).list[0].state, State::Failed);
        d.clear();
        assert_eq!(d.list.len(), 1, "the running private one stays");
        assert_eq!(d.view(false)["items"].as_array().unwrap().len(), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
