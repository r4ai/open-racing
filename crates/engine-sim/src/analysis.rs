//! A recording described in numbers, for reading rather than listening: its level, its
//! strongest frequencies, and how strong each engine order is as the speed changes. An
//! order is a multiple of the crank's rotation frequency: a four-cylinder four-stroke
//! fires twice a turn (order 2), a V8 four times (order 4); half orders come from
//! cylinders or banks that differ.
//!
//! How it would be heard is put in Zwicker's terms: loudness (sone) and sharpness (acum,
//! DIN 45692), from the specific loudness of each critical band (ISO 532-1's formula,
//! without the masking slopes, so a little low for narrow sounds), and how tonal it is:
//! the share of its power on the half orders. Production cars' tailpipes at full load run
//! at about 0.7–1.2 acum (Krüger, Castor & Müller, DAGA 2004).

use serde::{Deserialize, Serialize};

use crate::dsp::{rms, spectrum, spl, tone};
use crate::render::Recording;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SoundReport {
    pub mics: Vec<MicReport>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MicReport {
    pub name: String,
    /// Overall sound pressure level, dB re 20 µPa.
    pub level_db: f64,
    /// Means over the order track's windows: loudness, sone; sharpness, acum; share of
    /// the power on the half orders, 0..1.
    pub loudness: f64,
    pub sharpness: f64,
    pub tonal: f64,
    /// Strongest spectral peaks over the whole recording: (Hz, dB).
    pub peaks: Vec<(f64, f64)>,
    /// Orders analysed.
    pub orders: Vec<f64>,
    /// Along the recording: speed and each order's level, dB.
    pub order_track: Vec<OrderSlice>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrderSlice {
    /// s, from the start.
    pub time: f64,
    pub rpm: f64,
    pub level_db: f64,
    pub orders_db: Vec<f64>,
    pub loudness: f64,
    pub sharpness: f64,
    pub tonal: f64,
}

/// Orders from ½ to 12 in halves.
pub fn default_orders() -> Vec<f64> {
    (1..=24).map(|k| k as f64 * 0.5).collect()
}

/// Describes a recording: `slices` windows along it for the order track.
pub fn analyse(rec: &Recording, slices: usize) -> SoundReport {
    let rate = rec.rate as f64;
    let orders = default_orders();
    let rpm_at = |t: f64| -> f64 {
        let tel = &rec.telemetry;
        match tel.iter().position(|k| k.0 >= t) {
            Some(i) => tel[i].1,
            None => tel.last().map(|k| k.1).unwrap_or(0.0),
        }
    };
    let mics = rec
        .channels
        .iter()
        .zip(&rec.mics)
        .map(|(ch, name)| {
            let x: Vec<f64> = ch.iter().map(|&v| v as f64).collect();
            let spec = spectrum(&x, rate);
            let mut peaks: Vec<(f64, f64)> = Vec::new();
            for i in 1..spec.len().saturating_sub(1) {
                let (f, a) = spec[i];
                if a > spec[i - 1].1 && a >= spec[i + 1].1 && f > 15.0 {
                    peaks.push((f, spl(a / std::f64::consts::SQRT_2)));
                }
            }
            peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
            peaks.truncate(12);
            let n = x.len();
            let win = (n / slices.max(1)).max(1024.min(n));
            let mut track = Vec::new();
            let mut start = 0;
            while start + win <= n && win > 0 {
                let w = &x[start..start + win];
                let t = (start + win / 2) as f64 / rate;
                let rpm = rpm_at(t);
                let f0 = rpm / 60.0;
                let spec = spectrum(w, rate);
                let (loudness, sharpness) = loudness_sharpness(&spec);
                track.push(OrderSlice {
                    time: t,
                    rpm,
                    level_db: spl(rms(w)),
                    loudness,
                    sharpness,
                    tonal: tonal_share(&spec, f0),
                    orders_db: orders
                        .iter()
                        .map(|o| spl(tone(w, rate, o * f0) / std::f64::consts::SQRT_2))
                        .collect(),
                });
                start += win;
            }
            let mean = |f: fn(&OrderSlice) -> f64| {
                track.iter().map(f).sum::<f64>() / track.len().max(1) as f64
            };
            MicReport {
                name: name.clone(),
                level_db: spl(rms(&x)),
                loudness: mean(|s| s.loudness),
                sharpness: mean(|s| s.sharpness),
                tonal: mean(|s| s.tonal),
                peaks,
                orders: orders.clone(),
                order_track: track,
            }
        })
        .collect();
    SoundReport { mics }
}

/// Share of an amplitude spectrum's power (40 Hz – 12 kHz) within a bin and a half of a
/// half order of the crank frequency `f0`, 0..1.
pub fn tonal_share(spec: &[(f64, f64)], f0: f64) -> f64 {
    let df = spec.get(1).map(|s| s.0).unwrap_or(1.0);
    let (mut all, mut on) = (0.0, 0.0);
    for &(f, a) in spec {
        if !(40.0..12_000.0).contains(&f) {
            continue;
        }
        let p = a * a;
        all += p;
        let k = (f / (0.5 * f0)).round();
        if k >= 1.0 && (f - k * 0.5 * f0).abs() < 1.5 * df {
            on += p;
        }
    }
    if all > 0.0 { on / all } else { 0.0 }
}

/// Loudness (sone) and sharpness (acum) of an amplitude spectrum (Pa, Hann window) of a
/// sound heard in a free field: Zwicker's specific loudness per 1-Bark band and DIN
/// 45692's weighting.
pub fn loudness_sharpness(spec: &[(f64, f64)]) -> (f64, f64) {
    let bark = |f: f64| 13.0 * (0.00076 * f).atan() + 3.5 * (f / 7500.0).powi(2).atan();
    let mut band = [0.0f64; 24];
    for &(f, a) in spec {
        let z = bark(f);
        if f >= 20.0 && z < 24.0 {
            // Hann's equivalent noise bandwidth is 1.5 bins.
            band[z as usize] += a * a / 3.0;
        }
    }
    let (mut n, mut weighted) = (0.0, 0.0);
    for (z, &power) in band.iter().enumerate() {
        let zc = z as f64 + 0.5;
        // Centre frequency of the band, and the threshold in quiet there (Terhardt).
        let (mut lo, mut hi) = (20.0f64, 20_000.0f64);
        for _ in 0..40 {
            let mid = (lo * hi).sqrt();
            if bark(mid) < zc { lo = mid } else { hi = mid }
        }
        let k = lo / 1000.0;
        let quiet = 3.64 * k.powf(-0.8) - 6.5 * (-0.6 * (k - 3.3).powi(2)).exp() + 1e-3 * k.powi(4);
        let level = 10.0 * (power / 4e-10).max(1e-12).log10();
        let specific = (0.0635
            * 10f64.powf(0.025 * quiet)
            * ((0.75 + 0.25 * 10f64.powf(0.1 * (level - quiet))).powf(0.25) - 1.0))
            .max(0.0);
        let g = if zc < 15.8 {
            1.0
        } else {
            0.066 * (0.171 * zc).exp()
        };
        n += specific;
        weighted += specific * g * zc;
    }
    (n, if n > 0.0 { 0.11 * weighted / n } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combustion::Rng;
    use crate::dsp::{Biquad, rms};

    /// Noise one critical band wide at 1 kHz, 60 dB: 1 acum by DIN 45692's definition;
    /// higher it is sharper.
    #[test]
    fn sharpness_of_the_reference_noise() {
        let rate = 48_000.0;
        let band = |f: f64, q: f64| {
            let mut rng = Rng::new(3);
            let (mut a, mut b) = (Biquad::bandpass(f, rate, q), Biquad::bandpass(f, rate, q));
            let x: Vec<f64> = (0..65_536)
                .map(|_| b.process(a.process(rng.normal())))
                .collect();
            let k = 0.02 / rms(&x);
            let x: Vec<f64> = x.iter().map(|v| v * k).collect();
            loudness_sharpness(&spectrum(&x, rate))
        };
        // 920–1080 Hz: Q ≈ 6.
        let (n, s) = band(1000.0, 6.0);
        assert!((s - 1.0).abs() < 0.15, "{s} acum");
        assert!(n > 2.0 && n < 6.0, "{n} sone");
        let (_, s4) = band(4000.0, 6.0);
        assert!(s4 > 2.0 * s, "{s4} acum at 4 kHz");
    }
}
