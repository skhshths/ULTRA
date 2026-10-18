//! macOS glue: what WebView2 gave ULTRA for free on Windows and WKWebView does not.
//!
//! - Shortcuts in every frame: one local NSEvent monitor sees each key before any page
//!   (WebView2's AcceleratorKeyPressed did this per webview).
//! - Browser accelerators WebView2 handles inside pages (reload, back/forward, page zoom,
//!   print, devtools, mouse back/forward buttons) are routed to the pane by hand.
//! - An application menu, without which ⌘C/⌘V/⌘Z/⌘A do nothing in a WKWebView.
//! - Probes for "is this page playing audio" and "which process renders it".

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObject, Sel},
    sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuItem, NSResponder,
    NSView, NSWindow,
};
use objc2_core_graphics::CGColor;
use objc2_foundation::{NSBundle, NSData, NSObjectProtocol, NSString};
use objc2_web_kit::{WKUserScript, WKUserScriptInjectionTime, WKWebView};
use wry::{WebView, WebViewExtMacOS};

/// Windows virtual-key codes, so the shortcut table in main.rs is shared with the Windows build.
pub const VK_TAB: u32 = 0x09;
const VK_ESC: u32 = 0x1B;
const VK_F5: u32 = 0x74;
const VK_F12: u32 = 0x7B;
const VK_PLUS: u32 = 0xBB;
const VK_MINUS: u32 = 0xBD;
const VK_COMMA: u32 = 0xBC;
const VK_LBRACKET: u32 = 0xDB;
const VK_RBRACKET: u32 = 0xDD;

/// Page background shown before a page paints and behind pages that set none
/// (WebView2's DefaultBackgroundColor on Windows): #141414.
const BACKDROP: (f64, f64, f64) = (20.0 / 255.0, 20.0 / 255.0, 20.0 / 255.0);

/// Pointer identity of a WKWebView, used to tell which pane an event belongs to.
pub fn id(v: &WebView) -> usize {
    Retained::as_ptr(&v.webview()) as *const WKWebView as usize
}

/// macOS key code -> Windows virtual-key code, for the keys ULTRA's shortcuts use.
/// Letters follow the active keyboard layout (like Windows VKs); everything else is positional.
fn vk(e: &NSEvent) -> Option<u32> {
    let code = e.keyCode();
    let digit = |n: u32| Some(0x30 + n);
    let pad = |n: u32| Some(0x60 + n);
    let positional = match code {
        0x12 => digit(1),
        0x13 => digit(2),
        0x14 => digit(3),
        0x15 => digit(4),
        0x17 => digit(5),
        0x16 => digit(6),
        0x1A => digit(7),
        0x1C => digit(8),
        0x19 => digit(9),
        0x1D => digit(0),
        0x52 => pad(0),
        0x53 => pad(1),
        0x54 => pad(2),
        0x55 => pad(3),
        0x56 => pad(4),
        0x57 => pad(5),
        0x58 => pad(6),
        0x59 => pad(7),
        0x5B => pad(8),
        0x5C => pad(9),
        0x7B => Some(0x25), // left
        0x7E => Some(0x26), // up
        0x7C => Some(0x27), // right
        0x7D => Some(0x28), // down
        0x30 => Some(VK_TAB),
        0x35 => Some(VK_ESC),
        0x2B => Some(VK_COMMA),
        0x21 => Some(VK_LBRACKET),
        0x1E => Some(VK_RBRACKET),
        0x18 => Some(VK_PLUS),
        0x1B => Some(VK_MINUS),
        0x45 => Some(0x6B), // keypad +
        0x4E => Some(0x6D), // keypad -
        0x60 => Some(VK_F5),
        0x65 => Some(0x78), // F9
        0x7A => Some(0x70), // F1
        0x78 => Some(0x71), // F2
        0x6F => Some(VK_F12),
        _ => None,
    };
    if positional.is_some() {
        return positional;
    }
    let layout_letter = e
        .charactersIgnoringModifiers()
        .map(|s| s.to_string())
        .and_then(|s| {
            let mut it = s.chars();
            match (it.next(), it.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase() as u32),
                _ => None,
            }
        });
    // Non-Latin layouts: fall back to the key's US position, as Windows does.
    layout_letter.or_else(|| {
        const ANSI: &[(u16, char)] = &[
            (0x00, 'A'), (0x0B, 'B'), (0x08, 'C'), (0x02, 'D'), (0x0E, 'E'), (0x03, 'F'), (0x05, 'G'),
            (0x04, 'H'), (0x22, 'I'), (0x26, 'J'), (0x28, 'K'), (0x25, 'L'), (0x2E, 'M'), (0x2D, 'N'),
            (0x1F, 'O'), (0x23, 'P'), (0x0C, 'Q'), (0x0F, 'R'), (0x01, 'S'), (0x11, 'T'), (0x20, 'U'),
            (0x09, 'V'), (0x0D, 'W'), (0x07, 'X'), (0x10, 'Y'), (0x06, 'Z'),
        ];
        ANSI.iter().find(|(k, _)| *k == code).map(|(_, c)| *c as u32)
    })
}

