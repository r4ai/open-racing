//! How each tunable of the engine, clutch and gearbox changes the drivetrain's behaviour,
//! stepping the drivetrain on its own with the driven wheels held at a fixed speed.

use std::path::Path;

use open_racing_sim::drivetrain::{self, DriveInput, DrivetrainState, RPM_PER_RAD_S, gear_ratio};
use open_racing_sim::engine::Ambient;
use open_racing_sim::*;

fn asset_car(name: &str) -> CarParams {
    CarParams::load(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../assets/cars/{name}.ron")),
    )
    .unwrap()
}

/// The GT3 car with a dual-clutch gearbox whose control unit is `control`.
fn dual_clutch(control: DualClutchControl) -> CarParams {
    let mut p = asset_car("gt3");
    p.gearbox.kind = GearboxKind::DualClutch {
        shift_time: 0.15,
        creep_torque: 0.0,
        launch_rpm: 4000.0,
        control,
    };
    p
}

/// Controls released, the driven wheels turning at `wheel_speed` and too heavy for the
/// drivetrain to change that.
fn held(wheel_speed: f64) -> DriveInput {
    DriveInput {
        throttle: 0.0,
        brake: 0.0,
        clutch_pedal: 0.0,
        shift: Shift::None,
        selector: None,
        air: Ambient::STANDARD,
        wheel_speed: [wheel_speed; 4],
        wheel_torque: [0.0; 4],
        wheel_inertia: [1e9; 4],
        dt: DT,
    }
}

/// Wheel speed at which `gear` turns the engine at `rpm`.
fn wheel_speed_for(p: &CarParams, gear: i32, rpm: f64) -> f64 {
    rpm / RPM_PER_RAD_S / gear_ratio(p, gear)
}

/// State in `gear` with the engine and input shaft at `rpm`.
fn at_rpm(p: &CarParams, gear: i32, rpm: f64) -> DrivetrainState {
    let mut s = DrivetrainState::new(p, gear);
    s.engine_speed = rpm / RPM_PER_RAD_S;
    s.input_speed = s.engine_speed;
    s
}

/// Steps once and returns the state after it.
fn step(mut s: DrivetrainState, p: &CarParams, input: &DriveInput) -> DrivetrainState {
    drivetrain::step(&mut s, p, &EngineModel::new(&p.engine), input);
    s
}

#[test]
fn the_ecu_blip_opens_fully_over_its_band() {
    // Between gears on a downshift, the engine 250 rpm short of 2nd's speed; the throttle
    // motor starts towards the blip.
    let blip = |band| {
        let mut p = asset_car("gt3");
        p.electronics.blip_band_rpm = band;
        let input = held(wheel_speed_for(&p, 2, 5000.0));
        let mut s = at_rpm(&p, 0, 4750.0);
        s.target_gear = 2;
        s.phase = ShiftPhase::Moving;
        step(s, &p, &input).engine.throttle
    };
    let (wide, narrow) = (blip(500.0), blip(250.0));
    assert!(
        wide > 0.0 && (narrow / wide - 2.0).abs() < 1e-9,
        "{wide:.4} / {narrow:.4}"
    );
}

#[test]
fn the_idle_control_band_sets_how_hard_it_holds_idle() {
    let gain = |band| {
        let mut p = asset_car("fr_sports");
        p.engine.idle_band_rpm = band;
        let rpm = p.engine.idle_rpm - 100.0;
        let input = DriveInput {
            clutch_pedal: 1.0,
            ..held(0.0)
        };
        step(at_rpm(&p, 0, rpm), &p, &input).rpm() - rpm
    };
    let (firm, lazy) = (gain(300.0), gain(1000.0));
    assert!(firm > 0.0 && firm > lazy, "{firm:.2} / {lazy:.2} rpm");
}

