//! Plausibility tests of the vehicle dynamics against reference figures of the bundled
//! cars, and of how the drive layout and weight distribution change a car's behaviour.

use std::sync::Arc;

use glam::DVec3;
use open_racing_sim::params::DifferentialParams;
use open_racing_sim::*;

fn circle(radius: f64) -> Track {
    let points = (0..32)
        .map(|i| {
            let a = i as f64 / 32.0 * std::f64::consts::TAU;
            TrackPoint {
                pos: (radius * a.cos(), radius * a.sin(), 0.0),
                width_left: 30.0,
                width_right: 30.0,
                bank: 0.0,
            }
        })
        .collect();
    Track::new(&TrackDef {
        name: "circle".into(),
        points,
        kerb_width: 1.0,
        kerb_height: 0.0,
        runoff_width: f64::INFINITY,
        spacing: 1.0,
    })
    .unwrap()
}

fn gt3() -> Arc<CarModel> {
    Arc::new(CarModel::gt3())
}

/// Upshifts just below the rev limiter.
fn shift_for(car: &Car) -> Shift {
    let (p, dt) = (&car.model.params, &car.state.drivetrain);
    let top = p.gearbox.ratios.len() as i32;
    if dt.rpm() > 0.96 * p.engine.limiter_rpm && dt.gear < top && dt.shift_timer == 0.0 {
        Shift::Up
    } else {
        Shift::None
    }
}

#[test]
fn settles_at_static_ride_height() {
    let track = circle(5000.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 0);
    let z0 = car.state.position.z;
    for _ in 0..3000 {
        car.step(
            &track,
            &Controls {
                brake: 0.3,
                ..Default::default()
            },
        );
    }
    assert!(car.speed() < 0.01, "speed {}", car.speed());
    assert!(
        (car.state.position.z - z0).abs() < 0.005,
        "z drift {}",
        car.state.position.z - z0
    );
    let total: f64 = car.telemetry.wheels.iter().map(|w| w.load).sum();
    let weight = car.model.params.mass * GRAVITY;
    assert!(
        (total - weight).abs() / weight < 0.01,
        "loads {total} vs {weight}"
    );
    let front = car.telemetry.wheels[FL].load + car.telemetry.wheels[FR].load;
    assert!((front / total - car.model.params.front_weight).abs() < 0.01);
}

