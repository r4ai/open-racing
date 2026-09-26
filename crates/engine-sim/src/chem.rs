//! Fuel burning away from the flame: in the exhaust, and in a cylinder after its flame.
//!
//! Fuel that leaves a cylinder unburned — a charge the spark never lit (a spark-cut
//! limiter, a misfire), a flame still burning when the exhaust valve opens (retarded
//! ignition, as on overrun "pop" maps and anti-lag), the excess of a rich mixture — is
//! carried down the exhaust with the gas. Where it is hot and meets oxygen it burns:
//! slowly below ~900 K, then, as its own heat raises the temperature, all at once. That
//! runaway in a manifold, a collector or a silencer is the afterfire's bang, and its
//! pressure pulse is heard at the tailpipe.
//!
//! The rate is a single-step global reaction in the form of Westbrook & Dryer's
//! (*Combust. Sci. Technol.* 27, 1981):
//!
//! ```text
//! ω = A·exp(−E/RT)·[F]^a·[O₂]^b      mol/(cm³·s), concentrations in mol/cm³
//! ```
//!
//! with their orders (for gasoline taken as iso-octane a = ¼, b = 1½), but not their A
//! and E: those are fitted to flame speeds, at 1800 K and more, and would light a
//! mixture at 800 K in a hundredth of a second. Here they are set so that a
//! stoichiometric mixture at 1 atm runs away on its own heat (after τ₀·T²/(T_a·ΔT_ad),
//! Frank-Kamenetskii) in about a millisecond at 1200 K, with the apparent activation
//! energy of 40 kcal/mol that shock-tube ignition delays of alkanes show at high
//! temperature (Davidson et al., *Proc. Combust. Inst.* 29, 2002, for iso-octane): tens
//! of milliseconds at 1000 K, most of a second at 800 K. So fuel mixed into burning or
//! freshly burned gas oxidises at once; unburned mixture cooler than about 1000 K passes
//! through or collects — in a collector, a silencer — until a hot slug arrives.
//!
//! It is a global fit: it knows nothing of the low-temperature chemistry that shapes a real
//! ignition delay, but it gives the thermal runaway at the temperatures an exhaust has,
//! which is what makes afterfire intermittent.

use crate::spec::{Combustion, Fuel};

/// Mass fraction of oxygen in air.
const O2_IN_AIR: f64 = 0.232;
/// Below this the reaction is too slow to matter within an engine's cycle, K.
pub const T_MIN: f64 = 650.0;

/// A fuel's global oxidation rate and what burning it gives.
#[derive(Clone, Debug)]
pub struct Chemistry {
    /// Stoichiometric air-fuel mass ratio.
    pub afr: f64,
    /// Heat released per kg of fuel burned, J/kg.
    pub heat: f64,
    /// Pre-exponential factor, (mol/cm³)^(1−a−b)/s.
    a: f64,
    /// Activation temperature E/R, K.
    ta: f64,
    /// Orders in the fuel and the oxygen.
    order_f: f64,
    order_o: f64,
    /// Molar mass of the fuel, kg/mol.
    molar: f64,
}

impl Chemistry {
    pub fn new(c: &Combustion) -> Self {
        let (lhv, afr) = c.fuel.properties();
        // Westbrook & Dryer's orders (table 2) and the molar mass; the rate fitted to
        // ignition delays (see the module's notes) for all.
        let (order_f, order_o, molar) = match c.fuel {
            Fuel::E85 => (0.15, 1.6, 0.046),
            Fuel::Methanol => (0.25, 1.5, 0.032),
            Fuel::Gasoline | Fuel::Custom { .. } => (0.25, 1.5, 0.114),
        };
        let (a, e) = (2.3e12, 40.0);
        Self {
            afr,
            heat: lhv * c.efficiency,
            a,
            ta: e * 4184.0 / 8.314,
            order_f,
            order_o,
            molar,
        }
    }

    /// Rate the fuel burns, as a mass fraction of the gas per second, in gas of density
    /// `rho` at `t` with burned fraction `y` and fuel fraction `f`.
    pub fn rate(&self, rho: f64, t: f64, y: f64, f: f64) -> f64 {
        let air = (1.0 - y - f).max(0.0);
        if t < T_MIN || f <= 0.0 || air <= 1e-9 {
            return 0.0;
        }
        // mol/cm³.
        let cf = rho * f / self.molar * 1e-6;
        let co = rho * O2_IN_AIR * air / 0.032 * 1e-6;
        let orders = if self.order_f == 0.25 && self.order_o == 1.5 {
            cf.sqrt().sqrt() * co * co.sqrt()
        } else {
            cf.powf(self.order_f) * co.powf(self.order_o)
        };
        let w = self.a * (-self.ta / t).exp() * orders;
        w * 1e6 * self.molar / rho
    }

