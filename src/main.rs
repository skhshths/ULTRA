#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! ULTRA — a minimalist tiling browser.
//!
//! Memory: every tab is just a (url, title) pair until it is shown. Tabs on
//! screen plus a small LRU set of recently used tabs keep a live WebView;
//! hidden ones are suspended after a short while and eventually put to sleep.
//! Workspaces each own a layout tree; only the visible one keeps pages on screen.
//!
//! Smoothness: WebView2 creation blocks the UI thread, so every webview reuses
//! one shared environment and blank "spare" webviews are pre-built while the
//! user is idle; waking a tab then just points a spare at a URL. UI updates
//! are coalesced to one per event-loop turn, and layout changes animate with
//! the same eased frames applied to both the native webviews and the chrome.

mod audio;
mod blocker;
mod layout;
mod library;
mod privacy;
mod stats;

use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use layout::{Node, Splitter, R};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::{IconExtWindows, WindowExtWindows},
    window::{Icon, ResizeDirection, Window, WindowBuilder},
};
use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_13, ICoreWebView2_3, ICoreWebView2_8, COREWEBVIEW2_KEY_EVENT_KIND,
        COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
        COREWEBVIEW2_PHYSICAL_KEY_STATUS, COREWEBVIEW2_PROCESS_FAILED_KIND,
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    },
    AcceleratorKeyPressedEventHandler, ProcessFailedEventHandler, TrySuspendCompletedHandler,
};
use windows_core::{Interface, BOOL, HSTRING};
use wry::{
    MemoryUsageLevel, NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

const BAR: f64 = 34.0;
/// Height of the bookmarks bar under the top bar (when shown).
const BMBAR: f64 = 26.0;
const GAP: f64 = 6.0;
/// Height of the per-pane header strip shown while "pane stats" is on.
const HDR: f64 = 18.0;
/// Smallest pane a split may produce; below this the tab replaces the pane instead.
const MIN_W: f64 = 260.0;
const MIN_H: f64 = 160.0;
const WORKSPACES: usize = 10;
/// The last workspace is private: InPrivate tabs, never saved, locked when idle.
const PRIVATE_WS: usize = 9;
/// Seconds without keyboard/mouse input before the private workspace locks.
const LOCK_AFTER: u64 = 300;
/// Wrong unlock attempts before the private workspace is wiped.
const MAX_UNLOCK_FAILS: u32 = 5;
const HOME: &str = "ultra:home";
const LEGACY_HOME: &str = "zenith:home";
const UI_HTML: &str = include_str!("ui.html");
const CONTENT_JS: &str = include_str!("content.js");
/// Hidden tabs that stay live (besides the visible ones) before the LRU one is put to sleep.
const DEFAULT_WARM: usize = 3;
/// Hidden live tabs are suspended (no CPU) after this long, and put to sleep after IDLE_SLEEP.
const SUSPEND_AFTER: Duration = Duration::from_secs(20);
const IDLE_SLEEP: Duration = Duration::from_secs(10 * 60);
const TICK: Duration = Duration::from_secs(15);
const STATS_EVERY: Duration = Duration::from_millis(1500);
/// Layout animation length and frame pacing.
const ANIM: Duration = Duration::from_millis(170);
const FRAME: Duration = Duration::from_millis(8);
/// The spare webview is rebuilt only after this much quiet, so its creation stall goes unnoticed.
const SPARE_IDLE: Duration = Duration::from_millis(1200);
/// Pre-built blank webviews kept ready (creating one costs ~80 ms of UI-thread time).
const SPARES: usize = 2;
/// Must be identical for every webview so they all share one WebView2 environment.
/// SmartScreen (phishing/malware protection) is deliberately left on.
const BROWSER_ARGS: &str = "--enable-features=msSmartScreenProtection";
const THEMES: &[&str] = &[
    "mint", "amber", "red", "mono", "ice", "matrix", "violet", "pink", "orange", "cyan", "gold", "lime", "custom",
];
const BG_MODES: &[&str] = &["swirl", "matrix", "stars", "gradient", "solid", "image"];
/// Largest background image accepted (as a base64 data URL).
const BG_MAX: usize = 16 * 1024 * 1024;

enum UserEvent {
    Ui(String),
    Content(u64, String),
    Title(u64, String),
    Load(u64, bool),
    /// A page asked for a new tab: (source slot, url).
    NewTab(u64, String),
    Procs(Vec<u32>),
    /// JS heap (MB) reported by a page.
    Heap(u64, String),
    /// A WebView2 process died: (slot, 0 = the chrome UI; failure kind).
    Failed(u64, i32),
}

struct Tab {
    id: u64,
    /// Identity of the webview currently backing this tab (its events carry it).
    slot: u64,
    ws: usize,
    url: String,
    title: String,
    loading: bool,
    view: Option<WebView>,
    last: Instant,
    hidden_at: Option<Instant>,
    suspended: bool,
    /// Last time this tab's renderer crashed (a second crash soon after puts it to sleep).
    crashed_at: Option<Instant>,
    /// Last address recorded in history (a page can report "finished" more than once).
    remembered: String,
}

struct Anim {
    from: HashMap<u64, R>,
    to: Vec<(u64, R)>,
    start: Instant,
}

#[derive(Serialize, Deserialize, Default)]
struct Session {
    tabs: Vec<(String, String)>,
    active: usize,
    /// Pre-workspace layout (tab indices); still read for old sessions.
    #[serde(default)]
    layout: Option<Node>,
    /// Workspace of each tab.
    #[serde(default)]
    ws: Vec<usize>,
    /// Per workspace: layout of tab indices and the focused tab index.
    #[serde(default)]
    layouts: Vec<Option<(Node, usize)>>,
    #[serde(default)]
    cur_ws: usize,
}

/// A named layout: its tabs and tree (leaves are indices into `tabs`).
#[derive(Serialize, Deserialize, Clone)]
struct SavedLayout {
    tabs: Vec<(String, String)>,
    tree: Node,
    focus: usize,
}

/// User settings, persisted to settings.json.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    /// 100% speed, 0% looks: no swirl canvas, no transitions, no pane animations.
    ultraspeed: bool,
    /// Hidden tabs kept live for instant switching.
    warm: usize,
    search: String,
    /// Letters/numbers typed outside a text field jump to the address bar.
    typeahead: bool,
    /// Show the tutorial every time ULTRA starts.
    tour: bool,
    /// Per-pane header with live memory/CPU.
    stats: bool,
    /// Ad & tracker blocking.
    block: bool,
    theme: String,
    scanlines: bool,
    /// Accent used by the "custom" theme (#rrggbb).
    accent: String,
    /// Base background colour (#rrggbb).
    bgcolor: String,
    /// Background style: "swirl", "solid" or "image".
    bgmode: String,
    /// How much a background image is darkened, 0-90 (%).
    bgdim: u8,
    /// Swirl glyph colour (#rrggbb); empty = follow the accent.
    swirlcolor: String,
    /// Swirl visibility, 30-300 (%).
    swirlbright: u16,
    /// Private-workspace password as `salt$hash` (never the password; never sent to the UI).
    privpass: String,
    /// Logo boot animation on launch.
    boot: bool,
    /// Backgrounds, focused border and logo pulse with audio playing in ULTRA.
    audio: bool,
    /// Bookmarks bar under the top bar.
    bmbar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let warm = std::env::var("ULTRA_WARM_TABS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_WARM);
        Settings {
            ultraspeed: false,
            warm,
            search: "duckduckgo".into(),
            typeahead: true,
            tour: true,
            stats: false,
            block: true,
            theme: "mint".into(),
            scanlines: false,
            accent: "#9fe3d0".into(),
            bgcolor: "#141414".into(),
            bgmode: "swirl".into(),
            bgdim: 45,
            swirlcolor: String::new(),
            swirlbright: 100,
            privpass: String::new(),
            boot: true,
            audio: true,
            bmbar: true,
        }
    }
}

fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// A background image is kept as its data URL; only real image types are accepted.
fn valid_bg_image(s: &str) -> bool {
    s.len() <= BG_MAX
        && ["data:image/png;base64,", "data:image/jpeg;base64,", "data:image/webp;base64,"]
            .iter()
            .any(|p| s.starts_with(p))
        && s.split_once(',').is_some_and(|(_, b)| {
            b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')
        })
}

fn search_url(engine: &str) -> &'static str {
    match engine {
        "google" => "https://www.google.com/search?q=",
        "bing" => "https://www.bing.com/search?q=",
        "brave" => "https://search.brave.com/search?q=",
        _ => "https://duckduckgo.com/?q=",
    }
}

struct App {
    window: Window,
    ui: WebView,
    ctx: WebContext,
    proxy: EventLoopProxy<UserEvent>,
    tabs: Vec<Tab>,
    /// Layout and focused tab of the current workspace.
    root: Node,
    focus: u64,
    ws: usize,
    /// Layouts of the other workspaces.
    stash: Vec<Option<(Node, u64)>>,
    /// Pane temporarily filling the whole workspace.
    zoom: Option<u64>,
    /// Next layout animation slides in from this side (workspace switch).
    slide: Option<f64>,
    next_id: u64,
    next_slot: u64,
    closed: Vec<(String, String)>,
    cfg: Settings,
    overlay: bool,
    /// Tutorial open: pages stay hidden while the user practises on the live layout.
    tour: bool,
    tour_offered: bool,
    data_dir: PathBuf,
    shared_env: bool,
    spares: Vec<(u64, WebView)>,
    spare_at: Option<Instant>,
    anim: Option<Anim>,
    /// Rects currently on screen (mid-animation values while animating).
    shown: Vec<(u64, R)>,
    blocked: Rc<Cell<u64>>,
    /// Latest (MB, CPU %) per tab id, and process CPU times for deltas.
    pane_stats: HashMap<u64, (f64, f64)>,
    cpu_prev: HashMap<u32, (u64, Instant)>,
    total_mb: f64,
    total_cpu: f64,
    stats_at: Instant,
    stats_pending: bool,
    /// Private workspace locked (password needed to see it again).
    locked: bool,
    unlock_fails: u32,
    lock_check_at: Instant,
    layouts: BTreeMap<String, SavedLayout>,
    /// History, bookmarks and palette history.
    lib: library::Library,
    /// Boot animation playing: pages stay hidden until the UI says it's done.
    booting: bool,
    /// For the daily digest: whether ULTRA has focus, the page being timed, blocked-count baseline.
    win_focused: bool,
    timing: Option<(String, Instant)>,
    last_blocked: u64,
    /// After the window is activated (alt+tab), check where keyboard focus landed.
    refocus_at: Option<Instant>,
    /// Audio-reactive visuals: mixer meters for the WebView2 process tree.
    meter: audio::Meter,
    engine_pids: HashSet<u32>,
    pids_dirty: bool,
    meter_at: Instant,
    audio_at: Instant,
    audio_on: bool,
    boot_until: Instant,
    dirty: Cell<bool>,
    visible: bool,
    show_at: Instant,
    next_tick: Instant,
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Ultra");
    // One-time migration from the browser's old name keeps sessions and sign-ins.
    let old = base.join("Zenith");
    if !dir.exists() && old.exists() {
        let _ = fs::rename(&old, &dir);
    }
    dir
}

