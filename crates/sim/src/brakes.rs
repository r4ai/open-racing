//! Brakes that heat up. The pads' friction on the discs follows the discs' temperature:
//! cold pads bite less and overheated ones fade. The pads pass a share of the heat to
//! the calipers, whose fluid boils when they get too hot, and the discs heat the wheels'
//! rims, which heat the tyres' carcass and the air inside them.
//!
//! Each wheel has three bodies: the disc, the caliper with its pads, pistons and fluid,
//! and the rim with the hub. The friction heat goes into the disc and the caliper on
//! every step; the bodies exchange heat with each other, the tyre and the air every
//! [`crate::THERMAL_STEPS`] steps, as they change over seconds.

use crate::AMBIENT_TEMPERATURE;
use crate::params::{BrakeParams, lookup};

/// Specific heat of cast iron at brake temperatures, J/(kg·K).
const IRON_HEAT_CAPACITY: f64 = 500.0;
/// Disc mass per N·m of the wheel's brake torque when the car does not give it, kg.
const DISC_MASS_PER_TORQUE: f64 = 0.0033;
/// Cooling at the reference speed per kg of disc when the car does not give it, W/(K·kg).
const DISC_COOLING_PER_MASS: f64 = 8.0;
/// Rolling speed at which the cooling is given, m/s.
const REFERENCE_SPEED: f64 = 50.0;
/// Turbulent forced convection grows with the air's speed raised to this.
const CONVECTION_EXPONENT: f64 = 0.8;
/// Cooling at rest (natural convection) relative to that at the reference speed.
const STILL_AIR: f64 = 0.04;
/// Radiating area of a disc (both faces and the vanes' openings) per kg, m²/kg, and its
/// emissivity (oxidised iron).
const RADIATING_AREA_PER_MASS: f64 = 0.018;
const EMISSIVITY: f64 = 0.6;
const STEFAN_BOLTZMANN: f64 = 5.670_374e-8;
/// Share of the disc's radiation that falls on the rim and the hub.
const RIM_VIEW: f64 = 0.3;
/// Share of the friction heat that goes into the pads rather than the disc: iron takes
/// the heat far more readily than the pad's compound.
const PAD_HEAT_SHARE: f64 = 0.06;
/// Heat capacity of the caliper, pads and fluid relative to the disc's.
const CALIPER_CAPACITY: f64 = 0.55;
/// Conductance from the disc through the pads and pistons into the caliper per kg of
/// disc, W/(K·kg).
const DISC_TO_CALIPER_PER_MASS: f64 = 0.3;
/// Cooling of the caliper relative to its disc's.
const CALIPER_COOLING: f64 = 0.1;
/// Conductance from the disc through its bell and the hub into the rim, W/K.
const DISC_TO_RIM: f64 = 6.0;
/// Heat capacity of the rim and hub, J/K, and their cooling at the reference speed, W/K.
const RIM_CAPACITY: f64 = 8000.0;
const RIM_COOLING: f64 = 30.0;
/// Conductance from the rim into the tyre's carcass and the air inside it, W/K.
const RIM_TO_TYRE: f64 = 20.0;
/// Range above the fluid's boiling point over which vapour builds up in the caliper, K,
/// and the share of the pressure it then takes.
const VAPOUR_BAND: f64 = 30.0;
const VAPOUR_LOSS: f64 = 0.6;
/// How far the caliper and the rim have come from the air towards the disc's temperature
/// after a reset: the brakes have been warmed on the way out.
const CALIPER_START: f64 = 0.25;
const RIM_START: f64 = 0.12;
/// 0 °C in K.
const KELVIN: f64 = 273.15;

/// Temperatures of one wheel's brake.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BrakeState {
    /// Temperature of the disc, of the caliper with its pads and fluid, and of the rim, °C.
    pub disc: f64,
    pub caliper: f64,
    pub rim: f64,
    /// Torque the brake gives relative to its share of the car's `max_torque`: the pads'
    /// friction at the disc's temperature, less the pressure lost to boiling fluid.
    pub effectiveness: f64,
}

/// One axle's brake hardware.
#[derive(Clone, Copy, Debug)]
struct Axle {
    /// Torque at full pedal and peak friction, per wheel, N·m.
    torque: f64,
    /// Heat capacities of the disc and the caliper, J/K.
    disc_capacity: f64,
    caliper_capacity: f64,
    /// Cooling of the disc and the caliper at the reference speed, W/K.
    disc_cooling: f64,
    caliper_cooling: f64,
    /// Disc to caliper conductance, W/K.
    to_caliper: f64,
    /// Emissivity × Stefan–Boltzmann × radiating area of the disc, W/K⁴.
    radiation: f64,
}