/// ULTRA's shortcut for a key, with ⌘ in place of ctrl. Ctrl also works when shift or option
/// is held (⌘⇧3/4/5 belong to macOS screenshots) and for ctrl+tab (⌘tab is the app switcher);
/// a bare ctrl+letter is left alone for the text-editing keys every Mac text field has.
/// On top of that: the in-page accelerators WebView2 provides natively.
pub fn mac_shortcut(vk: u32, cmd: bool, ctrl: bool, shift: bool, opt: bool) -> Option<String> {
    if cmd && ctrl {
        return None; // ⌃⌘F full screen, ⌃⌘Q lock screen, ⌃⌘space emoji...
    }
    let primary = cmd || (ctrl && (shift || opt || vk == VK_TAB));
    if let Some(m) = crate::shortcut(vk, primary, shift, opt) {
        return Some(m);
    }
    let plain = cmd && !shift && !opt;
    let letter = char::from_u32(vk).filter(|c| c.is_ascii_uppercase());
    Some(
        match (vk, letter) {
            (VK_F5, _) if !cmd && !ctrl => "reload",
            (VK_F12, _) => "devtools",
            (_, Some('I')) if cmd && opt => "devtools",
            (_, Some('R')) if plain => "reload",
            (_, Some('R')) if cmd && shift && !opt => "reader", // Safari's reader key
            (_, Some('P')) if plain => "print",
            (VK_LBRACKET, _) if plain => "back",
            (VK_RBRACKET, _) if plain => "fwd",
            (VK_PLUS | 0x6B, _) if cmd && !opt => "zoomin",
            (VK_MINUS | 0x6D, _) if plain => "zoomout",
            (0x30 | 0x60, _) if plain => "zoomreset",
            _ => return None,
        }
        .into(),
    )
}

/// The WKWebView a view belongs to (itself or an ancestor), if any.
fn webview_of(mut v: Option<Retained<NSView>>) -> Option<usize> {
    while let Some(view) = v {
        if view.isKindOfClass(WKWebView::class()) {
            return Some(Retained::as_ptr(&view) as usize);
        }
        v = unsafe { view.superview() };
    }
    None
}

fn responder_view(r: Option<Retained<NSResponder>>) -> Option<Retained<NSView>> {
    r.and_then(|r| r.downcast::<NSView>().ok())
}

/// Keyboard focus is on the bare window rather than in any page or the chrome.
pub fn focus_is_loose(window: &NSWindow) -> bool {
    webview_of(responder_view(window.firstResponder())).is_none()
}