/// Turn whatever was typed into the address bar into a URL.
fn resolve(input: &str, engine: &str) -> String {
    let s = input.trim();
    if s.is_empty() || s == "home" || s == HOME || s == LEGACY_HOME {
        return HOME.into();
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("javascript:") {
        return HOME.into();
    }
    if ["http://", "https://", "file://", "about:", "edge://"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return s.into();
    }
    let looks_like_host = !s.contains(' ')
        && (s.contains('.') || lower.starts_with("localhost") || s.contains(':'));
    if looks_like_host {
        let scheme = if lower.starts_with("localhost") || lower.starts_with("127.") {
            "http"
        } else {
            "https"
        };
        return format!("{scheme}://{s}");
    }
    let mut q = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => q.push(b as char),
            b' ' => q.push('+'),
            _ => q.push_str(&format!("%{b:02X}")),
        }
    }
    format!("{}{q}", search_url(engine))
}

/// Pick the most useful split of pane `r` for a cursor at (px, py):
/// the nearest edge wins, biased so wide panes split side by side and tall
/// panes stack. Returns (side_by_side, new_goes_first), or None if too small.
fn pick_split(r: R, px: f64, py: f64) -> Option<(bool, bool)> {
    let nx = ((px - r.x) / r.w).clamp(0.0, 1.0);
    let ny = ((py - r.y) / r.h).clamp(0.0, 1.0);
    let aspect = (r.w / r.h).max(0.01).sqrt();
    let prefer_v = nx.min(1.0 - nx) / aspect <= ny.min(1.0 - ny) * aspect;
    let can_v = (r.w - GAP) / 2.0 >= MIN_W;
    let can_h = (r.h - GAP) / 2.0 >= MIN_H;
    let v = match (prefer_v, can_v, can_h) {
        (true, true, _) | (false, true, false) => true,
        (false, _, true) | (true, false, true) => false,
        _ => return None,
    };
    Some((v, if v { nx < 0.5 } else { ny < 0.5 }))
}

/// Webview bounds inside a pane: 1px border, plus the header strip when shown.
fn inset(r: R, top: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(r.x + 1.0, r.y + 1.0 + top).into(),
        size: LogicalSize::new((r.w - 2.0).max(1.0), (r.h - 2.0 - top).max(1.0)).into(),
    }
}

fn ease(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

fn lerp(a: R, b: R, t: f64) -> R {
    R {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        w: a.w + (b.w - a.w) * t,
        h: a.h + (b.h - a.h) * t,
    }
}

/// Where a newly tiled pane grows from.
fn popin(r: R) -> R {
    let (cx, cy) = r.center();
    R { x: cx - r.w * 0.45, y: cy - r.h * 0.45, w: r.w * 0.9, h: r.h * 0.9 }
}

fn playing_audio(v: &WebView) -> bool {
    let Ok(w) = v.webview().cast::<ICoreWebView2_8>() else { return false };
    let mut b = BOOL(0);
    unsafe { w.IsDocumentPlayingAudio(&mut b).is_ok() && b.as_bool() }
}

/// Register a script for every future document of an existing webview
/// (later scripts run after earlier ones, so a newer flag value wins).
fn add_doc_script(v: &WebView, js: &str) {
    let h = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(|_, _| Ok(())));
    let js = HSTRING::from(js);
    unsafe {
        let _ = v.webview().AddScriptToExecuteOnDocumentCreated(&js, &h);
    }
}

fn typeahead_js(on: bool) -> String {
    format!("window.__ultraType = {on};")
}

/// Freeze a hidden tab's renderer; WebView2 resumes it automatically when shown.
fn try_suspend(v: &WebView) {
    if let Ok(w) = v.webview().cast::<ICoreWebView2_3>() {
        let h = TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
        unsafe {
            let _ = w.TrySuspend(&h);
        }
    }
}

/// Report WebView2 process failures for this webview to the event loop.
fn watch_failures(v: &WebView, proxy: EventLoopProxy<UserEvent>, slot: u64) {
    let h = ProcessFailedEventHandler::create(Box::new(move |_, args| {
        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
        if let Some(a) = args {
            unsafe {
                let _ = a.ProcessFailedKind(&mut kind);
            }
        }
        let _ = proxy.send_event(UserEvent::Failed(slot, kind.0));
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = v.webview().add_ProcessFailed(&h, &mut token);
    }
}

/// Map a Ctrl/Alt key combo to an ULTRA command (the same messages pages used to send).
fn shortcut(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<String> {
    if !ctrl {
        return None;
    }
    let digit = match vk {
        0x31..=0x39 => Some(vk - 0x30),
        0x61..=0x69 => Some(vk - 0x60),
        _ => None,
    };
    let zero = vk == 0x30 || vk == 0x60;
    let key = char::from_u32(vk).filter(|c| c.is_ascii_uppercase());
    let arrow = match vk {
        0x25 => Some('l'),
        0x26 => Some('u'),
        0x27 => Some('r'),
        0x28 => Some('d'),
        _ => None,
    };
    Some(match (alt, shift, digit, key) {
        (true, true, Some(d), _) => format!("tows:{d}"),
        (true, false, Some(d), _) => format!("ws:{d}"),
        (true, false, None, _) if zero => "ws:0".into(),
        (true, _, None, Some('L')) => "lock".into(),
        (true, _, None, None) if arrow.is_some() => format!("dir:{}", arrow?),
        (false, true, Some(d), _) => format!("mv:{d}"),
        (false, true, None, Some('F')) => "zoom".into(),
        (false, true, None, Some('P')) => "palette".into(),
        (false, true, None, Some('E')) => "mvnew".into(),
        (false, true, None, Some('W')) => "closeat".into(),
        (false, true, None, Some('T')) => "reopen".into(),
        (false, false, None, Some('T')) => "new".into(),
        (false, false, None, Some('W')) => "close".into(),
        (false, false, None, Some('L')) => "focusurl".into(),
        (false, false, None, Some('K')) => "overview".into(),
        (false, false, None, Some('D')) => "bookmark".into(),
        (false, false, Some(d), _) => format!("tab:{d}"),
        (false, _, None, None) if vk == 0x09 => if shift { "prev".into() } else { "next".into() },
        (false, false, None, None) if vk == 0xBC => "settings".into(),
        _ => return None,
    })
}

/// ULTRA's shortcuts, caught natively before the page sees them. Unlike a page script this
/// works whichever frame has focus (YouTube's player, ad iframes, PDFs, sites that eat keys).
fn watch_keys(v: &WebView, proxy: EventLoopProxy<UserEvent>, slot: u64) {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    let h = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(a) = args else { return Ok(()) };
        let (mut kind, mut vk, mut st) = (COREWEBVIEW2_KEY_EVENT_KIND::default(), 0u32, COREWEBVIEW2_PHYSICAL_KEY_STATUS::default());
        unsafe {
            a.KeyEventKind(&mut kind)?;
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
                return Ok(());
            }
            a.VirtualKey(&mut vk)?;
            a.PhysicalKeyStatus(&mut st)?;
            // Physical state: key events are processed in WebView2's process, not this thread.
            let down = |k: i32| GetAsyncKeyState(k) < 0;
            let Some(msg) = shortcut(vk, down(0x11), down(0x10), down(0x12)) else { return Ok(()) };
            a.SetHandled(true)?;
            // Held keys auto-repeat: act once, except ctrl+tab cycling.
            if st.WasKeyDown.as_bool() && vk != 0x09 {
                return Ok(());
            }
            let _ = proxy.send_event(UserEvent::Content(slot, msg));
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = v.controller().add_AcceleratorKeyPressed(&h, &mut token);
    }
}

/// Start a fresh ULTRA (session already saved) and exit this one. A relaunched
/// instance that fails again within 20 s does not relaunch, to avoid a loop.
fn relaunch() {
    let recovered_at: Option<u64> = std::env::var("ULTRA_RECOVERED_AT").ok().and_then(|v| v.parse().ok());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if !recovered_at.is_some_and(|t| now.saturating_sub(t) < 20) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).env("ULTRA_RECOVERED_AT", now.to_string()).spawn();
        }
    }
    std::process::exit(0);
}

/// Settings as the chrome UI may see them: the password hash never leaves Rust.
fn ui_cfg(cfg: &Settings) -> Value {
    let mut v = serde_json::to_value(cfg).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("privpass");
        o.insert("haspass".into(), json!(!cfg.privpass.is_empty()));
    }
    v
}

/// The exe's embedded icon (see build.rs), so the window and taskbar match it at every size.
fn make_icon() -> Option<Icon> {
    Icon::from_resource(1, None).ok()
}

impl App {
    fn logical_size(&self) -> (f64, f64) {
        let s = self.window.inner_size().to_logical::<f64>(self.window.scale_factor());
        (s.width, s.height)
    }

