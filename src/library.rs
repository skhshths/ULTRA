//! Local history, bookmarks and command-palette history. Private-workspace pages
//! never reach this module. Everything lives in %LOCALAPPDATA%\Ultra as JSON.

use std::{cell::Cell, fs, path::Path};

use serde::{Deserialize, Serialize};

const MAX_HISTORY: usize = 5000;
const MAX_COMMANDS: usize = 50;

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

#[derive(Default)]
pub struct Library {
    pub history: Vec<Visit>,
    pub bookmarks: Vec<Bookmark>,
    pub commands: Vec<String>,
    dirty: Cell<bool>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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
                let _ = fs::write(dir.join(name), j);
            }
        };
        write("history.json", &self.history);
        write("bookmarks.json", &self.bookmarks);
        write("commands.json", &self.commands);
    }

    pub fn visit(&mut self, url: &str, title: &str) {
        if !rememberable(url) {
            return;
        }
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

    pub fn clear_history(&mut self) {
        self.history.clear();
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
}
