//! Signal processing for the sound: filters, resampling from the solver's rate to 48 kHz,
//! delay lines, and spectra.

use std::f64::consts::PI;

/// Output sample rate, Hz.
pub const OUTPUT_RATE: u32 = 48_000;

/// A second-order section (Bristow-Johnson's cookbook), transposed direct form II.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    fn from(b: [f64; 3], a0: f64, a: [f64; 2]) -> Self {
        Self {
            b: [b[0] / a0, b[1] / a0, b[2] / a0],
            a: [a[0] / a0, a[1] / a0],
            z: [0.0; 2],
        }
    }

    fn w(f: f64, rate: f64, q: f64) -> (f64, f64, f64) {
        let w = 2.0 * PI * (f / rate).min(0.49);
        (w.cos(), w.sin(), w.sin() / (2.0 * q))
    }

    pub fn lowpass(f: f64, rate: f64, q: f64) -> Self {
        let (c, _, al) = Self::w(f, rate, q);
        Self::from(
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],
            1.0 + al,
            [-2.0 * c, 1.0 - al],
        )
    }

    pub fn highpass(f: f64, rate: f64, q: f64) -> Self {
        let (c, _, al) = Self::w(f, rate, q);
        Self::from(
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0],
            1.0 + al,
            [-2.0 * c, 1.0 - al],
        )
    }

    /// Band pass with 0 dB peak gain.
    pub fn bandpass(f: f64, rate: f64, q: f64) -> Self {
        let (c, _, al) = Self::w(f, rate, q);
        Self::from([al, 0.0, -al], 1.0 + al, [-2.0 * c, 1.0 - al])
    }

    /// Peaking equaliser, `gain_db` at `f`.
    pub fn peak(f: f64, rate: f64, q: f64, gain_db: f64) -> Self {
        let (c, _, al) = Self::w(f, rate, q);
        let a = 10f64.powf(gain_db / 40.0);
        Self::from(
            [1.0 + al * a, -2.0 * c, 1.0 - al * a],
            1.0 + al / a,
            [-2.0 * c, 1.0 - al / a],
        )
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Removes the mean: a one-pole high pass at a few hertz.
#[derive(Clone, Copy, Debug)]
pub struct DcBlocker {
    r: f64,
    x1: f64,
    y1: f64,
}

impl DcBlocker {
    pub fn new(cutoff: f64, rate: f64) -> Self {
        Self {
            r: (-2.0 * PI * cutoff / rate).exp(),
            x1: 0.0,
            y1: 0.0,
        }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// Windowed-sinc low-pass taps (Blackman window), cutoff as a share of the sample rate.
pub fn lowpass_taps(n: usize, cutoff: f64) -> Vec<f64> {
    let m = (n - 1) as f64;
    let mut taps: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - m / 2.0;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * x).sin() / (PI * x)
            };
            let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / m).cos()
                + 0.08 * (4.0 * PI * i as f64 / m).cos();
            sinc * w
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    for t in &mut taps {
        *t /= sum;
    }
    taps
}

/// Converts a signal at the solver's rate (24, 48, 96 or 192 kHz) to 48 kHz: a
/// band-limiting FIR, then keeping every `k`-th sample, or doubling with zeros between.
#[derive(Clone, Debug)]
pub struct Resampler {
    /// Output samples per input sample: 2 (up), 1, or 1/2, 1/4 (down).
    up: usize,
    down: usize,
    taps: Vec<f64>,
    history: Vec<f64>,
    pos: usize,
    phase: usize,
}

impl Resampler {
    pub fn new(input_rate: u32) -> Self {
        let (up, down) = if input_rate >= OUTPUT_RATE {
            (1, (input_rate / OUTPUT_RATE) as usize)
        } else {
            ((OUTPUT_RATE / input_rate) as usize, 1)
        };
        let factor = up.max(down);
        let taps = if factor == 1 {
            vec![1.0]
        } else {
            // Pass up to 20 kHz of the 48 kHz output.
            let cut = 21_000.0 / (OUTPUT_RATE as f64 * factor as f64);
            let mut t = lowpass_taps(24 * factor + 1, cut);
            for v in &mut t {
                *v *= up as f64;
            }
            t
        };
        let n = taps.len();
        Self {
            up,
            down,
            taps,
            history: vec![0.0; n],
            pos: 0,
            phase: 0,
        }
    }

    fn push(&mut self, x: f64) {
        self.pos = (self.pos + 1) % self.history.len();
        self.history[self.pos] = x;
    }

    fn filtered(&self) -> f64 {
        let n = self.history.len();
        let mut acc = 0.0;
        for (k, t) in self.taps.iter().enumerate() {
            acc += t * self.history[(self.pos + n - k) % n];
        }
        acc
    }

