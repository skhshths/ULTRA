//! Ad & tracker blocking, with Brave's filter engine (the `adblock` crate) and the same lists
//! uBlock Origin starts with: EasyList, EasyPrivacy, uBlock filters and privacy, Peter Lowe's.
//! The lists ship inside the exe (packed by build.rs); the engine is built from them off the UI
//! thread the first time and cached in the data folder, so later launches load it in a moment.
//!
//! - Every request a page makes (scripts, images, XHR, beacons…) is checked; blocked ones get
//!   an empty 403. Until the engine is ready, a small built-in list of ad networks covers it.
//! - Frames (where most ads live) are checked as they start navigating and cancelled.
//! - The page you navigate to yourself is never blocked, so a sponsored link still opens.
//! - Each site's own hide rules ("##.ad-slot") are applied when its page loads, so blocked ads
//!   don't leave empty boxes.
//! - Edge's own tracking prevention stays on its default (Balanced): set to Strict it blocks the
//!   big trackers before ULTRA sees them, so the shield couldn't count them; the lists cover them.
//! - Blocking can be switched off per site (the shield in the address bar).

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    sync::OnceLock,
};

use adblock::{lists::ParseOptions, request::Request, Engine, FilterSet};
use webview2_com::{
    take_pwstr, DOMContentLoadedEventHandler,
    Microsoft::Web::WebView2::Win32::*,
    NavigationStartingEventHandler, WebResourceRequestedEventHandler,
};
use windows::Win32::System::Com::IStream;
use windows_core::{Interface, HSTRING, PWSTR};
use wry::{WebView, WebViewExtWindows};

/// The bundled filter lists (see build.rs) and their fingerprint.
const LISTS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/filters.bin"));
const LISTS_HASH: &str = env!("ULTRA_FILTERS_HASH");
const CACHE: &str = "adblock.dat";

static ENGINE: OnceLock<Engine> = OnceLock::new();
static RULES: OnceLock<usize> = OnceLock::new();

/// What blocking has done, shared by every webview's handlers (all on the UI thread).
#[derive(Default)]
pub struct Shields {
    /// Everything blocked since ULTRA started.
    pub total: Cell<u64>,
    /// Blocked on the page each webview (by slot) is showing now.
    pub per_page: RefCell<HashMap<u64, u32>>,
    /// Sites (host names) where blocking is switched off.
    pub off: RefCell<HashSet<String>>,
}

impl Shields {
    fn count(&self, slot: u64) {
        self.total.set(self.total.get() + 1);
        *self.per_page.borrow_mut().entry(slot).or_default() += 1;
    }
    /// A new page in this webview: its count starts over.
    pub fn reset(&self, slot: u64) {
        self.per_page.borrow_mut().remove(&slot);
    }
    pub fn on_page(&self, slot: u64) -> u32 {
        self.per_page.borrow().get(&slot).copied().unwrap_or(0)
    }
    fn allowed(&self, page: &str) -> bool {
        crate::library::host(page).is_some_and(|h| self.off.borrow().contains(&h))
    }
}

/// Unpack the bundled lists: (name, text) pairs.
fn lists() -> Vec<(String, String)> {
    let Ok(raw) = miniz_oxide::inflate::decompress_to_vec(LISTS) else { return Vec::new() };
    let mut out = Vec::new();
    let mut rest = &raw[..];
    while !rest.is_empty() {
        let mut header = rest.splitn(3, |&b| b == b'\n');
        let (Some(name), Some(len), Some(body)) = (header.next(), header.next(), header.next()) else { break };
        let len: usize = std::str::from_utf8(len).ok().and_then(|l| l.parse().ok()).unwrap_or(0);
        let len = len.min(body.len());
        out.push((String::from_utf8_lossy(name).to_string(), String::from_utf8_lossy(&body[..len]).to_string()));
        rest = &body[len..];
    }
    out
}

/// How many rules the bundled lists hold (counted once, while the engine loads).
pub fn rules() -> usize {
    RULES.get().copied().unwrap_or(0)
}

fn rule_count() -> usize {
    lists().iter().map(|(_, t)| t.lines().filter(|l| !l.is_empty() && !l.starts_with('!') && !l.starts_with('[')).count()).sum()
}

fn build() -> Engine {
    let mut set = FilterSet::new(false);
    for (_, text) in lists() {
        set.add_filter_list(text, ParseOptions::default());
    }
    Engine::new_with_filter_set(set)
}

