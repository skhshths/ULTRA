//! Live memory/CPU. WebKit renders pages in helper processes (WebContent per page, plus
//! shared Networking and GPU processes) that macOS launches on ULTRA's behalf and records
//! as ULTRA's responsibility, the same attribution Activity Monitor uses. Each process's
//! memory (its "footprint", Activity Monitor's Memory column) and CPU time come from
//! proc_pid_rusage. Per-pane numbers are that page's own WebContent process.
//! Only sampled while the "pane stats" setting is on (or audio visuals need the list).

use std::{
    ffi::{c_int, c_void, CStr},
    sync::OnceLock,
};

pub struct Usage {
    /// Physical footprint in bytes (what Activity Monitor calls memory).
    pub mem: u64,
    /// User + system CPU time, in 100 ns units (the unit the Windows build uses).
    pub cpu: u64,
}

/// `struct rusage_info_v2` from <sys/resource.h>.
#[repr(C)]
#[derive(Default)]
struct RusageInfoV2 {
    uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    pkg_idle_wkups: u64,
    interrupt_wkups: u64,
    pageins: u64,
    wired_size: u64,
    resident_size: u64,
    phys_footprint: u64,
    proc_start_abstime: u64,
    proc_exit_abstime: u64,
    child_user_time: u64,
    child_system_time: u64,
    child_pkg_idle_wkups: u64,
    child_interrupt_wkups: u64,
    child_pageins: u64,
    child_elapsed_abstime: u64,
    diskio_bytesread: u64,
    diskio_byteswritten: u64,
}

const RUSAGE_INFO_V2: c_int = 2;

#[repr(C)]
#[derive(Default)]
struct Timebase {
    numer: u32,
    denom: u32,
}

extern "C" {
    fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut RusageInfoV2) -> c_int;
    fn proc_listallpids(buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn mach_timebase_info(info: *mut Timebase) -> c_int;
}

/// `responsibility_get_pid_responsible_for_pid` lives in libSystem but has no header, so it is
/// looked up at runtime; without it ULTRA falls back to the page processes WebKit reports.
type Responsible = unsafe extern "C" fn(c_int) -> c_int;

fn responsible_fn() -> Option<Responsible> {
    static F: OnceLock<usize> = OnceLock::new();
    let p = *F.get_or_init(|| {
        let name: &CStr = c"responsibility_get_pid_responsible_for_pid";
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) as usize }
    });
    // SAFETY: a non-null result is that function.
    (p != 0).then(|| unsafe { std::mem::transmute::<usize, Responsible>(p) })
}

/// CPU times from proc_pid_rusage are in Mach ticks (1 ns on Intel, ~41.7 ns on Apple silicon).
fn ticks_to_100ns(t: u64) -> u64 {
    static TB: OnceLock<(u64, u64)> = OnceLock::new();
    let (n, d) = *TB.get_or_init(|| {
        let mut tb = Timebase::default();
        unsafe { mach_timebase_info(&mut tb) };
        if tb.denom == 0 {
            (1, 1)
        } else {
            (tb.numer as u64, tb.denom as u64)
        }
    });
    ((t as u128 * n as u128 / d as u128) / 100) as u64
}

/// Every process working for ULTRA: `known` (page processes WebKit reported) plus whatever
/// macOS says ULTRA is responsible for (Networking, GPU, ...). ULTRA's own pid not included.
pub fn engine_pids(known: &[u32]) -> Vec<u32> {
    let me = std::process::id() as c_int;
    let mut out: Vec<u32> = known.to_vec();
    if let Some(responsible) = responsible_fn() {
        let n = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
        if n > 0 {
            let mut pids = vec![0 as c_int; n as usize + 64];
            let bytes = (pids.len() * std::mem::size_of::<c_int>()) as c_int;
            let got = unsafe { proc_listallpids(pids.as_mut_ptr() as *mut c_void, bytes) };
            for &pid in pids.iter().take(got.max(0) as usize) {
                if pid > 0 && pid != me && unsafe { responsible(pid) } == me {
                    out.push(pid as u32);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn usage(pid: u32) -> Option<Usage> {
    let mut ri = RusageInfoV2::default();
    let ok = unsafe { proc_pid_rusage(pid as c_int, RUSAGE_INFO_V2, &mut ri) } == 0;
    ok.then(|| Usage { mem: ri.phys_footprint, cpu: ticks_to_100ns(ri.user_time + ri.system_time) })
}
