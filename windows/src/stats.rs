//! Live memory/CPU. WebView2 lists its processes (ids only); each process's private
//! memory and CPU time come from Win32. Per-pane numbers are each page's own JS heap,
//! read by a tiny script. Only sampled while the "pane stats" setting is on.
//!
//! Do NOT walk `AssociatedFrameInfos()` here: on WebView2 155, reading an entry from
//! that collection (`GetCurrent`) crashes the host process with an access violation.

use webview2_com::{
    GetProcessExtendedInfosCompletedHandler,
    Microsoft::Web::WebView2::Win32::{ICoreWebView2Environment13, ICoreWebView2ProcessExtendedInfoCollection},
};
use windows::Win32::{
    Foundation::{CloseHandle, FILETIME},
    System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};
use windows_core::Interface;
use wry::{WebView, WebViewExtWindows};

/// Script returning the page's JS heap in MB (or -1 where the browser doesn't expose it).
pub const HEAP_JS: &str =
    "(performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1048576) : -1)";

pub struct Usage {
    /// Private bytes (what Task Manager calls memory).
    pub mem: u64,
    /// Kernel + user CPU time, in 100 ns units.
    pub cpu: u64,
}

/// Ask WebView2 for its process ids; `done` runs on the UI thread when they arrive.
pub fn request(ui: &WebView, done: impl FnOnce(Vec<u32>) + 'static) -> bool {
    let Ok(env) = ui.environment().cast::<ICoreWebView2Environment13>() else { return false };
    let handler = GetProcessExtendedInfosCompletedHandler::create(Box::new(move |_, list| {
        done(list.map(|l| unsafe { pids(&l) }).unwrap_or_default());
        Ok(())
    }));
    unsafe { env.GetProcessExtendedInfos(&handler).is_ok() }
}

unsafe fn pids(list: &ICoreWebView2ProcessExtendedInfoCollection) -> Vec<u32> {
    let mut n = 0u32;
    if list.Count(&mut n).is_err() {
        return Vec::new();
    }
    (0..n)
        .filter_map(|i| {
            let mut pid = 0i32;
            let info = list.GetValueAtIndex(i).ok()?;
            info.ProcessInfo().and_then(|p| p.ProcessId(&mut pid)).ok()?;
            Some(pid as u32)
        })
        .collect()
}

pub fn usage(pid: u32) -> Option<Usage> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut m = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        let mem_ok = GetProcessMemoryInfo(h, &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb).is_ok();
        let (mut c, mut e, mut k, mut u) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let t_ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
        let _ = CloseHandle(h);
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        (mem_ok && t_ok).then(|| Usage { mem: m.PrivateUsage as u64, cpu: ft(k) + ft(u) })
    }
}