/// Watch every key and extra mouse button headed for ULTRA's window. `send(webview, message)`
/// gets the shortcut and the pane it came from (0 when no page has focus).
/// Keys typed into the chrome UI are left to its own JavaScript, exactly as on Windows.
pub fn watch_input(window: &NSWindow, ui: usize, send: impl Fn(usize, String) + 'static) -> Option<Retained<AnyObject>> {
    let win = window as *const NSWindow as usize;
    let block = RcBlock::new(move |ev: NonNull<NSEvent>| -> *mut NSEvent {
        let pass = ev.as_ptr();
        let e = unsafe { ev.as_ref() };
        let Some(mtm) = MainThreadMarker::new() else { return pass };
        // Sign-in popups and sheets keep their own keys.
        let Some(w) = e.window(mtm) else { return pass };
        if Retained::as_ptr(&w) as usize != win {
            return pass;
        }
        match e.r#type() {
            NSEventType::KeyDown => {
                let f = e.modifierFlags();
                let has = |m: NSEventModifierFlags| f.contains(m);
                let (cmd, ctrl, shift, opt) = (
                    has(NSEventModifierFlags::Command),
                    has(NSEventModifierFlags::Control),
                    has(NSEventModifierFlags::Shift),
                    has(NSEventModifierFlags::Option),
                );
                let target = webview_of(responder_view(w.firstResponder()));
                if target == Some(ui) {
                    return pass;
                }
                let Some(k) = vk(e) else { return pass };
                let Some(msg) = mac_shortcut(k, cmd, ctrl, shift, opt) else { return pass };
                // Held keys auto-repeat: act once, except ctrl+tab cycling and zooming.
                let repeats = k == VK_TAB || msg.starts_with("zoom") && msg != "zoom";
                if !e.isARepeat() || repeats {
                    send(target.unwrap_or(0), msg);
                }
                std::ptr::null_mut()
            }
            NSEventType::OtherMouseDown => {
                // Mouse back/forward buttons over a page.
                let msg = match e.buttonNumber() {
                    3 => "back",
                    4 => "fwd",
                    _ => return pass,
                };
                let hit = w.contentView().and_then(|c| c.hitTest(e.locationInWindow()));
                match webview_of(hit) {
                    Some(t) if t != ui => {
                        send(t, msg.into());
                        std::ptr::null_mut()
                    }
                    _ => pass,
                }
            }
            _ => pass,
        }
    });
    unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown | NSEventMask::OtherMouseDown, &block) }
}

/// Is this page making sound right now? (WebKit's own flag, the one Safari's tab speaker uses.)
pub fn playing_audio(v: &WebView) -> bool {
    let w = v.webview();
    let s = sel!(_isPlayingAudio);
    if !w.respondsToSelector(s) {
        return false;
    }
    let b: Bool = unsafe { msg_send![&*w, _isPlayingAudio] };
    b.as_bool()
}

/// The WebContent process rendering this page, if WebKit has started one.
pub fn web_pid(v: &WebView) -> Option<u32> {
    let w = v.webview();
    if !w.respondsToSelector(sel!(_webProcessIdentifier)) {
        return None;
    }
    let pid: i32 = unsafe { msg_send![&*w, _webProcessIdentifier] };
    (pid > 0).then_some(pid as u32)
}

/// Per-page setup WebView2 does by default: swipe/mouse-button navigation, pinch zoom,
/// a dark backdrop behind pages that set no background, phishing protection.
pub fn prepare(v: &WebView) {
    let w = v.webview();
    unsafe {
        w.setAllowsBackForwardNavigationGestures(true);
        w.setAllowsMagnification(true);
        w.configuration().preferences().setFraudulentWebsiteWarningEnabled(true);
    }
    w.setWantsLayer(true);
    if let Some(layer) = w.layer() {
        let (r, g, b) = BACKDROP;
        layer.setBackgroundColor(Some(&CGColor::new_srgb(r, g, b, 1.0)));
    }
}

/// Register a script for every future document (top frame) of an existing webview.
pub fn add_doc_script(v: &WebView, js: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let script = unsafe {
        WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            mtm.alloc(),
            &NSString::from_str(js),
            WKUserScriptInjectionTime::AtDocumentStart,
            true,
        )
    };
    unsafe { v.manager().addUserScript(&script) };
}

/// Page zoom (⌘+ / ⌘- / ⌘0) in Chrome's steps, as WebView2 does.
pub fn zoom(v: &WebView, how: &str) {
    const STEPS: &[f64] = &[0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0];
    let w = v.webview();
    let cur = unsafe { w.pageZoom() };
    let next = match how {
        "zoomin" => STEPS.iter().copied().find(|&s| s > cur + 0.001).unwrap_or(5.0),
        "zoomout" => STEPS.iter().rev().copied().find(|&s| s < cur - 0.001).unwrap_or(0.25),
        _ => 1.0,
    };
    unsafe { w.setPageZoom(next) };
}

