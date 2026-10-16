//! Audio bars: captures *only ULTRA's own audio* (WASAPI process loopback on the WebView2
//! process tree; other apps and the microphone are never touched), runs an FFT and keeps
//! ~40 smoothed, log-spaced band levels. Samples live only in memory on a background
//! thread that runs while the bars background is shown and a tab is playing sound.

use std::{
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

use windows::{
    core::{implement, Interface, Ref, HRESULT},
    Win32::{
        Foundation::{CloseHandle, E_FAIL},
        Media::Audio::{
            ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient, AUDCLNT_BUFFERFLAGS_SILENT,
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
            AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
            WAVE_FORMAT_PCM,
        },
        System::{
            Com::{
                CoInitializeEx, IAgileObject, IAgileObject_Impl,
                StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0},
                BLOB, COINIT_MULTITHREADED,
            },
            Threading::{CreateEventW, WaitForSingleObject},
            Variant::VT_BLOB,
        },
    },
};

pub const BANDS: usize = 40;
const RATE: u32 = 48_000;
const N: usize = 2048;

pub struct Spectrum {
    running: Arc<AtomicBool>,
    bands: Arc<Mutex<Option<Vec<f32>>>>,
}

impl Spectrum {
    /// Start capturing the audio of process `pid` and everything it spawned.
    pub fn start(pid: u32) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let bands = Arc::new(Mutex::new(None));
        let (r, b) = (running.clone(), bands.clone());
        std::thread::spawn(move || {
            let _ = capture(pid, &r, &b);
        });
        Spectrum { running, bands }
    }

    /// Latest band levels (0..1), or None if capture isn't (yet) working.
    pub fn bands(&self) -> Option<Vec<f32>> {
        self.bands.lock().ok()?.clone()
    }
}

impl Drop for Spectrum {
    fn drop(&mut self) {
        // The thread notices within ~100 ms, stops the stream and exits.
        self.running.store(false, Ordering::Relaxed);
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Activated(mpsc::Sender<()>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, _op: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        let _ = self.0.send(());
        Ok(())
    }
}

impl IAgileObject_Impl for Activated_Impl {}

fn capture(pid: u32, running: &AtomicBool, out: &Mutex<Option<Vec<f32>>>) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        // A VT_BLOB PROPVARIANT pointing at the params (ManuallyDrop: nothing to free).
        let pv = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: &mut params as *mut _ as *mut u8,
                        },
                    },
                }),
            },
        };
        let (tx, rx) = mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = Activated(tx).into();
        let op = ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&pv), &handler)?;
        rx.recv_timeout(Duration::from_secs(3)).map_err(|_| windows::core::Error::from(E_FAIL))?;
        let (mut hr, mut unk) = (HRESULT(0), None);
        op.GetActivateResult(&mut hr, &mut unk)?;
        hr.ok()?;
        let client: IAudioClient = unk.ok_or_else(|| windows::core::Error::from(E_FAIL))?.cast()?;

        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 2,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 4,
            nBlockAlign: 4,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            200_000,
            0,
            &fmt,
            None,
        )?;
        let ev = CreateEventW(None, false, false, None)?;
        client.SetEventHandle(ev)?;
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        let mut ring = vec![0f32; N];
        let mut pos = 0usize;
        let mut smooth = vec![0f32; BANDS];
        let mut last = Instant::now();
        while running.load(Ordering::Relaxed) {
            WaitForSingleObject(ev, 100);
            loop {
                let n = cap.GetNextPacketSize().unwrap_or(0);
                if n == 0 {
                    break;
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                if cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
                    break;
                }
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let s = std::slice::from_raw_parts(data as *const i16, frames as usize * 2);
                for f in 0..frames as usize {
                    ring[pos] = if silent { 0.0 } else { (s[f * 2] as f32 + s[f * 2 + 1] as f32) / 65536.0 };
                    pos = (pos + 1) % N;
                }
                let _ = cap.ReleaseBuffer(frames);
            }
            if last.elapsed() >= Duration::from_millis(30) {
                last = Instant::now();
                let raw = analyse(&ring, pos);
                for (s, r) in smooth.iter_mut().zip(raw) {
                    // Fast rise, slow fall: the classic bouncing-bar feel.
                    *s = if r > *s { *s + (r - *s) * 0.7 } else { *s * 0.82 + r * 0.18 };
                }
                if let Ok(mut o) = out.lock() {
                    *o = Some(smooth.clone());
                }
            }
        }
        let _ = client.Stop();
        let _ = CloseHandle(ev);
    }
    Ok(())
}

/// Hann-windowed FFT of the last N samples -> BANDS log-spaced levels in 0..1.
fn analyse(ring: &[f32], pos: usize) -> Vec<f32> {
    let mut re: Vec<f32> = (0..N)
        .map(|i| {
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (N - 1) as f32).cos();
            ring[(pos + i) % N] * w
        })
        .collect();
    let mut im = vec![0f32; N];
    fft(&mut re, &mut im);
    let bin_hz = RATE as f32 / N as f32;
    let (lo, hi) = (40f32, 16_000f32);
    (0..BANDS)
        .map(|b| {
            let f0 = lo * (hi / lo).powf(b as f32 / BANDS as f32);
            let f1 = lo * (hi / lo).powf((b + 1) as f32 / BANDS as f32);
            let (k0, k1) = ((f0 / bin_hz) as usize, ((f1 / bin_hz) as usize).max((f0 / bin_hz) as usize + 1));
            let mag = (k0..k1.min(N / 2)).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).fold(0.0, f32::max);
            let db = 20.0 * (mag * 4.0 / N as f32 + 1e-9).log10();
            ((db + 70.0) / 60.0).clamp(0.0, 1.0)
        })
        .collect()
}

/// In-place iterative radix-2 FFT.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1f32, 0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_lights_its_band() {
        // A 1 kHz sine should peak in the band containing 1 kHz, and not in the bass.
        let ring: Vec<f32> = (0..N).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / RATE as f32).sin() * 0.5).collect();
        let b = analyse(&ring, 0);
        let peak = (0..BANDS).max_by(|&x, &y| b[x].total_cmp(&b[y])).unwrap();
        let f = |i: usize| 40f32 * (16_000f32 / 40.0).powf(i as f32 / BANDS as f32);
        assert!(f(peak) <= 1000.0 && 1000.0 < f(peak + 1), "peak band {peak}");
        assert!(b[0] < 0.3);
    }
}