#[test]
fn acceleration_and_top_speed() {
    let track = circle(5000.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
    let mut t100 = None;
    for _ in 0..70_000 {
        let shift = shift_for(&car);
        car.step(
            &track,
            &Controls {
                throttle: 1.0,
                shift,
                ..Default::default()
            },
        );
        if t100.is_none() && car.speed() >= 100.0 / 3.6 {
            t100 = Some(car.state.time);
        }
    }
    let t100 = t100.expect("reaches 100 km/h");
    let top = car.speed() * 3.6;
    // GT3: 0-100 in roughly 3-4 s (traction limited), top speed ~270-290 km/h.
    assert!((2.8..4.2).contains(&t100), "0-100 km/h in {t100:.2} s");
    assert!((255.0..300.0).contains(&top), "top speed {top:.1} km/h");
}

#[test]
fn braking_distance_from_100() {
    let track = circle(5000.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 100.0 / 3.6, 3);
    let start = car.state.position;
    // Threshold braking without ABS: find the best pedal pressure.
    let best = (50..=100)
        .step_by(5)
        .map(|b| {
            car.reset(&track, 0.0, 0.0, 100.0 / 3.6, 3);
            while car.speed() > 0.2 && car.state.time < 10.0 {
                car.step(
                    &track,
                    &Controls {
                        brake: b as f64 / 100.0,
                        clutch: 1.0,
                        ..Default::default()
                    },
                );
            }
            (car.state.position - start).length()
        })
        .fold(f64::INFINITY, f64::min);
    // Slick-shod GT3 cars stop from 100 km/h in roughly 25-35 m.
    assert!((22.0..36.0).contains(&best), "100-0 km/h in {best:.1} m");
}

/// Drives a constant-radius circle with a simple path/speed controller and returns the
/// highest lateral acceleration sustained for 3 s while staying on the line.
fn skidpad(model: Arc<CarModel>, radius: f64) -> f64 {
    let track = circle(radius);
    let mut car = Car::new(model, &track, 0.0, 0.0, 10.0, 2);
    let mut target = 10.0_f64;
    let mut best = 0.0_f64;
    let mut stable_since = 0.0;
    let mut hint = 0;
    let ratio = car.model.params.steering.ratio;
    let wheelbase = car.model.params.wheelbase;
    while car.state.time < 120.0 && target < 80.0 {
        let q = track.query(car.state.position, hint);
        hint = q.index;
        let heading_err = {
            let fwd = car.state.orientation * glam::DVec3::X;
            q.tangent.truncate().perp_dot(fwd.truncate()).asin()
        };
        let feedforward = (wheelbase / radius).atan();
        let steer = (feedforward
            - 0.15 * q.d
            - 1.5 * heading_err
            - 0.5 * car.local_velocity().y / car.speed().max(1.0))
            * ratio;
        let v = car.local_velocity().x;
        let throttle = (0.3 * (target - v) + 0.25).clamp(0.0, 1.0);
        // Brakes lightly when light throttle alone runs too fast.
        let brake = (0.3 * (v - target - 0.2)).clamp(0.0, 1.0);
        let shift = shift_for(&car);
        car.step(
            &track,
            &Controls {
                steer_wheel_angle: steer,
                throttle,
                brake,
                shift,
                ..Default::default()
            },
        );

        let ay = v * v / radius;
        if q.d.abs() < 1.0 && (v - target).abs() < 0.5 {
            if car.state.time - stable_since > 3.0 {
                best = best.max(ay);
                target += 0.5;
                stable_since = car.state.time;
            }
        } else if q.d.abs() >= 1.0 {
            stable_since = car.state.time;
            if q.d.abs() > 8.0 {
                break;
            }
        }
    }
    best / GRAVITY
}

#[test]
fn skidpad_lateral_grip() {
    let g = skidpad(gt3(), 60.0);
    eprintln!("skidpad: {g:.3} g");
    // Mechanical grip of a GT3 car on slicks at low speed: ~1.3-1.7 g.
    assert!((1.25..1.8).contains(&g), "max lateral {g:.2} g");
}

#[test]
fn deterministic() {
    let track = Track::default_circuit();
    let run = || {
        let mut car = Car::new(gt3(), &track, 50.0, 1.0, 20.0, 2);
        for k in 0..20_000 {
            let steer = (k as f64 * 0.001).sin() * 0.8;
            car.step(
                &track,
                &Controls {
                    steer_wheel_angle: steer,
                    throttle: 0.6,
                    ..Default::default()
                },
            );
        }
        car.state
    };
    let (a, b) = (run(), run());
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}

#[test]
fn runoff_barrier_contains_the_car() {
    let track = Track::default_circuit();
    // Start on the main straight, aimed 45° to the left at speed.
    let mut car = Car::new(gt3(), &track, 100.0, 0.0, 40.0, 3);
    let turn = glam::DQuat::from_rotation_z(std::f64::consts::FRAC_PI_4);
    car.state.orientation = turn * car.state.orientation;
    car.state.velocity = turn * car.state.velocity;
    let mut hint = 0;
    let mut closest = f64::NEG_INFINITY;
    for _ in 0..10_000 {
        car.step(
            &track,
            &Controls {
                throttle: 0.3,
                ..Default::default()
            },
        );
        let q = track.query(car.state.position, hint);
        hint = q.index;
        closest = closest.max(q.beyond_barrier(&track));
        assert!(closest < 0.0, "escaped: d = {}", q.d);
    }
    // The CG stops about half a car width inside the barrier.
    assert!(closest > -2.0, "never reached the barrier: {closest}");
}

/// Circle track whose tyres ride on a flat mesh, with a wall across the road ahead.
fn walled_circle() -> Track {
    let mut ground = GroundMeshBuilder::new();
    let road = ground.add_surface(SurfaceProps::of(Surface::Asphalt));
    let g = [
        DVec3::new(-200.0, -200.0, 0.0),
        DVec3::new(200.0, -200.0, 0.0),
        DVec3::new(200.0, 200.0, 0.0),
        DVec3::new(-200.0, 200.0, 0.0),
    ];
    ground.add_ground(&g, &[], &[0, 1, 2, 0, 2, 3], road);
    // The car starts at (100, 0) heading +Y; the wall stands at y = 20.
    let w = [
        DVec3::new(80.0, 20.0, 0.0),
        DVec3::new(120.0, 20.0, 0.0),
        DVec3::new(120.0, 20.0, 2.0),
        DVec3::new(80.0, 20.0, 2.0),
    ];
    ground.add_wall(&w, &[0, 1, 2, 0, 2, 3]);
    circle(100.0).with_ground(ground.build())
}

#[test]
fn rides_on_ground_mesh() {
    let track = walled_circle();
    let plain = circle(100.0);
    let mut on_mesh = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 0);
    let mut on_plane = Car::new(gt3(), &plain, 0.0, 0.0, 0.0, 0);
    for _ in 0..3000 {
        on_mesh.step(&track, &Controls::default());
        on_plane.step(&plain, &Controls::default());
    }
    assert!((on_mesh.state.position - on_plane.state.position).length() < 1e-6);
}

