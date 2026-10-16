//! Audio-reactive visuals: a Core Audio process tap (macOS 14.2+) on ULTRA's own processes
//! (the WebKit helpers that play its pages' sound; other apps and the microphone are never
//! touched). It feeds both the level meter and the audio bars. The tap is private, leaves the
//! sound playing as normal, keeps samples only in memory, and exists only while a tab is
//! actually playing and an audio visual is on.
//!
//! macOS asks once for "System Audio Recording" permission the first time the tap starts
//! (the Windows mixer needs none). If it is refused, the visuals simply stay still.

use std::{
    collections::HashSet,
    ffi::{c_void, CStr},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};

use objc2::{
    msg_send,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject, Bool},
    sel,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use crate::spectrum::{self, Smoother, N};

type ObjId = u32;
type OSStatus = i32;

#[repr(C)]
struct Addr {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const SYSTEM_OBJECT: ObjId = 1;
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const PROCESS_OBJECT_LIST: u32 = fourcc(b"prs#");
const PROCESS_PID: u32 = fourcc(b"ppid");
const DEFAULT_SYSTEM_OUTPUT: u32 = fourcc(b"sOut");
const DEVICE_UID: u32 = fourcc(b"uid ");
const TAP_FORMAT: u32 = fourcc(b"tfmt");
const FORMAT_LPCM: u32 = fourcc(b"lpcm");
const FLAG_FLOAT: u32 = 1;
const FLAG_NON_INTERLEAVED: u32 = 1 << 5;

/// AudioStreamBasicDescription.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Asbd {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

type IoProc = unsafe extern "C" fn(
    ObjId,
    *const c_void,
    *const AudioBufferList,
    *const c_void,
    *mut AudioBufferList,
    *const c_void,
    *mut c_void,
) -> OSStatus;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyDataSize(id: ObjId, addr: *const Addr, qsize: u32, qdata: *const c_void, size: *mut u32) -> OSStatus;
    fn AudioObjectGetPropertyData(
        id: ObjId,
        addr: *const Addr,
        qsize: u32,
        qdata: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
    fn AudioHardwareCreateAggregateDevice(desc: *const c_void, out: *mut ObjId) -> OSStatus;
    fn AudioHardwareDestroyAggregateDevice(id: ObjId) -> OSStatus;
    fn AudioDeviceCreateIOProcID(dev: ObjId, proc_: IoProc, client: *mut c_void, out: *mut *mut c_void) -> OSStatus;
    fn AudioDeviceDestroyIOProcID(dev: ObjId, id: *mut c_void) -> OSStatus;
    fn AudioDeviceStart(dev: ObjId, id: *mut c_void) -> OSStatus;
    fn AudioDeviceStop(dev: ObjId, id: *mut c_void) -> OSStatus;
}

/// Process taps are macOS 14.2+, so they are looked up at runtime: ULTRA still runs
/// (without audio visuals) on older systems.
type CreateTap = unsafe extern "C" fn(*mut AnyObject, *mut ObjId) -> OSStatus;
type DestroyTap = unsafe extern "C" fn(ObjId) -> OSStatus;

fn sym<T>(name: &CStr) -> Option<T> {
    let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    // SAFETY: callers name a function whose signature matches T.
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

fn addr(selector: u32) -> Addr {
    Addr { selector, scope: SCOPE_GLOBAL, element: 0 }
}

/// Read a fixed-size property.
unsafe fn get<T: Default>(id: ObjId, selector: u32) -> Option<T> {
    let mut v = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    (AudioObjectGetPropertyData(id, &addr(selector), 0, std::ptr::null(), &mut size, &mut v as *mut T as *mut c_void) == 0)
        .then_some(v)
}

/// A CFString property, owned.
unsafe fn get_string(id: ObjId, selector: u32) -> Option<Retained<NSString>> {
    let mut s: *mut NSString = std::ptr::null_mut();
    let mut size = std::mem::size_of::<*mut NSString>() as u32;
    let ok = AudioObjectGetPropertyData(id, &addr(selector), 0, std::ptr::null(), &mut size, &mut s as *mut _ as *mut c_void) == 0;
    // The HAL returns +1 (a "Copy" result): take ownership.
    if ok {
        Retained::from_raw(s)
    } else {
        None
    }
}

/// Core Audio's process objects belonging to `pids`, sorted.
fn process_objects(pids: &HashSet<u32>) -> Vec<ObjId> {
    unsafe {
        let a = addr(PROCESS_OBJECT_LIST);
        let mut size = 0u32;
        if AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &a, 0, std::ptr::null(), &mut size) != 0 {
            return Vec::new();
        }
        let mut list = vec![0 as ObjId; size as usize / std::mem::size_of::<ObjId>()];
        if AudioObjectGetPropertyData(SYSTEM_OBJECT, &a, 0, std::ptr::null(), &mut size, list.as_mut_ptr() as *mut c_void) != 0 {
            return Vec::new();
        }
        list.truncate(size as usize / std::mem::size_of::<ObjId>());
        let mut out: Vec<ObjId> =
            list.into_iter().filter(|&o| get::<i32>(o, PROCESS_PID).is_some_and(|p| p > 0 && pids.contains(&(p as u32)))).collect();
        out.sort_unstable();
        out
    }
}

/// Written by the real-time audio thread, read by the UI thread.
struct Shared {
    /// Loudest |sample| since the last read, as f32 bits (non-negative floats order like ints).
    peak: AtomicU32,
    /// Anything but silence arrived (until then the UI keeps its own fallback animation).
    heard: AtomicBool,
    ring: Mutex<(Vec<f32>, usize)>,
    non_interleaved: bool,
}

unsafe extern "C" fn on_audio(
    _dev: ObjId,
    _now: *const c_void,
    input: *const AudioBufferList,
    _in_time: *const c_void,
    _output: *mut AudioBufferList,
    _out_time: *const c_void,
    client: *mut c_void,
) -> OSStatus {
    if input.is_null() || client.is_null() {
        return 0;
    }
    let s = &*(client as *const Shared);
    let list = &*input;
    let bufs = std::slice::from_raw_parts(list.buffers.as_ptr(), list.count as usize);
    let Some(first) = bufs.first() else { return 0 };
    if first.data.is_null() {
        return 0;
    }
    // One channel per buffer (planar) or all channels in the first buffer (interleaved).
    let (chans, frames) = if s.non_interleaved {
        (bufs.len(), first.size as usize / 4)
    } else {
        let c = first.channels.max(1) as usize;
        (c, first.size as usize / 4 / c)
    };
    let sample = |f: usize, c: usize| -> f32 {
        if s.non_interleaved {
            let b = &bufs[c];
            if b.data.is_null() || f >= b.size as usize / 4 {
                0.0
            } else {
                *(b.data as *const f32).add(f)
            }
        } else {
            *(first.data as *const f32).add(f * chans + c)
        }
    };
    let mut peak = 0f32;
    // Never block the audio thread: if the UI is reading the ring, skip this buffer's samples.
    let mut ring = s.ring.try_lock().ok();
    for f in 0..frames {
        let mut sum = 0f32;
        for c in 0..chans {
            let v = sample(f, c);
            peak = peak.max(v.abs());
            sum += v;
        }
        if let Some(r) = ring.as_mut() {
            let (buf, pos) = &mut **r;
            buf[*pos] = sum / chans as f32;
            *pos = (*pos + 1) % N;
        }
    }
    if peak > 0.0 {
        s.heard.store(true, Ordering::Relaxed);
        s.peak.fetch_max(peak.min(1.0).to_bits(), Ordering::Relaxed);
    }
    0
}

/// A running tap: tap object -> private aggregate device -> IO callback.
struct Tap {
    shared: *const Shared,
    tap: ObjId,
    device: ObjId,
    io: *mut c_void,
    rate: f32,
    destroy: DestroyTap,
}

impl Tap {
    fn start(procs: &[ObjId]) -> Option<Tap> {
        let create: CreateTap = sym(c"AudioHardwareCreateProcessTap")?;
        let destroy: DestroyTap = sym(c"AudioHardwareDestroyProcessTap")?;
        let cls = AnyClass::get(c"CATapDescription")?;
        unsafe {
            let nums: Vec<Retained<NSNumber>> = procs.iter().map(|&p| NSNumber::new_u32(p)).collect();
            let arr = NSArray::from_retained_slice(&nums);
            let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
            let desc: Option<Retained<AnyObject>> = msg_send![alloc, initStereoMixdownOfProcesses: &*arr];
            let desc = desc?;
            // Private: invisible to other apps. Mute behaviour stays "unmuted" (the default).
            if desc.class().responds_to(sel!(setPrivateTap:)) {
                let _: () = msg_send![&*desc, setPrivateTap: Bool::YES];
            }
            let uuid: Retained<NSUUID> = msg_send![&*desc, UUID];
            let tap_uid = uuid.UUIDString();
            let mut tap: ObjId = 0;
            if create(Retained::as_ptr(&desc) as *mut AnyObject, &mut tap) != 0 || tap == 0 {
                return None;
            }
            let fail = |device: ObjId| {
                if device != 0 {
                    AudioHardwareDestroyAggregateDevice(device);
                }
                destroy(tap);
                None
            };
            let fmt: Asbd = get(tap, TAP_FORMAT).unwrap_or_default();
            if fmt.format_id != FORMAT_LPCM || fmt.format_flags & FLAG_FLOAT == 0 || fmt.bits != 32 {
                return fail(0);
            }
            let Some(output) = get::<ObjId>(SYSTEM_OBJECT, DEFAULT_SYSTEM_OUTPUT).and_then(|d| get_string(d, DEVICE_UID)) else {
                return fail(0);
            };
            let s = |v: &str| NSString::from_str(v);
            let dict = |pairs: &[(&str, &AnyObject)]| {
                let keys: Vec<Retained<NSString>> = pairs.iter().map(|(k, _)| s(k)).collect();
                let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
                let vals: Vec<&AnyObject> = pairs.iter().map(|(_, v)| *v).collect();
                NSDictionary::<NSString, AnyObject>::from_slices(&key_refs, &vals)
            };
            let yes = NSNumber::new_bool(true);
            let no = NSNumber::new_bool(false);
            let sub = dict(&[("uid", &output)]);
            let tap_entry = dict(&[("uid", &tap_uid), ("drift", &yes)]);
            let subs = NSArray::from_retained_slice(&[sub]);
            let taps = NSArray::from_retained_slice(&[tap_entry]);
            let name = s("ULTRA audio visuals");
            let agg_uid = NSUUID::new().UUIDString();
            let desc = dict(&[
                ("name", &name),
                ("uid", &agg_uid),
                ("master", &output),
                ("private", &yes),
                ("stacked", &no),
                ("tapautostart", &yes),
                ("subdevices", &subs),
                ("taps", &taps),
            ]);
            let mut device: ObjId = 0;
            if AudioHardwareCreateAggregateDevice(Retained::as_ptr(&desc) as *const c_void, &mut device) != 0 || device == 0 {
                return fail(0);
            }
            let shared = Arc::into_raw(Arc::new(Shared {
                peak: AtomicU32::new(0),
                heard: AtomicBool::new(false),
                ring: Mutex::new((vec![0.0; N], 0)),
                non_interleaved: fmt.format_flags & FLAG_NON_INTERLEAVED != 0,
            }));
            let mut io: *mut c_void = std::ptr::null_mut();
            if AudioDeviceCreateIOProcID(device, on_audio, shared as *mut c_void, &mut io) != 0 {
                drop(Arc::from_raw(shared));
                return fail(device);
            }
            if AudioDeviceStart(device, io) != 0 {
                AudioDeviceDestroyIOProcID(device, io);
                drop(Arc::from_raw(shared));
                return fail(device);
            }
            let rate = if fmt.sample_rate > 0.0 { fmt.sample_rate as f32 } else { spectrum::RATE as f32 };
            Some(Tap { shared, tap, device, io, rate, destroy })
        }
    }