    /// Share of the gas's mass that is fuel burning over `dt`: at the rate, but never more
    /// than the fuel there is or the oxygen can take, approached smoothly when the rate
    /// would finish it within the step.
    pub fn burn(&self, rho: f64, t: f64, y: f64, f: f64, dt: f64) -> f64 {
        let r = self.rate(rho, t, y, f);
        if r <= 0.0 {
            return 0.0;
        }
        let air = (1.0 - y - f).max(0.0);
        let most = f.min(air / self.afr);
        most * (1.0 - (-r * dt / most).exp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stoichiometric mixture at 1 atm in a closed volume runs away on its own heat
    /// after about a millisecond at 1200 K, tens of them at 1000 K and longer than an
    /// exhaust holds it at 900 K.
    #[test]
    fn mixture_runs_away_when_hot() {
        let e = crate::samples::i4();
        let chem = Chemistry::new(&e.combustion);
        let gas = crate::gas::Gas::new();
        let delay = |t0: f64| -> f64 {
            // Constant volume, 1 bar at t0: fuel with its air.
            let f0 = 1.0 / (1.0 + chem.afr);
            let rho = 1e5 / (gas.r(0.0) * t0);
            let (mut f, mut y, mut u) = (f0, 0.0, gas.energy(t0, 0.0));
            let mut t = t0;
            let dt = 2e-6;
            for k in 0..500_000 {
                let b = chem.burn(rho, t, y, f, dt);
                f -= b;
                y += b * (1.0 + chem.afr);
                u += b * chem.heat;
                t = gas.temperature_near(u, y, t);
                if f < 0.5 * f0 {
                    return k as f64 * dt;
                }
            }
            f64::INFINITY
        };
        let (cool, warm, hot) = (delay(900.0), delay(1000.0), delay(1200.0));
        assert!(cool > 0.05, "{cool}");
        assert!(warm > 0.005 && warm < 0.04, "{warm}");
        assert!(hot > 0.0003 && hot < 0.003, "{hot}");
    }

    /// Bouncing off the limiter, a spark cut sends the cut charges down the exhaust
    /// unburned, and the first fired ones after them light it; a fuel cut sends only air.
    #[test]
    fn a_spark_cut_limiter_afterfires() {
        use crate::{Build, Controls, Load, Quality, samples};
        let afterfire = |cut: crate::spec::Cut| -> f64 {
            let mut e = samples::i4();
            e.ecu.limiter_cut = cut;
            // Stoichiometric: a rich map's excess fuel burns too when air comes after it.
            e.ecu.lambda = crate::spec::Map2::constant(1.0);
            let (mut m, _) = Build::new(&e)
                .system("intake", &samples::i4_intake())
                .system("exhaust", &samples::i4_exhaust())
                .quality(Quality::Draft)
                .build()
                .unwrap();
            let rpm = e.ecu.limiter_rpm - 300.0;
            m.set_crank(0.0, rpm);
            let mut c = Controls {
                pedal: 1.0,
                load: Load::Speed(rpm),
                ..Default::default()
            };
            let mut heat = 0.0;
            let steps = (0.6 / m.dt) as usize;
            for k in 0..steps {
                if k >= steps / 2 {
                    // In and out of the cut every 50 ms.
                    let over = (k as f64 * m.dt / 0.05) as usize % 2 == 0;
                    let hold = if over { 100.0 } else { -300.0 };
                    c.load = Load::Speed(e.ecu.limiter_rpm + hold);
                }
                m.step(&c);
                if k > steps / 2 {
                    heat += m.afterfire;
                }
            }
            assert!(m.fault.is_none(), "{:?}", m.fault);
            heat
        };
        let (spark, fuel) = (
            afterfire(crate::spec::Cut::Spark),
            afterfire(crate::spec::Cut::Fuel),
        );
        assert!(spark > 1000.0, "{spark}");
        assert!(fuel < 0.1 * spark, "{fuel} {spark}");
    }

    #[test]
    fn nothing_burns_without_oxygen() {
        let chem = Chemistry::new(&crate::samples::i4().combustion);
        assert_eq!(chem.burn(0.3, 1500.0, 0.99, 0.01, 1e-4), 0.0);
        let b = chem.burn(0.3, 1500.0, 0.9, 0.01, 1.0);
        assert!((b - 0.09 / chem.afr).abs() < 1e-9, "{b}");
    }
}