#[test]
fn wall_stops_the_car() {
    let track = walled_circle();
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 15.0, 2);
    let mut max_y = f64::NEG_INFINITY;
    for _ in 0..5000 {
        car.step(
            &track,
            &Controls {
                throttle: 0.3,
                ..Default::default()
            },
        );
        max_y = max_y.max(car.state.position.y);
    }
    // The CG stops about half a wheelbase plus a tyre radius short of the wall.
    assert!(max_y < 20.0, "went through the wall: y = {max_y:.2}");
    assert!(max_y > 16.0, "never reached the wall: y = {max_y:.2}");
    assert!(car.speed() < 5.0, "still moving at {:.1} m/s", car.speed());
}

fn asset_car(name: &str) -> Arc<CarModel> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/cars")
        .join(format!("{name}.ron"));
    Arc::new(CarModel::load(path).unwrap())
}

/// The GT3 car driving the given wheels.
fn gt3_driving(drive: Drive) -> Arc<CarModel> {
    let gt3 = CarModel::gt3();
    let params = CarParams {
        drive,
        ..gt3.params
    };
    Arc::new(CarModel::new(params, gt3.front_tire.p, gt3.rear_tire.p).unwrap())
}

/// A launch from standstill in first gear, the throttle eased off when the driven wheels
/// spin: the time to 100 km/h and the speed after `seconds`, km/h.
fn launch(model: Arc<CarModel>, seconds: f64) -> (Option<f64>, f64) {
    let track = circle(5000.0);
    let mut car = Car::new(model, &track, 0.0, 0.0, 0.0, 1);
    let mut t100 = None;
    while car.state.time < seconds {
        let shift = shift_for(&car);
        let spin = (0..4)
            .filter(|&i| car.model.corners[i].driven)
            .map(|i| car.state.wheels[i].kappa)
            .fold(0.0, f64::max);
        car.step(
            &track,
            &Controls {
                throttle: (1.0 - 8.0 * (spin - 0.1)).clamp(0.0, 1.0),
                shift,
                ..Default::default()
            },
        );
        if t100.is_none() && car.speed() >= 100.0 / 3.6 {
            t100 = Some(car.state.time);
        }
    }
    (t100, car.speed() * 3.6)
}

/// Slip ratios of the four wheels 0.3 s into a full-throttle launch.
fn launch_slips(model: Arc<CarModel>) -> [f64; 4] {
    let track = circle(5000.0);
    let mut car = Car::new(model, &track, 0.0, 0.0, 0.0, 1);
    while car.state.time < 0.3 {
        car.step(
            &track,
            &Controls {
                throttle: 1.0,
                ..Default::default()
            },
        );
    }
    car.state.wheels.map(|w| w.kappa)
}

fn open_differential() -> DifferentialParams {
    DifferentialParams {
        preload: 0.0,
        power_ramp: 0.0,
        coast_ramp: 0.0,
    }
}

#[test]
fn engine_drives_the_wheels_of_its_layout() {
    // An open centre differential: a locking one would hold back the GT3 car's smaller
    // front wheels, which turn faster than the rear ones.
    let all = Drive::All {
        front_share: 0.4,
        centre_differential: open_differential(),
        front_differential: CarModel::gt3().params.differential,
    };
    for (drive, driven) in [
        (Drive::Rear, [false, false, true, true]),
        (Drive::Front, [true, true, false, false]),
        (all, [true; 4]),
    ] {
        let slips = launch_slips(gt3_driving(drive.clone()));
        for (i, (&slip, &driven)) in slips.iter().zip(&driven).enumerate() {
            // A driven wheel slips forwards; a free one rolls and lags a little.
            if driven {
                assert!(slip > 0.01, "{drive:?}: wheel {i} slip {slip:.4}");
            } else {
                assert!(slip.abs() < 0.005, "{drive:?}: wheel {i} slip {slip:.4}");
            }
        }
    }
}