    fn shared(&self) -> &Shared {
        // SAFETY: freed only in Drop, after the device is stopped.
        unsafe { &*self.shared }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe {
            AudioDeviceStop(self.device, self.io);
            AudioDeviceDestroyIOProcID(self.device, self.io);
            AudioHardwareDestroyAggregateDevice(self.device);
            (self.destroy)(self.tap);
            drop(Arc::from_raw(self.shared));
        }
    }
}

#[derive(Default)]
pub struct Meter {
    tap: Option<Tap>,
    procs: Vec<ObjId>,
    smooth: Smoother,
}

impl Meter {
    /// Point the tap at whichever of `pids` (ULTRA and its WebKit helpers) Core Audio knows.
    /// Called every couple of seconds while something plays; only rebuilds when that set changes.
    pub fn refresh(&mut self, pids: &HashSet<u32>) {
        let procs = process_objects(pids);
        if procs == self.procs && (self.tap.is_some() || procs.is_empty()) {
            return;
        }
        self.tap = None;
        self.procs = procs;
        if !self.procs.is_empty() {
            self.tap = Tap::start(&self.procs);
        }
    }

    /// Nothing playing: release the tap and its device.
    pub fn stop(&mut self) {
        self.tap = None;
        self.procs.clear();
    }

    /// Loudest peak (0..1) since the last call.
    pub fn peak(&self) -> f32 {
        self.tap.as_ref().map_or(0.0, |t| f32::from_bits(t.shared().peak.swap(0, Ordering::Relaxed)))
    }

    pub fn is_empty(&self) -> bool {
        self.tap.is_none()
    }

    /// Smoothed band levels for the audio bars, or None until real sound has been heard.
    pub fn bands(&mut self) -> Option<Vec<f32>> {
        let tap = self.tap.as_ref()?;
        let sh = tap.shared();
        if !sh.heard.load(Ordering::Relaxed) {
            return None;
        }
        let raw = {
            let r = sh.ring.lock().ok()?;
            spectrum::analyse(&r.0, r.1, tap.rate)
        };
        Some(self.smooth.feed(raw))
    }
}
