//! Temperatures and wear of the engine's parts.
//!
//! Three bodies hold the heat: the cylinders (heads, liners and pistons), the coolant with
//! the block, and the oil. Combustion heats the cylinder walls with a share of the fuel's
//! energy, and friction heats the oil and the liners. The coolant carries the cylinders'
//! heat to the radiator, whose flow the thermostat opens to hold the coolant at its
//! temperature; the oil sheds heat in its cooler and to the coolant. The radiator and the
//! oil cooler take air in through the car's nose, or from a fan at low speed, and a hit
//! on the nose crushes them.
//!
//! The temperatures change what the engine gives:
//! - thick cold oil adds friction, thin hot oil takes some away;
//! - hot cylinders knock, and the control unit retards the ignition;
//! - coolant boiling in the head leaves the walls in steam, which carries little heat, so
//!   the cylinders overheat.
//!
//! With failures on, parts wear beyond their limits: the pistons and the head gasket in
//! overheated cylinders, the bearings in overheated oil, the valvetrain when over-revved.
//! Worn pistons and valves lose compression and power, worn bearings add friction, and a
//! broken part stops the engine for good.
//!
//! Heat goes into the parts on every step; they exchange it with each other and the air
//! every [`crate::THERMAL_STEPS`] steps.

use crate::drivetrain::RPM_PER_RAD_S;
use crate::params::EngineParams;

/// Lower heating value of petrol, J/kg.
const FUEL_HEATING_VALUE: f64 = 43e6;
/// Share of the fuel's energy that heats the cylinder walls (the coolant's load).
const WALL_HEAT_SHARE: f64 = 0.22;
/// Share of the friction heat that goes into the oil (bearings, valvetrain); the rest
/// heats the liners (piston rings).
const FRICTION_TO_OIL: f64 = 0.6;
/// Heat capacities per litre of displacement, J/(K·l): cylinder heads, liners and
/// pistons; coolant and block; oil.
const CYLINDER_CAPACITY: f64 = 4000.0;
const COOLANT_CAPACITY: f64 = 15000.0;
const OIL_CAPACITY: f64 = 3000.0;
/// Conductances per litre of displacement, W/(K·l): cylinder walls to coolant, pistons
/// to the oil jets under them, oil to coolant in the oil-water exchanger.
const CYLINDER_TO_COOLANT: f64 = 1500.0;
const CYLINDER_TO_OIL: f64 = 150.0;
const OIL_TO_COOLANT: f64 = 200.0;
/// Radiator and oil cooler per W of the engine's peak power when the car does not give
/// them, W/K per W.
const RADIATOR_PER_POWER: f64 = 0.015;
const OIL_COOLER_PER_POWER: f64 = 0.002;
/// Air speed at which the coolers are rated, m/s, and the exponent of forced convection.
const REFERENCE_SPEED: f64 = 50.0;
const CONVECTION_EXPONENT: f64 = 0.8;
/// Heat the block sheds into the engine bay, relative to the radiator's at its rating.
const BLOCK_LOSS: f64 = 0.01;
/// Range over which the thermostat opens, K, and the share of the flow it passes shut.
const THERMOSTAT_RANGE: f64 = 10.0;
const THERMOSTAT_LEAK: f64 = 0.02;
/// Coolant temperature above the thermostat at which the fan switches on, K, and the air
/// speed through the radiator it gives, m/s.
const FAN_ON: f64 = 12.0;
const FAN_SPEED: f64 = 3.0;
/// Range above the boiling point over which steam blankets the cylinder walls, K, and
/// the share of their heat transfer it then takes.
const BOIL_BAND: f64 = 10.0;
const STEAM_LOSS: f64 = 0.8;
/// Oil temperature at which the drag curve holds, °C; the oil's viscosity against
/// temperature (Vogel: ln ν = A + B / (T + C), a 10W-60 racing oil); the share of the
/// friction that is hydrodynamic, which goes with the square root of the viscosity.
const OIL_REFERENCE: f64 = 90.0;
const VOGEL_B: f64 = 860.0;
const VOGEL_C: f64 = 95.0;
const HYDRODYNAMIC_FRICTION: f64 = 0.4;
/// Cylinder temperature above which the control unit retards the ignition against
/// knock, °C, the torque lost per K above it, and the most it retards.
const KNOCK_TEMPERATURE: f64 = 170.0;
const KNOCK_LOSS_PER_K: f64 = 0.004;
const KNOCK_MAX_LOSS: f64 = 0.25;
/// Cylinder and oil temperatures above which the pistons and bearings wear, °C; their
/// life at that temperature, s, and the rise that halves it, K.
const PISTON_LIMIT: f64 = 230.0;
const OIL_LIMIT: f64 = 150.0;
const PART_LIFE: f64 = 120.0;
const LIFE_HALVING: f64 = 15.0;
/// Valvetrain wear per rpm above the over-rev speed per s, and the speed relative to it
/// at which the valves float into the pistons and break at once.
const VALVE_WEAR_PER_RPM_S: f64 = 1.0 / 2000.0;
const VALVE_CONTACT: f64 = 1.12;
/// Torque lost with worn-out pistons or valves (compression), and friction added by
/// worn-out bearings, relative to a sound engine.
const COMPRESSION_LOSS: f64 = 0.3;
const BEARING_FRICTION: f64 = 2.0;
/// Radiator and oil cooler area crushed per m/s of damage to the nose.
const COOLER_DAMAGE: f64 = 0.02;
/// Over-rev speed relative to the limiter when the car does not give it.
const OVER_REV: f64 = 1.07;