#[test]
fn anti_stall_opens_the_clutch_over_its_band() {
    // At rest in 1st, the engine 100 rpm above where anti-stall opens the clutch fully.
    let torque = |band| {
        let mut p = asset_car("gt3");
        let a = p.electronics.anti_stall.as_mut().unwrap();
        a.band_rpm = band;
        let rpm = a.rpm + 100.0;
        step(at_rpm(&p, 1, rpm), &p, &held(0.0)).clutch_torque
    };
    let (narrow, wide) = (torque(200.0), torque(1000.0));
    assert!(
        narrow > 3.0 * wide && wide > 0.0,
        "{narrow:.0} / {wide:.0} N·m"
    );
}

/// State with an H-pattern lever in 3rd's gate and the synchroniser at work, the input
/// shaft `slip` rad/s faster than 3rd turns it at 3000 rpm.
fn synchronising(p: &CarParams, slip: f64) -> (DrivetrainState, DriveInput) {
    let input = held(wheel_speed_for(p, 3, 3000.0));
    let mut s = at_rpm(p, 0, 3000.0);
    s.input_speed += slip;
    s.target_gear = 3;
    s.phase = ShiftPhase::Synchronising;
    (s, input)
}

#[test]
fn the_dogs_mesh_within_the_sync_window() {
    let gear = |window| {
        let mut p = asset_car("fr_sports");
        if let GearboxKind::HPattern { sync_window, .. } = &mut p.gearbox.kind {
            *sync_window = window;
        }
        let (s, input) = synchronising(&p, 5.0);
        let input = DriveInput {
            clutch_pedal: 1.0,
            ..input
        };
        step(s, &p, &input).gear
    };
    assert_eq!(gear(3.0), 0);
    assert_eq!(gear(10.0), 3);
}

#[test]
fn a_synchroniser_grinds_after_its_grind_time() {
    // With the clutch in, the engine keeps the input shaft away from 3rd's speed.
    let grinding = |time| {
        let mut p = asset_car("fr_sports");
        if let GearboxKind::HPattern { grind_time, .. } = &mut p.gearbox.kind {
            *grind_time = time;
        }
        let (mut s, input) = synchronising(&p, 50.0);
        s.phase_time = 0.5;
        step(s, &p, &input).grinding
    };
    assert!(grinding(0.3));
    assert!(!grinding(1.0));
}

#[test]
fn input_drag_slows_a_free_input_shaft() {
    let slowing = |drag| {
        let mut p = asset_car("fr_sports");
        p.gearbox.input_drag = drag;
        let input = DriveInput {
            clutch_pedal: 1.0,
            ..held(0.0)
        };
        let mut s = at_rpm(&p, 0, p.engine.idle_rpm);
        s.input_speed = 100.0;
        100.0 - step(s, &p, &input).input_speed
    };
    let (light, heavy) = (slowing(1.0), slowing(5.0));
    assert!(
        (heavy / light - 5.0).abs() < 0.01,
        "{light:.4} / {heavy:.4} rad/s"
    );
}

/// Time from putting the lever into reverse, clutch down and at rest, with the input shaft
/// still turning at idle, until reverse is in; and whether it ground meanwhile.
fn into_reverse(reverse: bool) -> (Option<f64>, bool) {
    let mut p = asset_car("fr_sports");
    if let GearboxKind::HPattern {
        reverse_synchro, ..
    } = &mut p.gearbox.kind
    {
        *reverse_synchro = reverse;
    }
    let input = DriveInput {
        clutch_pedal: 1.0,
        selector: Some(-1),
        ..held(0.0)
    };
    let mut s = at_rpm(&p, 0, p.engine.idle_rpm);
    let mut ground = false;
    for k in 0..5000 {
        drivetrain::step(&mut s, &p, &EngineModel::new(&p.engine), &input);
        ground |= s.grinding;
        if s.gear == -1 {
            return (Some(k as f64 * DT), ground);
        }
    }
    (None, ground)
}

