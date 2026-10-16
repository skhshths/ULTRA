//! Audio-reactive visuals: reads the peak level of ULTRA's own audio sessions from the
//! Windows mixer (no capture, no recording, other apps ignored). Only polled while a
//! tab is actually playing sound and the "audio reactive" setting is on.

use std::collections::HashSet;

use windows::Win32::{
    Media::Audio::{
        eConsole, eRender, Endpoints::IAudioMeterInformation, IAudioSessionControl2, IAudioSessionManager2,
        IMMDeviceEnumerator, MMDeviceEnumerator,
    },
    System::Com::{CoCreateInstance, CLSCTX_ALL},
};
use windows_core::Interface;

#[derive(Default)]
pub struct Meter {
    meters: Vec<IAudioMeterInformation>,
}

impl Meter {
    /// Re-scan the default output device's sessions and keep the ones owned by `pids`
    /// (the WebView2 process tree). Called every couple of seconds; cheap.
    pub fn refresh(&mut self, pids: &HashSet<u32>) {
        self.meters.clear();
        unsafe {
            let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };
            let Ok(mgr) = en
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .and_then(|d| d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None))
            else {
                return;
            };
            let Ok(list) = mgr.GetSessionEnumerator() else { return };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(s) = list.GetSession(i) else { continue };
                let ours = s.cast::<IAudioSessionControl2>().and_then(|s2| s2.GetProcessId()).is_ok_and(|p| pids.contains(&p));
                if let (true, Ok(m)) = (ours, s.cast::<IAudioMeterInformation>()) {
                    self.meters.push(m);
                }
            }
        }
    }

    /// Loudest current peak (0..1) across ULTRA's sessions.
    pub fn peak(&self) -> f32 {
        self.meters.iter().filter_map(|m| unsafe { m.GetPeakValue().ok() }).fold(0.0, f32::max)
    }

    pub fn is_empty(&self) -> bool {
        self.meters.is_empty()
    }
}
