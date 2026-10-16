//! Ad & tracker blocking. Each blocked domain is registered as a WebView2 request
//! filter, so only matching requests ever cross into ULTRA; ordinary page loads
//! pay nothing. Matching requests are answered with an empty 403.

use std::{cell::Cell, rc::Rc};

use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_22, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
    },
    WebResourceRequestedEventHandler,
};
use windows::Win32::System::Com::IStream;
use windows_core::{Interface, HSTRING};
use wry::{WebView, WebViewExtWindows};

/// Ad networks and trackers. Sign-in providers (accounts.google.com,
/// connect.facebook.net, appleid…) are deliberately absent so logins keep working.
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

/// Attach the (always-on) handler; filters decide whether anything reaches it.
pub fn install(v: &WebView, blocked: Rc<Cell<u64>>) {
    let env = v.environment();
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
        if let Some(args) = args {
            unsafe {
                let resp = env.CreateWebResourceResponse(
                    None::<&IStream>,
                    403,
                    &HSTRING::from("Blocked by ULTRA"),
                    &HSTRING::new(),
                )?;
                args.SetResponse(&resp)?;
            }
            blocked.set(blocked.get() + 1);
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe {
        let _ = v.webview().add_WebResourceRequested(&handler, &mut token);
    }
}

/// Add or remove the per-domain filters (covers iframes and workers where supported).
pub fn set_filters(v: &WebView, on: bool) {
    let wv = v.webview();
    let wv22 = wv.cast::<ICoreWebView2_22>().ok();
    let (ctx, kinds) = (COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL);
    for d in DOMAINS {
        // "*domain/*" covers the domain and all its subdomains with one filter.
        let p = HSTRING::from(format!("*{d}/*"));
        unsafe {
            let _ = match (&wv22, on) {
                (Some(w), true) => w.AddWebResourceRequestedFilterWithRequestSourceKinds(&p, ctx, kinds),
                (Some(w), false) => w.RemoveWebResourceRequestedFilterWithRequestSourceKinds(&p, ctx, kinds),
                (None, true) => wv.AddWebResourceRequestedFilter(&p, ctx),
                (None, false) => wv.RemoveWebResourceRequestedFilter(&p, ctx),
            };
        }
    }
}