/// Wear of the engine's parts, 0 = sound, 1 = broken.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EngineWear {
    /// Pistons and head gasket, from overheated cylinders.
    pub pistons: f64,
    /// Crank and rod bearings, from overheated oil.
    pub bearings: f64,
    /// Valves, springs and cams, from over-revving.
    pub valvetrain: f64,
}

impl EngineWear {
    /// Whether a part has broken, which stops the engine.
    pub fn broken(&self) -> bool {
        self.pistons.max(self.bearings).max(self.valvetrain) >= 1.0
    }
}

/// Temperatures and wear of the engine, and what they do to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineHeat {
    /// Temperature of the cylinders, of the coolant and of the oil, °C.
    pub cylinder: f64,
    pub coolant: f64,
    pub oil: f64,
    pub wear: EngineWear,
    /// Combustion torque relative to a sound engine at its working temperature: the
    /// ignition retarded against knock, compression lost to worn parts.
    pub power: f64,
    /// Friction relative to the drag curve's (warm oil, sound bearings).
    pub friction: f64,
}

impl EngineHeat {
    /// An engine warmed up to its working temperature.
    pub const WARM: Self = Self {
        cylinder: 100.0,
        coolant: 85.0,
        oil: OIL_REFERENCE,
        wear: EngineWear {
            pistons: 0.0,
            bearings: 0.0,
            valvetrain: 0.0,
        },
        power: 1.0,
        friction: 1.0,
    };

    /// Whether a part has broken: the engine will not run again.
    pub fn failed(&self) -> bool {
        self.wear.broken()
    }
}

impl Default for EngineHeat {
    fn default() -> Self {
        Self::WARM
    }
}

/// Simulation-ready cooling system of an engine.
#[derive(Clone, Debug)]
pub struct EngineThermal {
    cylinder_capacity: f64,
    coolant_capacity: f64,
    oil_capacity: f64,
    cylinder_to_coolant: f64,
    cylinder_to_oil: f64,
    oil_to_coolant: f64,
    radiator: f64,
    oil_cooler: f64,
    /// Coolant temperature at which the thermostat starts to open and at which it boils, °C.
    pub thermostat: f64,
    pub boiling_point: f64,
    fan: bool,
    /// Speed above which the valvetrain wears, rpm.
    pub over_rev_rpm: f64,
}

impl EngineThermal {
    /// The cooling system of the engine `e`, whose swept volume is `displacement` m³ and
    /// peak power `peak_power` W.
    pub fn new(e: &EngineParams, displacement: f64, peak_power: f64) -> Self {
        let litres = displacement * 1e3;
        let c = &e.cooling;
        Self {
            cylinder_capacity: CYLINDER_CAPACITY * litres,
            coolant_capacity: COOLANT_CAPACITY * litres,
            oil_capacity: OIL_CAPACITY * litres,
            cylinder_to_coolant: CYLINDER_TO_COOLANT * litres,
            cylinder_to_oil: CYLINDER_TO_OIL * litres,
            oil_to_coolant: OIL_TO_COOLANT * litres,
            radiator: c.radiator.unwrap_or(RADIATOR_PER_POWER * peak_power),
            oil_cooler: c.oil_cooler.unwrap_or(OIL_COOLER_PER_POWER * peak_power),
            thermostat: c.thermostat,
            boiling_point: c.boiling_point,
            fan: c.fan,
            over_rev_rpm: e.over_rev_rpm.unwrap_or(OVER_REV * e.limiter_rpm),
        }
    }