/// Separate persistent data stores per workspace need macOS 14 (`WKWebsiteDataStore(forIdentifier:)`).
pub fn separate_stores() -> bool {
    objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion().majorVersion >= 14
}

/// A stable data-store id for workspace `w` (a version-4-shaped UUID derived from its number).
pub fn store_id(w: usize) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(format!("ultra workspace {}", w + 1).as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&h[..16]);
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    id
}

/// Dock icon when running outside an .app bundle (`cargo run`); a bundle uses ultra.icns.
pub fn set_app_icon(png: &[u8]) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if NSBundle::mainBundle().bundleIdentifier().is_some() {
        return;
    }
    let data = NSData::with_bytes(png);
    if let Some(img) = NSImage::initWithData(mtm.alloc(), &data) {
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&img)) };
    }
}

pub struct MenuIvars {
    on_command: Box<dyn Fn(&str)>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuIvars]
    pub struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        #[unsafe(method(ultraCommand:))]
        fn ultra_command(&self, item: &NSMenuItem) {
            let cmd = match item.tag() {
                1 => "settings",
                _ => return,
            };
            (self.ivars().on_command)(cmd);
        }
    }
);

fn item(mtm: MainThreadMarker, title: &str, action: Option<Sel>, key: &str, mods: NSEventModifierFlags) -> Retained<NSMenuItem> {
    let it = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), action, &NSString::from_str(key))
    };
    if !key.is_empty() {
        it.setKeyEquivalentModifierMask(mods);
    }
    it
}

fn submenu(mtm: MainThreadMarker, bar: &NSMenu, title: &str, items: Vec<Retained<NSMenuItem>>) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
    for it in &items {
        menu.addItem(it);
    }
    let holder = item(mtm, title, None, "", NSEventModifierFlags::empty());
    holder.setSubmenu(Some(&menu));
    bar.addItem(&holder);
    menu
}

/// The menu bar: ULTRA (about, settings, hide, quit), Edit (so copy/paste work in pages)
/// and Window. Returns the target that must stay alive.
pub fn install_menu(on_command: impl Fn(&str) + 'static) -> Option<Retained<MenuTarget>> {
    let mtm = MainThreadMarker::new()?;
    let target = MenuTarget::alloc(mtm).set_ivars(MenuIvars { on_command: Box::new(on_command) });
    let target: Retained<MenuTarget> = unsafe { msg_send![super(target), init] };
    let cmd = NSEventModifierFlags::Command;
    let sep = || NSMenuItem::separatorItem(mtm);
    let ours = |title: &str, tag: isize, key: &str, mods: NSEventModifierFlags| {
        let it = item(mtm, title, Some(sel!(ultraCommand:)), key, mods);
        it.setTag(tag);
        unsafe { it.setTarget(Some(&target)) };
        it
    };
    let bar = NSMenu::new(mtm);
    submenu(mtm, &bar, "ULTRA", vec![
        item(mtm, "About ULTRA", Some(sel!(orderFrontStandardAboutPanel:)), "", cmd),
        sep(),
        ours("Settings…", 1, ",", cmd),
        sep(),
        item(mtm, "Hide ULTRA", Some(sel!(hide:)), "h", cmd),
        item(mtm, "Hide Others", Some(sel!(hideOtherApplications:)), "h", cmd | NSEventModifierFlags::Option),
        item(mtm, "Show All", Some(sel!(unhideAllApplications:)), "", cmd),
        sep(),
        item(mtm, "Quit ULTRA", Some(sel!(terminate:)), "q", cmd),
    ]);
    submenu(mtm, &bar, "Edit", vec![
        item(mtm, "Undo", Some(sel!(undo:)), "z", cmd),
        item(mtm, "Redo", Some(sel!(redo:)), "z", cmd | NSEventModifierFlags::Shift),
        sep(),
        item(mtm, "Cut", Some(sel!(cut:)), "x", cmd),
        item(mtm, "Copy", Some(sel!(copy:)), "c", cmd),
        item(mtm, "Paste", Some(sel!(paste:)), "v", cmd),
        item(mtm, "Paste and Match Style", Some(sel!(pasteAsPlainText:)), "v", cmd | NSEventModifierFlags::Option | NSEventModifierFlags::Shift),
        item(mtm, "Delete", Some(sel!(delete:)), "", cmd),
        item(mtm, "Select All", Some(sel!(selectAll:)), "a", cmd),
    ]);
    let window = submenu(mtm, &bar, "Window", vec![
        item(mtm, "Minimize", Some(sel!(performMiniaturize:)), "m", cmd),
        item(mtm, "Zoom", Some(sel!(performZoom:)), "", cmd),
        sep(),
        item(mtm, "Enter Full Screen", Some(sel!(toggleFullScreen:)), "f", cmd | NSEventModifierFlags::Control),
    ]);
    let app = NSApplication::sharedApplication(mtm);
    app.setMainMenu(Some(&bar));
    app.setWindowsMenu(Some(&window));
    Some(target)
}

