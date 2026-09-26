//! Heat release, heat loss and friction in the cylinders.
//!
//! - Burn: the Wiebe function x_b = 1 − exp(−a·((θ − θ_s)/Δθ)^(m+1)) from the spark θ_s
//!   (Heywood, *Internal Combustion Engine Fundamentals*, §9.2.1), with the duration
//!   growing less than proportionally with speed, longer lean and rich, and a seeded
//!   cycle-to-cycle spread.
//! - Heat transfer: Woschni (1967, SAE 670931).
//! - Knock: the Livengood–Wu integral of the end gas's ignition delay, Douaud & Eyzat's
//!   correlation (1978, SAE 780080).
//! - Friction: Chen & Flynn (1965, SAE 650733).

use crate::spec::{Combustion, Friction};

/// Burned fraction of the Wiebe function at `f` = share of the burn duration elapsed.
#[inline]
pub fn wiebe(a: f64, m: f64, f: f64) -> f64 {
    if f <= 0.0 {
        0.0
    } else {
        let f = f.min(1.0);
        1.0 - (-a * f.powf(m + 1.0)).exp()
    }
}

/// Burn duration, crank degrees, at a speed and excess-air ratio, before the spread.
pub fn duration_deg(c: &Combustion, rpm: f64, lambda: f64) -> f64 {
    let speed = (rpm.max(300.0) / c.reference_rpm).powf(c.speed_exponent);
    // Laminar flame speed peaks slightly rich (λ ≈ 0.9) and falls off either side.
    let mix = 1.0 + 2.2 * (lambda - 0.9).powi(2);
    c.duration_deg * speed * mix
}

/// Share of the fuel that can burn at excess-air ratio `lambda`: rich of stoichiometric
/// there is not enough oxygen for all of it.
#[inline]
pub fn burnable(lambda: f64) -> f64 {
    lambda.min(1.0)
}

/// A small deterministic random generator (xorshift64*), for the cycle-to-cycle spread.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        let u = self.uniform().max(1e-12);
        let v = self.uniform();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

/// Woschni's heat transfer coefficient, W/(m²·K).
///
/// `p` Pa, `t` K, `bore` m, `mean_piston_speed` m/s; `p_motored` is the pressure the
/// cylinder would have without combustion (for the combustion-induced gas velocity),
/// and `reference` = V_d·T_r/(p_r·V_r) at the reference state (inlet valve closing).
pub fn woschni(
    p: f64,
    t: f64,
    bore: f64,
    mean_piston_speed: f64,
    phase: Phase,
    p_motored: f64,
    reference: f64,
) -> f64 {
    let (c1, c2) = match phase {
        Phase::GasExchange => (6.18, 0.0),
        Phase::Compression => (2.28, 0.0),
        Phase::Combustion => (2.28, 3.24e-3),
    };
    let w = c1 * mean_piston_speed + c2 * reference * (p - p_motored).max(0.0);
    3.26 * bore.powf(-0.2) * (p * 1e-3).powf(0.8) * t.max(200.0).powf(-0.55) * w.max(0.1).powf(0.8)
}

/// Part of the cycle, for Woschni's gas velocity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    GasExchange,
    Compression,
    Combustion,
}

/// Ignition delay of the end gas (Douaud & Eyzat), s, at `p` Pa and `t` K.
pub fn ignition_delay(octane: f64, p: f64, t: f64) -> f64 {
    let atm = p / 101_325.0;
    17.68e-3 * (octane / 100.0).powf(3.402) * atm.max(0.1).powf(-1.7) * (3800.0 / t).exp()
}

/// Friction mean effective pressure, Pa.
pub fn fmep(
    f: &Friction,
    peak_pressure: f64,
    mean_piston_speed: f64,
    oil_viscosity_ratio: f64,
) -> f64 {
    let s = mean_piston_speed.abs();
    f.constant
        + f.peak_pressure * peak_pressure
        + oil_viscosity_ratio * (f.piston_speed * s + f.piston_speed_sq * s * s)
        + f.accessories
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiebe_ends_near_one() {
        assert_eq!(wiebe(5.0, 2.0, 0.0), 0.0);
        assert!((wiebe(5.0, 2.0, 1.0) - 0.99326).abs() < 1e-4);
        // 50 % burned a little before the middle of the burn for m = 2.
        let half = (0.5f64.ln() / -5.0).powf(1.0 / 3.0);
        assert!((wiebe(5.0, 2.0, half) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn woschni_is_in_the_textbook_range() {
        // Mid-combustion at 3000 rpm: a few kW/(m²·K).
        let h = woschni(60e5, 2400.0, 0.086, 8.6, Phase::Combustion, 30e5, 0.0);
        assert!(h > 800.0 && h < 5000.0, "{h}");
    }

    #[test]
    fn rng_is_repeatable() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..10 {
            assert_eq!(a.uniform(), b.uniform());
        }
        let mean: f64 = (0..10000).map(|_| a.normal()).sum::<f64>() / 10000.0;
        assert!(mean.abs() < 0.05);
    }
}