/// Simulation-ready brakes: the per-axle constants of a [`BrakeParams`].
#[derive(Clone, Debug)]
pub struct BrakeModel {
    /// Pad friction against the disc temperature, relative to its peak.
    friction: Box<[(f64, f64)]>,
    fluid_boiling_point: f64,
    start_temperature: f64,
    axles: [Axle; 2],
}

impl BrakeModel {
    pub fn new(p: &BrakeParams) -> Self {
        let peak = p.pad_friction.iter().map(|f| f.1).fold(0.0, f64::max);
        let torque = [p.front_bias, 1.0 - p.front_bias].map(|share| 0.5 * p.max_torque * share);
        let axle = |k: usize| {
            let pick = |pair: (f64, f64)| if k == 0 { pair.0 } else { pair.1 };
            let mass = p
                .disc_mass
                .map_or(DISC_MASS_PER_TORQUE * torque[k], pick)
                .max(0.5);
            let cooling = p.disc_cooling.map_or(DISC_COOLING_PER_MASS * mass, pick);
            let disc_capacity = IRON_HEAT_CAPACITY * mass;
            Axle {
                torque: torque[k],
                disc_capacity,
                caliper_capacity: CALIPER_CAPACITY * disc_capacity,
                disc_cooling: cooling,
                caliper_cooling: CALIPER_COOLING * cooling,
                to_caliper: DISC_TO_CALIPER_PER_MASS * mass,
                radiation: EMISSIVITY * STEFAN_BOLTZMANN * RADIATING_AREA_PER_MASS * mass,
            }
        };
        Self {
            friction: p.pad_friction.iter().map(|&(t, f)| (t, f / peak)).collect(),
            fluid_boiling_point: p.fluid_boiling_point,
            start_temperature: p.start_temperature,
            axles: [axle(0), axle(1)],
        }
    }

    #[inline]
    fn axle(&self, wheel: usize) -> &Axle {
        &self.axles[usize::from(wheel >= 2)]
    }

    /// A brake after a reset.
    pub fn fresh(&self) -> BrakeState {
        let (air, t) = (AMBIENT_TEMPERATURE, self.start_temperature);
        let mut s = BrakeState {
            disc: t,
            caliper: air + CALIPER_START * (t - air),
            rim: air + RIM_START * (t - air),
            effectiveness: 0.0,
        };
        s.effectiveness = self.effectiveness(&s);
        s
    }

    /// Torque of `wheel`'s brake at `pedal` (0..1), N·m.
    #[inline]
    pub fn torque(&self, wheel: usize, pedal: f64, s: &BrakeState) -> f64 {
        pedal * self.axle(wheel).torque * s.effectiveness
    }

    /// Puts `energy` J of friction heat into `wheel`'s disc and pads.
    #[inline]
    pub fn heat(&self, wheel: usize, s: &mut BrakeState, energy: f64) {
        let a = self.axle(wheel);
        s.disc += (1.0 - PAD_HEAT_SHARE) * energy / a.disc_capacity;
        s.caliper += PAD_HEAT_SHARE * energy / a.caliper_capacity;
    }

    /// Share of the full torque the brake gives at its temperatures.
    fn effectiveness(&self, s: &BrakeState) -> f64 {
        let vapour = ((s.caliper - self.fluid_boiling_point) / VAPOUR_BAND).clamp(0.0, 1.0);
        lookup(&self.friction, s.disc) * (1.0 - VAPOUR_LOSS * vapour)
    }

