//! Turbochargers: a radial turbine in the exhaust driving a centrifugal compressor in the
//! intake on one free shaft.
//!
//! - The compressor is Greitzer's (*J. Eng. Power* 98, 1976; Moore & Greitzer 1986): the
//!   air in its flow path has inertia, so the mass flow follows
//!   dṁ/dt = (A/L)·(p₁·Π(ṁ, U) − p₂), the pressure ratio Π its characteristic makes at the
//!   flow and tip speed pushing against the outlet's pressure. Left of the surge line the
//!   characteristic's slope turns positive, and with the charge pipe's volume behind it
//!   the flow oscillates and reverses: surge, the flutter a turbocharged engine makes when
//!   its throttle shuts on boost without a blow-off valve (Galindo et al., *Exp. Therm.
//!   Fluid Sci.* 30, 2006, measured it in automotive compressors). Right of it the head
//!   falls away to choke. The head rises from its shut-off value at no flow to the peak
//!   along a parabola — still rising at no flow, as centrifugal stages' measured
//!   characteristics do, so a shut throttle cannot hold the compressor still there — falls
//!   along another parabola to choke, and resists reversed flow like a throttle.
//! - The turbine is a nozzle following Stodola's ellipse law,
//!   ṁ·√(R·T₃)/p₃ = C·√(1 − (p₄/p₃)²) (Watson & Janota, *Turbocharging the Internal
//!   Combustion Engine*, 1982, ch. 4), and turns the isentropic enthalpy drop to work with
//!   an efficiency parabolic in the blade speed ratio U/C_s, best at 0.7. Its torque stays
//!   finite at rest, so it spins up from nothing.
//! - The shaft's speed comes from the two torques, less the bearings' drag, on its inertia.
//!   That lag, and the pulses the turbine takes the energy of (a turbocharged engine's
//!   exhaust is quieter for it), are the turbocharger's part in the engine's sound; its
//!   own whistle is in [`crate::acoustics`].

use crate::gas::Gas;
use crate::model::Lump;
use crate::spec::{Compressor, Turbine};

/// Drag of the shaft's journal bearings, N·m per rad/s (≈ 200 W at 150 000 rpm).
const BEARING: f64 = 8e-7;

/// Blade speed ratio U/C_s at which a radial turbine is best.
const BEST_BSR: f64 = 0.7;

/// One turbocharger in a model. A half without the other (an intake with a compressor
/// fitted to an exhaust without its turbine) spins free on the shaft.
#[derive(Clone, Debug)]
pub struct Turbo {
    pub name: String,
    pub compressor: Option<Compressor>,
    pub turbine: Option<Turbine>,
    /// The volumes (lumps) each side runs between: inlet and outlet.
    pub compressor_ports: [usize; 2],
    pub turbine_ports: [usize; 2],
    /// Where the compressor is, m, for its sound.
    pub position: [f64; 3],
    /// Shaft speed, rad/s.
    pub omega: f64,
    /// Mass flow through the compressor (of the air in its flow path) and the turbine,
    /// kg/s.
    pub compressor_flow: f64,
    pub turbine_flow: f64,
    /// Power the compressor puts into the air and the turbine takes from the gas, W.
    pub compressor_power: f64,
    pub turbine_power: f64,
    /// The compressor's flow coefficient and total pressure ratio.
    pub flow_coefficient: f64,
    pub pressure_ratio: f64,
    /// How far open the wastegate is, 0..1.
    pub wastegate: f64,
}

impl Turbo {
    pub fn new(
        name: String,
        compressor: Option<(Compressor, [usize; 2], [f64; 3])>,
        turbine: Option<(Turbine, [usize; 2])>,
    ) -> Self {
        let (compressor, compressor_ports, position) = match compressor {
            Some((c, p, at)) => (Some(c), p, at),
            None => (None, [0; 2], [0.0; 3]),
        };
        let (turbine, turbine_ports) = match turbine {
            Some((t, p)) => (Some(t), p),
            None => (None, [0; 2]),
        };
        Self {
            name,
            compressor,
            turbine,
            compressor_ports,
            turbine_ports,
            position,
            omega: 0.0,
            compressor_flow: 0.0,
            turbine_flow: 0.0,
            compressor_power: 0.0,
            turbine_power: 0.0,
            flow_coefficient: 0.0,
            pressure_ratio: 1.0,
            wastegate: 0.0,
        }
    }

    /// Shaft speed, rpm.
    pub fn rpm(&self) -> f64 {
        self.omega * 30.0 / std::f64::consts::PI
    }