    fn full_rect(&self) -> Rect {
        let (w, h) = self.logical_size();
        Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(w, h).into(),
        }
    }

    fn area(&self) -> R {
        let (w, h) = self.logical_size();
        let top = BAR + if self.cfg.bmbar { BMBAR } else { 0.0 };
        R { x: GAP, y: top, w: (w - 2.0 * GAP).max(1.0), h: (h - top - GAP).max(1.0) }
    }

    fn top(&self) -> f64 {
        if self.cfg.stats {
            HDR
        } else {
            0.0
        }
    }

    /// Visible panes of the current workspace (just the zoomed one while zoomed).
    fn panes(&self) -> Vec<(u64, R)> {
        if let Some(z) = self.zoom.filter(|z| self.root.contains(*z)) {
            return vec![(z, self.area())];
        }
        let mut v = Vec::new();
        self.root.rects(self.area(), GAP, &mut v);
        v
    }

    fn pane_of(&self, id: u64) -> Option<R> {
        self.panes().into_iter().find(|(x, _)| *x == id).map(|(_, r)| r)
    }

    /// Cursor position in logical window coordinates.
    fn cursor(&self) -> Option<(f64, f64)> {
        let c = self.window.cursor_position().ok()?;
        let o = self.window.inner_position().ok()?;
        let s = self.window.scale_factor();
        Some(((c.x - o.x as f64) / s, (c.y - o.y as f64) / s))
    }

    fn pane_under_cursor(&self) -> Option<u64> {
        let (px, py) = self.cursor()?;
        self.panes().into_iter().find(|(_, r)| r.contains(px, py)).map(|(id, _)| id)
    }

    fn idx(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn idx_slot(&self, slot: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.slot == slot && t.view.is_some())
    }

    fn cur(&self) -> usize {
        self.idx(self.focus).unwrap_or(0)
    }

    /// Global indices of the current workspace's tabs, in bar order.
    fn ws_tabs(&self) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].ws == self.ws).collect()
    }

    /// Request a UI refresh; it is sent once at the end of the event-loop turn.
    fn sync(&self) {
        self.dirty.set(true);
    }

    fn build_view(&mut self, slot: u64, url: Option<&str>, bounds: Rect, private: bool) -> Option<WebView> {
        let (p1, p2, p3, p4) = (
            self.proxy.clone(),
            self.proxy.clone(),
            self.proxy.clone(),
            self.proxy.clone(),
        );
        let env = self.shared_env.then(|| self.ui.environment());
        let mut b = WebViewBuilder::new_with_web_context(&mut self.ctx);
        if let Some(env) = env {
            b = b.with_environment(env);
        }
        if let Some(u) = url {
            b = b.with_url(u);
        }
        let res = b
            .with_bounds(bounds)
            .with_visible(false)
            .with_background_color((20, 20, 20, 255))
            .with_devtools(true)
            // Private tabs use WebView2's InPrivate profile: nothing reaches disk.
            .with_incognito(private)
            .with_additional_browser_args(BROWSER_ARGS)
            .with_initialization_script_for_main_only(typeahead_js(self.cfg.typeahead), true)
            .with_initialization_script_for_main_only(CONTENT_JS, true)
            .with_ipc_handler(move |r| {
                let _ = p1.send_event(UserEvent::Content(slot, r.into_body()));
            })
            .with_document_title_changed_handler(move |t| {
                let _ = p2.send_event(UserEvent::Title(slot, t));
            })
            .with_on_page_load_handler(move |e, _| {
                let _ = p3.send_event(UserEvent::Load(slot, matches!(e, PageLoadEvent::Started)));
            })
            .with_new_window_req_handler(move |u, f| {
                // Scripted popups (window.open with a size, e.g. "Sign in with Google")
                // get a real popup that keeps window.opener so OAuth can complete.
                // Plain target=_blank links open as a new tab without an opener.
                if f.size.is_some() {
                    return NewWindowResponse::Allow;
                }
                let _ = p4.send_event(UserEvent::NewTab(slot, u));
                NewWindowResponse::Deny
            })
            .build_as_child(&self.window);
        match res {
            Ok(v) => {
                watch_failures(&v, self.proxy.clone(), slot);
                watch_keys(&v, self.proxy.clone(), slot);
                blocker::install(&v, self.blocked.clone());
                if self.cfg.block {
                    blocker::set_filters(&v, true);
                }
                Some(v)
            }
            Err(e) => {
                eprintln!("ultra: webview failed: {e}");
                None
            }
        }
    }

    fn new_slot(&mut self) -> u64 {
        self.next_slot += 1;
        self.next_slot
    }

    /// Give tab `i` a live webview, preferring the pre-built spare (no stall).
    fn wake(&mut self, i: usize, bounds: Rect) {
        if self.tabs[i].view.is_some() || self.tabs[i].url == HOME {
            return;
        }
        let url = self.tabs[i].url.clone();
        let private = self.tabs[i].ws == PRIVATE_WS;
        // Spares are normal-profile webviews, so private tabs always get a fresh InPrivate one.
        let spare = if private { None } else { self.spares.pop() };
        let got = match spare {
            Some((slot, v)) => {
                let _ = v.set_bounds(bounds);
                let _ = v.load_url(&url);
                Some((slot, v))
            }
            None => {
                let slot = self.new_slot();
                self.build_view(slot, Some(&url), bounds, private).map(|v| (slot, v))
            }
        };
        self.spare_at = Some(Instant::now() + SPARE_IDLE);
        if let Some((slot, v)) = got {
            let t = &mut self.tabs[i];
            t.slot = slot;
            t.view = Some(v);
            t.loading = true;
            t.suspended = false;
        }
    }

    /// Build one spare per quiet moment until the pool is full.
    fn refill_spare(&mut self) {
        if self.spares.len() >= SPARES {
            return;
        }
        if self.anim.is_some() {
            self.spare_at = Some(Instant::now() + SPARE_IDLE);
            return;
        }
        let slot = self.new_slot();
        let a = self.area();
        if let Some(v) = self.build_view(slot, None, inset(a, 0.0), false) {
            self.spares.push((slot, v));
        }
        if self.spares.len() < SPARES {
            self.spare_at = Some(Instant::now() + Duration::from_millis(400));
        }
    }

    /// Push the spare rebuild back while the user is busy.
    fn note_activity(&mut self) {
        if self.spares.len() < SPARES {
            self.spare_at = Some(Instant::now() + SPARE_IDLE);
        }
    }

    fn live_count(&self) -> usize {
        self.tabs.iter().filter(|t| t.view.is_some()).count()
    }

    fn all_views(&self) -> impl Iterator<Item = &WebView> {
        self.tabs.iter().filter_map(|t| t.view.as_ref()).chain(self.spares.iter().map(|(_, v)| v))
    }

    fn sleep(&mut self, i: usize) {
        let t = &mut self.tabs[i];
        if let Some(v) = t.view.take() {
            if let Ok(u) = v.url() {
                if !u.is_empty() && u != "about:blank" {
                    t.url = u;
                }
            }
        }
        t.loading = false;
        t.suspended = false;
        t.slot = 0;
    }

    /// Position, show and hide every webview to match the layout tree.
    fn apply_layout(&mut self, animate: bool) {
        let targets = self.panes();
        let top = self.top();
        for &(id, r) in &targets {
            if let Some(i) = self.idx(id) {
                self.wake(i, inset(r, top));
            }
        }
        let now = Instant::now();
        // Keep private pages out of screenshots and screen sharing while that workspace is shown.
        privacy::exclude_from_capture(self.window.hwnd() as isize, self.ws == PRIVATE_WS);
        let hidden = self.overlay || self.tour || self.gate() || self.booting;
        let animate = animate && !hidden && !self.cfg.ultraspeed;
        let slide = self.slide.take();
        if animate {
            let area = self.area();
            let from = targets
                .iter()
                .map(|&(id, r)| {
                    let start = match slide {
                        Some(d) => R { x: r.x + d * area.w, ..r },
                        None => self.shown.iter().find(|(x, _)| *x == id).map(|&(_, r)| r).unwrap_or_else(|| popin(r)),
                    };
                    (id, start)
                })
                .collect();
            self.anim = Some(Anim { from, to: targets.clone(), start: now });
        } else {
            self.anim = None;
        }
        for t in &mut self.tabs {
            match targets.iter().find(|(id, _)| *id == t.id) {
                Some(&(_, r)) => {
                    t.hidden_at = None;
                    t.suspended = false;
                    if let Some(v) = &t.view {
                        if !animate {
                            let _ = v.set_bounds(inset(r, top));
                        }
                        let _ = v.set_memory_usage_level(MemoryUsageLevel::Normal);
                        let _ = v.set_visible(!hidden);
                    }
                }
                None => {
                    if let Some(v) = &t.view {
                        let _ = v.set_visible(false);
                        let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                        t.hidden_at.get_or_insert(now);
                    }
                }
            }
        }
        if animate {
            self.step_anim();
        } else {
            self.shown = targets;
        }
        // Put the least-recently-used hidden tabs to sleep until we're under budget,
        // never cutting off a tab that is playing audio.
        while self.live_count() > self.cfg.warm + self.shown.len() {
            let victim = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, t)| !self.shown.iter().any(|(id, _)| *id == t.id))
                .filter(|(_, t)| t.view.as_ref().is_some_and(|v| !playing_audio(v)))
                .min_by_key(|(_, t)| t.last)
                .map(|(i, _)| i);
            match victim {
                Some(i) => self.sleep(i),
                None => break,
            }
        }
        self.sync();
    }

    /// Advance the layout animation one frame. Webviews and chrome get the same rects.
    fn step_anim(&mut self) {
        let Some(a) = &self.anim else { return };
        let t = (a.start.elapsed().as_secs_f64() / ANIM.as_secs_f64()).min(1.0);
        let k = ease(t);
        let top = self.top();
        let frame: Vec<(u64, R)> = a
            .to
            .iter()
            .map(|&(id, to)| (id, lerp(a.from.get(&id).copied().unwrap_or(to), to, k)))
            .collect();
        for &(id, r) in &frame {
            if let Some(v) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.view.as_ref()) {
                let _ = v.set_bounds(inset(r, top));
            }
        }
        let js: Vec<Value> = frame.iter().map(|(id, r)| json!([id, r.x, r.y, r.w, r.h])).collect();
        let _ = self.ui.evaluate_script(&format!("ultra.frame({})", Value::Array(js)));
        self.shown = frame;
        if t >= 1.0 {
            self.anim = None;
            self.sync();
        }
    }

    fn focus_active(&self) {
        if self.tour || self.gate() {
            let _ = self.ui.focus();
            return;
        }
        let i = self.cur();
        match &self.tabs[i].view {
            Some(v) => {
                let _ = v.focus();
            }
            None => {
                let _ = self.ui.focus();
                let _ = self.ui.evaluate_script(&format!("ultra.focusHome({})", self.tabs[i].id));
            }
        }
    }

    fn set_focus(&mut self, id: u64) {
        self.focus = id;
        if let Some(i) = self.idx(id) {
            self.tabs[i].last = Instant::now();
        }
        self.sync();
    }

    /// Show tab `i`: focus its pane if visible, otherwise put it in the focused pane
    /// (switching workspace first if it lives elsewhere).
    fn activate(&mut self, i: usize) {
        let Some((id, ws)) = self.tabs.get(i).map(|t| (t.id, t.ws)) else { return };
        let switched = ws != self.ws;
        if switched {
            self.enter_ws(ws);
        }
        if !self.root.contains(id) && !self.root.replace(self.focus, id) {
            self.root = Node::Leaf(id);
        }
        if self.zoom.is_some() {
            self.zoom = Some(id);
        }
        self.overlay = false;
        self.set_focus(id);
        self.apply_layout(switched);
        self.focus_active();
    }

    /// Swap the current workspace's layout out and workspace `n`'s in (no relayout).
    fn enter_ws(&mut self, n: usize) {
        if n == self.ws || n >= WORKSPACES {
            return;
        }
        self.slide = Some(if n > self.ws { 1.0 } else { -1.0 });
        // A workspace holding nothing but one blank tab disappears when left.
        let lone_home = self.root.is_leaf(self.focus)
            && self.ws_tabs().len() == 1
            && self.idx(self.focus).is_some_and(|i| self.tabs[i].url == HOME);
        let old = std::mem::replace(&mut self.root, Node::Leaf(0));
        if lone_home {
            if let Some(i) = self.idx(self.focus) {
                self.tabs.remove(i);
            }
            self.stash[self.ws] = None;
        } else {
            self.stash[self.ws] = Some((old, self.focus));
        }
        self.ws = n;
        self.zoom = None;
        let (root, focus) = match self.stash[n].take() {
            Some(s) => s,
            None => {
                let id = match self.tabs.iter().find(|t| t.ws == n) {
                    Some(t) => t.id,
                    None => self.push_tab(self.tabs.len(), HOME.into(), String::new()),
                };
                (Node::Leaf(id), id)
            }
        };
        self.root = root;
        self.focus = focus;
    }

    fn switch_ws(&mut self, n: usize) {
        if n == self.ws || n >= WORKSPACES {
            return;
        }
        self.enter_ws(n);
        self.overlay = false;
        self.sync();
        self.apply_layout(true);
        self.focus_active();
    }

    /// Ctrl+Alt+Shift+N: send the focused tab to workspace `n` (tiled into its layout).
    fn send_to_ws(&mut self, n: usize) {
        // Tabs never cross the private boundary: their webviews belong to one profile.
        if n == self.ws || n >= WORKSPACES || n == PRIVATE_WS || self.ws == PRIVATE_WS {
            return;
        }
        let id = self.focus;
        if self.root.is_leaf(id) {
            let other = self.tabs.iter().find(|t| t.ws == self.ws && t.id != id).map(|t| t.id);
            let rep = match other {
                Some(r) => r,
                None => self.push_tab(self.tabs.len(), HOME.into(), String::new()),
            };
            self.root = Node::Leaf(rep);
        } else {
            self.root.remove(id);
        }
        self.zoom = None;
        self.focus = self.root.first_leaf();
        if let Some(i) = self.idx(id) {
            self.tabs[i].ws = n;
        }
        match &mut self.stash[n] {
            Some((root, f)) => {
                root.split(*f, id, true, false);
            }
            slot @ None => *slot = Some((Node::Leaf(id), id)),
        }
        self.apply_layout(true);
        self.focus_active();
    }

    fn push_tab(&mut self, at: usize, url: String, title: String) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let tab = Tab {
            id,
            slot: 0,
            ws: self.ws,
            url,
            title,
            loading: false,
            view: None,
            last: Instant::now(),
            hidden_at: None,
            suspended: false,
            crashed_at: None,
            remembered: String::new(),
        };
        self.tabs.insert(at.min(self.tabs.len()), tab);
        id
    }

    fn new_tab(&mut self, url: String, title: String, activate: bool) {
        let at = self.cur() + 1;
        self.push_tab(at, url, title);
        if activate {
            self.activate(at.min(self.tabs.len() - 1));
        } else {
            self.sync();
        }
    }

    fn close_tab(&mut self, i: usize) {
        let Some((id, ws)) = self.tabs.get(i).map(|t| (t.id, t.ws)) else { return };
        let tiled = !self.root.is_leaf(id) && self.root.contains(id);
        if self.zoom == Some(id) {
            self.zoom = None;
        }
        if self.root.is_leaf(id) {
            // Last pane: show the nearest hidden tab of this workspace, or a fresh home tab.
            let rep = (i + 1..self.tabs.len())
                .chain((0..i).rev())
                .find(|&j| self.tabs[j].ws == ws)
                .map(|j| self.tabs[j].id);
            let rep = match rep {
                Some(r) => r,
                None => self.push_tab(self.tabs.len(), HOME.into(), String::new()),
            };
            self.root = Node::Leaf(rep);
        } else {
            self.root.remove(id);
        }
        for s in self.stash.iter_mut() {
            if matches!(s, Some((r, _)) if r.is_leaf(id)) {
                *s = None;
            } else if let Some((r, f)) = s {
                if r.remove(id) && *f == id {
                    *f = r.first_leaf();
                }
            }
        }
        if self.focus == id || !self.root.contains(self.focus) {
            self.focus = self.root.first_leaf();
        }
        let i = self.idx(id).unwrap_or(i);
        let t = self.tabs.remove(i);
        if t.url != HOME && t.ws != PRIVATE_WS {
            self.closed.push((t.url, t.title));
            if self.closed.len() > 32 {
                self.closed.remove(0);
            }
        }
        drop(t.view);
        self.shown.retain(|(x, _)| *x != id);
        self.apply_layout(tiled);
        self.focus_active();
    }

    /// Ctrl+Shift+N / Ctrl+Shift+E: place a tab in the pane under the cursor,
    /// splitting it in the most useful direction.
    fn place_at_cursor(&mut self, id: u64) {
        self.zoom = None;
        let panes = self.panes();
        let hit = self.cursor().and_then(|(px, py)| {
            panes.iter().find(|(_, r)| r.contains(px, py)).map(|&(t, r)| (t, r, px, py))
        });
        // Cursor outside every pane: split the focused pane toward its far side.
        let (target, r, px, py) = hit.unwrap_or_else(|| {
            let r = self.pane_of(self.focus).unwrap_or(self.area());
            (self.focus, r, r.x + r.w * 0.95, r.y + r.h * 0.5)
        });
        if target == id {
            self.activate(self.idx(id).unwrap_or(0));
            return;
        }
        match pick_split(r, px, py) {
            Some((v, first)) => {
                self.root.remove(id);
                self.root.split(target, id, v, first);
            }
            None if self.root.contains(id) => self.root.swap(target, id),
            None => {
                self.root.replace(target, id);
            }
        }
        self.overlay = false;
        self.set_focus(id);
        self.apply_layout(true);
        self.focus_active();
    }

    /// Split the focused pane in a fixed direction (command palette).
    fn split_dir(&mut self, dir: &str, url: String) {
        self.zoom = None;
        let at = self.cur() + 1;
        let id = self.push_tab(at, url, String::new());
        let (v, first) = match dir {
            "l" => (true, true),
            "r" => (true, false),
            "u" => (false, true),
            _ => (false, false),
        };
        self.root.split(self.focus, id, v, first);
        self.set_focus(id);
        self.apply_layout(true);
        self.focus_active();
    }

    /// Ctrl+Shift+F: zoom the pane under the cursor to fill the workspace, or unzoom.
    fn toggle_zoom(&mut self, target: Option<u64>) {
        let id = target.or_else(|| self.pane_under_cursor()).unwrap_or(self.focus);
        if self.zoom.is_some() && (target.is_none() || self.zoom == Some(id)) {
            self.zoom = None;
        } else {
            if self.root.is_leaf(id) || !self.root.contains(id) {
                return;
            }
            self.zoom = Some(id);
            self.set_focus(id);
        }
        self.apply_layout(true);
        self.focus_active();
    }

    /// Ctrl+Shift+W: close the tab in the pane under the cursor (focused pane as fallback).
    fn close_at_cursor(&mut self) {
        let id = self.pane_under_cursor().unwrap_or(self.focus);
        if let Some(i) = self.idx(id) {
            self.close_tab(i);
        }
    }

    /// Ctrl+Alt+Arrow: move focus to the nearest pane in a direction.
    fn focus_dir(&mut self, d: &str) {
        if self.zoom.is_some() {
            return;
        }
        let panes = self.panes();
        let Some(cur) = self.pane_of(self.focus) else { return };
        let (cx, cy) = cur.center();
        let best = panes
            .iter()
            .filter(|(id, _)| *id != self.focus)
            .filter_map(|&(id, r)| {
                let (x, y) = r.center();
                let (dx, dy) = (x - cx, y - cy);
                let ok = match d {
                    "l" => dx < -1.0,
                    "r" => dx > 1.0,
                    "u" => dy < -1.0,
                    _ => dy > 1.0,
                };
                ok.then_some((id, dx * dx + dy * dy))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, _)) = best {
            self.set_focus(id);
            self.focus_active();
        }
    }

    /// Put every hidden live tab to sleep now (music keeps playing).
    fn trim(&mut self) {
        for i in 0..self.tabs.len() {
            let t = &self.tabs[i];
            let visible = self.shown.iter().any(|(id, _)| *id == t.id);
            if !visible && t.view.as_ref().is_some_and(|v| !playing_audio(v)) {
                self.sleep(i);
            }
        }
        self.sync();
    }

    fn navigate(&mut self, i: usize, input: &str) {
        let url = resolve(input, &self.cfg.search);
        let Some(t) = self.tabs.get_mut(i) else { return };
        if url == HOME {
            t.view = None;
            t.slot = 0;
            t.url = HOME.into();
            t.title.clear();
        } else {
            t.url = url.clone();
            if let Some(v) = &t.view {
                let _ = v.load_url(&url);
                t.loading = true;
            }
        }
        self.activate(i);
    }

    fn with_active(&self, f: impl FnOnce(&WebView)) {
        if let Some(v) = &self.tabs[self.cur()].view {
            f(v)
        }
    }

    /// Kick off an async process sample; results arrive as UserEvent::Procs.
    fn sample_stats(&mut self) {
        let proxy = self.proxy.clone();
        self.stats_pending = stats::request(&self.ui, move |p| {
            let _ = proxy.send_event(UserEvent::Procs(p));
        });
    }

    fn on_procs(&mut self, procs: Vec<u32>) {
        self.stats_pending = false;
        let pids: HashSet<u32> = procs.iter().copied().chain([std::process::id()]).collect();
        if pids != self.engine_pids {
            self.engine_pids = pids;
            self.pids_dirty = true;
        }
        if !self.cfg.stats {
            return;
        }
        let now = Instant::now();
        let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        let mut per: HashMap<u32, (f64, f64)> = HashMap::new();
        let (mut total, mut total_cpu) = (0.0, 0.0);
        let pids = procs.iter().copied().chain([std::process::id()]);
        for pid in pids {
            let Some(u) = stats::usage(pid) else { continue };
            let mb = u.mem as f64 / 1_048_576.0;
            let cpu = match self.cpu_prev.get(&pid) {
                Some(&(t0, i0)) => {
                    let wall = (now - i0).as_secs_f64().max(0.001);
                    u.cpu.saturating_sub(t0) as f64 / 1e7 / wall / ncpu * 100.0
                }
                None => 0.0,
            };
            self.cpu_prev.insert(pid, (u.cpu, now));
            per.insert(pid, (mb, cpu));
            total += mb;
            total_cpu += cpu;
        }
        self.cpu_prev.retain(|p, _| per.contains_key(p));
        self.pane_stats.retain(|id, _| self.shown.iter().any(|(x, _)| x == id));
        // Per pane: ask each visible page for its own JS heap.
        for t in self.tabs.iter().filter(|t| self.shown.iter().any(|(x, _)| *x == t.id)) {
            if let Some(v) = &t.view {
                let (proxy, slot) = (self.proxy.clone(), t.slot);
                let _ = v.evaluate_script_with_callback(stats::HEAP_JS, move |r| {
                    let _ = proxy.send_event(UserEvent::Heap(slot, r));
                });
            }
        }
        self.total_mb = total;
        self.total_cpu = total_cpu;
        self.sync();
    }

    /// Send the full state to the chrome UI (at most once per event-loop turn).
    fn flush(&self) {
        self.dirty.set(false);
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .map(|t| {
                let s = if t.url == HOME {
                    "home"
                } else if t.loading {
                    "load"
                } else if t.view.is_some() {
                    "live"
                } else {
                    "sleep"
                };
                let private = t.ws == PRIVATE_WS;
                // While locked, private tabs show no title or address anywhere.
                let (title, url) = if private && self.locked { ("locked", "") } else { (t.title.as_str(), t.url.as_str()) };
                json!({ "id": t.id, "t": title, "u": url, "s": s, "w": t.ws, "p": private, "v": self.root.contains(t.id) })
            })
            .collect();
        let panes: Vec<Value> = self
            .shown
            .iter()
            .filter_map(|(id, r)| self.idx(*id).map(|i| json!({ "id": id, "i": i, "r": r })))
            .collect();
        let mut splits: Vec<Splitter> = Vec::new();
        if self.anim.is_none() && self.zoom.is_none() {
            self.root.splitters(self.area(), GAP, &mut String::new(), &mut splits);
        }
        let mut wss: Vec<usize> = self.tabs.iter().map(|t| t.ws).chain([self.ws]).collect();
        wss.sort_unstable();
        wss.dedup();
        let ps: HashMap<String, [f64; 2]> =
            self.pane_stats.iter().map(|(id, (m, c))| (id.to_string(), [*m, *c])).collect();
        let state = json!({
            "tabs": tabs,
            "a": self.cur(),
            "panes": panes,
            "splits": splits,
            "live": self.live_count(),
            "ws": self.ws,
            "wss": wss,
            "zoom": self.zoom,
            "ps": ps,
            "mem": self.total_mb,
            "cpu": self.total_cpu,
            "blocked": self.blocked.get(),
            "cfg": ui_cfg(&self.cfg),
            "locked": self.locked,
            "gate": self.gate(),
            "layouts": self.layouts.keys().collect::<Vec<_>>(),
            "bm": self.tabs.get(self.cur()).is_some_and(|t| t.ws != PRIVATE_WS && self.lib.is_bookmarked(&t.url)),
            "palhist": self.lib.commands,
            "digest": self.lib.digest_view(),
            "bms": self.lib.bookmarks.iter().map(|b| json!({ "u": b.url, "t": b.title })).collect::<Vec<_>>(),
            "tour": self.tour,
            "max": self.window.is_maximized(),
        });
        let _ = self.ui.evaluate_script(&format!("ultra.render({state})"));
    }

    fn layout(&mut self) {
        let _ = self.ui.set_bounds(self.full_rect());
        self.apply_layout(false);
    }

    fn save_session(&self) {
        // Private tabs are never written anywhere: positions are among normal tabs only.
        let kept: Vec<u64> = self.tabs.iter().filter(|t| t.ws != PRIVATE_WS).map(|t| t.id).collect();
        let index = |id: u64| kept.iter().position(|&k| k == id).map(|i| i as u64);
        let layout_of = |w: usize| -> Option<(Node, usize)> {
            if w == PRIVATE_WS {
                return None;
            }
            let (root, focus) = if w == self.ws {
                (&self.root, self.focus)
            } else {
                let (r, f) = self.stash[w].as_ref()?;
                (r, *f)
            };
            Some((root.map(&index)?, index(focus).unwrap_or(0) as usize))
        };
        let s = Session {
            tabs: self
                .tabs
                .iter()
                .filter(|t| t.ws != PRIVATE_WS)
                .map(|t| {
                    let url = t
                        .view
                        .as_ref()
                        .and_then(|v| v.url().ok())
                        .filter(|u| !u.is_empty() && u != "about:blank")
                        .unwrap_or_else(|| t.url.clone());
                    (url, t.title.clone())
                })
                .collect(),
            active: index(self.focus).unwrap_or(0) as usize,
            layout: None,
            ws: self.tabs.iter().filter(|t| t.ws != PRIVATE_WS).map(|t| t.ws).collect(),
            layouts: (0..WORKSPACES).map(layout_of).collect(),
            cur_ws: if self.ws == PRIVATE_WS { 0 } else { self.ws },
        };
        if let Ok(j) = serde_json::to_string(&s) {
            let _ = fs::write(self.data_dir.join("session.json"), j);
        }
        self.lib.save(&self.data_dir);
    }

    fn save_settings(&self) {
        if let Ok(j) = serde_json::to_string_pretty(&self.cfg) {
            let _ = fs::write(self.data_dir.join("settings.json"), j);
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        for i in 0..self.tabs.len() {
            let t = &self.tabs[i];
            let (Some(v), Some(h)) = (&t.view, t.hidden_at) else { continue };
            if playing_audio(v) {
                continue;
            }
            if now - h > IDLE_SLEEP && now - t.last > IDLE_SLEEP {
                self.sleep(i);
            } else if !t.suspended && now - h > SUSPEND_AFTER {
                try_suspend(v);
                self.tabs[i].suspended = true;
            }
        }
        // Refresh every tick so the home screen's digest stays current.
        self.sync();
        self.save_session();
    }

    fn show_window(&mut self) {
        if !self.visible {
            self.visible = true;
            self.window.set_visible(true);
            self.window.set_focus();
            self.focus_active();
        }
    }

    /// Next moment the event loop has to wake up on its own.
    fn deadline(&self) -> Instant {
        let now = Instant::now();
        let mut d = self.next_tick;
        if let Some(s) = self.spare_at {
            d = d.min(s);
        }
        if self.anim.is_some() {
            d = d.min(now + FRAME);
        }
        if !self.visible {
            d = d.min(self.show_at);
        }
        if self.cfg.stats && !self.stats_pending {
            d = d.min(self.stats_at);
        }
        if !self.locked && self.has_private() {
            d = d.min(self.lock_check_at);
        }
        if self.booting {
            d = d.min(self.boot_until);
        }
        if let Some(t) = self.refocus_at {
            d = d.min(t);
        }
        if (self.cfg.audio && !self.cfg.ultraspeed) || self.audio_on {
            d = d.min(self.audio_at);
        }
        d
    }

    /// Audio-reactive visuals: while a tab plays sound, send the mixer level to the UI ~30x/s;
    /// otherwise just check twice a second whether anything started playing.
    fn poll_audio(&mut self, now: Instant) {
        let enabled = self.cfg.audio && !self.cfg.ultraspeed && self.visible;
        let playing = enabled && self.tabs.iter().any(|t| t.view.as_ref().is_some_and(playing_audio));
        if !playing {
            if self.audio_on {
                self.audio_on = false;
                let _ = self.ui.evaluate_script("ultra.audio(0)");
            }
            self.audio_at = now + Duration::from_millis(600);
            return;
        }
        // Keep the process list and mixer sessions fresh (output device or tabs may change).
        if now >= self.meter_at {
            self.meter_at = now + Duration::from_secs(2);
            if !self.stats_pending {
                self.sample_stats();
            }
            self.pids_dirty = true;
        }
        if self.pids_dirty {
            self.pids_dirty = false;
            self.meter.refresh(&self.engine_pids);
        }
        self.audio_on = true;
        let level = if self.meter.is_empty() { 0.0 } else { self.meter.peak() };
        let _ = self.ui.evaluate_script(&format!("ultra.audio({level:.3})"));
        self.audio_at = now + Duration::from_millis(33);
    }

    /// Alt+tab activates the top-level window, which leaves keyboard focus on that bare
    /// window rather than in any webview. If that's where it is, hand it to the focused pane.
    /// (A click into a pane has already put focus inside that page, so it's left alone.)
    fn restore_focus(&self) {
        use windows::Win32::{Foundation::HWND, UI::Input::KeyboardAndMouse::GetFocus};
        let f = unsafe { GetFocus() };
        if f.0.is_null() || f == HWND(self.window.hwnd() as *mut _) {
            self.focus_active();
        }
    }

    fn set_setting(&mut self, key: &str, v: &Value) {
        match key {
            "ultraspeed" => self.cfg.ultraspeed = v.as_bool().unwrap_or(false),
            "tour" => self.cfg.tour = v.as_bool().unwrap_or(true),
            "typeahead" => {
                self.cfg.typeahead = v.as_bool().unwrap_or(true);
                // Live pages and spares were built with the old flag: update the current
                // document and every future one.
                let js = typeahead_js(self.cfg.typeahead);
                for view in self.all_views() {
                    add_doc_script(view, &js);
                    let _ = view.evaluate_script(&js);
                }
            }
            "warm" => self.cfg.warm = v.as_u64().unwrap_or(3).min(12) as usize,
            "search" => {
                if let Some(e) = v.as_str().filter(|e| ["duckduckgo", "google", "bing", "brave"].contains(e)) {
                    self.cfg.search = e.to_string();
                }
            }
            "stats" => {
                self.cfg.stats = v.as_bool().unwrap_or(false);
                self.pane_stats.clear();
                self.stats_at = Instant::now();
            }
            "block" => {
                self.cfg.block = v.as_bool().unwrap_or(true);
                for view in self.all_views() {
                    blocker::set_filters(view, self.cfg.block);
                }
            }
            "theme" => {
                if let Some(t) = v.as_str().filter(|t| THEMES.contains(t)) {
                    self.cfg.theme = t.to_string();
                }
            }
            "scanlines" => self.cfg.scanlines = v.as_bool().unwrap_or(false),
            "boot" => self.cfg.boot = v.as_bool().unwrap_or(true),
            "audio" => self.cfg.audio = v.as_bool().unwrap_or(true),
            "bmbar" => self.cfg.bmbar = v.as_bool().unwrap_or(true),
            "accent" => {
                if let Some(c) = v.as_str().filter(|c| is_hex_color(c)) {
                    self.cfg.accent = c.to_ascii_lowercase();
                    self.cfg.theme = "custom".into();
                }
            }
            "bgcolor" => {
                if let Some(c) = v.as_str().filter(|c| is_hex_color(c)) {
                    self.cfg.bgcolor = c.to_ascii_lowercase();
                }
            }
            "bgmode" => {
                if let Some(m) = v.as_str().filter(|m| BG_MODES.contains(m)) {
                    self.cfg.bgmode = m.to_string();
                }
            }
            "bgdim" => self.cfg.bgdim = v.as_u64().unwrap_or(45).min(90) as u8,
            "swirlcolor" => match v.as_str() {
                Some(c) if is_hex_color(c) => self.cfg.swirlcolor = c.to_ascii_lowercase(),
                Some("") | Some("auto") => self.cfg.swirlcolor.clear(),
                _ => return,
            },
            "swirlbright" => self.cfg.swirlbright = v.as_u64().unwrap_or(100).clamp(30, 300) as u16,
            _ => return,
        }
        self.save_settings();
        self.apply_layout(false);
    }

    /// Hand the saved background image (if any) to the chrome UI, once.
    fn send_bg_image(&self) {
        if let Ok(d) = fs::read_to_string(self.data_dir.join("background.dataurl")) {
            if valid_bg_image(&d) {
                let _ = self.ui.evaluate_script(&format!("ultra.setBgImage({})", json!(d)));
            }
        }
    }

    /// The private workspace is on screen but must not show (locked, or no password yet).
    fn gate(&self) -> bool {
        self.ws == PRIVATE_WS && (self.locked || self.cfg.privpass.is_empty())
    }

    fn has_private(&self) -> bool {
        self.tabs.iter().any(|t| t.ws == PRIVATE_WS)
    }

    fn toast(&self, msg: &str) {
        let _ = self.ui.evaluate_script(&format!("ultra.toast({})", json!(msg)));
    }

    /// Open a url as a new tab in workspace `ws` (in the background if that isn't the current one).
    fn open_in(&mut self, ws: usize, url: String) {
        if ws == self.ws {
            self.new_tab(url, String::new(), true);
        } else if !(ws == PRIVATE_WS && self.locked) {
            let id = self.push_tab(self.tabs.len(), url, String::new());
            if let Some(i) = self.idx(id) {
                self.tabs[i].ws = ws;
            }
            self.sync();
        }
    }

    fn check_idle_lock(&mut self) {
        if !self.locked && self.has_private() && !self.cfg.privpass.is_empty() && privacy::idle_secs() >= LOCK_AFTER {
            self.lock_private();
        }
    }

    fn lock_private(&mut self) {
        if self.cfg.privpass.is_empty() || self.locked {
            return;
        }
        self.locked = true;
        self.apply_layout(false);
        self.focus_active();
    }

    fn unlock_private(&mut self, password: &str) {
        if !self.locked {
            return;
        }
        if privacy::verify_password(&self.cfg.privpass, password) {
            self.locked = false;
            self.unlock_fails = 0;
            self.apply_layout(false);
            self.focus_active();
            return;
        }
        self.unlock_fails += 1;
        if self.unlock_fails >= MAX_UNLOCK_FAILS {
            self.wipe_private();
            self.locked = false;
            self.unlock_fails = 0;
            self.apply_layout(false);
            self.toast("too many wrong passwords: private workspace wiped");
        } else {
            let left = MAX_UNLOCK_FAILS - self.unlock_fails;
            let msg = format!("wrong password · {left} {} left before the private workspace is wiped", if left == 1 { "try" } else { "tries" });
            let _ = self.ui.evaluate_script(&format!("ultra.lockError({})", json!(msg)));
        }
    }

    /// Close every private tab; their InPrivate data dies with the webviews.
    fn wipe_private(&mut self) {
        self.tabs.retain(|t| t.ws != PRIVATE_WS);
        self.stash[PRIVATE_WS] = None;
        if self.ws == PRIVATE_WS {
            self.zoom = None;
            let id = self.push_tab(self.tabs.len(), HOME.into(), String::new());
            self.root = Node::Leaf(id);
            self.focus = id;
        }
        self.shown.retain(|(id, _)| self.tabs.iter().any(|t| t.id == *id));
        self.apply_layout(false);
    }

    /// Daily digest bookkeeping: credit time to the page being looked at (focused window,
    /// recent input, a real page, not private) and add newly blocked trackers.
    fn track(&mut self) {
        let now = Instant::now();
        let b = self.blocked.get();
        if b > self.last_blocked {
            self.lib.add_blocked(b - self.last_blocked);
            self.last_blocked = b;
        }
        let watching = self.win_focused && !self.overlay && !self.tour && !self.gate() && privacy::idle_secs() < 120;
        let cur = if watching {
            self.tabs.get(self.cur()).filter(|t| t.ws != PRIVATE_WS && t.view.is_some()).map(|t| t.url.clone())
        } else {
            None
        };
        // Same page, recent start: keep accumulating (segments are capped so idle time can't pile up).
        if let (Some((u, since)), Some(c)) = (&self.timing, &cur) {
            if u == c && now - *since < Duration::from_secs(15) {
                return;
            }
        }
        if let Some((u, since)) = self.timing.take() {
            self.lib.add_time(&u, (now - since).as_secs_f64().round() as u64);
        }
        self.timing = cur.map(|c| (c, now));
    }

    /// Record a finished page load in history (never for private tabs).
    fn remember(&mut self, i: usize) {
        let t = &mut self.tabs[i];
        if t.ws != PRIVATE_WS && t.remembered != t.url {
            t.remembered = t.url.clone();
            let (u, ti) = (t.url.clone(), t.title.clone());
            self.lib.visit(&u, &ti);
        }
    }

    fn save_layouts(&self) {
        if let Ok(j) = serde_json::to_string_pretty(&self.layouts) {
            let _ = fs::write(self.data_dir.join("layouts.json"), j);
        }
    }

    /// `layout save|load|delete <name>` from the command palette.
    fn layout_cmd(&mut self, op: &str, name: &str) {
        let name: String = name.trim().chars().take(40).collect();
        match op {
            "save" => {
                if self.ws == PRIVATE_WS {
                    self.toast("private layouts are never saved");
                    return;
                }
                if name.is_empty() {
                    self.toast("give the layout a name: layout save <name>");
                    return;
                }
                let mut ids = Vec::new();
                self.root.leaves(&mut ids);
                let tabs = ids
                    .iter()
                    .filter_map(|&id| self.idx(id))
                    .map(|i| {
                        let t = &self.tabs[i];
                        let url = t.view.as_ref().and_then(|v| v.url().ok()).filter(|u| !u.is_empty() && u != "about:blank");
                        (url.unwrap_or_else(|| t.url.clone()), t.title.clone())
                    })
                    .collect();
                let Some(tree) = self.root.map(&|id| ids.iter().position(|&x| x == id).map(|p| p as u64)) else { return };
                let focus = ids.iter().position(|&x| x == self.focus).unwrap_or(0);
                self.layouts.insert(name.clone(), SavedLayout { tabs, tree, focus });
                self.save_layouts();
                self.toast(&format!("saved layout \"{name}\""));
                self.sync();
            }
            "load" => {
                let Some(l) = self.layouts.get(&name).cloned() else {
                    self.toast(&format!("no layout called \"{name}\""));
                    return;
                };
                // Loads into the first empty workspace, so nothing open is replaced.
                let Some(w) = (0..PRIVATE_WS).find(|&w| w != self.ws && self.stash[w].is_none() && !self.tabs.iter().any(|t| t.ws == w)) else {
                    self.toast("every workspace is in use: close one first");
                    return;
                };
                let ids: Vec<u64> = l
                    .tabs
                    .iter()
                    .map(|(u, t)| {
                        let id = self.push_tab(self.tabs.len(), u.clone(), t.clone());
                        if let Some(i) = self.idx(id) {
                            self.tabs[i].ws = w;
                        }
                        id
                    })
                    .collect();
                let Some(tree) = l.tree.map(&|i| ids.get(i as usize).copied()) else { return };
                let focus = ids.get(l.focus).copied().unwrap_or_else(|| tree.first_leaf());
                self.stash[w] = Some((tree, focus));
                self.switch_ws(w);
                self.toast(&format!("loaded \"{name}\" into workspace {}", w + 1));
            }
            "delete" => {
                if self.layouts.remove(&name).is_some() {
                    self.save_layouts();
                    self.toast(&format!("deleted layout \"{name}\""));
                    self.sync();
                }
            }
            _ => {}
        }
    }

    /// Self-healing when a WebView2 process dies.
    fn on_failed(&mut self, slot: u64, kind: COREWEBVIEW2_PROCESS_FAILED_KIND) {
        if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
            // The whole engine is gone and no webview can be revived: save and relaunch.
            self.save_session();
            relaunch();
            return;
        }
        let renderer = kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
            || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
            || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED;
        if !renderer {
            return; // GPU/utility processes are restarted by WebView2 itself.
        }
        if slot == 0 {
            // The chrome UI itself: reload it; it re-sends `ready` and gets full state.
            let _ = self.ui.load_html(UI_HTML);
            return;
        }
        let Some(i) = self.idx_slot(slot) else { return };
        let now = Instant::now();
        let again = self.tabs[i].crashed_at.is_some_and(|t| now - t < Duration::from_secs(10));
        self.tabs[i].crashed_at = Some(now);
        if again {
            // Crashing repeatedly: drop the webview; a fresh one is built when shown.
            self.sleep(i);
            self.apply_layout(false);
        } else if let Some(v) = &self.tabs[i].view {
            let _ = v.reload();
        }
    }

    /// Commands shared by the chrome UI and keyboard shortcuts forwarded from pages.
    fn command(&mut self, cmd: &str, arg: &Value) {
        let n = self.tabs.len();
        let cur = self.cur();
        let ws_tabs = self.ws_tabs();
        let slot_tab = |k: Option<u64>| k.and_then(|k| ws_tabs.get(k as usize).copied());
        match cmd {
            "new" => self.new_tab(HOME.into(), String::new(), true),
            "newurl" => {
                let url = resolve(arg.as_str().unwrap_or(""), &self.cfg.search);
                self.new_tab(url, String::new(), true);
            }
            "close" => {
                let i = arg.as_u64().map(|v| v as usize).unwrap_or(cur);
                self.close_tab(i);
            }
            "closeid" => {
                if let Some(i) = arg.as_u64().and_then(|id| self.idx(id)) {
                    self.close_tab(i);
                }
            }
            // Global tab index (tab bar clicks, overview).
            "tab" => {
                if let Some(i) = arg.as_u64() {
                    self.activate(i as usize);
                }
            }
            // Position within the current workspace (ctrl+1‥8).
            "slot" => {
                if let Some(i) = slot_tab(arg.as_u64()) {
                    self.activate(i);
                }
            }
            "next" | "prev" => {
                if let Some(p) = ws_tabs.iter().position(|&j| j == cur) {
                    let len = ws_tabs.len();
                    let k = if cmd == "next" { (p + 1) % len } else { (p + len - 1) % len };
                    self.activate(ws_tabs[k]);
                }
            }
            "last" => {
                if let Some(&i) = ws_tabs.last() {
                    self.activate(i);
                }
            }
            "mv" => {
                if let Some(id) = slot_tab(arg.as_u64()).map(|i| self.tabs[i].id) {
                    self.place_at_cursor(id);
                }
            }
            "mvnew" => {
                let id = self.push_tab(n, HOME.into(), String::new());
                self.place_at_cursor(id);
            }
            "splitdir" => {
                let url = arg[1].as_str().filter(|u| !u.trim().is_empty());
                let url = url.map(|u| resolve(u, &self.cfg.search)).unwrap_or_else(|| HOME.into());
                self.split_dir(arg[0].as_str().unwrap_or("r"), url);
            }
            "closeat" => self.close_at_cursor(),
            "zoom" => self.toggle_zoom(None),
            "zoomid" => self.toggle_zoom(arg.as_u64()),
            "ws" => {
                if let Some(w) = arg.as_u64() {
                    self.switch_ws(w as usize);
                }
            }
            "tows" => {
                if let Some(w) = arg.as_u64() {
                    self.send_to_ws(w as usize);
                }
            }
            "recycle" => {
                // Drop and rebuild a pane's webview (frees whatever the page leaked).
                if let Some(i) = arg.as_u64().and_then(|id| self.idx(id)) {
                    self.sleep(i);
                    self.apply_layout(false);
                }
            }
            "trim" => self.trim(),
            "dir" => self.focus_dir(arg.as_str().unwrap_or("r")),
            "ratio" => {
                if let (Some(p), Some(r)) = (arg[0].as_str(), arg[1].as_f64()) {
                    self.root.set_ratio(p, r);
                    self.apply_layout(false);
                }
            }
            "focuspane" => {
                // Arg is a tab id (home panes are keyed by id).
                if let Some(id) = arg.as_u64() {
                    if self.root.contains(id) && id != self.focus {
                        self.set_focus(id);
                        self.focus_active();
                    }
                }
            }
            "reopen" => {
                if let Some((u, t)) = self.closed.pop() {
                    self.new_tab(u, t, true);
                }
            }
            "go" => {
                // Either "query" (focused pane) or [tab id, "query"] from a home pane.
                let (i, q) = match arg {
                    Value::Array(a) => (
                        a[0].as_u64().and_then(|id| self.idx(id)).unwrap_or(cur),
                        a[1].as_str().unwrap_or(""),
                    ),
                    _ => (cur, arg.as_str().unwrap_or("")),
                };
                let q = q.to_string();
                self.navigate(i, &q);
            }
            "back" => self.with_active(|v| {
                let _ = v.evaluate_script("history.back()");
            }),
            "fwd" => self.with_active(|v| {
                let _ = v.evaluate_script("history.forward()");
            }),
            "reload" => self.with_active(|v| {
                let _ = v.reload();
            }),
            "devtools" => self.with_active(|v| v.open_devtools()),
            "sleep" => {
                if let Some(i) = arg.as_u64().map(|v| v as usize) {
                    if i < n && !self.root.contains(self.tabs[i].id) {
                        self.sleep(i);
                        self.sync();
                    }
                }
            }
            "move" => {
                if let (Some(from), Some(to)) = (arg[0].as_u64(), arg[1].as_u64()) {
                    let (from, to) = (from as usize, to as usize);
                    if from < n && to < n && from != to {
                        let t = self.tabs.remove(from);
                        self.tabs.insert(to, t);
                        self.sync();
                    }
                }
            }
            "focusurl" => {
                let _ = self.ui.focus();
                let _ = self.ui.evaluate_script("ultra.focusUrl()");
            }
            "overview" | "settings" | "palette" | "urlpanel" => {
                // These panels cover the content area, so pages are hidden while open.
                let on = arg.as_bool().unwrap_or(!self.overlay);
                self.overlay = on;
                self.apply_layout(false);
                if on {
                    let _ = self.ui.focus();
                    let _ = self.ui.evaluate_script(&format!("ultra.{cmd}(true)"));
                } else {
                    self.focus_active();
                }
            }
            "tour" => {
                self.tour = arg.as_bool().unwrap_or(!self.tour);
                self.apply_layout(false);
                if self.tour {
                    let _ = self.ui.focus();
                    let _ = self.ui.evaluate_script("ultra.tour(true)");
                } else {
                    self.focus_active();
                }
            }
            "type" => {
                // A letter/number typed outside a text field: hand it to the address bar.
                if let Some(c) = arg.as_str().filter(|c| c.chars().count() == 1) {
                    let _ = self.ui.focus();
                    let _ = self.ui.evaluate_script(&format!("ultra.typeUrl({})", json!(c)));
                }
            }
            "set" => {
                if let Some(k) = arg[0].as_str() {
                    let k = k.to_string();
                    self.set_setting(&k, &arg[1].clone());
                }
            }
            "bgimage" => {
                // A picked image arrives as a data URL; keep it on disk and switch to it.
                if let Some(d) = arg.as_str().filter(|d| valid_bg_image(d)) {
                    let _ = fs::write(self.data_dir.join("background.dataurl"), d);
                    self.cfg.bgmode = "image".into();
                    self.save_settings();
                    self.send_bg_image();
                    self.sync();
                }
            }
            "bgclear" => {
                let _ = fs::remove_file(self.data_dir.join("background.dataurl"));
                if self.cfg.bgmode == "image" {
                    self.cfg.bgmode = "swirl".into();
                }
                self.save_settings();
                let _ = self.ui.evaluate_script("ultra.setBgImage(null)");
                self.sync();
            }
            "lock" => {
                if self.cfg.privpass.is_empty() {
                    self.toast("set a private password in settings first");
                } else {
                    self.lock_private();
                }
            }
            "unlock" => {
                let pw = arg.as_str().unwrap_or("").to_string();
                self.unlock_private(&pw);
            }
            "setpass" => {
                let (old, new) = (arg[0].as_str().unwrap_or(""), arg[1].as_str().unwrap_or(""));
                if !self.cfg.privpass.is_empty() && !privacy::verify_password(&self.cfg.privpass, old) {
                    let _ = self.ui.evaluate_script(&format!("ultra.lockError({})", json!("current password is wrong")));
                    return;
                }
                if new.chars().count() < 4 {
                    let _ = self.ui.evaluate_script(&format!("ultra.lockError({})", json!("use at least 4 characters")));
                    return;
                }
                self.cfg.privpass = privacy::hash_password(new);
                self.locked = false;
                self.unlock_fails = 0;
                self.save_settings();
                self.apply_layout(false);
                self.focus_active();
                let _ = self.ui.evaluate_script("ultra.passSet()");
                self.toast("private password set");
            }
            "wipe" => {
                self.wipe_private();
                self.toast("private workspace wiped");
            }
            "layout" => {
                let (op, name) = (arg[0].as_str().unwrap_or("").to_string(), arg[1].as_str().unwrap_or("").to_string());
                self.layout_cmd(&op, &name);
            }
            "suggest" => {
                let q = arg.as_str().unwrap_or("").to_string();
                let items: Vec<Value> =
                    self.lib.suggest(&q, 8).into_iter().map(|(u, t, b)| json!({ "u": u, "t": t, "b": b })).collect();
                let _ = self.ui.evaluate_script(&format!("ultra.suggestions({})", json!({ "q": q, "items": items })));
            }
            "bookmark" => {
                let Some(t) = self.tabs.get(self.cur()) else { return };
                if t.ws == PRIVATE_WS {
                    self.toast("bookmarks are off in the private workspace");
                } else if t.url == HOME || !(t.url.starts_with("http://") || t.url.starts_with("https://")) {
                    self.toast("nothing to bookmark here");
                } else {
                    let (u, ti) = (t.url.clone(), t.title.clone());
                    let on = self.lib.toggle_bookmark(&u, &ti);
                    self.lib.save(&self.data_dir);
                    self.toast(if on { "★ bookmarked" } else { "bookmark removed" });
                    self.sync();
                }
            }
            "openbm" => {
                // Bookmarks bar: [url, new_tab]
                let url = arg[0].as_str().unwrap_or("").to_string();
                if !url.is_empty() {
                    if arg[1].as_bool().unwrap_or(false) {
                        self.new_tab(url, String::new(), true);
                    } else {
                        let c = self.cur();
                        self.navigate(c, &url);
                    }
                }
            }
            "unbookmark" => {
                if let Some(u) = arg.as_str() {
                    if self.lib.is_bookmarked(u) {
                        self.lib.toggle_bookmark(u, "");
                        self.lib.save(&self.data_dir);
                        self.toast("bookmark removed");
                        self.sync();
                    }
                }
            }
            "palhist" => {
                // Commands typed in the private workspace are not remembered.
                if self.ws != PRIVATE_WS {
                    if let Some(c) = arg.as_str() {
                        self.lib.push_command(c);
                        self.sync();
                    }
                }
            }
            "clearhistory" => {
                self.lib.clear_history();
                self.lib.save(&self.data_dir);
                self.toast("history cleared");
            }
            "booted" => {
                if self.booting {
                    self.booting = false;
                    self.apply_layout(false);
                    self.focus_active();
                }
            }
            "content" => self.focus_active(),
            "drag" => {
                let _ = self.window.drag_window();
            }
            "resize" => {
                let d = match arg.as_str().unwrap_or("") {
                    "n" => ResizeDirection::North,
                    "s" => ResizeDirection::South,
                    "e" => ResizeDirection::East,
                    "w" => ResizeDirection::West,
                    "ne" => ResizeDirection::NorthEast,
                    "nw" => ResizeDirection::NorthWest,
                    "se" => ResizeDirection::SouthEast,
                    _ => ResizeDirection::SouthWest,
                };
                let _ = self.window.drag_resize_window(d);
            }
            "min" => self.window.set_minimized(true),
            "max" => self.window.set_maximized(!self.window.is_maximized()),
            "ready" => {
                self.send_bg_image();
                self.flush();
                self.show_window();
                if self.cfg.tour && !self.tour_offered {
                    self.tour_offered = true;
                    self.command("tour", &json!(true));
                }
            }
            _ => {}
        }
    }

    fn on_event(&mut self, ev: UserEvent, flow: &mut ControlFlow) {
        match ev {
            UserEvent::Ui(msg) => {
                let v: Value = serde_json::from_str(&msg).unwrap_or(Value::Null);
                let cmd = v["cmd"].as_str().unwrap_or("");
                if cmd == "quit" {
                    self.save_session();
                    *flow = ControlFlow::Exit;
                    return;
                }
                self.command(cmd, &v["arg"]);
            }
            UserEvent::Content(slot, msg) => {
                // Pages are untrusted: only a fixed set of shortcut commands is accepted,
                // and only from a tab that is on screen.
                let Some(i) = self.idx_slot(slot) else { return };
                let id = self.tabs[i].id;
                if msg == "url" {
                    // Never trust a url reported by the page itself (address bar spoofing):
                    // ask WebView2 for the real one.
                    if let Some(u) = self.tabs[i].view.as_ref().and_then(|v| v.url().ok()) {
                        if !u.is_empty() && u != self.tabs[i].url {
                            self.tabs[i].url = u;
                            self.remember(i);
                            self.sync();
                        }
                    }
                    return;
                }
                if !self.shown.iter().any(|(x, _)| *x == id) {
                    return;
                }
                if id != self.focus {
                    self.set_focus(id);
                }
                let digit = |p: &str| msg.strip_prefix(p).and_then(|k| k.parse::<u64>().ok()).filter(|k| (1..=9).contains(k));
                if let Some(k) = digit("tab:") {
                    if k == 9 {
                        self.command("last", &Value::Null);
                    } else {
                        self.command("slot", &json!(k - 1));
                    }
                    return;
                }
                if let Some(k) = digit("mv:") {
                    self.command("mv", &json!(k - 1));
                    return;
                }
                if msg == "ws:0" {
                    self.command("ws", &json!(PRIVATE_WS));
                    return;
                }
                if let Some(k) = digit("ws:") {
                    self.command("ws", &json!(k - 1));
                    return;
                }
                if let Some(k) = digit("tows:") {
                    self.command("tows", &json!(k - 1));
                    return;
                }
                if let Some(c) = msg.strip_prefix("type:") {
                    let mut it = c.chars();
                    if let (Some(ch), None) = (it.next(), it.next()) {
                        if self.cfg.typeahead && ch.is_alphanumeric() {
                            self.command("type", &json!(c));
                        }
                    }
                    return;
                }
                if let Some(d) = msg.strip_prefix("dir:") {
                    if matches!(d, "l" | "r" | "u" | "d") {
                        self.focus_dir(d);
                    }
                    return;
                }
                match msg.as_str() {
                    "new" | "close" | "next" | "prev" | "reopen" | "focusurl" | "overview" | "settings"
                    | "palette" | "mvnew" | "closeat" | "zoom" | "lock" | "bookmark" => self.command(&msg, &Value::Null),
                    _ => {}
                }
            }
            UserEvent::Title(slot, title) => {
                if let Some(i) = self.idx_slot(slot) {
                    if self.tabs[i].ws != PRIVATE_WS {
                        self.lib.retitle(&self.tabs[i].url, &title);
                    }
                    self.tabs[i].title = title;
                    if let Some(u) = self.tabs[i].view.as_ref().and_then(|v| v.url().ok()) {
                        if !u.is_empty() {
                            self.tabs[i].url = u;
                        }
                    }
                    self.sync();
                }
            }
            UserEvent::Load(slot, started) => {
                if let Some(i) = self.idx_slot(slot) {
                    self.tabs[i].loading = started;
                    if let Some(u) = self.tabs[i].view.as_ref().and_then(|v| v.url().ok()) {
                        if !u.is_empty() && u != "about:blank" {
                            self.tabs[i].url = u;
                        }
                    }
                    if !started {
                        self.remember(i);
                    }
                    self.sync();
                }
            }
            UserEvent::NewTab(slot, url) => {
                // Links open in the workspace of the page that opened them (private stays private).
                let ws = self.idx_slot(slot).map(|i| self.tabs[i].ws).unwrap_or(self.ws);
                self.open_in(ws, url);
            }
            UserEvent::Procs(p) => self.on_procs(p),
            UserEvent::Heap(slot, r) => {
                if let (Some(i), Ok(mb)) = (self.idx_slot(slot), r.trim().parse::<f64>()) {
                    if self.cfg.stats && mb >= 0.0 {
                        self.pane_stats.insert(self.tabs[i].id, (mb, -1.0));
                        self.sync();
                    }
                }
            }
            UserEvent::Failed(slot, kind) => self.on_failed(slot, COREWEBVIEW2_PROCESS_FAILED_KIND(kind)),
        }
    }
}