#[test]
fn launch_follows_the_driven_axles_load() {
    // The GT3 car carries 55 % of its weight on the rear and shifts more there as it
    // accelerates: the front wheels alone put the least power down, all four the most.
    let t100 = |drive| {
        launch(gt3_driving(drive), 12.0)
            .0
            .expect("reaches 100 km/h")
    };
    let gt3 = CarModel::gt3().params;
    let (rear, front, all) = (
        t100(Drive::Rear),
        t100(Drive::Front),
        t100(Drive::All {
            front_share: 0.4,
            centre_differential: open_differential(),
            front_differential: gt3.differential,
        }),
    );
    eprintln!("GT3 0-100 km/h: rear {rear:.2} s, front {front:.2} s, all {all:.2} s");
    assert!(
        all < rear && rear < front,
        "{all:.2} / {rear:.2} / {front:.2} s"
    );
}

#[test]
fn hot_hatch_figures() {
    let (t100, top) = launch(asset_car("hot_hatch"), 60.0);
    let t100 = t100.expect("reaches 100 km/h");
    let g = skidpad(asset_car("hot_hatch"), 60.0);
    eprintln!("hot hatch: 0-100 km/h {t100:.2} s, {top:.0} km/h after 60 s, skidpad {g:.3} g");
    // Front-wheel-drive hot hatches with ~230 kW: 0-100 in ~5.5-7 s, ~250 km/h, ~0.9-1 g.
    assert!((5.0..7.5).contains(&t100), "0-100 km/h in {t100:.2} s");
    assert!((225.0..265.0).contains(&top), "top speed {top:.0} km/h");
    assert!((0.85..1.15).contains(&g), "max lateral {g:.2} g");
}

#[test]
fn awd_sedan_figures() {
    let (t100, top) = launch(asset_car("awd_sedan"), 60.0);
    let t100 = t100.expect("reaches 100 km/h");
    let g = skidpad(asset_car("awd_sedan"), 60.0);
    eprintln!("AWD sedan: 0-100 km/h {t100:.2} s, {top:.0} km/h after 60 s, skidpad {g:.3} g");
    // All-wheel-drive sports sedans with ~250 kW: 0-100 in ~4.5-5.5 s, ~250 km/h, ~0.9-1 g.
    assert!((4.0..6.0).contains(&t100), "0-100 km/h in {t100:.2} s");
    assert!((225.0..275.0).contains(&top), "top speed {top:.0} km/h");
    assert!((0.85..1.15).contains(&g), "max lateral {g:.2} g");
}

/// Holds the car on a circle of `radius` at the speed of `lateral_g` for 20 s; returns
/// the mean road wheel angle over the last 3 s, the car and the circle.
fn hold_circle(model: Arc<CarModel>, radius: f64, lateral_g: f64) -> (f64, Car, Track) {
    let track = circle(radius);
    let target = (lateral_g * GRAVITY * radius).sqrt();
    let mut car = Car::new(model, &track, 0.0, 0.0, target, 2);
    let (ratio, wheelbase) = (car.model.params.steering.ratio, car.model.params.wheelbase);
    let (mut hint, mut integral, mut sum, mut n) = (0, 0.0, 0.0, 0);
    while car.state.time < 20.0 {
        let q = track.query(car.state.position, hint);
        hint = q.index;
        let fwd = car.state.orientation * DVec3::X;
        let heading_err = q.tangent.truncate().perp_dot(fwd.truncate()).asin();
        let steer = (wheelbase / radius).atan()
            - 0.15 * q.d
            - 1.5 * heading_err
            - 0.5 * car.local_velocity().y / car.speed().max(1.0);
        let v = car.local_velocity().x;
        integral += (target - v) * DT;
        let shift = AutoShift.shift(&car);
        car.step(
            &track,
            &Controls {
                steer_wheel_angle: steer * ratio,
                throttle: (0.5 * (target - v) + 0.3 * integral + 0.2).clamp(0.0, 1.0),
                brake: (0.3 * (v - target - 0.3)).clamp(0.0, 1.0),
                shift,
                ..Default::default()
            },
        );
        if car.state.time > 17.0 {
            sum += steer;
            n += 1;
        }
    }
    (sum / n as f64, car, track)
}