    /// Heats the parts for `dt` by burning `fuel_flow` kg/s and losing `friction_power` W
    /// to friction.
    #[inline]
    pub fn heat(&self, h: &mut EngineHeat, fuel_flow: f64, friction_power: f64, dt: f64) {
        let wall = WALL_HEAT_SHARE * FUEL_HEATING_VALUE * fuel_flow;
        let friction = friction_power.max(0.0);
        h.cylinder += dt * (wall + (1.0 - FRICTION_TO_OIL) * friction) / self.cylinder_capacity;
        h.oil += dt * FRICTION_TO_OIL * friction / self.oil_capacity;
    }

    /// Exchanges heat for `dt` between the parts and the air at `air` °C, which flows
    /// into the car's nose at `airspeed` m/s; `nose_damage` (m/s of impacts) has crushed
    /// the coolers behind it. With `failures`, parts beyond their limits wear, and the
    /// engine turning at `speed` rad/s wears its valvetrain above the over-rev speed.
    #[allow(clippy::too_many_arguments)]
    pub fn exchange(
        &self,
        h: &mut EngineHeat,
        speed: f64,
        airspeed: f64,
        air: f64,
        nose_damage: f64,
        failures: bool,
        dt: f64,
    ) {
        let fan = if self.fan && h.coolant > self.thermostat + FAN_ON {
            FAN_SPEED
        } else {
            0.0
        };
        let intact = (1.0 - COOLER_DAMAGE * nose_damage).max(0.0);
        let through =
            intact * (airspeed.max(fan).max(0.0) / REFERENCE_SPEED).powf(CONVECTION_EXPONENT);
        let opening =
            ((h.coolant - self.thermostat) / THERMOSTAT_RANGE).clamp(THERMOSTAT_LEAK, 1.0);
        let boil = ((h.coolant - self.boiling_point) / BOIL_BAND).clamp(0.0, 1.0);
        let cylinder_to_coolant =
            self.cylinder_to_coolant * (1.0 - STEAM_LOSS * boil) * (h.cylinder - h.coolant);
        let cylinder_to_oil = self.cylinder_to_oil * (h.cylinder - h.oil);
        let oil_to_coolant = self.oil_to_coolant * (h.oil - h.coolant);
        let radiator = self.radiator * (opening * through + BLOCK_LOSS) * (h.coolant - air);
        let oil_cooler = self.oil_cooler * through * (h.oil - air);
        h.cylinder -= dt * (cylinder_to_coolant + cylinder_to_oil) / self.cylinder_capacity;
        h.coolant += dt * (cylinder_to_coolant + oil_to_coolant - radiator) / self.coolant_capacity;
        h.oil += dt * (cylinder_to_oil - oil_to_coolant - oil_cooler) / self.oil_capacity;

        let w = &mut h.wear;
        if failures {
            let wear = |temperature: f64, limit: f64| {
                if temperature > limit {
                    dt / PART_LIFE * ((temperature - limit) / LIFE_HALVING).exp2()
                } else {
                    0.0
                }
            };
            w.pistons = (w.pistons + wear(h.cylinder, PISTON_LIMIT)).min(1.0);
            w.bearings = (w.bearings + wear(h.oil, OIL_LIMIT)).min(1.0);
            let rpm = speed * RPM_PER_RAD_S;
            if rpm > VALVE_CONTACT * self.over_rev_rpm {
                w.valvetrain = 1.0;
            } else if rpm > self.over_rev_rpm {
                w.valvetrain =
                    (w.valvetrain + dt * (rpm - self.over_rev_rpm) * VALVE_WEAR_PER_RPM_S).min(1.0);
            }
        }
        let knock =
            (KNOCK_LOSS_PER_K * (h.cylinder - KNOCK_TEMPERATURE)).clamp(0.0, KNOCK_MAX_LOSS);
        h.power = (1.0 - knock)
            * (1.0 - COMPRESSION_LOSS * w.pistons)
            * (1.0 - COMPRESSION_LOSS * w.valvetrain);
        h.friction = viscous_friction(h.oil) * (1.0 + BEARING_FRICTION * w.bearings);
    }
}