    /// Exchanges heat for `dt` between `wheel`'s disc, caliper and rim, the air at `air`
    /// °C flowing at `speed` m/s (the wheel's rolling speed: the car's speed through the
    /// ducts, the vanes' pumping) and the tyre's carcass at `tyre` °C. Returns the heat
    /// flowing from the rim into the tyre, W.
    pub fn exchange(
        &self,
        wheel: usize,
        s: &mut BrakeState,
        speed: f64,
        air: f64,
        tyre: f64,
        dt: f64,
    ) -> f64 {
        let a = self.axle(wheel);
        let flow = STILL_AIR + (speed.abs() / REFERENCE_SPEED).powf(CONVECTION_EXPONENT);
        let (disc_k, air_k) = (s.disc + KELVIN, air + KELVIN);
        let radiated = a.radiation * (disc_k.powi(4) - air_k.powi(4));
        let to_caliper = a.to_caliper * (s.disc - s.caliper);
        let to_rim = DISC_TO_RIM * (s.disc - s.rim) + RIM_VIEW * radiated;
        let to_tyre = RIM_TO_TYRE * (s.rim - tyre);
        let disc_loss = a.disc_cooling * flow * (s.disc - air) + radiated;
        s.disc -= dt * (disc_loss + DISC_TO_RIM * (s.disc - s.rim) + to_caliper) / a.disc_capacity;
        s.caliper +=
            dt * (to_caliper - a.caliper_cooling * flow * (s.caliper - air)) / a.caliper_capacity;
        s.rim += dt * (to_rim - to_tyre - RIM_COOLING * flow * (s.rim - air)) / RIM_CAPACITY;
        s.effectiveness = self.effectiveness(s);
        to_tyre
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CarModel;

    fn gt3() -> BrakeModel {
        CarModel::gt3().brakes
    }

    /// Runs the exchange of the front-left brake for `seconds` at `speed`, with the tyre's
    /// carcass at 80 °C.
    fn cool(m: &BrakeModel, s: &mut BrakeState, speed: f64, seconds: f64) {
        let dt = 0.01;
        for _ in 0..(seconds / dt) as usize {
            m.exchange(0, s, speed, 25.0, 80.0, dt);
        }
    }

    #[test]
    fn a_stop_heats_the_disc_by_its_share_of_the_energy() {
        let m = gt3();
        let mut s = m.fresh();
        // A GT3 car from 250 to 80 km/h: 0.86 MJ into each front brake.
        m.heat(0, &mut s, 0.86e6);
        let rise = s.disc - m.start_temperature;
        assert!((100.0..250.0).contains(&rise), "disc +{rise:.0} K");
        assert!(s.caliper - m.start_temperature < 0.5 * rise);
    }

    #[test]
    fn pads_bite_less_cold_and_fade_overheated() {
        let m = gt3();
        let at = |disc: f64| {
            let mut s = m.fresh();
            s.disc = disc;
            m.exchange(0, &mut s, 0.0, 25.0, 80.0, 0.0);
            s.effectiveness
        };
        assert!((at(500.0) - 1.0).abs() < 1e-9);
        assert!(at(25.0) < 0.8, "cold {}", at(25.0));
        assert!(at(900.0) < 0.8, "fade {}", at(900.0));
    }

    #[test]
    fn boiling_fluid_takes_the_pressure() {
        let m = gt3();
        let mut s = m.fresh();
        s.disc = 500.0;
        s.caliper = m.fluid_boiling_point - 10.0;
        m.exchange(0, &mut s, 0.0, 25.0, 80.0, 0.0);
        let sound = s.effectiveness;
        s.caliper = m.fluid_boiling_point + VAPOUR_BAND;
        m.exchange(0, &mut s, 0.0, 25.0, 80.0, 0.0);
        assert!(
            s.effectiveness < 0.5 * sound,
            "{} vs {sound}",
            s.effectiveness
        );
    }

    #[test]
    fn discs_cool_faster_at_speed_and_warm_the_rim() {
        let m = gt3();
        let hot = BrakeState {
            disc: 700.0,
            caliper: 150.0,
            rim: 60.0,
            effectiveness: 1.0,
        };
        let (mut fast, mut slow) = (hot, hot);
        cool(&m, &mut fast, 60.0, 10.0);
        cool(&m, &mut slow, 5.0, 10.0);
        assert!(
            fast.disc < slow.disc - 100.0,
            "{} / {}",
            fast.disc,
            slow.disc
        );
        // On a straight a racing disc sheds a couple of hundred kelvin in seconds.
        assert!(fast.disc < 550.0, "{}", fast.disc);
        assert!(slow.rim > hot.rim, "the disc heats the rim: {}", slow.rim);
        // Left alone everything settles at the air's temperature.
        cool(&m, &mut fast, 30.0, 3000.0);
        assert!((fast.disc - 25.0).abs() < 5.0 && (fast.caliper - 25.0).abs() < 5.0);
    }

    #[test]
    fn a_hot_rim_heats_the_tyre() {
        let m = gt3();
        let mut s = m.fresh();
        s.rim = 120.0;
        assert!(m.exchange(0, &mut s, 30.0, 25.0, 80.0, 0.01) > 0.0);
        s.rim = 40.0;
        assert!(m.exchange(0, &mut s, 30.0, 25.0, 80.0, 0.01) < 0.0);
    }
}
