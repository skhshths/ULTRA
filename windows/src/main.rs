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
mod crash;
mod default_browser;
mod edition;
mod identity;
mod downloads;
mod instance;
mod layout;
mod library;
mod pages;
mod social;
mod privacy;
mod reentry;
mod slideshow;
mod spectrum;
mod updater;
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
    dpi::{LogicalPosition, LogicalSize, PhysicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::{IconExtWindows, WindowExtWindows},
    window::{Fullscreen, Icon, ResizeDirection, Window, WindowBuilder},
};
use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2DownloadOperation, ICoreWebView2_13, ICoreWebView2_3, ICoreWebView2_4, ICoreWebView2_8,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON, COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED, COREWEBVIEW2_DOWNLOAD_STATE,
        COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS, COREWEBVIEW2_KEY_EVENT_KIND,
        COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
        COREWEBVIEW2_PHYSICAL_KEY_STATUS, COREWEBVIEW2_PROCESS_FAILED_KIND,
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    },
    take_pwstr, AcceleratorKeyPressedEventHandler, BytesReceivedChangedEventHandler, DownloadStartingEventHandler,
    ProcessFailedEventHandler, StateChangedEventHandler, TrySuspendCompletedHandler, CallDevToolsProtocolMethodCompletedHandler,
    ContainsFullScreenElementChangedEventHandler,
};
use windows_core::{Interface, BOOL, HSTRING};
use wry::{
    MemoryUsageLevel, NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

const BAR: f64 = 34.0;
/// At most this many new tabs or pop-ups from one page per POPUP_WINDOW.
const POPUP_MAX: usize = 4;
const POPUP_WINDOW: Duration = Duration::from_secs(10);
/// Settings are written this long after the last change.
const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(500);
/// How long the audio-bars helper keeps running after the sound stops.
const BARS_LINGER: Duration = Duration::from_secs(30);
/// Height of the bookmarks bar under the top bar (when shown).
const BMBAR: f64 = 26.0;
/// Height of the occasional "support us" strip under the bars.
const COFFEE_BAR: f64 = 30.0;
/// The support strip appears every this many launches (the 3rd, 6th, 9th...).
const COFFEE_EVERY: u32 = 3;
/// ...this long after the window is up, so it never competes with startup.
const COFFEE_DELAY: Duration = Duration::from_secs(4);
/// Where "buy us a coffee" goes (empty: no strip at all).
const SUPPORT_URL: &str = "https://ko-fi.com/skhshths/";
/// Height of the title strip above a glance preview.
const GLANCE_HDR: f64 = 26.0;
/// Largest memory cap accepted (MB).
const MEMCAP_MAX: u64 = 65_536;
/// Most elements remembered as hidden per site.
const MAX_ZAPS: usize = 50;
const READER_JS: &str = include_str!("reader.js");
/// Accent of each built-in theme (mirrors THEMES in ui.html), for pages ULTRA styles itself.
const THEME_ACCENTS: &[(&str, &str)] = &[
    ("mint", "#9fe3d0"), ("amber", "#ffb454"), ("red", "#ff6b5f"), ("mono", "#e8e8e8"), ("ice", "#8ab4ff"), ("matrix", "#4af626"),
    ("violet", "#b18cff"), ("pink", "#ff7eb6"), ("orange", "#ff9150"), ("cyan", "#5ce1e6"), ("gold", "#ffd75e"), ("lime", "#b6f35c"),
];
/// The picture-in-picture keys as the help text names them.
const PIP_KEYS: &str = "ctrl+alt+p";
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
/// A spare webview is built only after this much quiet, so its creation stall (~60 ms) goes
/// unnoticed. Most spares don't need building at all: tabs put to sleep hand theirs back.
const SPARE_IDLE: Duration = Duration::from_millis(600);
/// Pre-built blank webviews kept ready (creating one costs ~80 ms of UI-thread time).
const SPARES: usize = 3;
/// Must be identical for every webview so they all share one WebView2 environment.
/// SmartScreen (phishing/malware protection) is deliberately left on.
const BROWSER_ARGS: &str = "--enable-features=msSmartScreenProtection";

/// The engine flags, plus a DevTools port when the developer sets ULTRA_DEVTOOLS_PORT (used to
/// drive ULTRA from test scripts). Computed once so every webview gets the identical string.
fn browser_args() -> &'static str {
    static ARGS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    // Only builds made for testing honour it (never the public one): a DevTools port lets any
    // program on this computer drive the browser.
    let port = if edition::TEST_SWITCHES { std::env::var("ULTRA_DEVTOOLS_PORT").ok().and_then(|p| p.parse::<u16>().ok()) } else { None };
    ARGS.get_or_init(|| match port {
        Some(port) => format!("{BROWSER_ARGS} --remote-debugging-port={port}"),
        None => BROWSER_ARGS.to_string(),
    })
}
const THEMES: &[&str] = &[
    "mint", "amber", "red", "mono", "ice", "matrix", "violet", "pink", "orange", "cyan", "gold", "lime", "custom",
];
const BG_MODES: &[&str] = &["swirl", "matrix", "stars", "gradient", "solid", "image", "slideshow"];
/// Largest background image accepted (as a base64 data URL).
const BG_MAX: usize = 16 * 1024 * 1024;

enum UserEvent {
    Ui(String),
    Content(u64, String),
    Title(u64, String),
    Load(u64, bool),
    /// A page asked for a new tab: (source slot, url).
    NewTab(u64, String),
    /// A page tried to open too many tabs or pop-ups at once.
    PopupBlocked(u64),
    Procs(Vec<u32>),
    /// JS heap (MB) reported by a page.
    Heap(u64, String),
    /// Next slideshow image, loaded off-thread: (data url, file name).
    Slide(String, String),
    /// A download began in a page: (key, slot, url, file path, total bytes or 0).
    DlStart(u64, u64, String, String, i64),
    /// Bytes so far / total for download `key`.
    DlProgress(u64, i64, i64),
    /// Download `key` changed state (final file path once known).
    DlState(u64, downloads::State, String),
    /// "Check for updates" finished: state for settings, and the update to install if any.
    UpdateChecked(Value, Option<updater::Update>, bool),
    /// A frame from the friends & chat server (or the connection's status).
    Social(Value),
    /// A page entered (true) or left (false) fullscreen, e.g. a YouTube video.
    PageFullscreen(u64, bool),
    /// An update was downloaded and swapped in (the exe to restart from), or failed.
    UpdateInstalled(Result<PathBuf, String>, bool),
    /// A ULTRA shortcut caught natively in a page's webview: (slot, message).
    Key(u64, String),
    /// A WebView2 process died: (slot, 0 = the chrome UI; failure kind).
    Failed(u64, i32),
    /// A link handed over by another app (or a second launch): URL, or empty to come forward.
    Open(String),
}

/// A closed tab, remembered so ctrl+shift+t can put it back: its page, workspace, place in
/// the tab strip, and (if it was tiled) the spot in the layout from `Node::place_of`.
struct Closed {
    url: String,
    title: String,
    ws: usize,
    at: usize,
    place: Option<(Vec<u64>, bool, f64, bool)>,
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
    /// The window's normal (not maximized) position and size, and whether it was maximized.
    #[serde(default)]
    window: Option<WinState>,
}

/// Where the window was: physical pixels, so it comes back on the same screen at the same size.
#[derive(Serialize, Deserialize, Clone, Copy, Default)]
struct WinState {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    max: bool,
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
    /// Check for updates now and then and install them by themselves.
    autoupdate: bool,
    /// The friends & chat server (ws://host:port/ws).
    chatserver: String,
    /// Sites where blocking is switched off (host names).
    blockoff: Vec<String>,
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
    /// Slideshow background: folder of images and seconds per image.
    slidedir: String,
    slidesecs: u32,
    /// Audio bars layered over whatever background is chosen.
    bars: bool,
    /// How many times ULTRA has been started (drives the occasional support strip).
    launches: u32,
    /// Memory ceiling in MB (0 = off): past it, the least recently used hidden tabs are put to sleep.
    memcap: u32,
    /// Workspaces that keep their own cookies and sign-ins.
    isolated: Vec<usize>,
    /// Hide cookie banners and consent overlays.
    declutter: bool,
    /// Shortcut tiles under the new-tab search box.
    shortcuts: bool,
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
            autoupdate: true,
            chatserver: String::new(),
            blockoff: Vec::new(),
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
            slidedir: String::new(),
            slidesecs: 60,
            bars: false,
            launches: 0,
            memcap: 0,
            isolated: Vec::new(),
            declutter: true,
            shortcuts: true,
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
    /// Recently closed tabs, newest last (ctrl+shift+t brings them back where they were).
    closed: Vec<Closed>,
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
    /// What the ad blocker has done (counts, sites switched off), shared with every webview.
    shields: Rc<blocker::Shields>,
    /// Blocking on (settings), as the webviews' handlers see it.
    block_on: Rc<Cell<bool>>,
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
    /// This computer's ULTRA ID, once made.
    me: Option<identity::Identity>,
    /// Friends & chat (needs an ULTRA ID).
    social: social::Social,
    /// The tab whose page is fullscreen right now: it covers the whole screen, chrome and all.
    page_full: Option<u64>,
    /// A newer build found by "check for updates", ready to install.
    update: Option<updater::Update>,
    /// When the next automatic update check is due.
    next_update_check: Instant,
    /// An update installed in the background: the app to restart into (restart now in settings).
    pending_restart: Option<PathBuf>,
    /// Other browsers found on this computer, offered for importing bookmarks.
    browsers: Vec<(&'static str, &'static str)>,
    /// Settings changed and not yet written (saved shortly after the last change).
    settings_save_at: Option<Instant>,
    /// Saved window placement, applied when the window first shows.
    start_place: Option<WinState>,
    /// The regular workspace shown before the current one (links land there while private is locked).
    last_ws: usize,
    /// Link ULTRA was started with, opened once the chrome is ready.
    pending_open: Option<String>,
    /// The last run ended in a crash (said once the chrome is ready).
    crashed: bool,
    /// Whether Windows opens web links in ULTRA (shown in settings).
    is_default: bool,
    timing: Option<(String, Instant)>,
    last_blocked: u64,
    /// After the window is activated (alt+tab), check where keyboard focus landed.
    refocus_at: Option<Instant>,
    /// The downloads panel's list, and download key (see `watch_downloads`) -> list id.
    dls: downloads::Downloads,
    dl_keys: HashMap<u64, u64>,
    /// Audio-reactive visuals: mixer meters for the WebView2 process tree.
    meter: audio::Meter,
    engine_pids: HashSet<u32>,
    pids_dirty: bool,
    meter_at: Instant,
    audio_at: Instant,
    audio_on: bool,
    /// Real audio bars (process-loopback capture), alive only while needed.
    spectrum: Option<spectrum::Spectrum>,
    /// When a tab last played sound (the bars helper lingers a while after).
    last_sound: Instant,
    /// Slideshow: images in the folder, position, next change, a load in flight.
    slides: Vec<PathBuf>,
    slide_idx: usize,
    slide_at: Instant,
    slide_loading: bool,
    boot_until: Instant,
    dirty: Cell<bool>,
    /// Support strip: when it is due to appear, and whether it is showing.
    coffee_at: Option<Instant>,
    coffee: bool,
    /// Tab shown as a floating preview over the layout (ctrl/cmd+shift+click a link).
    glance: Option<u64>,
    /// Elements hidden per site with the zap picker: host -> CSS selectors.
    zaps: BTreeMap<String, Vec<String>>,
    visible: bool,
    show_at: Instant,
    next_tick: Instant,
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(edition::DATA_DIR);
    // One-time migration from the browser's old name keeps sessions and sign-ins (for the
    // regular build only: the experimental one must not take them over).
    let old = base.join("Zenith");
    if edition::DATA_DIR == "Ultra" && !dir.exists() && old.exists() {
        let _ = fs::rename(&old, &dir);
    }
    if edition::DATA_DIR != "Ultra" {
        seed_from(&base.join("Ultra"), &dir);
    }
    dir
}

/// A build with its own data folder (experimental) starts from a copy of the regular ULTRA's
/// settings, bookmarks and history, so it feels like home. Open tabs and sign-ins stay separate.
fn seed_from(from: &std::path::Path, to: &std::path::Path) {
    if to.exists() || !from.exists() {
        return;
    }
    let _ = fs::create_dir_all(to);
    for f in ["bookmarks.json", "history.json", "commands.json", "layouts.json", "zaps.json"] {
        let _ = fs::copy(from.join(f), to.join(f));
    }
    let _ = fs::copy(from.join("settings.json"), to.join("settings.json"));
}

