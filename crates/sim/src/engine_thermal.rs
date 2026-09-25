//! Temperatures and wear of the engine's parts.
//!
//! Three bodies hold the heat: the cylinders (heads, liners and pistons), the coolant with
//! the block, and the oil. Combustion heats the cylinder walls with a share of the fuel's
//! energy, and friction heats the oil and the liners. The coolant carries the cylinders'
//! heat to the radiator, whose flow the thermostat opens to hold the coolant at its
//! temperature; the oil sheds heat in its cooler and to the coolant. The radiator and the
//! oil cooler take air in through the car's nose, or from a fan at low speed, and a hit
//! on the nose crushes them. Their cooling goes with the air's mass flow, so thin air,
//! high or hot, cools less.
//!
//! The heat the engine sheds warms the air around it in the engine bay, the more the
//! less air flows through: standing or crawling, the bay gets hot. The intake draws part
//! of its air from the bay, and the gearbox, bolted to the engine, sits in it. The
//! gearbox's oil is heated by its losses and by the engine through the bellhousing.
//!
//! The temperatures change what the engine gives:
//! - thick cold oil adds friction, thin hot oil takes some away;
//! - hot cylinders knock, and the control unit retards the ignition;
//! - coolant boiling in the head leaves the walls in steam, which carries little heat, so
//!   the cylinders overheat;
//! - hot intake air is thin, which the engine takes as lower charge density;
//! - thick cold gear oil churns, adding to the gearbox's losses.
//!
//! With failures on, parts wear beyond their limits: the pistons and the head gasket in
//! overheated cylinders, the bearings in overheated oil, the valvetrain when over-revved.
//! Worn pistons and valves lose compression and power, worn bearings add friction, and a
//! broken part stops the engine for good.
//!
//! Heat goes into the parts on every step; they exchange it with each other and the air
//! every [`crate::THERMAL_STEPS`] steps.

use crate::Airflow;
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
/// Heat the block sheds into the engine bay, relative to the radiator's at its rating.
const BLOCK_LOSS: f64 = 0.01;
/// The radiator's conductance relative to the heat capacity flow (ṁ·c_p) of the air
/// through it: the air through the bay at the rated airspeed carries the radiator's
/// rating over this, W/K.
const RADIATOR_EFFECTIVENESS: f64 = 0.5;
/// Air through the bay standing still in calm air (natural draught) relative to that at
/// the rated airspeed.
const STILL_BAY: f64 = 0.03;
/// Share of the intake's air drawn from the engine bay rather than from outside, and
/// the time the airbox and the manifold take to follow it, s.
const INTAKE_FROM_BAY: f64 = 0.3;
const INTAKE_TIME: f64 = 20.0;
/// Heat capacity of the gearbox with its oil and its cooling in the bay at the rated
/// airspeed, per W of the engine's peak power, J/K per W and W/K per W; conductance of
/// the bellhousing from the engine's oil to the gearbox's per litre of displacement,
/// W/(K·l).
const GEARBOX_CAPACITY_PER_POWER: f64 = 0.06;
const GEARBOX_COOLING_PER_POWER: f64 = 0.0005;
const BELLHOUSING: f64 = 10.0;
/// Cooling of the gearbox standing still relative to that at the rated airspeed.
const GEARBOX_STILL_AIR: f64 = 0.05;
/// Share of the gearbox's losses that is oil churning, which goes with the square root
/// of the gear oil's viscosity.
const CHURNING: f64 = 0.3;
/// The out-lap the engine is warmed on before a reset: its share of the peak power at
/// the crank, the engine's efficiency from fuel to crank, the friction's share of the
/// peak power, the gearbox's losses' share of the power and the airspeed, m/s.
const OUT_LAP_POWER: f64 = 0.3;
const OUT_LAP_EFFICIENCY: f64 = 0.3;
const OUT_LAP_FRICTION: f64 = 0.04;
const OUT_LAP_GEAR_LOSS: f64 = 0.05;
const OUT_LAP_SPEED: f64 = 30.0;
/// Length of the out-lap and the step it is run at, s.
const OUT_LAP_TIME: f64 = 900.0;
const OUT_LAP_STEP: f64 = 0.2;
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
    /// Temperature of the gearbox's oil, °C.
    pub gearbox: f64,
    /// Temperature of the air around the engine and of the air the intake draws, °C.
    pub bay: f64,
    pub intake: f64,
    pub wear: EngineWear,
    /// Combustion torque relative to a sound engine at its working temperature: the
    /// ignition retarded against knock, compression lost to worn parts.
    pub power: f64,
    /// Friction relative to the drag curve's (warm oil, sound bearings).
    pub friction: f64,
    /// The gearbox's losses relative to its efficiency's (warm gear oil).
    pub gearbox_loss: f64,
}

