//! Ad & tracker blocking with WebKit content rules, from the same lists as Windows (EasyList,
//! EasyPrivacy, uBlock filters and privacy, Peter Lowe's): build.rs converts them with Brave's
//! converter into WebKit rules, split into lists of at most 45,000 rules (the limit on macOS
//! 11 and 12). WebKit matches them natively, off the main thread, so page loads pay nothing.
//!
//! - Network rules block subresources and frames; the page you navigate to yourself is never
//!   blocked, so a sponsored link still opens.
//! - Cosmetic rules become WebKit's own "css-display-none", so blocked ads leave no empty boxes.
//! - Lists are compiled once and kept in WebKit's rule store under an ID that changes with the
//!   lists, so later launches just look them up.
//! - Blocking can be switched off per site (the shield in the address bar).

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

use block2::RcBlock;
use objc2::{
    ffi,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, Imp, Sel},
    sel, MainThreadMarker,
};
use objc2_foundation::{NSArray, NSError, NSString};
use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore};
use wry::{WebView, WebViewExtMacOS};

/// The converted rule lists (see build.rs) and their fingerprint.
const RULES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rules.bin"));
const RULES_HASH: &str = env!("ULTRA_RULES_HASH");
/// How many rules the lists hold (for settings).
pub const RULE_COUNT: usize = match usize::from_str_radix(env!("ULTRA_RULES_COUNT"), 10) {
    Ok(n) => n,
    Err(_) => 0,
};
const ID_PREFIX: &str = "ultra-blocker";

thread_local! {
    static LISTS: RefCell<Vec<Retained<WKContentRuleList>>> = const { RefCell::new(Vec::new()) };
    static SHIELDS: RefCell<Option<Rc<Shields>>> = const { RefCell::new(None) };
}

/// What blocking has done, shared with the app (all on the main thread).
#[derive(Default)]
pub struct Shields {
    /// Everything blocked since ULTRA started.
    pub total: Cell<u64>,
    /// Blocked on the page each webview (by its pointer identity, see `macos::id`) shows now.
    pub per_page: RefCell<HashMap<usize, u32>>,
    /// Sites (host names) where blocking is switched off.
    pub off: RefCell<HashSet<String>>,
}

impl Shields {
    pub fn reset(&self, view: usize) {
        self.per_page.borrow_mut().remove(&view);
    }
    pub fn on_page(&self, view: usize) -> u32 {
        self.per_page.borrow().get(&view).copied().unwrap_or(0)
    }
    pub fn allowed(&self, page: &str) -> bool {
        crate::library::host(page).is_some_and(|h| self.off.borrow().contains(&h))
    }
}

/// The rule lists as WebKit JSON, unpacked from the build.
fn chunks() -> Vec<String> {
    let Ok(raw) = miniz_oxide::inflate::decompress_to_vec(RULES) else { return Vec::new() };
    let mut out = Vec::new();
    let mut rest = &raw[..];
    while let Some(nl) = rest.iter().position(|&b| b == b'\n') {
        let len: usize = std::str::from_utf8(&rest[..nl]).ok().and_then(|l| l.parse().ok()).unwrap_or(0);
        let body = &rest[nl + 1..];
        let len = len.min(body.len());
        out.push(String::from_utf8_lossy(&body[..len]).to_string());
        rest = &body[len..];
    }
    out
}

pub fn ready() -> bool {
    LISTS.with(|l| !l.borrow().is_empty())
}