/// `ultra.exe --bench`: time webview creation with an invisible, unfocused window
/// (nothing on screen, no input) and write the numbers to bench.txt.
fn bench(app: &mut App) {
    let mut out = String::new();
    let rect = inset(app.area(), 0.0);
    for shared in [false, true] {
        app.shared_env = shared;
        let mut views = Vec::new();
        let mut ms = Vec::new();
        for _ in 0..6 {
            let slot = app.new_slot();
            let t = Instant::now();
            views.push(app.build_view(slot, Some("about:blank"), rect, false));
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        out += &format!("build (shared_env={shared}): {ms:.1?} ms\n");
    }
    app.shared_env = true;
    let slot = app.new_slot();
    let spare = app.build_view(slot, None, rect, false).expect("spare");
    let pslot = app.new_slot();
    let pv = app.build_view(pslot, Some("about:blank"), rect, true);
    let inprivate = pv.as_ref().and_then(|v| v.webview().cast::<ICoreWebView2_13>().ok()).and_then(|w| unsafe { w.Profile().ok() }).map(|p| {
        let mut b = BOOL(0);
        unsafe {
            let _ = p.IsInPrivateModeEnabled(&mut b);
        }
        b.as_bool()
    });
    out += &format!("private webview is InPrivate: {inprivate:?}\n");
    let t = Instant::now();
    let _ = spare.set_bounds(rect);
    let _ = spare.load_url("about:blank");
    let _ = spare.set_visible(true);
    out += &format!("wake from spare: {:.2} ms\n", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    blocker::set_filters(&spare, false);
    blocker::set_filters(&spare, true);
    out += &format!("blocker filters toggle: {:.2} ms\n", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let u = stats::usage(std::process::id());
    out += &format!(
        "process usage read: {:.3} ms ({} MB)\n",
        t.elapsed().as_secs_f64() * 1000.0,
        u.map(|u| u.mem / 1_048_576).unwrap_or(0)
    );
    let t = Instant::now();
    for _ in 0..100 {
        app.dirty.set(true);
        app.flush();
    }
    out += &format!("ui flush: {:.3} ms each\n", t.elapsed().as_secs_f64() * 10.0);
    let _ = fs::write(app.data_dir.join("bench.txt"), out);
}

fn main() {
    let bench_mode = std::env::args().any(|a| a == "--bench");
    let data_dir = data_dir();
    let _ = fs::create_dir_all(&data_dir);
    let cfg: Settings = fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    // Hidden until the chrome has painted, so there is no white flash on startup.
    let window = WindowBuilder::new()
        .with_title("ULTRA")
        .with_decorations(false)
        .with_visible(false)
        .with_focused(!bench_mode)
        .with_window_icon(make_icon())
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(480.0, 320.0))
        .build(&event_loop)
        .expect("window");

    let mut ctx = WebContext::new(Some(data_dir.join("webview")));
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let pui = proxy.clone();
    let ui = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_html(UI_HTML)
        // Settings are known before the first paint, so ULTRASPEED never spends a frame on looks.
        .with_initialization_script(format!(
            "window.__ultraCfg = {};",
            ui_cfg(&cfg)
        ))
        .with_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(size.width, size.height).into(),
        })
        .with_background_color((20, 20, 20, 255))
        .with_default_context_menus(false)
        .with_additional_browser_args(BROWSER_ARGS)
        // The chrome UI never navigates anywhere; refuse anything that tries.
        .with_navigation_handler(|u| u.starts_with("about:") || u.starts_with("data:"))
        .with_ipc_handler(move |r| {
            let _ = pui.send_event(UserEvent::Ui(r.into_body()));
        })
        .build_as_child(&window)
        .expect("ui webview");
    watch_failures(&ui, proxy.clone(), 0);

    let now = Instant::now();
    let mut app = App {
        window,
        ui,
        ctx,
        proxy,
        tabs: Vec::new(),
        root: Node::Leaf(0),
        focus: 0,
        ws: 0,
        stash: vec![None; WORKSPACES],
        zoom: None,
        slide: None,
        next_id: 1,
        next_slot: 0,
        closed: Vec::new(),
        cfg,
        overlay: false,
        tour: false,
        tour_offered: false,
        data_dir,
        shared_env: true,
        spares: Vec::new(),
        spare_at: Some(now + Duration::from_millis(1500)),
        anim: None,
        shown: Vec::new(),
        blocked: Rc::new(Cell::new(0)),
        pane_stats: HashMap::new(),
        cpu_prev: HashMap::new(),
        total_mb: 0.0,
        total_cpu: 0.0,
        stats_at: now,
        stats_pending: false,
        locked: false,
        unlock_fails: 0,
        lock_check_at: now,
        layouts: BTreeMap::new(),
        lib: library::Library::default(),
        booting: false,
        win_focused: true,
        timing: None,
        last_blocked: 0,
        refocus_at: None,
        meter: audio::Meter::default(),
        engine_pids: HashSet::new(),
        pids_dirty: false,
        meter_at: now,
        audio_at: now,
        audio_on: false,
        boot_until: now,
        dirty: Cell::new(false),
        visible: false,
        show_at: now + Duration::from_millis(900),
        next_tick: now + TICK,
    };

    if bench_mode {
        bench(&mut app);
        return;
    }

    let session: Session = fs::read_to_string(app.data_dir.join("session.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    // Restored tabs start asleep; only the tabs in the visible layout are woken.
    for (i, (url, title)) in session.tabs.into_iter().enumerate() {
        let url = if url == LEGACY_HOME { HOME.to_string() } else { url };
        app.ws = session.ws.get(i).copied().unwrap_or(0).min(PRIVATE_WS - 1);
        app.push_tab(usize::MAX, url, title);
    }
    app.ws = session.cur_ws.min(PRIVATE_WS - 1);
    let ids: Vec<u64> = app.tabs.iter().map(|t| t.id).collect();
    app.lib = library::Library::load(&app.data_dir);
    app.booting = app.cfg.boot && !app.cfg.ultraspeed;
    app.boot_until = Instant::now() + Duration::from_secs(4);
    app.layouts = fs::read_to_string(app.data_dir.join("layouts.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    for (w, l) in session.layouts.into_iter().enumerate().take(PRIVATE_WS) {
        if let Some((node, f)) = l {
            if let (Some(node), Some(&fid)) = (node.map(&|i| ids.get(i as usize).copied()), ids.get(f)) {
                let fid = if node.contains(fid) { fid } else { node.first_leaf() };
                app.stash[w] = Some((node, fid));
            }
        }
    }
    match app.stash[app.ws].take() {
        Some((root, focus)) => {
            app.root = root;
            app.focus = focus;
        }
        None => {
            // Old single-layout sessions, or an empty current workspace.
            let legacy = session.layout.and_then(|l| l.map(&|i| ids.get(i as usize).copied()));
            let first = app.tabs.iter().find(|t| t.ws == app.ws).map(|t| t.id);
            let focus = match (ids.get(session.active).copied(), first) {
                (Some(a), _) if app.tabs.iter().any(|t| t.id == a && t.ws == app.ws) => a,
                (_, Some(f)) => f,
                _ => app.push_tab(usize::MAX, HOME.into(), String::new()),
            };
            app.root = legacy.filter(|l| l.contains(focus)).unwrap_or(Node::Leaf(focus));
            app.focus = focus;
        }
    }
    app.apply_layout(false);

    event_loop.run(move |event, _, flow| {
        match event {
            Event::UserEvent(ev) => {
                if !matches!(ev, UserEvent::Procs(_)) {
                    app.note_activity();
                }
                app.on_event(ev, flow);
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    app.save_session();
                    *flow = ControlFlow::Exit;
                }
                WindowEvent::Focused(f) => {
                    app.win_focused = f;
                    app.track();
                    // Give Windows a moment to settle activation before checking focus.
                    app.refocus_at = f.then(|| Instant::now() + Duration::from_millis(40));
                }
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    app.note_activity();
                    app.layout();
                }
                _ => {}
            },
            Event::MainEventsCleared => {
                let now = Instant::now();
                if app.anim.is_some() {
                    app.step_anim();
                }
                if now >= app.next_tick {
                    app.next_tick = now + TICK;
                    app.tick();
                }
                if app.spare_at.is_some_and(|t| now >= t) {
                    app.spare_at = None;
                    app.refill_spare();
                }
                app.track();
                if now >= app.audio_at {
                    app.poll_audio(now);
                }
                if app.refocus_at.is_some_and(|t| now >= t) {
                    app.refocus_at = None;
                    app.restore_focus();
                }
                if app.booting && now >= app.boot_until {
                    app.command("booted", &Value::Null);
                }
                if now >= app.lock_check_at {
                    app.lock_check_at = now + Duration::from_secs(5);
                    app.check_idle_lock();
                }
                if app.cfg.stats && app.visible && !app.stats_pending && now >= app.stats_at {
                    app.stats_at = now + STATS_EVERY;
                    app.sample_stats();
                }
                if !app.visible && now >= app.show_at {
                    app.show_window();
                }
                if app.dirty.get() {
                    app.flush();
                }
            }
            _ => {}
        }
        if !matches!(*flow, ControlFlow::Exit) {
            *flow = ControlFlow::WaitUntil(app.deadline());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::shortcut;

    #[test]
    fn shortcuts() {
        let k = |c: char| c as u32;
        assert_eq!(shortcut(k('T'), true, false, false).as_deref(), Some("new"));
        assert_eq!(shortcut(k('W'), true, false, false).as_deref(), Some("close"));
        assert_eq!(shortcut(k('W'), true, true, false).as_deref(), Some("closeat"));
        assert_eq!(shortcut(k('3'), true, false, false).as_deref(), Some("tab:3"));
        assert_eq!(shortcut(k('3'), true, true, false).as_deref(), Some("mv:3"));
        assert_eq!(shortcut(k('3'), true, false, true).as_deref(), Some("ws:3"));
        assert_eq!(shortcut(k('3'), true, true, true).as_deref(), Some("tows:3"));
        assert_eq!(shortcut(k('0'), true, false, true).as_deref(), Some("ws:0"));
        assert_eq!(shortcut(0x27, true, false, true).as_deref(), Some("dir:r"));
        assert_eq!(shortcut(0x09, true, true, false).as_deref(), Some("prev"));
        assert_eq!(shortcut(0xBC, true, false, false).as_deref(), Some("settings"));
        // Not ours: plain keys, ctrl+c, ctrl+f (page find) pass through.
        assert_eq!(shortcut(k('T'), false, false, false), None);
        assert_eq!(shortcut(k('C'), true, false, false), None);
        assert_eq!(shortcut(k('F'), true, false, false), None);
    }
}