/// Safari's user agent, with the installed Safari's version. WKWebView's own user agent
/// leaves out "Version/… Safari/…", and sites such as Gmail then call the browser outdated.
pub fn safari_user_agent() -> String {
    let v = std::fs::read_to_string("/Applications/Safari.app/Contents/Info.plist")
        .ok()
        .and_then(|s| {
            let rest = &s[s.find("<key>CFBundleShortVersionString</key>")?..];
            let a = rest.find("<string>")? + "<string>".len();
            let b = rest[a..].find("</string>")?;
            Some(rest[a..a + b].trim().to_string())
        })
        .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .unwrap_or_else(|| "18.0".into());
    format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{v} Safari/605.1.15")
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn LSSetDefaultHandlerForURLScheme(scheme: *const std::ffi::c_void, handler: *const std::ffi::c_void) -> i32;
    fn LSSetDefaultRoleHandlerForContentType(kind: *const std::ffi::c_void, role: u32, handler: *const std::ffi::c_void) -> i32;
    fn LSCopyDefaultHandlerForURLScheme(scheme: *const std::ffi::c_void) -> *const std::ffi::c_void;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const std::ffi::c_void);
}

/// tools/bundle.sh writes the same id into the app's Info.plist.
use crate::edition::BUNDLE_ID;

/// NSString is toll-free bridged to CFString.
fn cf(s: &NSString) -> *const std::ffi::c_void {
    s as *const NSString as *const std::ffi::c_void
}

/// Ask macOS to make ULTRA the default browser (macOS shows its own confirmation).
pub fn make_default() -> bool {
    let id = NSString::from_str(BUNDLE_ID);
    let mut ok = true;
    for scheme in ["http", "https"] {
        ok &= unsafe { LSSetDefaultHandlerForURLScheme(cf(&NSString::from_str(scheme)), cf(&id)) } == 0;
    }
    // kLSRolesAll: .html files open in ULTRA too.
    unsafe { LSSetDefaultRoleHandlerForContentType(cf(&NSString::from_str("public.html")), 0xFFFF_FFFF, cf(&id)) };
    ok
}

/// Whether web links currently open in ULTRA.
pub fn is_default() -> bool {
    let p = unsafe { LSCopyDefaultHandlerForURLScheme(cf(&NSString::from_str("https"))) };
    if p.is_null() {
        return false;
    }
    let id = unsafe { &*(p as *const NSString) }.to_string();
    unsafe { CFRelease(p) };
    id.eq_ignore_ascii_case(BUNDLE_ID)
}

/// Let WebKit draw its normal page background again: white, or dark for pages that use a dark
/// color scheme. New pages are created without it (ULTRA's dark backdrop shows instead), so
/// switching pages never flashes white.
pub fn draw_page_background(v: &WebView) {
    let w = v.webview();
    let yes = objc2_foundation::NSNumber::numberWithBool(true);
    let key = NSString::from_str("drawsBackground");
    unsafe {
        let _: () = msg_send![&*w, setValue: &*yes, forKey: &*key];
    }
}