    /// Advances the flows through both wheels and the shaft by `h`, adding the flows to
    /// the lumps' rates.
    pub(crate) fn advance(&mut self, gas: &Gas, lumps: &mut [Lump], h: f64) {
        let tc = self.compress(gas, lumps, h);
        let tt = self.expand(gas, lumps);
        let inertia = self.turbine.as_ref().map_or(1e-5, |t| t.inertia.max(1e-7));
        self.omega = (self.omega + h * (tt - tc - BEARING * self.omega) / inertia).max(0.0);
    }

    /// The compressor: advances the air's momentum in it, moves the air; returns the torque
    /// it takes, N·m.
    fn compress(&mut self, gas: &Gas, lumps: &mut [Lump], h: f64) -> f64 {
        let Some(c) = &self.compressor else {
            return 0.0;
        };
        let [i, o] = self.compressor_ports;
        let (a, b) = (&lumps[i], &lumps[o]);
        let rho = a.p / (gas.r(a.y) * a.t);
        let d = c.wheel;
        let u = 0.5 * self.omega * d;
        let cx = self.compressor_flow / (rho * d * d);
        let dh = head(c, u, cx);
        let gamma = gas.gamma(a.t, a.y);
        let cp = gamma * gas.r(a.y) / (gamma - 1.0);
        let pr = (1.0 + dh / (cp * a.t))
            .max(0.05)
            .powf(gamma / (gamma - 1.0));
        let area = std::f64::consts::PI * 0.25 * c.inducer * c.inducer;
        // Semi-implicit: the new flow moves the gas.
        self.compressor_flow += h * area / c.duct_length * (a.p * pr - b.p);
        let flow = self.compressor_flow;
        let phi = if u > 1.0 { cx / u } else { 0.0 };
        self.flow_coefficient = phi;
        self.pressure_ratio = pr;
        // Work done on the air per kg: the head over the efficiency; reversed flow is
        // churned by the wheel at its shut-off head.
        let work = if flow >= 0.0 {
            dh.max(0.0) / efficiency(c, phi)
        } else {
            c.head * c.shutoff * u * u
        };
        let (from, to, q) = if flow >= 0.0 {
            (i, o, flow)
        } else {
            (o, i, -flow)
        };
        transfer(gas, lumps, from, to, q, work);
        self.compressor_power = q * work;
        if self.omega > 1.0 {
            self.compressor_power / self.omega
        } else {
            0.0
        }
    }

    /// The turbine and its wastegate's pressure drop: moves the gas; returns the torque it
    /// gives, N·m.
    fn expand(&mut self, gas: &Gas, lumps: &mut [Lump]) -> f64 {
        let Some(t) = &self.turbine else {
            return 0.0;
        };
        let [i, o] = self.turbine_ports;
        let (up, down) = if lumps[i].p >= lumps[o].p {
            (i, o)
        } else {
            (o, i)
        };
        let (a, b) = (&lumps[up], &lumps[down]);
        let r = gas.r(a.y);
        let ratio = b.p / a.p;
        // Stodola's ellipse, scaled to a nozzle's choked flow.
        let flow = 0.685 * t.area * a.p / (r * a.t).sqrt() * (1.0 - ratio * ratio).max(0.0).sqrt();
        let gamma = gas.gamma(a.t, a.y);
        let cp = gamma * r / (gamma - 1.0);
        let dh = cp * a.t * (1.0 - ratio.powf((gamma - 1.0) / gamma));
        // η = η*·(2x − x²), x = (U/C_s)/0.7: the torque ṁ·Δh·η/ω, finite at rest.
        let torque = if up == i && dh > 0.0 {
            let cs = (2.0 * dh).sqrt();
            let radius = 0.5 * t.wheel;
            let x = self.omega * radius / (BEST_BSR * cs);
            flow * dh * t.efficiency * radius / (BEST_BSR * cs) * (2.0 - x).max(0.0)
        } else {
            0.0
        };
        let power = torque * self.omega;
        transfer(gas, lumps, up, down, flow, -power / flow.max(1e-9));
        self.turbine_flow = if up == i { flow } else { -flow };
        self.turbine_power = power;
        torque
    }
}

/// Moves `q` kg/s of `from`'s gas into `to`, adding `work` J/kg to it on the way.
fn transfer(gas: &Gas, lumps: &mut [Lump], from: usize, to: usize, q: f64, work: f64) {
    if q <= 0.0 {
        return;
    }
    let (h, y, f) = {
        let l = &lumps[from];
        (gas.enthalpy(l.t, l.y), l.y, l.fuel / l.mass)
    };
    lumps[from].add_rates(-q, -q * h, -q * y, -q * f);
    lumps[to].add_rates(q, q * (h + work), q * y, q * f);
}