/// Extra road wheel angle, degrees, the car needs at 0.85 g over 0.3 g on a 60 m circle:
/// how much it understeers.
fn understeer(model: Arc<CarModel>) -> f64 {
    let steer = |g| hold_circle(model.clone(), 60.0, g).0;
    (steer(0.85) - steer(0.3)).to_degrees()
}

/// Time for the yaw rate to reach 90 % of its steady value after a 2° step of the road
/// wheels at 80 km/h, s.
fn yaw_response(model: Arc<CarModel>) -> f64 {
    let track = circle(5000.0);
    let ratio = model.params.steering.ratio;
    let mut car = Car::new(model, &track, 0.0, 0.0, 80.0 / 3.6, 3);
    let mut rates = Vec::new();
    while car.state.time < 3.0 {
        car.step(
            &track,
            &Controls {
                steer_wheel_angle: 2f64.to_radians() * ratio,
                throttle: 0.25,
                ..Default::default()
            },
        );
        rates.push((car.state.time, car.state.angular_velocity.z));
    }
    let steady = rates[rates.len() - 1].1;
    rates.iter().find(|r| r.1 >= 0.9 * steady).unwrap().0
}

/// Body slip angle, degrees, gained within 2 s of lifting off the throttle at 0.8 g on a
/// 60 m circle with the steering held: how far the rear steps out.
fn lift_off_rotation(model: Arc<CarModel>) -> f64 {
    let (steer, mut car, track) = hold_circle(model, 60.0, 0.8);
    let ratio = car.model.params.steering.ratio;
    let slip = |car: &Car| {
        let v = car.local_velocity();
        v.y.atan2(v.x).to_degrees().abs()
    };
    let before = slip(&car);
    let mut most = before;
    for _ in 0..2000 {
        car.step(
            &track,
            &Controls {
                steer_wheel_angle: steer * ratio,
                ..Default::default()
            },
        );
        most = most.max(slip(&car));
    }
    most - before
}

#[test]
fn mid_engine_puts_power_down_better_than_front_engine() {
    // Same engine, gearing and tyres: the mid-engined car's 58 % on the driven rear axle
    // lets it use more of the power off the line.
    let t100 = |name| launch(asset_car(name), 12.0).0.expect("reaches 100 km/h");
    let (fr, mr) = (t100("fr_coupe"), t100("mr_coupe"));
    eprintln!("0-100 km/h: FR {fr:.2} s, MR {mr:.2} s");
    assert!(mr < fr - 0.15, "MR {mr:.2} s, FR {fr:.2} s");
}

#[test]
fn mid_engine_turns_in_quicker_than_front_engine() {
    // Masses near the centre: less yaw inertia, a quicker yaw response.
    let (fr, mr) = (
        yaw_response(asset_car("fr_coupe")),
        yaw_response(asset_car("mr_coupe")),
    );
    eprintln!("yaw rate to 90 %: FR {fr:.3} s, MR {mr:.3} s");
    assert!(mr < fr, "MR {mr:.3} s, FR {fr:.3} s");
}

#[test]
fn understeer_follows_the_weight_on_the_front() {
    // Nose-heavy cars run wide as grip runs out: the front-drive hatch (61 % front) most,
    // then the front-engined coupé (52 %), the mid-engined one (42 %) least.
    let [hatch, fr, mr] = ["hot_hatch", "fr_coupe", "mr_coupe"].map(|n| understeer(asset_car(n)));
    eprintln!("understeer 0.3 -> 0.85 g: hatch {hatch:.2}°, FR {fr:.2}°, MR {mr:.2}°");
    assert!(hatch > fr && fr > mr, "{hatch:.2}° / {fr:.2}° / {mr:.2}°");
    assert!(mr > 0.0, "MR oversteers in steady state: {mr:.2}°");
}

#[test]
fn mid_engine_rotates_more_on_lift_off() {
    // Lifting mid-corner moves load to the front; with more weight at the rear and less
    // yaw inertia, the mid-engined car's tail steps out further.
    let (fr, mr) = (
        lift_off_rotation(asset_car("fr_coupe")),
        lift_off_rotation(asset_car("mr_coupe")),
    );
    eprintln!("lift-off body slip gain: FR {fr:.2}°, MR {mr:.2}°");
    assert!(mr > fr * 1.2, "MR {mr:.2}°, FR {fr:.2}°");
}
