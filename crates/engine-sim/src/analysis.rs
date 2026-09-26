//! A recording described in numbers, for reading rather than listening: its level, its
//! strongest frequencies, and how strong each engine order is as the speed changes. An
//! order is a multiple of the crank's rotation frequency: a four-cylinder four-stroke
//! fires twice a turn (order 2), a V8 four times (order 4); half orders come from
//! cylinders or banks that differ.

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
                track.push(OrderSlice {
                    time: t,
                    rpm,
                    level_db: spl(rms(w)),
                    orders_db: orders
                        .iter()
                        .map(|o| spl(tone(w, rate, o * f0) / std::f64::consts::SQRT_2))
                        .collect(),
                });
                start += win;
            }
            MicReport {
                name: name.clone(),
                level_db: spl(rms(&x)),
                peaks,
                orders: orders.clone(),
                order_track: track,
            }
        })
        .collect();
    SoundReport { mics }
}