/// Isentropic head of a compressor, J/kg, at tip speed `u` and flow `cx` = ṁ/(ρ₀·D²) (a
/// velocity: the flow coefficient times the tip speed).
pub fn head(c: &Compressor, u: f64, cx: f64) -> f64 {
    let (fs, fc) = (c.surge_flow, c.choke_flow);
    let k = c.head / (fc * fc - fs * fs);
    let cs = fs * u;
    if cx >= cs {
        c.head * u * u - k * (cx * cx - cs * cs)
    } else if cx >= 0.0 {
        let x = cx / cs;
        c.head * (c.shutoff + (1.0 - c.shutoff) * x * (2.0 - x)) * u * u
    } else {
        c.head * c.shutoff * u * u + k * cx * cx
    }
}

/// A compressor's isentropic efficiency at flow coefficient `phi`: best midway between
/// surge and choke, falling to 60 % of that at either.
pub fn efficiency(c: &Compressor, phi: f64) -> f64 {
    let (fs, fc) = (c.surge_flow, c.choke_flow);
    let x = (phi - 0.5 * (fs + fc)) / (fc - fs);
    (c.efficiency * (1.0 - 1.6 * x * x)).clamp(0.3 * c.efficiency, c.efficiency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Build, Controls, Load, Quality, samples};

    /// Held at 5000 rpm with the throttle open, the flat four's turbocharger spools up
    /// and its wastegate holds about a bar of boost. Shutting the throttle then, the
    /// blow-off valve vents the charge; without one the compressor surges, its flow
    /// reversing again and again.
    #[test]
    fn spools_holds_its_boost_and_surges_without_a_blow_off_valve() {
        let lift_off = |blow_off: bool| {
            let mut intake = samples::flat4_turbo_intake();
            if !blow_off {
                intake.orifices.clear();
            }
            let (mut m, _) = Build::new(&samples::flat4_turbo())
                .system("intake", &intake)
                .system("exhaust", &samples::flat4_turbo_exhaust())
                .quality(Quality::Draft)
                .build()
                .unwrap();
            m.set_crank(0.0, 5000.0);
            let mut c = Controls {
                pedal: 1.0,
                load: Load::Speed(5000.0),
                ..Default::default()
            };
            let plenum = m
                .lumps
                .iter()
                .position(|l| l.name.ends_with("plenum"))
                .unwrap();
            let mut boost = 0.0;
            let steps = (1.2 / m.dt) as usize;
            for k in 0..steps {
                m.step(&c);
                if k > steps * 3 / 4 {
                    boost += (m.lumps[plenum].p - m.ambient.pressure) / (steps / 4) as f64;
                }
            }
            let spun = m.turbos[0].rpm();
            c.pedal = 0.0;
            let (mut reversals, mut last) = (0, 1.0);
            for _ in 0..(0.4 / m.dt) as usize {
                m.step(&c);
                let f = m.turbos[0].compressor_flow;
                if f.abs() > 0.01 && f.signum() != last {
                    reversals += 1;
                    last = f.signum();
                }
            }
            assert!(m.fault.is_none(), "{:?}", m.fault);
            (spun, boost, reversals)
        };
        let (spun, boost, calm) = lift_off(true);
        assert!(spun > 100_000.0, "{spun}");
        assert!(boost > 0.7e5 && boost < 1.3e5, "{boost}");
        assert_eq!(calm, 0);
        let (_, _, surging) = lift_off(false);
        assert!(surging >= 6, "{surging}");
    }

    fn compressor() -> Compressor {
        Compressor {
            name: "c".into(),
            shaft: "s".into(),
            inlet: "a".into(),
            outlet: "b".into(),
            wheel: 0.052,
            inducer: 0.038,
            blades: 6,
            head: 0.6,
            shutoff: 0.85,
            surge_flow: 0.045,
            choke_flow: 0.16,
            efficiency: 0.76,
            duct_length: 0.25,
            at: [0.0; 3],
        }
    }

    /// The characteristic: a peak at the surge line, rising towards it from no flow and
    /// falling to nothing at choke; a stopped wheel only resists.
    #[test]
    fn characteristic_has_its_shape() {
        let c = compressor();
        let u = 400.0;
        let at = |phi: f64| head(&c, u, phi * u);
        assert!((at(c.surge_flow) - c.head * u * u).abs() < 1e-6);
        assert!(at(0.0) < at(0.5 * c.surge_flow) && at(0.5 * c.surge_flow) < at(c.surge_flow));
        assert!(at(0.1) < at(c.surge_flow));
        assert!(at(c.choke_flow).abs() < 1e-6);
        assert!(at(-0.02) > at(0.0));
        assert!(head(&c, 0.0, 30.0) < 0.0);
        // About 2.5:1 at 400 m/s tip speed.
        let pr = (1.0 + at(c.surge_flow) / (1005.0 * 298.0)).powf(3.5);
        assert!(pr > 2.2 && pr < 2.8, "{pr}");
    }
}