/// Get every rule list ready: looked up in WebKit's store, or compiled there the first time
/// (a few seconds, once). `ready` runs on the main thread when all of them can be attached.
/// Lists from older builds are removed from the store.
pub fn compile(ready: impl Fn() + 'static) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(mtm) }) else { return };
    let jsons = chunks();
    let n = jsons.len();
    if n == 0 {
        return;
    }
    let ids: Vec<String> = (0..n).map(|i| format!("{ID_PREFIX}-{RULES_HASH}-{i}")).collect();
    // Old lists (other fingerprints) only take up space.
    {
        let keep: HashSet<String> = ids.iter().cloned().collect();
        let store2 = store.clone();
        let tidy = RcBlock::new(move |found: *mut NSArray<NSString>| {
            let Some(found) = (unsafe { found.as_ref() }) else { return };
            for id in found.iter() {
                let id = id.to_string();
                if id.starts_with(ID_PREFIX) && !keep.contains(&id) {
                    let done = RcBlock::new(|_: *mut NSError| {});
                    unsafe { store2.removeContentRuleListForIdentifier_completionHandler(Some(&NSString::from_str(&id)), Some(&done)) };
                }
            }
        });
        unsafe { store.getAvailableContentRuleListIdentifiers(Some(&tidy)) };
    }
    let got: Rc<RefCell<Vec<Option<Retained<WKContentRuleList>>>>> = Rc::new(RefCell::new(vec![None; n]));
    let left = Rc::new(Cell::new(n));
    let ready = Rc::new(ready);
    for (i, (id, json)) in ids.into_iter().zip(jsons).enumerate() {
        let (got, left, ready, store2) = (got.clone(), left.clone(), ready.clone(), store.clone());
        let finish = Rc::new(move |list: Option<Retained<WKContentRuleList>>| {
            got.borrow_mut()[i] = list;
            left.set(left.get() - 1);
            if left.get() == 0 {
                let lists: Vec<_> = got.borrow_mut().drain(..).flatten().collect();
                LISTS.with(|l| *l.borrow_mut() = lists);
                ready();
            }
        });
        let id_ns = NSString::from_str(&id);
        let id_ns2 = id_ns.clone();
        let looked = RcBlock::new(move |list: *mut WKContentRuleList, _err: *mut NSError| {
            if let Some(list) = unsafe { Retained::retain(list) } {
                return finish(Some(list));
            }
            // Not in the store yet: compile it (WebKit keeps it for next time).
            let finish = finish.clone();
            let compiled = RcBlock::new(move |list: *mut WKContentRuleList, err: *mut NSError| {
                if !err.is_null() {
                    let e = unsafe { &*err };
                    eprintln!("ultra: blocker rules failed to compile: {}", e.localizedDescription());
                }
                finish(unsafe { Retained::retain(list) });
            });
            unsafe {
                store2.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                    Some(&id_ns2),
                    Some(&NSString::from_str(&json)),
                    Some(&compiled),
                );
            }
        });
        unsafe { store.lookUpContentRuleListForIdentifier_completionHandler(Some(&id_ns), Some(&looked)) };
    }
}

/// Attach or detach the rules on one page. Before the lists are ready this does nothing;
/// `compile`'s callback re-applies them everywhere.
pub fn set_filters(v: &WebView, on: bool) {
    LISTS.with(|l| {
        let m = v.manager();
        for list in l.borrow().iter() {
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
/// class that method so blocked requests are counted, per page and in total.
/// Call after the first webview exists (the class is registered then) and before any page:
/// WebKit checks which methods a delegate has when the delegate is set.
pub fn install_counter(any_view: &WebView, shields: Rc<Shields>) {
    SHIELDS.with(|s| *s.borrow_mut() = Some(shields));
    let Some(delegate) = (unsafe { any_view.webview().navigationDelegate() }) else { return };
    let obj: &AnyObject = delegate.as_ref();
    let cls: *const AnyClass = obj.class();
    let name = sel!(_webView:contentRuleListWithIdentifier:performedAction:forURL:);
    unsafe extern "C-unwind" fn performed(_this: *mut AnyObject, _cmd: Sel, wv: *mut AnyObject, _id: *mut AnyObject, action: *mut AnyObject, _url: *mut AnyObject) {
        if action.is_null() {
            return;
        }
        let action = unsafe { &*action };
        if !action.class().responds_to(sel!(blockedLoad)) {
            return;
        }
        let blocked: Bool = unsafe { objc2::msg_send![action, blockedLoad] };
        if blocked.as_bool() {
            SHIELDS.with(|s| {
                if let Some(s) = s.borrow().as_ref() {
                    s.total.set(s.total.get() + 1);
                    *s.per_page.borrow_mut().entry(wv as usize).or_default() += 1;
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
    fn bundled_rules_unpack_as_webkit_json() {
        let chunks = super::chunks();
        assert!(chunks.len() >= 2, "{} chunks", chunks.len());
        for c in &chunks {
            let v: serde_json::Value = serde_json::from_str(c).unwrap();
            let rules = v.as_array().unwrap();
            assert!(rules.len() <= 45_000);
            // every list ends with the exceptions, so they apply to its own rules
            assert_eq!(rules.last().unwrap()["action"]["type"], "ignore-previous-rules");
        }
        assert!(super::RULE_COUNT > 100_000);
    }
}