    /// Feeds one input sample; appends the output samples it completes.
    pub fn process(&mut self, x: f64, out: &mut Vec<f64>) {
        for i in 0..self.up {
            self.push(if i == 0 { x } else { 0.0 });
            self.phase += 1;
            if self.phase >= self.down {
                self.phase = 0;
                out.push(self.filtered());
            }
        }
    }
}

/// A delay line with a fractional delay (linear interpolation).
#[derive(Clone, Debug)]
pub struct Delay {
    buf: Vec<f64>,
    pos: usize,
    delay: f64,
}

impl Delay {
    /// A delay of `samples` (fractional).
    pub fn new(samples: f64) -> Self {
        let n = samples.ceil() as usize + 2;
        Self {
            buf: vec![0.0; n],
            pos: 0,
            delay: samples,
        }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let n = self.buf.len();
        self.pos = (self.pos + 1) % n;
        self.buf[self.pos] = x;
        let d = self.delay;
        let i = d.floor() as usize;
        let f = d - i as f64;
        let a = self.buf[(self.pos + n - i) % n];
        let b = self.buf[(self.pos + n - i - 1) % n];
        a + f * (b - a)
    }
}

/// In-place radix-2 FFT of complex values (re, im); the length must be a power of two.
pub fn fft(re: &mut [f64], im: &mut [f64]) {
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
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Amplitude spectrum of a signal (Hann window), as (Hz, amplitude) up to the Nyquist
/// frequency; the length used is the largest power of two in the signal (at most 65536).
pub fn spectrum(x: &[f64], rate: f64) -> Vec<(f64, f64)> {
    let mut n = 1;
    while n * 2 <= x.len().min(65_536) {
        n *= 2;
    }
    if n < 16 {
        return Vec::new();
    }
    let x = &x[x.len() - n..];
    let mut re: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, v)| v * (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()))
        .collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    (0..n / 2)
        .map(|k| {
            let amp = 4.0 * (re[k] * re[k] + im[k] * im[k]).sqrt() / n as f64;
            (k as f64 * rate / n as f64, amp)
        })
        .collect()
}

/// Amplitude of the component at frequency `f` (Goertzel over the whole signal, Hann window).
pub fn tone(x: &[f64], rate: f64, f: f64) -> f64 {
    let n = x.len();
    if n == 0 {
        return 0.0;
    }
    let w = 2.0 * PI * f / rate;
    let (mut s1, mut s2) = (0.0, 0.0);
    let coeff = 2.0 * w.cos();
    for (i, v) in x.iter().enumerate() {
        let win = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
        let s = v * win + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    4.0 * power.max(0.0).sqrt() / n as f64
}

/// Root-mean-square value.
pub fn rms(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
}

/// Sound pressure level of an RMS pressure, dB re 20 µPa.
pub fn spl(p_rms: f64) -> f64 {
    20.0 * (p_rms.max(1e-9) / 20e-6).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f64, rate: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| (2.0 * PI * f * i as f64 / rate).sin())
            .collect()
    }

    #[test]
    fn resampler_keeps_the_audio_band_and_removes_what_would_alias() {
        for rate in [24_000, 96_000, 192_000] {
            let mut r = Resampler::new(rate);
            let mut out = Vec::new();
            for v in sine(1000.0, rate as f64, rate as usize / 4) {
                r.process(v, &mut out);
            }
            assert!((out.len() as f64 - 12_000.0).abs() < 2.0);
            let a = tone(&out[2000..], 48_000.0, 1000.0);
            assert!((a - 1.0).abs() < 0.02, "{rate}: {a}");
        }
        // 40 kHz at 192 kHz would fold to 8 kHz.
        let mut r = Resampler::new(192_000);
        let mut out = Vec::new();
        for v in sine(40_000.0, 192_000.0, 48_000) {
            r.process(v, &mut out);
        }
        assert!(tone(&out[2000..], 48_000.0, 8000.0) < 0.01);
    }

    #[test]
    fn spectrum_finds_a_tone() {
        let x = sine(440.0, 48_000.0, 48_000);
        let s = spectrum(&x, 48_000.0);
        let peak = s.iter().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        assert!((peak.0 - 440.0).abs() < 1.0);
        assert!((tone(&x, 48_000.0, 440.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn filters_pass_and_stop() {
        let rate = 48_000.0;
        let mut lp = Biquad::lowpass(500.0, rate, 0.707);
        let hi: Vec<f64> = sine(8000.0, rate, 9600)
            .into_iter()
            .map(|v| lp.process(v))
            .collect();
        assert!(rms(&hi[4800..]) < 0.01);
        let mut lp = Biquad::lowpass(500.0, rate, 0.707);
        let lo: Vec<f64> = sine(50.0, rate, 9600)
            .into_iter()
            .map(|v| lp.process(v))
            .collect();
        assert!((rms(&lo[4800..]) - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }
}