/// Friction relative to that with the oil at its reference temperature: the hydrodynamic
/// share goes with the square root of the oil's viscosity.
fn viscous_friction(oil: f64) -> f64 {
    let viscosity =
        (VOGEL_B * (1.0 / (oil.max(-30.0) + VOGEL_C) - 1.0 / (OIL_REFERENCE + VOGEL_C))).exp();
    1.0 - HYDRODYNAMIC_FRICTION + HYDRODYNAMIC_FRICTION * viscosity.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CarModel;

    fn gt3() -> EngineThermal {
        CarModel::gt3().engine.thermal.clone()
    }

    /// Runs the engine for `seconds` burning `fuel_flow` with `friction_power`, at
    /// 7000 rpm, in air at 25 °C flowing at `airspeed`.
    #[allow(clippy::too_many_arguments)]
    fn run(
        m: &EngineThermal,
        h: &mut EngineHeat,
        fuel_flow: f64,
        friction_power: f64,
        airspeed: f64,
        nose_damage: f64,
        failures: bool,
        seconds: f64,
    ) {
        let dt = 0.01;
        for _ in 0..(seconds / dt) as usize {
            m.heat(h, fuel_flow, friction_power, dt);
            m.exchange(
                h,
                7000.0 / RPM_PER_RAD_S,
                airspeed,
                25.0,
                nose_damage,
                failures,
                dt,
            );
        }
    }

    #[test]
    fn viscous_friction_is_one_warm_high_cold_and_low_hot() {
        assert!((viscous_friction(OIL_REFERENCE) - 1.0).abs() < 1e-12);
        let cold = viscous_friction(20.0);
        assert!((1.6..2.4).contains(&cold), "{cold}");
        assert!(viscous_friction(140.0) < 0.9);
    }

    #[test]
    fn the_thermostat_holds_the_coolant_at_speed() {
        let m = gt3();
        let mut h = EngineHeat::WARM;
        // Most of a GT3 car's full-throttle fuel flow, at racing speed.
        run(&m, &mut h, 0.02, 40e3, 55.0, 0.0, true, 600.0);
        assert!(
            (m.thermostat..m.thermostat + THERMOSTAT_RANGE + 5.0).contains(&h.coolant),
            "{h:?}"
        );
        assert!((80.0..130.0).contains(&h.oil), "{h:?}");
        assert!(h.cylinder < KNOCK_TEMPERATURE, "{h:?}");
        assert_eq!(h.wear, EngineWear::default());
        assert!((h.power - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_crushed_nose_overheats_the_engine_until_it_fails() {
        let m = gt3();
        let mut h = EngineHeat::WARM;
        run(&m, &mut h, 0.02, 40e3, 55.0, 45.0, true, 600.0);
        assert!(h.coolant > m.boiling_point, "{h:?}");
        assert!(h.failed(), "{h:?}");
        // Without failures it runs on, hot and down on power.
        let mut h = EngineHeat::WARM;
        run(&m, &mut h, 0.02, 40e3, 55.0, 45.0, false, 600.0);
        assert!(!h.failed() && h.power < 0.9, "{h:?}");
    }

    #[test]
    fn over_revving_wears_the_valvetrain_and_far_over_breaks_it() {
        let m = gt3();
        let mut h = EngineHeat::WARM;
        let over = (m.over_rev_rpm + 1000.0) / RPM_PER_RAD_S;
        for _ in 0..20 {
            m.exchange(&mut h, over, 50.0, 25.0, 0.0, true, 0.01);
        }
        assert!((0.05..0.2).contains(&h.wear.valvetrain), "{h:?}");
        assert!(h.power < 1.0 && !h.failed());
        m.exchange(
            &mut h,
            1.2 * m.over_rev_rpm / RPM_PER_RAD_S,
            50.0,
            25.0,
            0.0,
            true,
            0.01,
        );
        assert!(h.failed());
        // Failures off: nothing wears.
        let mut h = EngineHeat::WARM;
        m.exchange(
            &mut h,
            1.2 * m.over_rev_rpm / RPM_PER_RAD_S,
            50.0,
            25.0,
            0.0,
            false,
            0.01,
        );
        assert_eq!(h.wear, EngineWear::default());
    }
}
