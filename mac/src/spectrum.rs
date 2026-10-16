//! Audio bars: turns ULTRA's own audio (captured by `audio::Tap`) into ~40 smoothed,
//! log-spaced band levels with an FFT. Samples live only in memory and only while the bars
//! layer is on and a tab is playing sound.

pub const BANDS: usize = 40;
/// Sample rate assumed when the device does not report one.
pub const RATE: u32 = 48_000;
/// Samples per analysis window.
pub const N: usize = 2048;

/// Smoothing for the bands: fast rise, slow fall (the classic bouncing-bar feel).
#[derive(Default)]
pub struct Smoother {
    levels: Vec<f32>,
}

impl Smoother {
    pub fn feed(&mut self, raw: Vec<f32>) -> Vec<f32> {
        if self.levels.len() != raw.len() {
            self.levels = vec![0.0; raw.len()];
        }
        for (s, r) in self.levels.iter_mut().zip(raw) {
            *s = if r > *s { *s + (r - *s) * 0.7 } else { *s * 0.82 + r * 0.18 };
        }
        self.levels.clone()
    }
}

/// Hann-windowed FFT of the last N samples -> BANDS log-spaced levels in 0..1.
pub fn analyse(ring: &[f32], pos: usize, rate: f32) -> Vec<f32> {
    let mut re: Vec<f32> = (0..N)
        .map(|i| {
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (N - 1) as f32).cos();
            ring[(pos + i) % N] * w
        })
        .collect();
    let mut im = vec![0f32; N];
    fft(&mut re, &mut im);
    let bin_hz = rate / N as f32;
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
        let b = analyse(&ring, 0, RATE as f32);
        let peak = (0..BANDS).max_by(|&x, &y| b[x].total_cmp(&b[y])).unwrap();
        let f = |i: usize| 40f32 * (16_000f32 / 40.0).powf(i as f32 / BANDS as f32);
        assert!(f(peak) <= 1000.0 && 1000.0 < f(peak + 1), "peak band {peak}");
        assert!(b[0] < 0.3);
    }
}
