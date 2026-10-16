//! Ad & tracker blocking with WebKit content rules: the domain list is compiled once into a
//! WKContentRuleList (WebKit matches it natively, off the main thread, so ordinary page loads
//! pay nothing) and attached to each page while blocking is on. Only subresources are
//! blocked (scripts, images, frames' contents, XHR/fetch, media); documents (pages and
//! frames) never are, so clicking a sponsored link that redirects through an ad domain still works.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use block2::RcBlock;
use objc2::{
    ffi,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, Imp, Sel},
    sel, MainThreadMarker,
};
use objc2_foundation::{NSError, NSString};
use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore};
use wry::{WebView, WebViewExtMacOS};

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

/// Everything except "document" (pages and frames) and "popup" (handled as new tabs).
const RESOURCE_TYPES: &[&str] = &["image", "style-sheet", "script", "font", "raw", "svg-document", "media"];
const LIST_ID: &str = "ultra-blocker";

thread_local! {
    static LIST: RefCell<Option<Retained<WKContentRuleList>>> = const { RefCell::new(None) };
    static BLOCKED: RefCell<Option<Rc<Cell<u64>>>> = const { RefCell::new(None) };
}

/// The rule list as WebKit's JSON: one rule per domain, matching it and all its subdomains.
pub fn rules_json() -> String {
    let rules: Vec<serde_json::Value> = DOMAINS
        .iter()
        .map(|d| {
            serde_json::json!({
                "trigger": {
                    "url-filter": format!(r"^[a-z]+://([a-z0-9._-]+\.)?{}[:/]", d.replace('.', r"\.")),
                    "resource-type": RESOURCE_TYPES,
                },
                "action": { "type": "block" },
            })
        })
        .collect();
    serde_json::Value::Array(rules).to_string()
}

/// Compile the rules (once per launch, a few ms on WebKit's side); `ready` runs on the main
/// thread when the list can be attached.
pub fn compile(ready: impl Fn() + 'static) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(mtm) }) else { return };
    let done = RcBlock::new(move |list: *mut WKContentRuleList, err: *mut NSError| {
        if !err.is_null() {
            let e = unsafe { &*err };
            eprintln!("ultra: blocker rules failed to compile: {}", e.localizedDescription());
            return;
        }
        // SAFETY: WebKit hands us a valid list when there is no error.
        if let Some(list) = unsafe { Retained::retain(list) } {
            LIST.with(|l| *l.borrow_mut() = Some(list));
            ready();
        }
    });
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(&NSString::from_str(LIST_ID)),
            Some(&NSString::from_str(&rules_json())),
            Some(&done),
        );
    }
}

/// Attach or detach the rules on one page. Before the list is compiled this does nothing;
/// `compile`'s callback re-applies it everywhere.
pub fn set_filters(v: &WebView, on: bool) {
    LIST.with(|l| {
        if let Some(list) = l.borrow().as_ref() {
            let m = v.manager();
            unsafe {
                // Remove first so turning it on twice never stacks two copies.
                m.removeContentRuleList(list);
                if on {
                    m.addContentRuleList(list);
                }
            }
        }
    });
}

/// WebKit reports each rule it applied to `_webView:contentRuleListWithIdentifier:performedAction:forURL:`
/// on the navigation delegate (WKNavigationDelegatePrivate, macOS 11+). Teach wry's delegate
/// class that method so blocked requests are counted for the daily digest, as on Windows.
/// Call after the first webview exists (the class is registered then) and before any page:
/// WebKit checks which methods a delegate has when the delegate is set.
pub fn install_counter(any_view: &WebView, blocked: Rc<Cell<u64>>) {
    BLOCKED.with(|b| *b.borrow_mut() = Some(blocked));
    let Some(delegate) = (unsafe { any_view.webview().navigationDelegate() }) else { return };
    let obj: &AnyObject = delegate.as_ref();
    let cls: *const AnyClass = obj.class();
    let name = sel!(_webView:contentRuleListWithIdentifier:performedAction:forURL:);
    unsafe extern "C-unwind" fn performed(_this: *mut AnyObject, _cmd: Sel, _wv: *mut AnyObject, _id: *mut AnyObject, action: *mut AnyObject, _url: *mut AnyObject) {
        if action.is_null() {
            return;
        }
        let action = unsafe { &*action };
        if !action.class().responds_to(sel!(blockedLoad)) {
            return;
        }
        let blocked: Bool = unsafe { objc2::msg_send![action, blockedLoad] };
        if blocked.as_bool() {
            BLOCKED.with(|b| {
                if let Some(c) = b.borrow().as_ref() {
                    c.set(c.get() + 1);
                }
            });
        }
    }
    type Method = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject, *mut AnyObject);
    unsafe {
        let imp: Imp = std::mem::transmute::<Method, Imp>(performed);
        // Fails harmlessly (returns NO) if the method is already there.
        let _ = ffi::class_addMethod(cls as *mut AnyClass, name, imp, c"v@:@@@@".as_ptr());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rules_are_valid_json_and_skip_documents() {
        let v: serde_json::Value = serde_json::from_str(&super::rules_json()).unwrap();
        let rules = v.as_array().unwrap();
        assert_eq!(rules.len(), super::DOMAINS.len());
        let f = rules[0]["trigger"]["url-filter"].as_str().unwrap();
        assert_eq!(f, r"^[a-z]+://([a-z0-9._-]+\.)?doubleclick\.net[:/]");
        assert!(!rules[0]["trigger"]["resource-type"].as_array().unwrap().iter().any(|t| t == "document"));
    }
}