impl EngineHeat {
    /// An engine warmed up to its working temperature in standard air: nominal values;
    /// [`EngineThermal::warmed`] settles them for the car and the air.
    pub const WARM: Self = Self {
        cylinder: 100.0,
        coolant: 85.0,
        oil: OIL_REFERENCE,
        gearbox: OIL_REFERENCE,
        bay: 35.0,
        intake: 28.0,
        wear: EngineWear {
            pistons: 0.0,
            bearings: 0.0,
            valvetrain: 0.0,
        },
        power: 1.0,
        friction: 1.0,
        gearbox_loss: 1.0,
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
    gearbox_capacity: f64,
    gearbox_cooling: f64,
    bellhousing: f64,
    peak_power: f64,
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
            gearbox_capacity: GEARBOX_CAPACITY_PER_POWER * peak_power,
            gearbox_cooling: GEARBOX_COOLING_PER_POWER * peak_power,
            bellhousing: BELLHOUSING * litres,
            peak_power,
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

    /// Heats the gearbox's oil for `dt` with the gearbox losing `loss_power` W.
    #[inline]
    pub fn heat_gearbox(&self, h: &mut EngineHeat, loss_power: f64, dt: f64) {
        h.gearbox += dt * loss_power.max(0.0) / self.gearbox_capacity;
    }

    /// The engine after an out-lap in air at `temperature` °C of `density` kg/m³: its
    /// parts settled at their working temperatures in that air, sound.
    pub fn warmed(&self, temperature: f64, density: f64) -> EngineHeat {
        let air = Airflow {
            speed: OUT_LAP_SPEED,
            temperature,
            density,
        };
        let power = OUT_LAP_POWER * self.peak_power;
        let fuel_flow = power / (OUT_LAP_EFFICIENCY * FUEL_HEATING_VALUE);
        let mut h = EngineHeat {
            cylinder: self.thermostat,
            coolant: self.thermostat,
            oil: self.thermostat,
            gearbox: self.thermostat,
            bay: temperature,
            intake: temperature,
            ..EngineHeat::WARM
        };
        for _ in 0..(OUT_LAP_TIME / OUT_LAP_STEP) as usize {
            self.heat(
                &mut h,
                fuel_flow,
                OUT_LAP_FRICTION * self.peak_power,
                OUT_LAP_STEP,
            );
            self.heat_gearbox(&mut h, OUT_LAP_GEAR_LOSS * power, OUT_LAP_STEP);
            self.exchange(&mut h, 0.0, &air, 0.0, false, OUT_LAP_STEP);
        }
        h
    }

    /// Exchanges heat for `dt` between the parts and the `air` flowing into the car's
    /// nose; `nose_damage` (m/s of impacts) has crushed the coolers behind it. With
    /// `failures`, parts beyond their limits wear, and the engine turning at `speed`
    /// rad/s wears its valvetrain above the over-rev speed.
    pub fn exchange(
        &self,
        h: &mut EngineHeat,
        speed: f64,
        air: &Airflow,
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
        let speed_through = air.speed.max(fan);
        let through = intact * air.convection(speed_through);
        let opening =
            ((h.coolant - self.thermostat) / THERMOSTAT_RANGE).clamp(THERMOSTAT_LEAK, 1.0);
        let boil = ((h.coolant - self.boiling_point) / BOIL_BAND).clamp(0.0, 1.0);
        let t_air = air.temperature;
        let cylinder_to_coolant =
            self.cylinder_to_coolant * (1.0 - STEAM_LOSS * boil) * (h.cylinder - h.coolant);
        let cylinder_to_oil = self.cylinder_to_oil * (h.cylinder - h.oil);
        let oil_to_coolant = self.oil_to_coolant * (h.oil - h.coolant);
        let radiator = self.radiator * (opening * through + BLOCK_LOSS) * (h.coolant - t_air);
        let oil_cooler = self.oil_cooler * through * (h.oil - t_air);
        let bellhousing = self.bellhousing * (h.oil - h.gearbox);
        let gearbox_cooling = self.gearbox_cooling
            * (GEARBOX_STILL_AIR + air.convection(air.speed))
            * (h.gearbox - h.bay);
        h.cylinder -= dt * (cylinder_to_coolant + cylinder_to_oil) / self.cylinder_capacity;
        h.coolant += dt * (cylinder_to_coolant + oil_to_coolant - radiator) / self.coolant_capacity;
        h.oil +=
            dt * (cylinder_to_oil - oil_to_coolant - oil_cooler - bellhousing) / self.oil_capacity;
        h.gearbox += dt * (bellhousing - gearbox_cooling) / self.gearbox_capacity;

        // The air through the bay carries off what the engine sheds.
        let bay_flow = self.radiator / RADIATOR_EFFECTIVENESS
            * (STILL_BAY + intact * air.mass_flow(speed_through));
        h.bay = t_air + (radiator + oil_cooler + gearbox_cooling).max(0.0) / bay_flow;
        let drawn = t_air + INTAKE_FROM_BAY * (h.bay - t_air);
        h.intake += (drawn - h.intake) * (dt / INTAKE_TIME).min(1.0);

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
        h.friction =
            viscous_friction(h.oil, HYDRODYNAMIC_FRICTION) * (1.0 + BEARING_FRICTION * w.bearings);
        h.gearbox_loss = viscous_friction(h.gearbox, CHURNING);
    }
}

/// Friction relative to that with the oil at its reference temperature, of which the
/// share `viscous` goes with the square root of the oil's viscosity.
fn viscous_friction(oil: f64, viscous: f64) -> f64 {
    let viscosity =
        (VOGEL_B * (1.0 / (oil.max(-30.0) + VOGEL_C) - 1.0 / (OIL_REFERENCE + VOGEL_C))).exp();
    1.0 - viscous + viscous * viscosity.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CarModel;

    fn gt3() -> EngineThermal {
        CarModel::gt3().engine.thermal.clone()
    }

    /// Standard air flowing into the nose at `speed`.
    fn air(speed: f64) -> Airflow {
        Airflow {
            speed,
            ..Airflow::STILL
        }
    }

    /// Runs the engine for `seconds` burning `fuel_flow` with `friction_power`, at
    /// 7000 rpm, in standard air flowing at `airspeed`.
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
                &air(airspeed),
                nose_damage,
                failures,
                dt,
            );
        }
    }

    #[test]
    fn viscous_friction_is_one_warm_high_cold_and_low_hot() {
        let f = |oil| viscous_friction(oil, HYDRODYNAMIC_FRICTION);
        assert!((f(OIL_REFERENCE) - 1.0).abs() < 1e-12);
        let cold = f(20.0);
        assert!((1.6..2.4).contains(&cold), "{cold}");
        assert!(f(140.0) < 0.9);
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
            m.exchange(&mut h, over, &air(50.0), 0.0, true, 0.01);
        }
        assert!((0.05..0.2).contains(&h.wear.valvetrain), "{h:?}");
        assert!(h.power < 1.0 && !h.failed());
        m.exchange(
            &mut h,
            1.2 * m.over_rev_rpm / RPM_PER_RAD_S,
            &air(50.0),
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
            &air(50.0),
            0.0,
            false,
            0.01,
        );
        assert_eq!(h.wear, EngineWear::default());
    }

    #[test]
    fn the_out_lap_warms_the_engine_to_its_working_temperatures() {
        let m = gt3();
        let h = m.warmed(25.0, crate::AIR_DENSITY);
        assert!(
            (m.thermostat..m.thermostat + THERMOSTAT_RANGE).contains(&h.coolant),
            "{h:?}"
        );
        assert!((75.0..110.0).contains(&h.oil), "{h:?}");
        assert!((60.0..120.0).contains(&h.gearbox), "{h:?}");
        assert!((0.9..1.15).contains(&h.friction), "{h:?}");
        assert!(h.intake > 25.0 && h.intake < h.bay, "{h:?}");
        // A winter morning leaves the oils colder and thicker; the thermostat still
        // holds the coolant.
        let cold = m.warmed(0.0, 1.29);
        eprintln!(
            "{h:?}
{cold:?}"
        );
        assert!(
            cold.oil < h.oil - 5.0 && cold.gearbox < h.gearbox - 5.0,
            "{cold:?}"
        );
        assert!(cold.friction > h.friction && cold.gearbox_loss > h.gearbox_loss);
        assert!((cold.coolant - h.coolant).abs() < 5.0, "{cold:?} / {h:?}");
    }

    #[test]
    fn thin_air_cools_less() {
        let m = gt3();
        let thin = Airflow {
            density: 0.75 * crate::AIR_DENSITY,
            ..air(20.0)
        };
        let (mut sea, mut high) = (EngineHeat::WARM, EngineHeat::WARM);
        for (h, air) in [(&mut sea, air(20.0)), (&mut high, thin)] {
            for _ in 0..30_000 {
                m.heat(h, 0.02, 40e3, 0.01);
                m.exchange(h, 0.0, &air, 0.0, false, 0.01);
            }
        }
        assert!(high.coolant > sea.coolant + 3.0, "{high:?} / {sea:?}");
    }

    #[test]
    fn a_slow_crawl_heats_the_bay_and_the_intake() {
        let m = gt3();
        let mut fast = EngineHeat::WARM;
        let mut slow = EngineHeat::WARM;
        run(&m, &mut fast, 0.006, 15e3, 50.0, 0.0, false, 300.0);
        run(&m, &mut slow, 0.006, 15e3, 3.0, 0.0, false, 300.0);
        assert!(slow.bay > fast.bay + 15.0, "{slow:?} / {fast:?}");
        assert!(slow.intake > fast.intake + 5.0, "{slow:?} / {fast:?}");
    }
}