fn cached(dir: &Path) -> Option<Engine> {
    let mut f = std::fs::File::open(dir.join(CACHE)).ok()?;
    let mut tag = [0u8; 16];
    f.read_exact(&mut tag).ok()?;
    if tag != LISTS_HASH.as_bytes()[..16] {
        return None;
    }
    let mut data = Vec::new();
    f.read_to_end(&mut data).ok()?;
    let mut e = Engine::default();
    e.deserialize(&data).ok()?;
    Some(e)
}

/// Prepare the engine in the background: from the cache, or built from the lists (then cached).
pub fn load(dir: PathBuf) {
    std::thread::spawn(move || {
        let engine = cached(&dir).unwrap_or_else(|| {
            let e = build();
            let mut data = LISTS_HASH.as_bytes()[..16].to_vec();
            data.extend(e.serialize());
            let _ = crate::library::write_atomic(&dir.join(CACHE), data);
            e
        });
        let _ = ENGINE.set(engine);
        let _ = RULES.set(rule_count());
    });
}

pub fn ready() -> bool {
    ENGINE.get().is_some()
}

/// Should this request be blocked? `kind` is the adblock request type ("script", "image"…).
fn should_block(url: &str, page: &str, kind: &str) -> bool {
    match ENGINE.get() {
        Some(e) => Request::new(url, page, kind, "GET").is_ok_and(|r| e.check_network_request(&r).should_block()),
        // Not loaded yet: the built-in ad networks, third-party requests only.
        None => crate::library::host(url).is_some_and(|h| {
            DOMAINS.iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))
                && crate::library::host(page).is_none_or(|p| p != h)
        }),
    }
}

/// CSS hiding this page's ad slots (the lists' rules for its site), one rule per selector so
/// one selector Chromium doesn't understand can't cancel the rest.
fn hide_css(page: &str) -> String {
    let Some(e) = ENGINE.get() else { return String::new() };
    let r = e.url_cosmetic_resources(page);
    r.hide_selectors.iter().map(|s| format!("{s}{{display:none!important}}\n")).collect()
}

fn request_kind(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> &'static str {
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => "document",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "stylesheet",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "image",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => "xmlhttprequest",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => "fetch",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => "websocket",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => "ping",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT => "csp_report",
        _ => "other",
    }
}

fn source(wv: &ICoreWebView2) -> String {
    let mut p = PWSTR::null();
    unsafe {
        if wv.Source(&mut p).is_ok() {
            return take_pwstr(p);
        }
    }
    String::new()
}