#[test]
fn reverse_without_a_synchroniser_waits_for_the_input_shaft_to_stop() {
    let (synchro, ground) = into_reverse(true);
    let synchro = synchro.expect("goes into reverse");
    assert!(synchro < 0.2 && !ground, "took {synchro:.2} s");
    // Only the input shaft's drag slows it: the gear grinds until it has nearly stopped.
    let (plain, ground) = into_reverse(false);
    let plain = plain.expect("goes into reverse once the shaft stops");
    assert!(plain > 1.0 && ground, "took {plain:.2} s");
}

#[test]
fn a_dual_clutch_bites_later_pulling_away_with_a_higher_bite_speed() {
    // Part throttle against held wheels: the engine settles where the clutch bites.
    let rpm = |bite_rpm| {
        let p = dual_clutch(DualClutchControl {
            bite_rpm,
            ..Default::default()
        });
        let input = DriveInput {
            throttle: 0.3,
            ..held(0.0)
        };
        let mut s = at_rpm(&p, 1, p.engine.idle_rpm);
        for _ in 0..1000 {
            drivetrain::step(&mut s, &p, &EngineModel::new(&p.engine), &input);
        }
        s.rpm()
    };
    let (low, high) = (rpm(150.0), rpm(1000.0));
    assert!(high > low + 300.0, "{low:.0} / {high:.0} rpm");
}

#[test]
fn a_dual_clutch_engages_over_its_band() {
    // At rest in 1st with the throttle closed, 250 rpm above the bite speed.
    let torque = |band| {
        let p = dual_clutch(DualClutchControl {
            engage_band_rpm: band,
            ..Default::default()
        });
        let rpm = p.engine.idle_rpm + 150.0 + 250.0;
        step(at_rpm(&p, 1, rpm), &p, &held(0.0)).clutch_torque
    };
    let (short, long) = (torque(500.0), torque(2000.0));
    assert!(
        short > 3.0 * long && long > 0.0,
        "{short:.0} / {long:.0} N·m"
    );
}

/// Clutch torque a dual clutch takes from the engine at full throttle, slipping `slip` rpm
/// over `gear` turning it at `gear_rpm`, well below the launch speed.
fn closing(control: DualClutchControl, gear: i32, gear_rpm: f64, slip: f64) -> f64 {
    let p = dual_clutch(control);
    let input = DriveInput {
        throttle: 1.0,
        ..held(wheel_speed_for(&p, gear, gear_rpm))
    };
    let mut s = at_rpm(&p, gear, gear_rpm + slip);
    s.engine = EngineModel::new(&p.engine).settled(gear_rpm + slip, 1.0);
    s.clutch_locked = [false; 2];
    step(s, &p, &input).clutch_torque
}

#[test]
fn a_dual_clutch_closes_within_its_lock_slip() {
    let torque = |lock_slip_rpm| {
        let control = DualClutchControl {
            lock_slip_rpm,
            ..Default::default()
        };
        closing(control, 2, 3000.0, 150.0)
    };
    assert_eq!(torque(100.0), 0.0);
    assert!(torque(200.0) > 100.0);
}

#[test]
fn a_dual_clutch_closes_only_above_its_lock_speed() {
    let torque = |lock_rpm| {
        let control = DualClutchControl {
            lock_rpm,
            ..Default::default()
        };
        let gear_rpm = asset_car("gt3").engine.idle_rpm + 50.0;
        closing(control, 1, gear_rpm, 0.0)
    };
    assert_eq!(torque(100.0), 0.0);
    assert!(torque(0.0) > 100.0);
}

#[test]
fn a_dual_clutch_changes_down_below_its_downshift_speed() {
    let shift = |downshift_rpm| {
        let p = dual_clutch(DualClutchControl {
            downshift_rpm,
            ..Default::default()
        });
        let rpm = p.engine.idle_rpm + 300.0;
        let s = step(at_rpm(&p, 3, rpm), &p, &held(wheel_speed_for(&p, 3, rpm)));
        (s.phase, s.target_gear)
    };
    assert_eq!(shift(0.0), (ShiftPhase::None, 3));
    assert_eq!(shift(500.0), (ShiftPhase::Handover, 2));
}
