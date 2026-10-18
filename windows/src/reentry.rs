//! Keeps tao's event loop from being re-entered while a webview is being created.
//!
//! wry creates a WebView2 controller synchronously: it waits for it in a nested message loop
//! that dispatches every message on this thread. If one of tao's own wake-ups is in the queue at
//! that moment (a user event sent from another thread, a timer, a redraw), tao's hidden
//! "Tao Thread Event Target" window handles it right there, inside ULTRA's event handler, and tao
//! aborts ("event handler is re-entrant"). Seen in crash.log under refill_spare -> build_view.
//!
//! While a webview is being built, a message hook (this thread only) turns messages for that
//! window into no-ops and keeps them; they are posted again as soon as the build is over, so
//! tao gets every one of them, just a few milliseconds later and from its own loop.

use std::cell::{Cell, RefCell};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{CallNextHookEx, GetClassNameW, PostMessageW, SetWindowsHookExW, HHOOK, MSG, PM_REMOVE, WH_GETMESSAGE, WM_NULL, WM_PAINT},
};

thread_local! {
    static BUILDING: Cell<bool> = const { Cell::new(false) };
    static HELD: RefCell<Vec<(isize, u32, usize, isize)>> = const { RefCell::new(Vec::new()) };
    /// tao's event-target window, once recognised.
    static TARGET: Cell<isize> = const { Cell::new(0) };
    static HOOK: Cell<isize> = const { Cell::new(0) };
}

fn is_target(hwnd: HWND) -> bool {
    if hwnd.0.is_null() {
        return false;
    }
    let known = TARGET.get();
    if known != 0 {
        return hwnd.0 as isize == known;
    }
    let mut name = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
    let yes = String::from_utf16_lossy(&name[..n.min(64)]) == "Tao Thread Event Target";
    if yes {
        TARGET.set(hwnd.0 as isize);
    }
    yes
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 == PM_REMOVE.0 as usize && BUILDING.get() {
        let msg = unsafe { &mut *(lparam.0 as *mut MSG) };
        if msg.message != WM_NULL && is_target(msg.hwnd) {
            // A paint request isn't queued (Windows makes it up while the window needs painting):
            // let tao's own loop pick it up afterwards instead of keeping a copy.
            if msg.message != WM_PAINT {
                HELD.with(|h| h.borrow_mut().push((msg.hwnd.0 as isize, msg.message, msg.wParam.0, msg.lParam.0)));
            }
            msg.message = WM_NULL;
        }
    }
    unsafe { CallNextHookEx(Some(HHOOK(HOOK.get() as *mut _)), code, wparam, lparam) }
}

/// Install the hook for this (the UI) thread. Call once, before the event loop runs.
pub fn install() {
    if HOOK.get() != 0 {
        return;
    }
    if let Ok(h) = unsafe { SetWindowsHookExW(WH_GETMESSAGE, Some(hook), None, GetCurrentThreadId()) } {
        HOOK.set(h.0 as isize);
    }
}

/// Run `f` (which builds a webview) with tao's messages held back, then deliver them.
pub fn building<R>(f: impl FnOnce() -> R) -> R {
    let outer = BUILDING.replace(true);
    let r = f();
    BUILDING.set(outer);
    if !outer {
        for (hwnd, message, wp, lp) in HELD.with(|h| std::mem::take(&mut *h.borrow_mut())) {
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as *mut _)), message, WPARAM(wp), LPARAM(lp));
            }
        }
    }
    r
}