/// Hook a webview up to the blocker. `slot` names it for the per-page count.
pub fn install(v: &WebView, slot: u64, shields: Rc<Shields>, enabled: Rc<Cell<bool>>) {
    let wv = v.webview();
    let env = v.environment();

    // Every subresource request.
    let (sh, on) = (shields.clone(), enabled.clone());
    let requests = WebResourceRequestedEventHandler::create(Box::new(move |sender, args| {
        let (Some(sender), Some(args)) = (sender, args) else { return Ok(()) };
        if !on.get() {
            return Ok(());
        }
        unsafe {
            let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
            args.ResourceContext(&mut ctx)?;
            if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
                return Ok(()); // pages and frames: frames are judged as they navigate (below)
            }
            let mut uri = PWSTR::null();
            args.Request()?.Uri(&mut uri)?;
            let url = take_pwstr(uri);
            let page = source(&sender);
            if sh.allowed(&page) || !should_block(&url, &page, request_kind(ctx)) {
                return Ok(());
            }
            let resp = env.CreateWebResourceResponse(None::<&IStream>, 403, &HSTRING::from("Blocked by ULTRA"), &HSTRING::new())?;
            args.SetResponse(&resp)?;
        }
        sh.count(slot);
        Ok(())
    }));
    // Frames: ad iframes are cancelled before they load anything.
    let (sh, on) = (shields.clone(), enabled.clone());
    let frames = NavigationStartingEventHandler::create(Box::new(move |sender, args| {
        let (Some(sender), Some(args)) = (sender, args) else { return Ok(()) };
        if !on.get() {
            return Ok(());
        }
        unsafe {
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            let url = take_pwstr(uri);
            let page = source(&sender);
            if url.starts_with("http") && !sh.allowed(&page) && should_block(&url, &page, "subdocument") {
                args.SetCancel(true)?;
                sh.count(slot);
            }
        }
        Ok(())
    }));
    // Each site's hide rules once its document is ready.
    let (sh, on) = (shields, enabled);
    let hide = DOMContentLoadedEventHandler::create(Box::new(move |sender, _| {
        let Some(sender) = sender else { return Ok(()) };
        let page = source(&sender);
        if !on.get() || sh.allowed(&page) {
            return Ok(());
        }
        let css = hide_css(&page);
        if !css.is_empty() {
            let js = format!(
                "(() => {{ const s = document.createElement('style'); s.dataset.ultra = 'hide'; s.textContent = {}; (document.head || document.documentElement).appendChild(s); }})()",
                serde_json::to_string(&css).unwrap_or_default()
            );
            unsafe {
                let _ = sender.ExecuteScript(&HSTRING::from(js), None);
            }
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = wv.add_WebResourceRequested(&requests, &mut token);
        let _ = wv.add_FrameNavigationStarting(&frames, &mut token);
        if let Ok(w2) = wv.cast::<ICoreWebView2_2>() {
            let _ = w2.add_DOMContentLoaded(&hide, &mut token);
        }
    }
}

/// Route every subresource request through the blocker (or stop).
pub fn set_filters(v: &WebView, on: bool) {
    let wv = v.webview();
    let all = HSTRING::from("*");
    unsafe {
        let _ = match (wv.cast::<ICoreWebView2_22>(), on) {
            (Ok(w), true) => w.AddWebResourceRequestedFilterWithRequestSourceKinds(&all, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL),
            (Ok(w), false) => w.RemoveWebResourceRequestedFilterWithRequestSourceKinds(&all, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL),
            (Err(_), true) => wv.AddWebResourceRequestedFilter(&all, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL),
            (Err(_), false) => wv.RemoveWebResourceRequestedFilter(&all, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL),
        };
    }
}

/// Fallback until the engine is ready: well-known ad networks and trackers. Sign-in providers
/// (accounts.google.com, connect.facebook.net, appleid…) are deliberately absent.
pub const DOMAINS: &[&str] = &[
    // ad networks / exchanges
    "doubleclick.net", "googlesyndication.com", "googleadservices.com", "adservice.google.com",
    "googletagservices.com", "2mdn.net", "amazon-adsystem.com", "adnxs.com", "adsrvr.org",
    "rubiconproject.com", "pubmatic.com", "openx.net", "criteo.com", "criteo.net", "casalemedia.com",
    "advertising.com", "adform.net", "smartadserver.com", "yieldmo.com", "sharethrough.com", "teads.tv",
    "33across.com", "bidswitch.net", "contextweb.com", "lijit.com", "media.net", "zedo.com",
    "revcontent.com", "mgid.com", "taboola.com", "outbrain.com", "popads.net", "propellerads.com",
    "adcolony.com", "applovin.com", "serving-sys.com", "adroll.com", "tradedoubler.com",
    "ads.linkedin.com", "ads-twitter.com", "static.ads-twitter.com", "an.yandex.ru",
    "adsafeprotected.com", "doubleverify.com", "moatads.com",
    // trackers / analytics beacons
    "google-analytics.com", "googletagmanager.com", "scorecardresearch.com", "quantserve.com",
    "hotjar.com", "clarity.ms", "bat.bing.com", "analytics.twitter.com", "demdex.net",
    "everesttech.net", "krxd.net", "bluekai.com", "exelator.com", "rlcdn.com", "agkn.com",
    "tapad.com", "mc.yandex.ru", "chartbeat.com", "mixpanel.com", "fullstory.com",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_lists_block_ads_and_spare_pages() {
        let names: Vec<String> = lists().into_iter().map(|(n, _)| n).collect();
        assert!(names.iter().any(|n| n == "easylist") && names.iter().any(|n| n == "easyprivacy"), "{names:?}");
        assert!(rule_count() > 50_000);
        let _ = ENGINE.set(build());
        let page = "https://www.example-news.com/story";
        assert!(should_block("https://securepubads.g.doubleclick.net/tag/js/gpt.js", page, "script"));
        assert!(should_block("https://www.google-analytics.com/analytics.js", page, "script"));
        assert!(should_block("https://static.doubleclick.net/instream/ad_status.js", page, "script"));
        assert!(!should_block("https://cdn.jsdelivr.net/npm/react@18.3.1/umd/react.production.min.js", page, "script"));
        assert!(!should_block("https://www.example-news.com/main.css", page, "stylesheet"));
        // engine round-trips through the cache format
        let data = ENGINE.get().unwrap().serialize();
        let mut e = Engine::default();
        assert!(e.deserialize(&data).is_ok());
    }
}