/// Whether address-bar text is a web address rather than a search: a host with a real
/// (letters-only) top-level domain, localhost or an IPv4 address, optionally with a numeric
/// port and a path. "3.14", "note:meeting" or "e.g." are searches.
fn looks_like_host(s: &str) -> bool {
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return false;
    }
    let hostport = s.split(['/', '?', '#']).next().unwrap_or("");
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (hostport, None),
    };
    if port.is_some_and(|p| p.is_empty() || p.len() > 5 || !p.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    let host = host.to_lowercase();
    if host == "localhost" {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() == 4 && parts.iter().all(|p| (1..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit())) {
        return true;
    }
    let label_ok = |p: &&str| !p.is_empty() && !p.starts_with('-') && !p.ends_with('-') && p.chars().all(|c| c.is_alphanumeric() || c == '-');
    parts.len() >= 2
        && parts.iter().all(label_ok)
        && parts.last().is_some_and(|tld| tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic))
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
    if let Some(page) = pages::normalize(s) {
        return page;
    }
    if ["http://", "https://", "file://", "about:", "edge://"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return s.into();
    }
    if looks_like_host(s) {
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

fn declutter_js(on: bool) -> String {
    format!("window.__ultraDeclutter = {on}; window.__ultraDeclutterApply && window.__ultraDeclutterApply();")
}

/// A selector picked with the zap picker, if it is safe to put in a style sheet.
fn valid_zap(sel: &str) -> bool {
    !sel.trim().is_empty() && sel.len() <= 300 && !sel.contains(['{', '}', '<', ';', '\n'])
}

/// Forget a webview's back/forward list (all but the page it's on), so a webview handed from one
/// tab to another carries nothing over.
fn reset_history(v: &WebView) {
    let h = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
    unsafe {
        let _ = v.webview().CallDevToolsProtocolMethod(&HSTRING::from("Page.resetNavigationHistory"), &HSTRING::from("{}"), &h);
    }
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

/// Tell the event loop when this webview's page enters or leaves fullscreen (a video's
/// fullscreen button, F11 in a game…). WebView2 only stretches the element over the webview's
/// own bounds; ULTRA makes that the whole screen.
fn watch_fullscreen(v: &WebView, proxy: EventLoopProxy<UserEvent>, slot: u64) {
    let h = ContainsFullScreenElementChangedEventHandler::create(Box::new(move |sender, _| {
        if let Some(wv) = sender {
            let mut on = BOOL(0);
            unsafe {
                let _ = wv.ContainsFullScreenElement(&mut on);
            }
            let _ = proxy.send_event(UserEvent::PageFullscreen(slot, on.as_bool()));
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = v.webview().add_ContainsFullScreenElementChanged(&h, &mut token);
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

thread_local! {
    /// Downloads in flight, by key: kept so the downloads panel can pause, resume and cancel them.
    /// WebView2 raises download events on the UI thread, the same one the event loop runs on.
    static DL_OPS: std::cell::RefCell<HashMap<u64, ICoreWebView2DownloadOperation>> = Default::default();
}
static DL_KEY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// ULTRA's own downloads list instead of WebView2's download bubble: every download a page
/// starts is reported (start, progress, end) and kept controllable from the downloads panel.
fn watch_downloads(v: &WebView, proxy: EventLoopProxy<UserEvent>, slot: u64) {
    let Ok(wv4) = v.webview().cast::<ICoreWebView2_4>() else { return };
    let h = DownloadStartingEventHandler::create(Box::new(move |_, args| {
        let Some(a) = args else { return Ok(()) };
        unsafe {
            a.SetHandled(true)?; // no WebView2 download bubble
            let op = a.DownloadOperation()?;
            let key = DL_KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let (mut uri, mut path, mut total) = (windows_core::PWSTR::null(), windows_core::PWSTR::null(), 0i64);
            op.Uri(&mut uri)?;
            a.ResultFilePath(&mut path)?;
            let _ = op.TotalBytesToReceive(&mut total);
            let _ = proxy.send_event(UserEvent::DlStart(key, slot, take_pwstr(uri), take_pwstr(path), total));
            let p = proxy.clone();
            let bytes = BytesReceivedChangedEventHandler::create(Box::new(move |op, _| {
                if let Some(op) = op {
                    let (mut got, mut total) = (0i64, 0i64);
                    let _ = op.BytesReceived(&mut got);
                    let _ = op.TotalBytesToReceive(&mut total);
                    let _ = p.send_event(UserEvent::DlProgress(key, got, total));
                }
                Ok(())
            }));
            let p = proxy.clone();
            let state = StateChangedEventHandler::create(Box::new(move |op, _| {
                let Some(op) = op else { return Ok(()) };
                let (mut st, mut why) = (COREWEBVIEW2_DOWNLOAD_STATE::default(), COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default());
                op.State(&mut st)?;
                let _ = op.InterruptReason(&mut why);
                let s = if st == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
                    downloads::State::Active
                } else if st == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                    downloads::State::Done
                } else if why == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
                    downloads::State::Paused
                } else if why == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED {
                    downloads::State::Cancelled
                } else {
                    downloads::State::Failed
                };
                let mut path = windows_core::PWSTR::null();
                let path = if op.ResultFilePath(&mut path).is_ok() { take_pwstr(path) } else { String::new() };
                let _ = p.send_event(UserEvent::DlState(key, s, path));
                Ok(())
            }));
            let mut t = 0i64;
            let _ = op.add_BytesReceivedChanged(&bytes, &mut t);
            let _ = op.add_StateChanged(&state, &mut t);
            DL_OPS.with(|m| m.borrow_mut().insert(key, op));
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = wv4.add_DownloadStarting(&h, &mut token);
    }
}

/// Virtual-key code for the keys ULTRA's shortcuts use (letters, digits, arrows, tab, comma).
fn window_vk(k: tao::keyboard::KeyCode) -> Option<u32> {
    use tao::keyboard::KeyCode as K;
    let letters = [
        K::KeyA, K::KeyB, K::KeyC, K::KeyD, K::KeyE, K::KeyF, K::KeyG, K::KeyH, K::KeyI, K::KeyJ, K::KeyK, K::KeyL, K::KeyM,
        K::KeyN, K::KeyO, K::KeyP, K::KeyQ, K::KeyR, K::KeyS, K::KeyT, K::KeyU, K::KeyV, K::KeyW, K::KeyX, K::KeyY, K::KeyZ,
    ];
    let digits = [K::Digit0, K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8, K::Digit9];
    let pad = [K::Numpad0, K::Numpad1, K::Numpad2, K::Numpad3, K::Numpad4, K::Numpad5, K::Numpad6, K::Numpad7, K::Numpad8, K::Numpad9];
    if let Some(i) = letters.iter().position(|x| *x == k) {
        return Some(0x41 + i as u32);
    }
    if let Some(i) = digits.iter().position(|x| *x == k) {
        return Some(0x30 + i as u32);
    }
    if let Some(i) = pad.iter().position(|x| *x == k) {
        return Some(0x60 + i as u32);
    }
    Some(match k {
        K::ArrowLeft => 0x25,
        K::ArrowUp => 0x26,
        K::ArrowRight => 0x27,
        K::ArrowDown => 0x28,
        K::Tab => 0x09,
        K::Comma => 0xBC,
        K::F1 => 0x70,
        K::F2 => 0x71,
        K::F9 => 0x78,
        _ => return None,
    })
}

/// Map a Ctrl/Alt key combo to an ULTRA command (the same messages pages used to send).
fn shortcut(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<String> {
    if vk == 0x78 && !ctrl && !shift && !alt {
        return Some("reader".into()); // F9
    }
    if (vk == 0x70 || vk == 0x71) && !ctrl && !shift && !alt {
        return Some(if vk == 0x70 { "back" } else { "fwd" }.into()); // F1 / F2
    }
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
        (false, true, None, Some('C')) => "friends".into(),
        (false, true, None, Some('W')) => "closeat".into(),
        (false, true, None, Some('Q')) => "hideat".into(),
        (false, true, None, Some('T')) => "reopen".into(),
        (false, true, None, Some('R')) => "blankat".into(),
        (false, false, None, Some('F')) => "focusurl".into(),
        (false, false, None, Some('J')) => "downloads".into(),
        (false, false, None, Some('T')) => "new".into(),
        (false, false, None, Some('R')) => "reload".into(),
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

thread_local! {
    /// When ctrl went down, as seen by the webviews' own key events (cleared on its key-up).
    static CTRL_DOWN: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
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
            a.VirtualKey(&mut vk)?;
            let pressed = kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN || kind == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN;
            if vk == 0x11 {
                CTRL_DOWN.with(|c| c.set(pressed.then(Instant::now)));
            }
            if !pressed {
                return Ok(());
            }
            a.PhysicalKeyStatus(&mut st)?;
            // Physical state: key events are processed in WebView2's process, not this thread.
            // A quick ctrl+T can let go of ctrl before this runs, so the ctrl press seen in this
            // same event stream counts too (briefly, in case its key-up never arrives here).
            let down = |k: i32| GetAsyncKeyState(k) < 0;
            let ctrl = down(0x11) || CTRL_DOWN.with(|c| c.get().is_some_and(|t| t.elapsed() < Duration::from_millis(1500)));
            let Some(msg) = shortcut(vk, ctrl, down(0x10), down(0x12)) else { return Ok(()) };
            a.SetHandled(true)?;
            // Held keys auto-repeat: act once, except ctrl+tab cycling.
            if st.WasKeyDown.as_bool() && vk != 0x09 {
                return Ok(());
            }
            let _ = proxy.send_event(UserEvent::Key(slot, msg));
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
        let top = BAR + if self.cfg.bmbar { BMBAR } else { 0.0 } + if self.coffee { COFFEE_BAR } else { 0.0 };
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
        (0..self.tabs.len()).filter(|&i| self.tabs[i].ws == self.ws && Some(self.tabs[i].id) != self.glance).collect()
    }

    /// Request a UI refresh; it is sent once at the end of the event-loop turn.
    fn sync(&self) {
        self.dirty.set(true);
    }

    fn build_view(&mut self, slot: u64, url: Option<&str>, bounds: Rect, private: bool, profile: Option<usize>) -> Option<WebView> {
        let (p1, p2, p3, p4) = (
            self.proxy.clone(),
            self.proxy.clone(),
            self.proxy.clone(),
            self.proxy.clone(),
        );
        let env = self.shared_env.then(|| self.ui.environment());
        let opened = std::sync::Mutex::new(Vec::new());
        let mut b = WebViewBuilder::new_with_web_context(&mut self.ctx);
        if let Some(env) = env {
            b = b.with_environment(env);
        }
        if let Some(u) = url {
            b = b.with_url(u);
        }
        if let Some(w) = profile {
            // A workspace with its own logins: a separate WebView2 profile in the same engine.
            b = b.with_profile_name(format!("ultra-ws{}", w + 1));
        }
        // Built with tao's wake-ups held back (see reentry.rs): creation pumps messages.
        let res = reentry::building(|| b
            .with_bounds(bounds)
            .with_visible(false)
            // Built in the background (spares, waking tabs): never take keyboard focus, or typing
            // in the page in front stops mid-word. Focus moves only when a tab is shown.
            .with_focused(false)
            .with_background_color((20, 20, 20, 255))
            .with_devtools(true)
            // Private tabs use WebView2's InPrivate profile: nothing reaches disk.
            .with_incognito(private)
            .with_additional_browser_args(browser_args())
            .with_initialization_script_for_main_only(typeahead_js(self.cfg.typeahead), true)
            .with_initialization_script_for_main_only(declutter_js(self.cfg.declutter), true)
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
                // A page may only open web pages, and only a few at a time: real clicks never
                // hit the limit, a page opening windows from a timer does (WebView2 has no
                // pop-up blocker of its own).
                let web = u.starts_with("https://") || u.starts_with("http://");
                let now = Instant::now();
                let flood = {
                    let mut recent = opened.lock().unwrap_or_else(|e| e.into_inner());
                    recent.retain(|t: &Instant| now.duration_since(*t) < POPUP_WINDOW);
                    recent.push(now);
                    recent.len() > POPUP_MAX
                };
                if flood {
                    let _ = p4.send_event(UserEvent::PopupBlocked(slot));
                    return NewWindowResponse::Deny;
                }
                // Scripted popups (window.open with a size, e.g. "Sign in with Google")
                // get a real popup that keeps window.opener so OAuth can complete (they may
                // start blank and navigate themselves).
                if f.size.is_some() && (web || u.is_empty() || u == "about:blank") {
                    return NewWindowResponse::Allow;
                }
                // Plain target=_blank links open as a new tab without an opener.
                if web {
                    let _ = p4.send_event(UserEvent::NewTab(slot, u));
                }
                NewWindowResponse::Deny
            })
            .build_as_child(&self.window));
        match res {
            Ok(v) => {
                watch_failures(&v, self.proxy.clone(), slot);
                watch_fullscreen(&v, self.proxy.clone(), slot);
                watch_keys(&v, self.proxy.clone(), slot);
                watch_downloads(&v, self.proxy.clone(), slot);
                blocker::install(&v, slot, self.shields.clone(), self.block_on.clone());
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
        let profile = self.profile_of(self.tabs[i].ws);
        // Spares are default-profile webviews, so private tabs and workspaces with their own
        // logins always get a fresh webview in the right profile.
        let spare = if private || profile.is_some() {
            None
        } else {
            // Only a spare already showing its blank page (a reused one may still be leaving the
            // last tab's page).
            let blank = |v: &WebView| v.url().is_ok_and(|u| u.is_empty() || u == "about:blank");
            self.spares.iter().rposition(|(_, v)| blank(v)).map(|k| self.spares.remove(k))
        };
        let got = match spare {
            Some((slot, v)) => {
                // A reused webview must not offer the last tab's pages under back/forward.
                reset_history(&v);
                let _ = v.set_bounds(bounds);
                if !pages::is_page(&url) {
                    let _ = v.load_url(&url);
                }
                Some((slot, v))
            }
            None => {
                let slot = self.new_slot();
                let first = if pages::is_page(&url) { "about:blank" } else { url.as_str() };
                self.build_view(slot, Some(first), bounds, private, profile).map(|v| (slot, v))
            }
        };
        self.spare_at = Some(Instant::now() + SPARE_IDLE);
        if let Some((slot, v)) = got {
            if pages::is_page(&url) {
                let _ = v.load_html(&pages::html(&url));
            }
            let t = &mut self.tabs[i];
            t.slot = slot;
            t.view = Some(v);
            t.loading = true;
            t.suspended = false;
        }
    }

    /// Build one spare per quiet moment until the pool is full.
    fn refill_spare(&mut self) {
        if self.spares.len() >= SPARES || self.over_memcap() {
            return;
        }
        if self.anim.is_some() {
            self.spare_at = Some(Instant::now() + SPARE_IDLE);
            return;
        }
        let slot = self.new_slot();
        let a = self.area();
        if let Some(v) = self.build_view(slot, None, inset(a, 0.0), false, None) {
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
        // A default-profile webview that isn't frozen goes back to the spare pool as a blank page
        // rather than being destroyed: waking the next tab then skips starting a new webview and
        // page process (~0.5 s). Private and per-workspace-profile webviews are never reused.
        let reuse = self.spares.len() < SPARES
            && self.tabs[i].ws != PRIVATE_WS
            && self.profile_of(self.tabs[i].ws).is_none()
            && !self.tabs[i].suspended;
        let t = &mut self.tabs[i];
        let slot = t.slot;
        if let Some(v) = t.view.take() {
            if let Ok(u) = v.url() {
                if !u.is_empty() && u != "about:blank" {
                    t.url = u;
                }
            }
            if reuse {
                let _ = v.set_visible(false);
                let _ = v.zoom(1.0);
                let _ = v.load_url("about:blank");
                self.spares.insert(0, (slot, v));
            }
        }
        let t = &mut self.tabs[i];
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
        let glance = self.glance.and_then(|g| self.idx(g).map(|i| (g, i)));
        if let Some((_, i)) = glance {
            let r = self.glance_rect();
            self.wake(i, inset(r, 0.0));
        }
        let now = Instant::now();
        // Keep private pages out of screenshots and screen sharing while that workspace is shown.
        privacy::exclude_from_capture(self.window.hwnd() as isize, self.ws == PRIVATE_WS);
        let hidden = self.overlay || self.tour || self.gate() || self.booting;
        // A glance covers the layout, so the panes behind it are hidden (but stay warm).
        let covered = hidden || glance.is_some();
        let animate = animate && !covered && !self.cfg.ultraspeed;
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
        let gid = self.glance;
        for t in &mut self.tabs {
            if Some(t.id) == gid {
                continue;
            }
            match targets.iter().find(|(id, _)| *id == t.id) {
                Some(&(_, r)) => {
                    t.hidden_at = None;
                    t.suspended = false;
                    if let Some(v) = &t.view {
                        if !animate {
                            let _ = v.set_bounds(inset(r, top));
                        }
                        let _ = v.set_memory_usage_level(MemoryUsageLevel::Normal);
                        let _ = v.set_visible(!covered);
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
        if let Some((_, i)) = glance {
            let r = self.glance_rect();
            let t = &mut self.tabs[i];
            t.hidden_at = None;
            t.suspended = false;
            t.last = now;
            if let Some(v) = &t.view {
                let _ = v.set_bounds(inset(r, 0.0));
                let _ = v.set_visible(!hidden);
            }
        }
        if animate {
            self.step_anim();
        } else {
            self.shown = targets;
        }
        // A fullscreen page covers the whole window (which is fullscreen too); everything else
        // hides. A panel opening over it ends fullscreen.
        if let Some(fid) = self.page_full {
            let on_screen = self.shown.iter().any(|(x, _)| *x == fid) || self.glance == Some(fid);
            match self.idx(fid).filter(|_| !hidden && on_screen) {
                Some(fi) => {
                    self.anim = None;
                    let full = self.full_rect();
                    for (k, t) in self.tabs.iter().enumerate() {
                        if let Some(v) = &t.view {
                            if k == fi {
                                let _ = v.set_bounds(full);
                                let _ = v.set_visible(true);
                            } else {
                                let _ = v.set_visible(false);
                            }
                        }
                    }
                }
                None => self.end_page_full(),
            }
        }
        // Put the least-recently-used hidden tabs to sleep until we're under budget,
        // never cutting off a tab that is playing audio.
        while self.live_count() > self.cfg.warm + self.shown.len() + usize::from(glance.is_some()) {
            let victim = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, t)| !self.shown.iter().any(|(id, _)| *id == t.id) && Some(t.id) != self.glance)
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
        if let Some(v) = self.glance.and_then(|g| self.idx(g)).and_then(|i| self.tabs[i].view.as_ref()) {
            let _ = v.focus();
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
        // Showing another tab closes any glance (removing it can shift indices, so go by id).
        let Some(id) = self.tabs.get(i).map(|t| t.id) else { return };
        if self.page_full.is_some_and(|f| f != id) {
            self.end_page_full();
        }
        self.drop_glance(Some(id));
        let Some(i) = self.idx(id) else { return };
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
        self.drop_glance(None);
        if self.ws != PRIVATE_WS {
            self.last_ws = self.ws;
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
        if n == self.ws || n >= WORKSPACES {
            return;
        }
        // Tabs never cross the private boundary: their webviews belong to one profile.
        if self.ws == PRIVATE_WS {
            self.toast("private tabs stay in the private workspace");
            return;
        }
        if n == PRIVATE_WS {
            self.toast("tabs can't move into the private workspace · open the page there instead");
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
            let old = self.tabs[i].ws;
            self.tabs[i].ws = n;
            // Its webview belongs to the old workspace's logins: rebuild it in the new one's.
            if self.profile_of(old) != self.profile_of(n) {
                self.sleep(i);
            }
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

    /// Ctrl+Shift+T: the most recently closed tab, back in its workspace, its place in the tab
    /// strip and, when the panes around it are still there, its old pane (same side, same size).
    fn reopen(&mut self) {
        let Some(c) = self.closed.pop() else {
            self.toast("no closed tabs to bring back");
            return;
        };
        self.drop_glance(None);
        let switched = c.ws != self.ws;
        if switched {
            self.enter_ws(c.ws);
        }
        let id = self.push_tab(c.at, c.url, c.title);
        self.zoom = None;
        self.overlay = false;
        let placed = c.place.as_ref().is_some_and(|(near, v, r, first)| {
            self.root.restore(near, id, *v, *r, *first)
                // The panes around it changed: split the first one still showing, same side.
                || near.iter().any(|&n| self.root.split(n, id, *v, *first))
        });
        if placed {
            self.set_focus(id);
            self.apply_layout(true);
            self.focus_active();
        } else if let Some(i) = self.idx(id) {
            // It was the only pane, or hidden: show it in the focused pane, as before.
            self.activate(i);
        }
    }

    fn close_tab(&mut self, i: usize) {
        let Some((id, ws)) = self.tabs.get(i).map(|t| (t.id, t.ws)) else { return };
        if self.glance == Some(id) {
            self.glance_cmd("close");
            return;
        }
        let tiled = !self.root.is_leaf(id) && self.root.contains(id);
        let place = if tiled { self.root.place_of(id) } else { None };
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
            self.closed.push(Closed { url: t.url, title: t.title, ws: t.ws, at: i, place });
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
    /// The pane a cursor shortcut acts on, its rect and the point used to pick a side.
    /// Cursor outside every pane: the focused pane, toward its far side.
    fn cursor_target(&self) -> (u64, R, f64, f64) {
        let panes = self.panes();
        let hit = self.cursor().and_then(|(px, py)| {
            panes.iter().find(|(_, r)| r.contains(px, py)).map(|&(t, r)| (t, r, px, py))
        });
        hit.unwrap_or_else(|| {
            let r = self.pane_of(self.focus).unwrap_or(self.area());
            (self.focus, r, r.x + r.w * 0.95, r.y + r.h * 0.5)
        })
    }

    fn place_at_cursor(&mut self, id: u64) {
        self.zoom = None;
        let (target, r, px, py) = self.cursor_target();
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
        let (v, first) = match dir {
            "l" => (true, true),
            "r" => (true, false),
            "u" => (false, true),
            _ => (false, false),
        };
        // Same minimum pane size as ctrl+shift+e (judged on the unzoomed layout).
        let zoom = self.zoom.take();
        let r = self.pane_of(self.focus).unwrap_or(self.area());
        let room = if v { (r.w - GAP) / 2.0 >= MIN_W } else { (r.h - GAP) / 2.0 >= MIN_H };
        if !room {
            self.zoom = zoom;
            self.toast("no room to split this pane further · close one (ctrl+shift+w) or zoom (ctrl+shift+f)");
            return;
        }
        let at = self.cur() + 1;
        let id = self.push_tab(at, url, String::new());
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

    /// Ctrl+Shift+Q: take the pane under the cursor out of the layout but keep its tab open
    /// (hidden, in the tab list; click it or ctrl+1..9 to bring it back).
    fn hide_at_cursor(&mut self) {
        let id = self.pane_under_cursor().unwrap_or(self.focus);
        if !self.root.contains(id) {
            return;
        }
        if self.root.is_leaf(id) {
            self.toast("this is the only pane · nothing to hide it behind");
            return;
        }
        if self.zoom == Some(id) {
            self.zoom = None;
        }
        self.root.remove(id);
        if self.focus == id || !self.root.contains(self.focus) {
            self.focus = self.root.first_leaf();
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
            if !visible && Some(t.id) != self.glance && t.view.as_ref().is_some_and(|v| !playing_audio(v)) {
                self.sleep(i);
            }
        }
        self.sync();
    }

    fn navigate(&mut self, i: usize, input: &str) {
        let url = resolve(input, &self.cfg.search);
        // ultra://settings and the like open a panel; the tab stays where it was.
        if let Some(cmd) = pages::panel(&url) {
            return self.command(cmd, &json!(true));
        }
        let Some(t) = self.tabs.get_mut(i) else { return };
        if url == HOME {
            t.view = None;
            t.slot = 0;
            t.url = HOME.into();
            t.title.clear();
        } else {
            t.url = url.clone();
            if let Some(v) = &t.view {
                if pages::is_page(&url) {
                    let _ = v.load_html(&pages::html(&url));
                } else {
                    let _ = v.load_url(&url);
                    t.loading = true;
                }
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
        if !self.cfg.stats && self.cfg.memcap == 0 {
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
        for t in self.tabs.iter().filter(|_| self.cfg.stats).filter(|t| self.shown.iter().any(|(x, _)| *x == t.id)) {
            if let Some(v) = &t.view {
                let (proxy, slot) = (self.proxy.clone(), t.slot);
                let _ = v.evaluate_script_with_callback(stats::HEAP_JS, move |r| {
                    let _ = proxy.send_event(UserEvent::Heap(slot, r));
                });
            }
        }
        self.total_mb = total;
        self.total_cpu = total_cpu;
        self.enforce_memcap();
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
                json!({ "id": t.id, "t": title, "u": url, "s": s, "w": t.ws, "p": private, "v": self.root.contains(t.id), "g": Some(t.id) == self.glance })
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
        let sc_list = self.lib.shortcuts_view();
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
            "blocked": self.shields.total.get(),
            "shield": self.shield_state(),
            "cfg": ui_cfg(&self.cfg),
            "locked": self.locked,
            "gate": self.gate(),
            "layouts": self.layouts.keys().collect::<Vec<_>>(),
            "bm": self.tabs.get(self.cur()).is_some_and(|t| t.ws != PRIVATE_WS && self.lib.is_bookmarked(&t.url)),
            "palhist": self.lib.commands,
            "digest": self.lib.digest_view(),
            "bms": self.lib.bookmarks.iter().map(|b| json!({ "u": b.url, "t": b.title })).collect::<Vec<_>>(),
            "tour": self.tour,
            "coffee": self.coffee,
            "glance": self.glance.and_then(|g| self.idx(g)).map(|i| {
                let t = &self.tabs[i];
                json!({ "id": t.id, "r": self.glance_rect(), "t": t.title, "u": t.url, "s": t.loading })
            }),
            "max": self.window.is_maximized(),
            "me": self.me,
            "social": self.social.state(),
            // New-tab shortcuts (or the most visited sites until there are some).
            "sc": { "list": sc_list.iter().map(|s| json!({ "u": s.url, "t": s.title })).collect::<Vec<_>>() },
            "version": updater::version(),
            "browsers": self.browsers,
            "ed": edition::TAG,
            "isdef": self.is_default,
            // Private-workspace downloads are only listed while it is unlocked.
            "dl": self.dls.view(!self.locked),
            // Back/forward buttons: whether the focused tab's own history allows each way.
            "nav": self.tabs.get(self.cur()).and_then(|t| t.view.as_ref()).map(|v| {
                [v.can_go_back().unwrap_or(false), v.can_go_forward().unwrap_or(false)]
            }).unwrap_or([false, false]),
        });
        let _ = self.ui.evaluate_script(&format!("ultra.render({state})"));
    }

    fn layout(&mut self) {
        let _ = self.ui.set_bounds(self.full_rect());
        self.apply_layout(false);
    }

    /// A page went fullscreen (or came back): the window follows, and the layout puts that page
    /// over everything (or back in its pane).
    fn on_page_fullscreen(&mut self, slot: u64, on: bool) {
        let Some(i) = self.idx_slot(slot) else { return };
        let id = self.tabs[i].id;
        if on {
            let visible = self.shown.iter().any(|(x, _)| *x == id) || self.glance == Some(id);
            if !visible || self.overlay {
                return;
            }
            self.page_full = Some(id);
            self.window.set_fullscreen(Some(Fullscreen::Borderless(None)));
            self.apply_layout(false);
            self.focus_active();
            // Say whose page took over the screen: a fullscreen page could draw a fake address bar.
            let host = library::host(&self.tabs[i].url).unwrap_or_else(|| "this page".into());
            self.notify_fullscreen(&host);
        } else if self.page_full == Some(id) {
            self.page_full = None;
            self.window.set_fullscreen(None);
            self.layout();
        }
    }

    /// A short banner over the fullscreen page itself (ULTRA's toast is underneath it).
    fn notify_fullscreen(&self, host: &str) {
        let Some(v) = self.page_full.and_then(|f| self.idx(f)).and_then(|i| self.tabs[i].view.as_ref()) else { return };
        let js = format!(
            "(() => {{ const d = document.createElement('div'); d.textContent = {}; Object.assign(d.style, {{ position: 'fixed', top: '18px', left: '50%', transform: 'translateX(-50%)', zIndex: 2147483647, background: 'rgba(20,20,20,.9)', color: '#eee', font: '13px system-ui, sans-serif', padding: '8px 14px', borderRadius: '6px', pointerEvents: 'none', transition: 'opacity .4s' }}); (document.fullscreenElement || document.body || document.documentElement).appendChild(d); setTimeout(() => d.style.opacity = '0', 2600); setTimeout(() => d.remove(), 3200); }})()",
            serde_json::to_string(&format!("{host} is full screen · press esc to exit")).unwrap_or_default()
        );
        let _ = v.evaluate_script(&js);
    }

    /// Leave page fullscreen from ULTRA's side (switching tabs, opening a panel…): tell the page,
    /// and put the window back straight away.
    fn end_page_full(&mut self) {
        let Some(fid) = self.page_full.take() else { return };
        if let Some(v) = self.idx(fid).and_then(|i| self.tabs[i].view.as_ref()) {
            let _ = v.evaluate_script("document.fullscreenElement && document.exitFullscreen().catch(() => {})");
        }
        self.window.set_fullscreen(None);
        let _ = self.ui.set_bounds(self.full_rect());
    }

    /// Ask GitHub for a newer build in the background. `auto`: the periodic check (or auto-update
    /// just switched on), which installs what it finds by itself.
    fn check_updates(&mut self, auto: bool) {
        let token = updater::load_token(&self.data_dir);
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let (state, up) = updater::check(edition::REPO, token);
            let _ = proxy.send_event(UserEvent::UpdateChecked(state, up, auto));
        });
    }

    fn on_update_checked(&mut self, state: Value, up: Option<updater::Update>, auto: bool) {
        self.update = up.clone();
        match up {
            // Auto-update: install it now; the new version starts the next time ULTRA opens.
            Some(up) if auto && self.cfg.autoupdate && self.pending_restart.is_none() => {
                let what = state["m"].as_str().unwrap_or("a new version").to_string();
                let st = json!({ "s": "installing", "m": format!("auto updating · {what}") });
                let _ = self.ui.evaluate_script(&format!("ultra.updateState({st})"));
                self.toast("auto updating ULTRA…");
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let _ = proxy.send_event(UserEvent::UpdateInstalled(updater::install(&up), true));
                });
            }
            _ => {
                // A quiet periodic check doesn't disturb a message the user is looking at.
                if !(auto && state["s"] != "available") || state["s"] == "uptodate" {
                    let _ = self.ui.evaluate_script(&format!("ultra.updateState({state})"));
                }
            }
        }
    }

    /// An update is in place. Asked for: restart into it now. Auto-update: leave the user be; the
    /// new version starts next time (or straight away with "restart now" in settings).
    fn on_update_installed(&mut self, exe: PathBuf, auto: bool, flow: &mut ControlFlow) {
        if auto {
            self.pending_restart = Some(exe);
            let st = json!({ "s": "restart", "m": "updated · the new version starts next time you open ULTRA" });
            let _ = self.ui.evaluate_script(&format!("ultra.updateState({st})"));
            self.toast("ULTRA updated · it starts with the new version next time (or restart now in settings)");
            return;
        }
        self.restart_into(&exe, flow);
    }

    /// Hand the session over to the new build: save everything, start it, close.
    fn restart_into(&mut self, exe: &std::path::Path, flow: &mut ControlFlow) {
        self.save_settings();
        self.save_session();
        if updater::restart(exe) {
            *flow = ControlFlow::Exit;
        } else {
            let _ = self.ui.evaluate_script(&format!("ultra.updateState({})", json!({ "s": "failed", "m": "updated · restart ULTRA to use the new build" })));
        }
    }

    /// Connect friends & chat (again), e.g. after the ULTRA ID is made or the server changes.
    fn start_social(&mut self) {
        let proxy = self.proxy.clone();
        // An encrypted (wss://) address in settings is used on its own (or plain ws:// to this
        // computer, for testing); anything else, like the unencrypted home address early builds
        // saved, falls back to the default relay.
        let saved = self.cfg.chatserver.trim();
        let usable = saved.starts_with("wss://") || saved.starts_with("ws://127.0.0.1") || saved.starts_with("ws://localhost");
        let servers = if usable && !social::DEFAULT_SERVERS.contains(&saved) {
            vec![saved.to_string()]
        } else {
            social::DEFAULT_SERVERS.iter().map(|s| s.to_string()).collect()
        };
        self.social.start(servers, move |v| {
            let _ = proxy.send_event(UserEvent::Social(v));
        });
    }

    fn save_session(&self) {
        // Private tabs are never written anywhere: positions are among normal tabs only.
        let kept: Vec<u64> = self.tabs.iter().filter(|t| t.ws != PRIVATE_WS && Some(t.id) != self.glance).map(|t| t.id).collect();
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
                .filter(|t| t.ws != PRIVATE_WS && Some(t.id) != self.glance)
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
            ws: self.tabs.iter().filter(|t| t.ws != PRIVATE_WS && Some(t.id) != self.glance).map(|t| t.ws).collect(),
            layouts: (0..WORKSPACES).map(layout_of).collect(),
            cur_ws: if self.ws == PRIVATE_WS { 0 } else { self.ws },
            window: self.placement(),
        };
        if let Ok(j) = serde_json::to_string(&s) {
            let _ = library::write_atomic(&self.data_dir.join("session.json"), j);
        }
        self.lib.save(&self.data_dir);
        self.dls.save(&self.data_dir);
    }

    fn save_settings(&self) {
        if let Ok(j) = serde_json::to_string_pretty(&self.cfg) {
            let _ = library::write_atomic(&self.data_dir.join("settings.json"), j);
        }
    }

    fn tick(&mut self) {
        if self.cfg.autoupdate && Instant::now() >= self.next_update_check {
            self.next_update_check = Instant::now() + Duration::from_secs(4 * 3600);
            self.check_updates(true);
        }
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

    /// The window's normal (restore) bounds and whether it is maximized, straight from Windows,
    /// which keeps them up to date through maximize, minimize and snapping.
    fn placement(&self) -> Option<WinState> {
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{GetWindowPlacement, SW_SHOWMAXIMIZED, WINDOWPLACEMENT},
        };
        if !self.visible {
            return self.start_place;
        }
        let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        unsafe { GetWindowPlacement(HWND(self.window.hwnd() as *mut _), &mut wp) }.ok()?;
        let r = wp.rcNormalPosition;
        Some(WinState {
            x: r.left,
            y: r.top,
            w: (r.right - r.left).max(0) as u32,
            h: (r.bottom - r.top).max(0) as u32,
            max: wp.showCmd == SW_SHOWMAXIMIZED.0 as u32 || self.window.is_maximized(),
        })
    }

    fn show_window(&mut self) {
        if !self.visible {
            self.visible = true;
            if let Some(w) = self.start_place.take() {
                // Back where it was, in Windows' own placement terms (the same ones it was saved
                // in): restore bounds plus maximized, in one step, so un-maximizing is right too.
                use windows::Win32::{
                    Foundation::{HWND, RECT},
                    UI::WindowsAndMessaging::{SetWindowPlacement, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, WINDOWPLACEMENT},
                };
                let wp = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    showCmd: if w.max { SW_SHOWMAXIMIZED.0 as u32 } else { SW_SHOWNORMAL.0 as u32 },
                    rcNormalPosition: RECT { left: w.x, top: w.y, right: w.x + w.w as i32, bottom: w.y + w.h as i32 },
                    ..Default::default()
                };
                unsafe {
                    let _ = SetWindowPlacement(HWND(self.window.hwnd() as *mut _), &wp);
                }
            }
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
        if (self.cfg.stats || self.cfg.memcap > 0) && !self.stats_pending {
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
        if let Some(t) = self.settings_save_at {
            d = d.min(t);
        }
        if let Some(t) = self.coffee_at {
            d = d.min(t.max(now + Duration::from_millis(250)));
        }
        if self.slideshow_on() {
            d = d.min(self.slide_at);
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
            // Keep the capture helper through pauses, ads and buffering; stop it once quiet.
            if now.duration_since(self.last_sound) > BARS_LINGER {
                self.spectrum = None;
            }
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
        self.last_sound = now;
        let level = if self.meter.is_empty() { 0.0 } else { self.meter.peak() };
        let _ = self.ui.evaluate_script(&format!("ultra.audio({level:.3})"));
        // Bars background: real spectrum from ULTRA's own audio (UI falls back to the level).
        if self.cfg.bars {
            if self.spectrum.is_none() {
                let mut pid = 0u32;
                unsafe {
                    let _ = self.ui.webview().BrowserProcessId(&mut pid);
                }
                if pid != 0 {
                    self.spectrum = Some(spectrum::Spectrum::start(pid));
                }
            }
            if let Some(b) = self.spectrum.as_ref().and_then(|s| s.bands()) {
                let list: Vec<String> = b.iter().map(|v| format!("{v:.2}")).collect();
                let _ = self.ui.evaluate_script(&format!("ultra.bars([{}])", list.join(",")));
            }
        } else {
            self.spectrum = None;
        }
        self.audio_at = now + Duration::from_millis(33);
    }

    /// Slideshow: load the next image in the folder on a background thread.
    fn next_slide(&mut self, now: Instant) {
        self.slide_at = now + Duration::from_secs(self.cfg.slidesecs.max(5) as u64);
        if self.slide_loading || self.cfg.slidedir.is_empty() {
            return;
        }
        // Rescan each time round, so images added to the folder show up.
        if self.slides.is_empty() || self.slide_idx >= self.slides.len() {
            self.slides = slideshow::list(&self.cfg.slidedir);
            self.slide_idx = 0;
        }
        let Some(path) = self.slides.get(self.slide_idx).cloned() else {
            self.toast("no png, jpg or webp images in that folder");
            return;
        };
        self.slide_idx += 1;
        self.slide_loading = true;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let data = slideshow::load(&path).unwrap_or_default();
            let _ = proxy.send_event(UserEvent::Slide(data, name));
        });
    }

    fn slideshow_on(&self) -> bool {
        self.cfg.bgmode == "slideshow" && !self.cfg.ultraspeed && !self.cfg.slidedir.is_empty()
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
            "autoupdate" => {
                self.cfg.autoupdate = v.as_bool().unwrap_or(true);
                // Switched on: look right away (and say "auto updating" if there's something).
                if self.cfg.autoupdate {
                    self.check_updates(true);
                }
            }
            "block" => {
                self.cfg.block = v.as_bool().unwrap_or(true);
                self.block_on.set(self.cfg.block);
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
            "shortcuts" => self.cfg.shortcuts = v.as_bool().unwrap_or(true),
            "slidesecs" => {
                self.cfg.slidesecs = v.as_u64().unwrap_or(60).clamp(5, 24 * 3600) as u32;
                self.slide_at = Instant::now() + Duration::from_secs(self.cfg.slidesecs as u64);
            }
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
            "bgmode" => match v.as_str() {
                // "bars" is a layer now, not a background of its own.
                Some("bars") => self.cfg.bars = true,
                Some(m) if BG_MODES.contains(&m) => self.cfg.bgmode = m.to_string(),
                _ => return,
            },
            "bars" => self.cfg.bars = v.as_bool().unwrap_or(false),
            "memcap" => {
                let mb = v.as_u64().unwrap_or(0).min(MEMCAP_MAX);
                // Anything below half a gigabyte would just keep every hidden tab asleep.
                self.cfg.memcap = if mb == 0 { 0 } else { mb.max(512) as u32 };
                self.stats_at = Instant::now();
            }
            "declutter" => {
                self.cfg.declutter = v.as_bool().unwrap_or(true);
                let js = declutter_js(self.cfg.declutter);
                for view in self.all_views() {
                    add_doc_script(view, &js);
                    let _ = view.evaluate_script(&js);
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
        // Save once the changes settle (dragging a slider sends many), and relayout only for the
        // settings that move or size panes; everything else is just redrawn.
        self.settings_save_at = Some(Instant::now() + SETTINGS_SAVE_DELAY);
        if matches!(key, "bmbar" | "stats" | "ultraspeed" | "warm") {
            self.apply_layout(false);
        } else {
            self.sync();
        }
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

    /// What a page script reports back when reader view or picture-in-picture can't help.
    fn page_notice(&self, msg: &str) {
        self.toast(&match msg {
            "pip:none" => "no video on this page".to_string(),
            "pip:fail" => format!("this page wouldn't pop its video out · click the page, then press {PIP_KEYS}"),
            _ => "no article found on this page".to_string(),
        });
    }

    fn toast(&self, msg: &str) {
        let _ = self.ui.evaluate_script(&format!("ultra.toast({})", json!(msg)));
    }

    /// A link from another app: a new tab in front of the user, and the window brought forward.
    /// An empty link (ULTRA started again) only brings the window forward.
    fn open_external(&mut self, url: &str) {
        let lower = url.to_ascii_lowercase();
        if ["http://", "https://", "file:///"].iter().any(|p| lower.starts_with(p)) {
            if self.ws == PRIVATE_WS && self.locked {
                // Not behind the lock screen: show it in the last regular workspace instead.
                let back = self.last_ws;
                self.switch_ws(back);
                self.toast(&format!("opened in workspace {} · the private workspace is locked", back + 1));
            }
            self.open_in(self.ws, url.to_string());
        }
        self.window.set_minimized(false);
        self.window.set_visible(true);
        self.window.set_focus();
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
            if privacy::needs_upgrade(&self.cfg.privpass) {
                self.cfg.privpass = privacy::hash_password(password);
                self.save_settings();
            }
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
        for id in self.dls.forget_private() {
            if let Some(key) = self.dl_keys.iter().find(|(_, v)| **v == id).map(|(k, _)| *k) {
                self.dl_keys.remove(&key);
                if let Some(op) = DL_OPS.with(|m| m.borrow_mut().remove(&key)) {
                    unsafe {
                        let _ = op.Cancel();
                    }
                }
            }
        }
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

    /// The address bar's shield: blocked on the focused page, and whether its site is exempt.
    fn shield_state(&self) -> Value {
        let t = self.tabs.get(self.cur());
        let site = t.and_then(|t| library::host(&t.url)).filter(|_| t.is_some_and(|t| t.url.starts_with("http")));
        json!({
            "n": t.map(|t| self.shields.on_page(t.slot)).unwrap_or(0),
            "site": site,
            "off": site.as_ref().is_some_and(|h| self.cfg.blockoff.contains(h)),
            "on": self.cfg.block,
            "ready": blocker::ready(),
            "rules": blocker::rules(),
        })
    }

    /// Daily digest bookkeeping: credit time to the page being looked at (focused window,
    /// recent input, a real page, not private) and add newly blocked trackers.
    fn track(&mut self) {
        let now = Instant::now();
        let b = self.shields.total.get();
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
            let _ = library::write_atomic(&self.data_dir.join("layouts.json"), j);
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

    /// One ULTRA shortcut, whether it came from a page (native accelerator keys) or from the
    /// bare window when no webview had keyboard focus.
    fn shortcut_msg(&mut self, msg: &str) {
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
            match msg {
                "new" | "close" | "next" | "prev" | "reopen" | "back" | "fwd" | "friends" | "focusurl" | "overview" | "settings"
                | "palette" | "mvnew" | "closeat" | "hideat" | "zoom" | "lock" | "bookmark" | "reader" | "blankat" | "downloads" | "reload" => {
                    self.command(msg, &Value::Null)
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
        if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE {
            // A page busy for a few seconds is not dead: reloading it would throw away whatever
            // the user typed. Say so and let them decide; it often recovers on its own.
            if let Some(i) = self.idx_slot(slot).filter(|&i| self.shown.iter().any(|(x, _)| *x == self.tabs[i].id)) {
                let name = library::host(&self.tabs[i].url).unwrap_or_else(|| "this page".into());
                self.toast(&format!("{name} isn't responding · wait, or press ctrl+r to reload it"));
            }
            return;
        }
        let renderer = kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
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

    /// Workspace `ws`'s own login profile, if it keeps one.
    fn profile_of(&self, ws: usize) -> Option<usize> {
        (ws != PRIVATE_WS && self.cfg.isolated.contains(&ws)).then_some(ws)
    }

    /// Give workspace `w` its own cookies and sign-ins, or share the default ones again.
    /// Its open pages are rebuilt in the right profile (addresses are kept).
    fn toggle_isolate(&mut self, w: usize) {
        if w >= PRIVATE_WS {
            self.toast("the private workspace is already sealed off");
            return;
        }
        let on = match self.cfg.isolated.iter().position(|&x| x == w) {
            Some(p) => {
                self.cfg.isolated.remove(p);
                false
            }
            None => {
                self.cfg.isolated.push(w);
                self.cfg.isolated.sort_unstable();
                true
            }
        };
        self.save_settings();
        for i in 0..self.tabs.len() {
            if self.tabs[i].ws == w && self.tabs[i].view.is_some() {
                self.sleep(i);
            }
        }
        self.apply_layout(false);
        self.focus_active();
        self.toast(&if on {
            format!("workspace {} now keeps its own logins · sign in there separately", w + 1)
        } else {
            format!("workspace {} shares logins with the others again", w + 1)
        });
    }

    fn over_memcap(&self) -> bool {
        self.cfg.memcap > 0 && self.total_mb > self.cfg.memcap as f64
    }

    /// Memory cap: while ULTRA is over it, put the least recently used hidden tab to sleep
    /// (one per sample, so memory has time to drop). Visible pages, the glance and tabs
    /// playing sound are never touched; spare webviews go last.
    fn enforce_memcap(&mut self) {
        if !self.over_memcap() {
            return;
        }
        let victim = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| Some(t.id) != self.glance && !self.shown.iter().any(|(id, _)| *id == t.id))
            .filter(|(_, t)| t.view.as_ref().is_some_and(|v| !playing_audio(v)))
            .min_by_key(|(_, t)| t.last)
            .map(|(i, _)| i);
        match victim {
            Some(i) => self.sleep(i),
            None => {
                self.spares.pop();
            }
        }
        self.sync();
    }

    /// Where a glance floats: most of the layout area, centred, under its title strip.
    fn glance_rect(&self) -> R {
        let a = self.area();
        let w = (a.w * 0.84).max(320.0).min(a.w);
        let h = (a.h * 0.86 - GLANCE_HDR).max(160.0).min(a.h - GLANCE_HDR).max(1.0);
        R { x: a.x + (a.w - w) / 2.0, y: a.y + GLANCE_HDR + (a.h - GLANCE_HDR - h) / 2.0, w, h }
    }

    /// Preview a link in a floating pane over the layout (a glance inside a glance replaces it).
    fn open_glance(&mut self, url: &str, ws: usize) {
        let ok = (url.starts_with("https://") || url.starts_with("http://")) && url.len() <= 4096;
        if !ok || self.overlay || self.tour || self.gate() {
            return;
        }
        match self.glance.and_then(|g| self.idx(g)) {
            Some(i) if self.tabs[i].ws == ws => {
                let t = &mut self.tabs[i];
                t.url = url.to_string();
                t.title.clear();
                if let Some(v) = &t.view {
                    let _ = v.load_url(url);
                    t.loading = true;
                }
            }
            _ => {
                self.drop_glance(None);
                let id = self.push_tab(self.tabs.len(), url.to_string(), String::new());
                if let Some(i) = self.idx(id) {
                    self.tabs[i].ws = ws;
                }
                self.glance = Some(id);
            }
        }
        self.apply_layout(false);
        self.focus_active();
    }

    /// Close the glance (its tab goes too) unless it is `keep`, which then becomes a normal tab.
    fn drop_glance(&mut self, keep: Option<u64>) {
        let Some(g) = self.glance.take() else { return };
        match self.idx(g) {
            Some(i) if Some(g) != keep => {
                self.tabs.remove(i);
            }
            Some(i) => {
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.evaluate_script("window.__ultraGlance = false;");
                }
            }
            None => {}
        }
    }

    /// The glance's buttons and keys: "split" tiles it next to the focused pane, "tab" keeps it
    /// as a background tab, anything else closes it.
    fn glance_cmd(&mut self, how: &str) {
        let Some(g) = self.glance else { return };
        match how {
            "split" => {
                self.drop_glance(Some(g));
                self.zoom = None;
                let r = self.pane_of(self.focus).unwrap_or(self.area());
                match pick_split(r, r.x + r.w * 0.95, r.y + r.h * 0.5) {
                    Some((v, first)) => {
                        self.root.split(self.focus, g, v, first);
                    }
                    None => {
                        self.root.replace(self.focus, g);
                    }
                }
                self.set_focus(g);
                self.apply_layout(true);
            }
            "tab" => {
                self.drop_glance(Some(g));
                self.toast("kept as a tab");
                self.apply_layout(false);
            }
            _ => {
                self.drop_glance(None);
                self.apply_layout(false);
            }
        }
        self.focus_active();
    }

    fn save_zaps(&self) {
        if let Ok(j) = serde_json::to_string_pretty(&self.zaps) {
            let _ = library::write_atomic(&self.data_dir.join("zaps.json"), j);
        }
    }

    /// Re-hide this site's zapped elements in tab `i` (after each page load).
    fn apply_zaps(&self, i: usize) {
        let t = &self.tabs[i];
        let Some(v) = &t.view else { return };
        if let Some(list) = library::host(&t.url).and_then(|h| self.zaps.get(&h)) {
            let _ = v.evaluate_script(&format!("window.__ultraZapApply && window.__ultraZapApply({})", json!(list)));
        }
    }

    /// The zap picker hid an element: remember it for this site (never for private tabs).
    fn add_zap(&mut self, i: usize, sel: &str) {
        if !valid_zap(sel) {
            return;
        }
        let t = &self.tabs[i];
        let Some(host) = library::host(&t.url) else { return };
        if t.ws == PRIVATE_WS {
            self.toast("hidden on this page only (private pages are never remembered)");
            return;
        }
        let list = self.zaps.entry(host.clone()).or_default();
        if !list.iter().any(|x| x == sel) {
            if list.len() >= MAX_ZAPS {
                list.remove(0);
            }
            list.push(sel.to_string());
        }
        self.save_zaps();
        self.toast(&format!("hidden on {host} for good · \"unzap\" in the palette brings it back"));
    }

    /// Accent colour of the current theme.
    fn accent(&self) -> String {
        if self.cfg.theme == "custom" {
            return self.cfg.accent.clone();
        }
        THEME_ACCENTS.iter().find(|(n, _)| *n == self.cfg.theme).map_or("#9fe3d0", |(_, c)| c).to_string()
    }

    /// The page in the focused pane, or in the glance while one is open.
    fn page(&self) -> Option<&WebView> {
        match self.glance.and_then(|g| self.idx(g)) {
            Some(i) => self.tabs[i].view.as_ref(),
            None => self.tabs.get(self.cur()).and_then(|t| t.view.as_ref()),
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
                if let Some(panel) = pages::panel(&url) {
                    return self.command(panel, &json!(true));
                }
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
                // Only when the pane under the cursor can actually split: on a full board a new
                // blank tab would just push a page out of view (and pile up hidden tabs).
                // (Judged on the unzoomed layout, which is where the new pane will go.)
                let zoom = self.zoom.take();
                let (_, r, px, py) = self.cursor_target();
                self.zoom = zoom;
                if pick_split(r, px, py).is_none() {
                    self.toast("no room for another pane here · close one (ctrl+shift+w) or zoom (ctrl+shift+f)");
                } else {
                    let id = self.push_tab(n, HOME.into(), String::new());
                    self.place_at_cursor(id);
                }
            }
            "splitdir" => {
                let url = arg[1].as_str().filter(|u| !u.trim().is_empty());
                let url = url.map(|u| resolve(u, &self.cfg.search)).unwrap_or_else(|| HOME.into());
                self.split_dir(arg[0].as_str().unwrap_or("r"), url);
            }
            "closeat" => self.close_at_cursor(),
            "hideat" => self.hide_at_cursor(),
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
            "reopen" => self.reopen(),
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
                let _ = v.go_back();
            }),
            "fwd" => self.with_active(|v| {
                let _ = v.go_forward();
            }),
            "reload" => {
                let url = self.tabs[self.cur()].url.clone();
                self.with_active(|v| {
                    let _ = if pages::is_page(&url) { v.load_html(&pages::html(&url)) } else { v.reload() };
                });
            }
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
            "overview" | "settings" | "palette" | "urlpanel" | "downloads" | "friends" => {
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
                    let _ = library::write_atomic(&self.data_dir.join("background.dataurl"), d);
                    self.cfg.bgmode = "image".into();
                    self.save_settings();
                    self.send_bg_image();
                    self.sync();
                }
            }
            "pickfolder" => {
                if let Some(dir) = slideshow::pick_folder(self.window.hwnd() as isize) {
                    self.cfg.slidedir = dir;
                    self.cfg.bgmode = "slideshow".into();
                    self.slides.clear();
                    self.save_settings();
                    self.next_slide(Instant::now());
                    self.sync();
                }
            }
            "nextslide" => self.next_slide(Instant::now()),
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
                let lq = q.trim().to_ascii_lowercase();
                let mut items: Vec<Value> = if lq.len() >= 2 && lq.get(..lq.len().min(8)).is_some_and(|p| "ultra://".starts_with(p)) || lq.starts_with("ultra:") {
                    let typed = lq.trim_start_matches("ultra:").trim_start_matches('/');
                    pages::PAGES
                        .iter()
                        .filter(|(n, _)| n.starts_with(typed))
                        .map(|(n, d)| json!({ "u": format!("ultra://{n}"), "t": d, "b": false }))
                        .collect()
                } else {
                    Vec::new()
                };
                items.extend(self.lib.suggest(&q, 8).into_iter().map(|(u, t, b)| json!({ "u": u, "t": t, "b": b })));
                items.truncate(8);
                let _ = self.ui.evaluate_script(&format!("ultra.suggestions({})", json!({ "q": q, "items": items })));
            }
            "blankat" => {
                // ctrl+shift+r: the pane under the cursor goes back to a blank tab.
                let id = self.pane_under_cursor().unwrap_or(self.focus);
                if let Some(i) = self.idx(id) {
                    self.set_focus(id);
                    self.navigate(i, HOME);
                }
            }
            "dl" => {
                // The downloads panel's buttons: [action, download id].
                let (act, id) = (arg[0].as_str().unwrap_or(""), arg[1].as_u64().unwrap_or(0));
                let op = self.dl_keys.iter().find(|(_, v)| **v == id).and_then(|(k, _)| DL_OPS.with(|m| m.borrow().get(k).cloned()));
                match act {
                    "open" => {
                        if let Some(p) = self.dls.path_of(id) {
                            downloads::open(&p);
                        }
                    }
                    "reveal" => {
                        if let Some(p) = self.dls.path_of(id) {
                            downloads::reveal(&p);
                        }
                    }
                    "folder" => downloads::open(&downloads::folder().to_string_lossy()),
                    "cancel" | "pause" | "resume" => {
                        if let Some(op) = op {
                            unsafe {
                                let _ = match act {
                                    "cancel" => op.Cancel(),
                                    "pause" => op.Pause(),
                                    _ => op.Resume(),
                                };
                            }
                        }
                    }
                    "remove" => {
                        if let Some(op) = op {
                            unsafe {
                                let _ = op.Cancel();
                            }
                        }
                        self.dls.remove(id);
                    }
                    "clear" => self.dls.clear(),
                    _ => {}
                }
                self.sync();
            }
            "makedefault" => {
                // Windows only lets the user pick the default browser: register, then open
                // its Default apps page at ULTRA.
                self.is_default = default_browser::is_default();
                if self.is_default {
                    self.toast("ULTRA is already your default browser");
                } else if std::env::current_exe().is_ok_and(|exe| default_browser::register(&exe)) {
                    default_browser::open_settings();
                    self.toast("in Windows settings, set ULTRA as your web browser");
                } else {
                    self.toast("couldn't register ULTRA with Windows");
                }
                self.sync();
            }
            "idcreate" => {
                let (name, handle) = (arg[0].as_str().unwrap_or(""), arg[1].as_str().unwrap_or(""));
                match identity::create(&self.data_dir, name, handle) {
                    Ok(me) => {
                        self.toast(&format!("welcome, {} · your ULTRA ID is ready", me.name));
                        self.me = Some(me);
                        self.start_social();
                    }
                    Err(e) => {
                        let _ = self.ui.evaluate_script(&format!("ultra.idError({})", json!(e)));
                    }
                }
                self.sync();
            }
            // Friends & chat, all by @handle.
            "sadd" => self.social.add(arg.as_str().unwrap_or("")),
            "saccept" => self.social.accept(arg.as_str().unwrap_or("")),
            "sdecline" => {
                self.social.decline(arg.as_str().unwrap_or(""));
                self.sync();
            }
            "sremove" => self.social.remove(arg.as_str().unwrap_or("")),
            "strust" => {
                if !self.social.trust(arg[0].as_str().unwrap_or(""), arg[1].as_str().unwrap_or("")) {
                    self.toast("the key changed again · compare the new safety code before trusting it");
                }
                self.sync();
            }
            "sopen" => {
                self.social.open(arg.as_str().map(String::from));
                self.sync();
            }
            "ssend" => {
                if let Err(e) = self.social.say(arg.as_str().unwrap_or("")) {
                    self.toast(&e);
                }
                self.social.save();
                self.sync();
            }
            // Delete the ULTRA ID everywhere: the relay first; this computer once it confirms.
            "iddelete" => {
                if !self.social.delete_account() {
                    self.toast("not connected to chat · connect, then try again");
                }
            }
            "idremove" => {
                identity::remove(&self.data_dir);
                self.me = None;
                self.social.stop();
                self.toast("ULTRA ID removed from this computer");
                self.sync();
            }
            // Test scripts check the crash log with a deliberate crash (testing builds only).
            "test_panic" if edition::TEST_SWITCHES => panic!("deliberate test crash"),
            // The shield: switch blocking off (or back on) for the focused page's site, and reload.
            "blocksite" => {
                let i = self.cur();
                let Some(h) = library::host(&self.tabs[i].url).filter(|_| self.tabs[i].url.starts_with("http")) else { return };
                let off = if let Some(k) = self.cfg.blockoff.iter().position(|x| *x == h) {
                    self.cfg.blockoff.remove(k);
                    false
                } else {
                    self.cfg.blockoff.push(h.clone());
                    true
                };
                *self.shields.off.borrow_mut() = self.cfg.blockoff.iter().cloned().collect();
                self.save_settings();
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.reload();
                }
                self.toast(&if off { format!("blocking off on {h} · reloaded") } else { format!("blocking back on for {h} · reloaded") });
                self.sync();
            }
            // Palette "crash log": the file in Explorer / Finder (or a note that there's none).
            "crashlog" => {
                let log = self.data_dir.join("crash.log");
                if log.exists() {
                    downloads::reveal(&log.to_string_lossy());
                } else {
                    self.toast("no crashes recorded");
                }
            }
            "update_check" => self.check_updates(false),
            "update_install" => {
                if let Some(up) = self.update.clone() {
                    let proxy = self.proxy.clone();
                    std::thread::spawn(move || {
                        let _ = proxy.send_event(UserEvent::UpdateInstalled(updater::install(&up), false));
                    });
                }
            }
            // Connect the experimental build to its private repository with a read-only token.
            "gh_connect" => {
                let ws = if self.ws == PRIVATE_WS { self.last_ws } else { self.ws };
                self.open_in(ws, updater::token_page(edition::REPO));
                self.command("settings", &json!(false));
            }
            "gh_token" => {
                let t = arg.as_str().unwrap_or("").trim().to_string();
                let st = if !updater::valid_token(&t) {
                    json!({ "s": "private", "canauth": true, "m": "that doesn't look like a GitHub token (they start with github_pat_)" })
                } else if !updater::save_token(&self.data_dir, &t) {
                    json!({ "s": "private", "canauth": true, "m": "couldn't store the token safely · nothing was saved" })
                } else {
                    json!({ "s": "checking", "m": "connected · asking GitHub…" })
                };
                let _ = self.ui.evaluate_script(&format!("ultra.updateState({st})"));
                if st["s"] == "checking" {
                    self.command("update_check", &Value::Null);
                }
            }
            "gh_forget" => {
                updater::forget_token(&self.data_dir);
                self.update = None;
                let _ = self.ui.evaluate_script(&format!("ultra.updateState({})", json!({ "s": "", "m": "GitHub disconnected · the token was removed from this computer" })));
            }
            "update_page" => {
                let ws = if self.ws == PRIVATE_WS { self.last_ws } else { self.ws };
                self.open_in(ws, updater::releases_page(edition::REPO));
                self.command("settings", &json!(false));
            }
            "scadd" => {
                // A shortcut on the new-tab page: the address given, or the page in front of you.
                let (url, title) = match arg.as_str().filter(|u| !u.trim().is_empty()) {
                    Some(u) => (resolve(u, &self.cfg.search), String::new()),
                    None => self.tabs.get(self.cur()).map(|t| (t.url.clone(), t.title.clone())).unwrap_or_default(),
                };
                let private = self.tabs.get(self.cur()).is_some_and(|t| t.ws == PRIVATE_WS) && arg.is_null();
                if private {
                    self.toast("private pages aren't added as shortcuts");
                } else if url.contains("duckduckgo.com/?q=") || url.contains("/search?q=") {
                    self.toast("that isn't a web address · try something like github.com");
                } else if self.lib.add_shortcut(&url, &title) {
                    self.lib.save(&self.data_dir);
                    self.toast(&format!("added {} to your shortcuts", library::host(&url).unwrap_or_default()));
                } else if self.lib.shortcuts.len() >= 12 {
                    self.toast("shortcuts are full · remove one first");
                } else {
                    self.toast("already in your shortcuts, or not a web page");
                }
                self.sync();
            }
            "screname" => {
                if let (Some(u), Some(t)) = (arg[0].as_str(), arg[1].as_str()) {
                    self.lib.rename_shortcut(u, t);
                    self.lib.save(&self.data_dir);
                    self.sync();
                }
            }
            "scrm" => {
                if let Some(u) = arg.as_str() {
                    self.lib.remove_shortcut(u);
                    self.lib.save(&self.data_dir);
                    self.sync();
                }
            }
            "import" | "importchrome" => {
                // Onboarding from another browser: its bookmarks land in the bookmarks bar.
                let id = if cmd == "importchrome" { "chrome" } else { arg.as_str().unwrap_or("chrome") };
                let name = library::browser_name(id);
                let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
                let msg = match self.lib.import_from(id) {
                    Ok(got) => {
                        let mut parts = Vec::new();
                        if got.bookmarks > 0 {
                            parts.push(plural(got.bookmarks, "bookmark"));
                        }
                        if got.shortcuts > 0 {
                            parts.push(plural(got.shortcuts, "shortcut"));
                        }
                        if got.history > 0 {
                            parts.push(format!("{} of history", plural(got.history, "page")));
                        }
                        // Its open tabs: asleep in the first empty workspace, so nothing loads
                        // until you look and the current layout is left alone.
                        // (Tabs already open in ULTRA, e.g. from an earlier import, aren't opened again.)
                        let mut tabs = got.tabs;
                        tabs.retain(|(_, u)| !self.tabs.iter().any(|t| &t.url == u));
                        let free = (0..PRIVATE_WS).find(|&w| w != self.ws && !self.tabs.iter().any(|t| t.ws == w) && self.stash[w].is_none());
                        if let (false, Some(w)) = (tabs.is_empty(), free) {
                            let n = tabs.len().min(40);
                            for (title, url) in tabs.into_iter().take(40) {
                                let tid = self.push_tab(self.tabs.len(), url, title);
                                if let Some(i) = self.idx(tid) {
                                    self.tabs[i].ws = w;
                                }
                            }
                            parts.push(format!("{} (in workspace {})", plural(n, "open tab"), w + 1));
                        }
                        if let Some(e) = got.search.filter(|e| *e != self.cfg.search) {
                            self.cfg.search = e.to_string();
                            self.save_settings();
                            parts.push(format!("search engine: {e}"));
                        }
                        self.lib.save(&self.data_dir);
                        if parts.is_empty() {
                            format!("nothing new to import from {name} ({})", got.label)
                        } else {
                            format!("from {name} ({}): {}", got.label, parts.join(", "))
                        }
                    }
                    Err(e) => e,
                };
                self.toast(&msg);
                let _ = self.ui.evaluate_script(&format!("ultra.imported({})", json!(msg)));
                self.sync();
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
            "coffee" => {
                if self.coffee {
                    self.coffee = false;
                    self.apply_layout(true);
                    self.focus_active();
                }
                // "buy us a coffee" opens the tip page once SUPPORT_URL is filled in.
                if arg.as_str() == Some("yes") && !SUPPORT_URL.is_empty() {
                    let ws = if self.ws == PRIVATE_WS { 0 } else { self.ws };
                    self.open_in(ws, SUPPORT_URL.to_string());
                }
            }
            "glance" => {
                let how = arg.as_str().unwrap_or("close").to_string();
                self.glance_cmd(&how);
            }
            "isolate" => {
                let w = arg.as_u64().map(|w| w as usize).unwrap_or(self.ws);
                self.toggle_isolate(w);
            }
            "reader" => {
                let js = format!("({READER_JS})({})", json!(self.accent()));
                match self.page() {
                    Some(v) => {
                        let _ = v.evaluate_script(&js);
                        let _ = v.focus();
                    }
                    None => self.toast("nothing to read here"),
                }
            }
            "pip" => match self.page() {
                Some(v) => {
                    let _ = v.evaluate_script("window.__ultraPip && window.__ultraPip()");
                }
                None => self.toast("no video here"),
            },
            "zap" => match self.page() {
                Some(v) => {
                    let _ = v.focus();
                    let _ = v.evaluate_script("window.__ultraZapPick && window.__ultraZapPick()");
                }
                None => self.toast("nothing to hide here"),
            },
            "unzap" => {
                let i = self.glance.and_then(|g| self.idx(g)).unwrap_or(cur);
                if let Some(host) = self.tabs.get(i).and_then(|t| library::host(&t.url)) {
                    if self.zaps.remove(&host).is_some() {
                        self.save_zaps();
                        if let Some(v) = &self.tabs[i].view {
                            let _ = v.evaluate_script("window.__ultraZapApply && window.__ultraZapApply([])");
                        }
                        self.toast(&format!("everything hidden on {host} is back"));
                    } else {
                        self.toast(&format!("nothing hidden on {host}"));
                    }
                }
            }
            "ready" => {
                if let Some(u) = self.pending_open.take() {
                    self.open_external(&u);
                }
                if std::mem::take(&mut self.crashed) {
                    self.toast("ULTRA closed unexpectedly last time · what happened is saved in crash.log (palette: crash log)");
                }
                // No tip page yet: no strip (its button would go nowhere).
                if !SUPPORT_URL.is_empty() && self.cfg.launches >= COFFEE_EVERY && self.cfg.launches % COFFEE_EVERY == 0 && self.coffee_at.is_none() && !self.coffee {
                    self.coffee_at = Some(Instant::now() + COFFEE_DELAY);
                }
                self.send_bg_image();
                if self.slideshow_on() {
                    self.next_slide(Instant::now());
                }
                self.flush();
                self.show_window();
                if self.cfg.tour && !self.tour_offered {
                    self.tour_offered = true;
                    self.command("tour", &json!(true));
                    // Shown once: after this it only runs from settings (or if switched back on).
                    self.cfg.tour = false;
                    self.save_settings();
                }
            }
            _ => {}
        }
    }

    /// A message from a page's webview: a shortcut caught from a real keypress (`native`), or
    /// something the page's script posted.
    fn page_msg(&mut self, slot: u64, msg: String, native: bool) {
        // Pages are untrusted: a page's script can only send the page protocol (url changes,
        // focus, type-to-search, glance links, zaps, notices), and only from a tab on screen.
        // ULTRA's shortcuts are accepted only when `native`: caught from a real keypress.
        let Some(i) = self.idx_slot(slot) else { return };
        let id = self.tabs[i].id;
        if let Some(name) = msg.strip_prefix("ultra:") {
            // A link on one of ULTRA's own pages (only those may open ultra:// addresses).
            if pages::is_page(&self.tabs[i].url) {
                if let Some(u) = pages::normalize(&format!("ultra://{name}")) {
                    self.navigate(i, &u);
                }
            }
            return;
        }
        if msg == "url" {
            // Never trust a url reported by the page itself (address bar spoofing):
            // ask WebView2 for the real one.
            if let Some(u) = self.tabs[i].view.as_ref().and_then(|v| v.url().ok()) {
                if !u.is_empty() && u != self.tabs[i].url && !(u == "about:blank" && pages::is_page(&self.tabs[i].url)) {
                    self.tabs[i].url = u;
                    self.remember(i);
                    self.sync();
                }
            }
            return;
        }
        let (i, id) = if self.glance == Some(id) {
            // Inside the glance: esc / ctrl+w close it, ctrl/cmd+enter keeps it as a split, a
            // glance-click replaces it; any other ULTRA shortcut closes it and acts on the layout.
            match msg.as_str() {
                "esc" => return self.glance_cmd("close"),
                "close" if native => return self.glance_cmd("close"),
                "keep" => return self.glance_cmd("split"),
                "reader" | "pip" | "zap" if native => return self.command(&msg, &Value::Null),
                "pip:none" | "pip:fail" | "reader:none" => return self.page_notice(&msg),
                m if m == "focus" || m.starts_with("type:") => return,
                _ => {}
            }
            if let Some(u) = msg.strip_prefix("glance:") {
                let ws = self.tabs[i].ws;
                return self.open_glance(u, ws);
            }
            if let Some(sel) = msg.strip_prefix("zap:") {
                return self.add_zap(i, sel);
            }
            if !native {
                return;
            }
            self.glance_cmd("close");
            match self.idx(self.focus) {
                Some(f) => (f, self.focus),
                None => return,
            }
        } else {
            (i, id)
        };
        if !self.shown.iter().any(|(x, _)| *x == id) {
            // A real keypress from a page that has just left the screen (keyboard focus lags a
            // tab switch or close by a moment): still honour the shortcut, on the layout.
            if native {
                self.shortcut_msg(&msg);
                self.focus_active();
            }
            return;
        }
        if id != self.focus {
            self.set_focus(id);
        }
        if let Some(u) = msg.strip_prefix("glance:") {
            let ws = self.tabs[i].ws;
            self.open_glance(u, ws);
            return;
        }
        if let Some(sel) = msg.strip_prefix("zap:") {
            self.add_zap(i, sel);
            return;
        }
        if msg == "pip:none" || msg == "pip:fail" || msg == "reader:none" {
            self.page_notice(&msg);
            return;
        }
        if native || msg.starts_with("type:") {
            self.shortcut_msg(&msg);
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
                if cmd == "update_restart" {
                    if let Some(exe) = self.pending_restart.clone() {
                        self.restart_into(&exe, flow);
                    }
                    return;
                }
                self.command(cmd, &v["arg"]);
            }
            // A shortcut caught natively in a page's webview (a real keypress), or a message the
            // page's own script posted (untrusted: only the page protocol is accepted from it).
            UserEvent::Key(slot, msg) => self.page_msg(slot, msg, true),
            UserEvent::Content(slot, msg) => self.page_msg(slot, msg, false),
            UserEvent::Title(slot, title) => {
                if let Some(i) = self.idx_slot(slot) {
                    if self.tabs[i].ws != PRIVATE_WS {
                        self.lib.retitle(&self.tabs[i].url, &title);
                    }
                    self.tabs[i].title = title;
                    if let Some(u) = self.tabs[i].view.as_ref().and_then(|v| v.url().ok()) {
                        if !u.is_empty() && u != "about:blank" {
                            self.tabs[i].url = u;
                        }
                    }
                    self.sync();
                }
            }
            UserEvent::Load(slot, started) => {
                if started {
                    self.shields.reset(slot);
                }
                if let Some(i) = self.idx_slot(slot) {
                    self.tabs[i].loading = started;
                    if self.glance == Some(self.tabs[i].id) {
                        if let Some(v) = &self.tabs[i].view {
                            let _ = v.evaluate_script("window.__ultraGlance = true;");
                        }
                    }
                    if let Some(u) = self.tabs[i].view.as_ref().and_then(|v| v.url().ok()) {
                        if !u.is_empty() && u != "about:blank" {
                            self.tabs[i].url = u;
                        }
                    }
                    if !started {
                        self.remember(i);
                        self.apply_zaps(i);
                        // New views start on ULTRA's dark backdrop (no white flash while the
                        // first page loads); after that, pages that set no background of their
                        // own get the white canvas they were designed for, as in other browsers.
                        if let Some(v) = &self.tabs[i].view {
                            let _ = v.set_background_color((255, 255, 255, 255));
                        }
                        // A navigation started from the address bar can leave keyboard focus on
                        // the bare window; once the page is in, make sure keys reach it.
                        if self.tabs[i].id == self.focus && self.win_focused {
                            self.restore_focus();
                        }
                    }
                    self.sync();
                }
            }
            UserEvent::NewTab(slot, url) => {
                // Only a page on screen (or in the glance) opens tabs, and only web pages.
                let Some(i) = self.idx_slot(slot) else { return };
                let id = self.tabs[i].id;
                let on_screen = self.shown.iter().any(|(x, _)| *x == id) || self.glance == Some(id);
                if on_screen && (url.starts_with("https://") || url.starts_with("http://")) {
                    // Links open in the workspace of the page that opened them (private stays private).
                    let ws = self.tabs[i].ws;
                    self.open_in(ws, url);
                }
            }
            UserEvent::PopupBlocked(slot) => {
                if let Some(h) = self.idx_slot(slot).and_then(|i| library::host(&self.tabs[i].url)) {
                    self.toast(&format!("blocked pop-ups from {h}"));
                }
            }
            UserEvent::Procs(p) => self.on_procs(p),
            UserEvent::PageFullscreen(slot, on) => self.on_page_fullscreen(slot, on),
            UserEvent::Social(v) => {
                match self.social.on_event(&v) {
                    social::Effect::Toast(m) => self.toast(&m),
                    social::Effect::Deleted => {
                        identity::remove(&self.data_dir);
                        self.me = None;
                        self.social.wipe();
                        self.toast("ULTRA ID deleted from the chat server and this computer");
                    }
                    social::Effect::None => {}
                }
                self.social.save();
                self.sync();
            }
            UserEvent::UpdateChecked(state, up, auto) => self.on_update_checked(state, up, auto),
            UserEvent::UpdateInstalled(Ok(exe), auto) => self.on_update_installed(exe, auto, flow),
            UserEvent::UpdateInstalled(Err(m), _) => {
                let _ = self.ui.evaluate_script(&format!("ultra.updateState({})", json!({ "s": "failed", "m": m })));
            }
            UserEvent::Open(u) => self.open_external(&u),
            UserEvent::DlStart(key, slot, url, path, total) => {
                let private = self.tabs.iter().any(|t| t.slot == slot && t.ws == PRIVATE_WS);
                let id = self.dls.start(&url, &path, total, private);
                self.dl_keys.insert(key, id);
                self.toast(&format!("downloading {} · ctrl+j for downloads", downloads::file_name(&path)));
                self.sync();
            }
            UserEvent::DlProgress(key, got, total) => {
                if let Some(&id) = self.dl_keys.get(&key) {
                    if self.dls.progress(id, got, total) {
                        self.sync();
                    }
                }
            }
            UserEvent::DlState(key, state, path) => {
                if let Some(&id) = self.dl_keys.get(&key) {
                    self.dls.set_state(id, state, Some(&path));
                    let name = self.dls.list.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or_default();
                    match state {
                        downloads::State::Done => self.toast(&format!("downloaded {name} · ctrl+j for downloads")),
                        downloads::State::Failed => self.toast(&format!("download failed: {name}")),
                        _ => {}
                    }
                    if !matches!(state, downloads::State::Active | downloads::State::Paused) {
                        self.dl_keys.remove(&key);
                        DL_OPS.with(|m| m.borrow_mut().remove(&key));
                    }
                    self.dls.save(&self.data_dir);
                    self.sync();
                }
            }
            UserEvent::Slide(data, name) => {
                self.slide_loading = false;
                if !data.is_empty() && self.slideshow_on() {
                    let _ = self.ui.evaluate_script(&format!("ultra.setSlide({}, {})", json!(data), json!(name)));
                }
            }
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
            views.push(app.build_view(slot, Some("about:blank"), rect, false, None));
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        out += &format!("build (shared_env={shared}): {ms:.1?} ms\n");
    }
    app.shared_env = true;
    let slot = app.new_slot();
    let spare = app.build_view(slot, None, rect, false, None).expect("spare");
    let pslot = app.new_slot();
    let pv = app.build_view(pslot, Some("about:blank"), rect, true, None);
    let inprivate = pv.as_ref().and_then(|v| v.webview().cast::<ICoreWebView2_13>().ok()).and_then(|w| unsafe { w.Profile().ok() }).map(|p| {
        let mut b = BOOL(0);
        unsafe {
            let _ = p.IsInPrivateModeEnabled(&mut b);
        }
        b.as_bool()
    });
    out += &format!("private webview is InPrivate: {inprivate:?}\n");
    let mut bpid = 0u32;
    unsafe {
        let _ = app.ui.webview().BrowserProcessId(&mut bpid);
    }
    let sp = spectrum::Spectrum::start(bpid);
    std::thread::sleep(Duration::from_millis(1500));
    let bands = sp.bands();
    out += &format!(
        "audio bars capture (process loopback on pid {bpid}): {}\n",
        match &bands {
            Some(b) => format!("ok, {} bands, max {:.2}", b.len(), b.iter().cloned().fold(0.0, f32::max)),
            None => "not available".into(),
        }
    );
    drop(sp);
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

/// Tell the user ULTRA couldn't start (instead of vanishing): a plain Windows message box.
fn startup_failed(msg: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let (text, title) = (HSTRING::from(msg), HSTRING::from(edition::NAME));
    unsafe {
        MessageBoxW(None, &text, &title, MB_OK | MB_ICONERROR);
    }
}

fn main() {
    // The audio-bars capture helper (see spectrum.rs): no window, no UI, just band levels.
    let args: Vec<String> = std::env::args().collect();
    if let Some(pid) = args.iter().position(|a| a == "--bars-helper").and_then(|i| args.get(i + 1)?.parse().ok()) {
        spectrum::run_helper(pid);
        return;
    }
    let bench_mode = std::env::args().any(|a| a == "--bench");
    // Just updated: let the previous ULTRA finish closing, then tidy up after it.
    updater::wait_for_previous();
    updater::clean_up();
    let data_dir = data_dir();
    let social_dir = data_dir.clone();
    let _ = fs::create_dir_all(&data_dir);
    // Any panic from here on is written to crash.log before ULTRA goes down.
    crash::install(data_dir.clone(), format!("{} {}", edition::NAME, updater::version()));
    // A link from another app (ULTRA as the default browser), or a plain second launch:
    // if ULTRA is already running, hand it over and leave.
    let link = std::env::args().skip(1).find(|a| !a.starts_with("--")).and_then(|a| default_browser::link_to_url(&a));
    let pipe = instance::pipe_name(&data_dir);
    if !bench_mode && instance::forward(&pipe, link.as_deref().unwrap_or("")) {
        return;
    }
    let cfg: Settings = fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Older settings had "bars" as a background; it is now a layer over the swirl.
    let mut cfg = if cfg.bgmode == "bars" { Settings { bgmode: "swirl".into(), bars: true, ..cfg } } else { cfg };
    if !bench_mode {
        cfg.launches = cfg.launches.saturating_add(1);
        if let Ok(j) = serde_json::to_string_pretty(&cfg) {
            let _ = fs::write(data_dir.join("settings.json"), j);
        }
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    if !bench_mode {
        let p = proxy.clone();
        instance::listen(&pipe, move |m| {
            let _ = p.send_event(UserEvent::Open(m));
        });
        if let Ok(exe) = std::env::current_exe() {
            default_browser::refresh(&exe);
        }
    }
    // Hidden until the chrome has painted, so there is no white flash on startup.
    // Put the window back where it was last time (size, spot, maximized), as long as that spot
    // is still on a screen. Given to the builder, so un-maximizing returns to the saved size.
    let saved: Option<WinState> = fs::read_to_string(data_dir.join("session.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Session>(&s).ok())
        .and_then(|s| s.window)
        .filter(|w| w.w >= 200 && w.h >= 150 && !bench_mode);
    let on_screen = saved.is_some_and(|w| {
        event_loop.available_monitors().any(|m| {
            let (p, sz) = (m.position(), m.size());
            w.x + 80 < p.x + sz.width as i32 && w.x + w.w as i32 > p.x + 80 && w.y + 20 < p.y + sz.height as i32 && w.y + 20 > p.y
        })
    });
    let mut wb = WindowBuilder::new();
    if let Some(w) = saved {
        // Position and maximized are applied when the window first shows (see show_window).
        wb = wb.with_inner_size(PhysicalSize::new(w.w, w.h));
    } else {
        wb = wb.with_inner_size(LogicalSize::new(1280.0, 800.0));
    }
    let window = wb
        .with_title(edition::NAME)
        .with_decorations(false)
        .with_visible(false)
        .with_focused(!bench_mode)
        .with_window_icon(make_icon())
        .with_min_inner_size(LogicalSize::new(480.0, 320.0))
        .build(&event_loop)
        .expect("window");

    let mut ctx = WebContext::new(Some(data_dir.join("webview")));
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    // Reopened right after closing, the web engine can still be shutting down for a moment:
    // wait for it (up to ~5 s) instead of failing.
    let mut tries = 0;
    let ui = loop {
        let pui = proxy.clone();
        let res = WebViewBuilder::new_with_web_context(&mut ctx)
            .with_html(UI_HTML)
            // Settings are known before the first paint, so ULTRASPEED never spends a frame on looks.
            .with_initialization_script(format!(
                "window.__ultraCfg = {}; window.__ultraNew = {};",
                ui_cfg(&cfg),
                json!(edition::NEW_HERE),
            ))
            .with_bounds(Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: LogicalSize::new(size.width, size.height).into(),
            })
            .with_background_color((20, 20, 20, 255))
            .with_default_context_menus(false)
            .with_additional_browser_args(browser_args())
            // The chrome UI never navigates anywhere; refuse anything that tries.
            .with_navigation_handler(|u| u.starts_with("about:") || u.starts_with("data:"))
            .with_ipc_handler(move |r| {
                let _ = pui.send_event(UserEvent::Ui(r.into_body()));
            })
            .build_as_child(&window);
        match res {
            Ok(v) => break v,
            Err(_) if tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => {
                startup_failed(&format!("ULTRA couldn't start its web engine ({e}). Try opening it again in a moment."));
                return;
            }
        }
    };
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
        shields: Rc::new(blocker::Shields::default()),
        block_on: Rc::new(Cell::new(true)),
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
        me: None,
        social: social::Social::new(&social_dir),
        page_full: None,
        update: None,
        // Every launch checks for an update (once the window is up), then every 4 hours.
        next_update_check: Instant::now() + Duration::from_secs(5),
        pending_restart: None,
        browsers: library::installed_browsers(),
        settings_save_at: None,
        start_place: None,
        last_ws: 0,
        pending_open: None,
        crashed: false,
        is_default: false,
        timing: None,
        last_blocked: 0,
        refocus_at: None,
        dls: downloads::Downloads::default(),
        dl_keys: HashMap::new(),
        meter: audio::Meter::default(),
        engine_pids: HashSet::new(),
        pids_dirty: false,
        meter_at: now,
        audio_at: now,
        audio_on: false,
        spectrum: None,
        last_sound: Instant::now(),
        slides: Vec::new(),
        slide_idx: 0,
        slide_at: now,
        slide_loading: false,
        boot_until: now,
        dirty: Cell::new(false),
        coffee_at: None,
        coffee: false,
        glance: None,
        zaps: BTreeMap::new(),
        visible: false,
        show_at: now + Duration::from_millis(900),
        next_tick: now + TICK,
    };

    if bench_mode {
        bench(&mut app);
        return;
    }

    app.start_place = saved.filter(|_| on_screen);
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
    // The ad-block engine: from its cache, or built from the bundled lists, off this thread.
    blocker::load(app.data_dir.clone());
    app.block_on.set(app.cfg.block);
    *app.shields.off.borrow_mut() = app.cfg.blockoff.iter().cloned().collect();
    // Copies of other browsers' history left by an interrupted import never linger.
    std::thread::spawn(library::clear_stale_imports);
    app.me = identity::load(&app.data_dir);
    // Friends & chat: sign in to the server in the background (only with an ULTRA ID).
    app.start_social();
    app.pending_open = link;
    app.crashed = crash::take_notice(&app.data_dir);
    app.is_default = default_browser::is_default();
    app.dls = downloads::Downloads::load(&app.data_dir);
    app.booting = app.cfg.boot && !app.cfg.ultraspeed;
    app.boot_until = Instant::now() + Duration::from_secs(4);
    app.zaps = fs::read_to_string(app.data_dir.join("zaps.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
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

    // Before the loop starts: no tao re-entry while webviews are created inside it.
    reentry::install();
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
                    if app.settings_save_at.take().is_some() {
                        app.save_settings();
                    }
                    app.save_session();
                    *flow = ControlFlow::Exit;
                }
                WindowEvent::KeyboardInput { event, .. } if event.state == tao::event::ElementState::Pressed => {
                    // Keys reach the bare window only when no webview holds focus (e.g. right
                    // after an address-bar search): still honour ULTRA's shortcuts, then hand
                    // focus back to the pane.
                    if let Some(vk) = window_vk(event.physical_key) {
                        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
                        let down = |k: i32| unsafe { GetAsyncKeyState(k) } < 0;
                        if let Some(msg) = shortcut(vk, down(0x11), down(0x10), down(0x12)) {
                            app.shortcut_msg(&msg);
                        }
                    }
                    app.restore_focus();
                }
                WindowEvent::Focused(f) => {
                    app.win_focused = f;
                    // Back from Windows' Default apps page? Settings shows the new state.
                    if f && app.is_default != default_browser::is_default() {
                        app.is_default = !app.is_default;
                        app.sync();
                    }
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
                if app.slideshow_on() && app.visible && now >= app.slide_at {
                    app.next_slide(now);
                }
                if now >= app.audio_at {
                    app.poll_audio(now);
                }
                if app.refocus_at.is_some_and(|t| now >= t) {
                    app.refocus_at = None;
                    app.restore_focus();
                }
                if app.settings_save_at.is_some_and(|t| now >= t) {
                    app.settings_save_at = None;
                    app.save_settings();
                }
                // The support strip waits until nothing else is on screen (boot, tour, panels, lock).
                if app.coffee_at.is_some_and(|t| now >= t) && !app.booting && !app.tour && !app.overlay && !app.gate() {
                    app.coffee_at = None;
                    app.coffee = true;
                    app.apply_layout(true);
                }
                if app.booting && now >= app.boot_until {
                    app.command("booted", &Value::Null);
                }
                if now >= app.lock_check_at {
                    app.lock_check_at = now + Duration::from_secs(5);
                    app.check_idle_lock();
                }
                if (app.cfg.stats || app.cfg.memcap > 0) && app.visible && !app.stats_pending && now >= app.stats_at {
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
    fn address_bar_tells_sites_from_searches() {
        let r = |s: &str| super::resolve(s, "duckduckgo");
        assert_eq!(r("example.com"), "https://example.com");
        assert_eq!(r("news.bbc.co.uk/sport?x=1"), "https://news.bbc.co.uk/sport?x=1");
        assert_eq!(r("localhost:3000/app"), "http://localhost:3000/app");
        assert_eq!(r("127.0.0.1:8770/p1.html"), "http://127.0.0.1:8770/p1.html");
        assert_eq!(r("192.168.1.1"), "https://192.168.1.1");
        assert_eq!(r("example.com:8080"), "https://example.com:8080");
        assert_eq!(r("münchen.de"), "https://münchen.de");
        for q in ["3.14", "note:meeting", "e.g.", "v1.2", "what is rust", "c++", "a.b"] {
            assert!(r(q).starts_with("https://duckduckgo.com/?q="), "{q} -> {}", r(q));
        }
    }

    #[test]
    fn shortcuts() {
        let k = |c: char| c as u32;
        assert_eq!(shortcut(k('T'), true, false, false).as_deref(), Some("new"));
        assert_eq!(shortcut(k('W'), true, false, false).as_deref(), Some("close"));
        assert_eq!(shortcut(k('W'), true, true, false).as_deref(), Some("closeat"));
        assert_eq!(shortcut(k('Q'), true, true, false).as_deref(), Some("hideat"));
        assert_eq!(shortcut(k('3'), true, false, false).as_deref(), Some("tab:3"));
        assert_eq!(shortcut(k('3'), true, true, false).as_deref(), Some("mv:3"));
        assert_eq!(shortcut(k('3'), true, false, true).as_deref(), Some("ws:3"));
        assert_eq!(shortcut(k('3'), true, true, true).as_deref(), Some("tows:3"));
        assert_eq!(shortcut(k('0'), true, false, true).as_deref(), Some("ws:0"));
        assert_eq!(shortcut(0x27, true, false, true).as_deref(), Some("dir:r"));
        assert_eq!(shortcut(0x09, true, true, false).as_deref(), Some("prev"));
        assert_eq!(shortcut(0xBC, true, false, false).as_deref(), Some("settings"));
        assert_eq!(shortcut(0x78, false, false, false).as_deref(), Some("reader"));
        // ctrl+alt+p is left to the page script (it needs the key press as a user gesture).
        assert_eq!(shortcut(k('P'), true, false, true), None);
        assert_eq!(shortcut(k('R'), true, true, false).as_deref(), Some("blankat"));
        assert_eq!(shortcut(k('F'), true, false, false).as_deref(), Some("focusurl"));
        assert_eq!(shortcut(k('J'), true, false, false).as_deref(), Some("downloads"));
        // ctrl+r reloads the tab (ULTRA's own, so it works whatever the page does with keys).
        assert_eq!(shortcut(k('R'), true, false, false).as_deref(), Some("reload"));
        // Not ours: plain keys and ctrl+c pass through.
        assert_eq!(shortcut(k('T'), false, false, false), None);
        assert_eq!(shortcut(k('C'), true, false, false), None);
    }
}
