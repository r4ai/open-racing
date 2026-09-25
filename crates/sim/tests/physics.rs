//! Plausibility tests of the vehicle dynamics against reference figures of the bundled
//! cars, and of how the drive layout and weight distribution change a car's behaviour.

use std::sync::Arc;

use glam::DVec3;
use open_racing_sim::params::DifferentialParams;
use open_racing_sim::*;

fn circle(radius: f64) -> Track {
    circle_of_width(radius, 30.0)
}

/// A circle `width` wide on each side of its centreline, with grass beyond its kerbs.
fn circle_of_width(radius: f64, width: f64) -> Track {
    let points = (0..32)
        .map(|i| {
            let a = i as f64 / 32.0 * std::f64::consts::TAU;
            TrackPoint {
                pos: (radius * a.cos(), radius * a.sin(), 0.0),
                width_left: width,
                width_right: width,
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

/// Steps the car with the clutch worked by the clutch assist, as a driver pulling away
/// would.
fn step_with_clutch(car: &mut Car, track: &Track, assist: &mut ClutchAssist, mut c: Controls) {
    assist.apply(car, &mut c);
    car.step(track, &c);
}

/// Upshifts just below the rev limiter.
fn shift_for(car: &Car) -> Shift {
    let (p, dt) = (&car.model.params, &car.state.drivetrain);
    let top = p.gearbox.ratios.len() as i32;
    if dt.rpm() > 0.96 * p.engine.limiter_rpm && dt.gear < top && !dt.shifting() {
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
    let mut clutch = ClutchAssist::default();
    let mut t100 = None;
    for _ in 0..70_000 {
        let shift = shift_for(&car);
        step_with_clutch(
            &mut car,
            &track,
            &mut clutch,
            Controls {
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

#[test]
fn dusty_track_brakes_longer_and_rubbers_in() {
    let track = circle(5000.0);
    let map = Arc::new(RubberMap::new(&track));
    let stop = |evolution: &mut TrackEvolution| {
        let mut car = Car::new(gt3(), &track, 0.0, 0.0, 100.0 / 3.6, 3);
        let start = car.state.position;
        while car.speed() > 0.2 && car.state.time < 10.0 {
            let controls = Controls {
                brake: 0.7,
                clutch: 1.0,
                ..Default::default()
            };
            car.step_evolving(&track, evolution, &controls);
        }
        (car.state.position - start).length()
    };
    let optimum = stop(&mut TrackEvolution::default());
    let mut dusty = TrackEvolution::new(map, TrackCondition::Dusty.grip(), 0.01);
    let on_dusty = stop(&mut dusty);
    assert!(
        on_dusty > optimum * 1.05,
        "dusty {on_dusty:.1} m vs optimum {optimum:.1} m"
    );
    // The first metres of the stop now have rubber under the wheels, not beside them.
    let (s, d) = (2.0, -0.8);
    let dusty_grip = TrackCondition::Dusty.grip();
    assert!(dusty.grip_at(Surface::Asphalt, s, d) > dusty_grip);
    assert_eq!(dusty.grip_at(Surface::Asphalt, s, d + 5.0), dusty_grip);
}

#[test]
fn grass_coats_the_tyres_and_they_drop_it_on_the_road() {
    let track = circle(5000.0);
    let map = Arc::new(RubberMap::new(&track));
    let mut evolution = TrackEvolution::new(map, 1.0, 0.0);
    // Beyond the 30 m of track and 1 m of kerb: grass.
    let mut car = Car::new(gt3(), &track, 0.0, 33.0, 20.0, 2);
    for _ in 0..1000 {
        car.step_evolving(&track, &mut evolution, &Controls::default());
    }
    let coated = car.state.wheels.map(|w| w.tire.dirt());
    assert!(coated.iter().all(|&d| d > 0.5), "coats {coated:?}");
    assert!(
        car.state
            .wheels
            .iter()
            .all(|w| w.tire.coat[Coat::Grass as usize] == w.tire.dirt())
    );
    let s = track.query(car.state.position, 0).s;
    car.reset(&track, s, 0.0, 20.0, 2);
    for w in &mut car.state.wheels {
        w.tire.coat = [0.8, 0.0, 0.0];
    }
    let clean = evolution.grip_at(Surface::Asphalt, s + 20.0, 0.8);
    for _ in 0..1000 {
        car.step_evolving(&track, &mut evolution, &Controls::default());
    }
    assert!(car.state.wheels.iter().all(|w| w.tire.dirt() < 0.8));
    let dirty = (0..20)
        .map(|k| evolution.grip_at(Surface::Asphalt, s + 5.0 + k as f64, 0.8))
        .fold(f64::INFINITY, f64::min);
    assert!(dirty < clean - 0.01, "road {dirty} vs {clean}");
}

#[test]
fn tyre_contact_heats_asphalt_and_wheelspin_adds_friction_heat() {
    let track = circle(500.0);
    let map = Arc::new(RubberMap::new(&track));
    let weather = Weather::new(&track, None, WeatherSettings::default());
    let sample = |spin: f64, airborne: bool| {
        let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
        if airborne {
            car.state.position.z += 2.0;
        }
        car.state.wheels[2].spin = spin;
        let mut evolution = TrackEvolution::new(map.clone(), 1.0, 0.0);
        car.step_in(&track, &mut evolution, &weather, &Controls::default());
        let wheel = &car.telemetry.wheels[2];
        let q = track.query(wheel.contact, 0);
        (
            wheel.load,
            evolution.road_temperature_at(&weather, wheel.surface, q.s, q.d)
                - weather.road_temperature(q.s, q.d),
        )
    };
    let (load, rolling) = sample(0.0, false);
    let (_, spinning) = sample(100.0, false);
    let (air_load, airborne) = sample(100.0, true);
    assert!(load > 0.0 && rolling > 0.0, "{load} N, {rolling} °C");
    assert!(
        spinning > rolling,
        "wheelspin: {spinning} vs rolling: {rolling}"
    );
    assert_eq!(air_load, 0.0);
    assert_eq!(airborne, 0.0);
}

#[test]
fn sustained_wheelspin_warms_one_patch_while_cold_tread_can_cool_it() {
    let track = circle(500.0);
    let weather = Weather::new(&track, None, WeatherSettings::default());
    let map = Arc::new(RubberMap::new(&track));
    let base_car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
    let contact = |spin: f64, cold: bool| {
        let mut road = TrackEvolution::new(map.clone(), 1.0, 0.0);
        let mut patch = None;
        // Hold the same chassis state for repeated contact so the road heat budget
        // can be measured without the car accelerating away from the patch.
        for _ in 0..1000 {
            let mut car = base_car.clone();
            car.state.wheels[2].spin = spin;
            if cold {
                car.state.wheels[2].tire.tread_temperature = [0.0; 3];
            }
            car.step_in(&track, &mut road, &weather, &Controls::default());
            let q = track.query(car.telemetry.wheels[2].contact, 0);
            patch = Some((q.s, q.d));
        }
        let (s, d) = patch.unwrap();
        road.road_temperature_at(&weather, Surface::Asphalt, s, d) - weather.road_temperature(s, d)
    };
    let rolling = contact(0.0, false);
    let spinning = contact(100.0, false);
    let cold = contact(0.0, true);
    assert!(spinning > rolling + 0.5, "{spinning} vs {rolling} °C");
    assert!(cold < 0.0, "cold tyre: {cold} °C");
}

#[test]
fn sustained_lateral_sliding_heats_asphalt_more_than_rolling() {
    let track = circle(500.0);
    let weather = Weather::new(&track, None, WeatherSettings::default());
    let map = Arc::new(RubberMap::new(&track));
    let base = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
    let contact = |lateral_speed: f64| {
        let mut car = base.clone();
        let mut road = TrackEvolution::new(map.clone(), 1.0, 0.0);
        let mut patch = None;
        // Hold the chassis at one patch while the tyre's transient slip settles.
        for _ in 0..500 {
            car.state.position = base.state.position;
            car.state.orientation = base.state.orientation;
            car.state.velocity = car.state.orientation * DVec3::new(10.0, lateral_speed, 0.0);
            car.state.angular_velocity = DVec3::ZERO;
            for (i, wheel) in car.state.wheels.iter_mut().enumerate() {
                wheel.spin = 10.0 / car.model.tire(i).p.radius;
            }
            car.step_in(&track, &mut road, &weather, &Controls::default());
            let q = track.query(car.telemetry.wheels[2].contact, 0);
            patch = Some((q.s, q.d));
        }
        let (s, d) = patch.unwrap();
        road.road_temperature_at(&weather, Surface::Asphalt, s, d) - weather.road_temperature(s, d)
    };
    let rolling = contact(0.0);
    let sliding = contact(5.0);
    assert!(sliding > rolling + 0.1, "{sliding} vs {rolling} °C");
}

#[test]
fn locally_warm_asphalt_feeds_back_into_tread_temperature() {
    let track = circle(500.0);
    let map = Arc::new(RubberMap::new(&track));
    let mut cool_car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
    let mut cool_road = TrackEvolution::new(map.clone(), 1.0, 0.0);
    cool_car.step_in(
        &track,
        &mut cool_road,
        &Weather::STANDARD,
        &Controls::default(),
    );
    let mut warm_car = cool_car.clone();
    let mut warm_road = cool_road.clone();
    let q = track.query(cool_car.telemetry.wheels[2].contact, 0);
    warm_road.deposit_heat(q.s, q.d, 200_000.0);
    for _ in 0..1000 {
        cool_car.step_in(
            &track,
            &mut cool_road,
            &Weather::STANDARD,
            &Controls::default(),
        );
        warm_car.step_in(
            &track,
            &mut warm_road,
            &Weather::STANDARD,
            &Controls::default(),
        );
    }
    let temp = |car: &Car| {
        car.state.wheels[2]
            .tire
            .tread_temperature
            .iter()
            .sum::<f64>()
            / 3.0
    };
    assert!(temp(&warm_car) > temp(&cool_car) + 0.005);
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
    let mut clutch = ClutchAssist::default();
    let mut t100 = None;
    while car.state.time < seconds {
        let shift = shift_for(&car);
        let spin = (0..4)
            .filter(|&i| car.model.corners[i].driven)
            .map(|i| car.state.wheels[i].kappa)
            .fold(0.0, f64::max);
        step_with_clutch(
            &mut car,
            &track,
            &mut clutch,
            Controls {
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
    let mut clutch = ClutchAssist::default();
    while car.state.time < 0.3 {
        step_with_clutch(
            &mut car,
            &track,
            &mut clutch,
            Controls {
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

#[test]
fn kerb_ridges_shake_the_steering_on_a_straight() {
    let shake = |kerb_height| {
        let mut track = circle(5000.0);
        track.kerb_height = kerb_height;
        // Left wheels in the middle of the left kerb, right wheels on the asphalt.
        let model = gt3();
        let d = 30.5 - 0.5 * model.params.track_front;
        let mut car = Car::new(model, &track, 0.0, d, 30.0, 4);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for k in 0..2000 {
            car.step(
                &track,
                &Controls {
                    throttle: 0.3,
                    ..Default::default()
                },
            );
            if k >= 500 {
                lo = lo.min(car.telemetry.steering_torque);
                hi = hi.max(car.telemetry.steering_torque);
            }
        }
        assert_eq!(car.telemetry.wheels[FL].surface, Surface::Kerb);
        hi - lo
    };
    let (flat, ridged) = (shake(0.0), shake(0.03));
    assert!(flat < 0.05, "flat kerb shakes the wheel by {flat:.2} N·m");
    assert!(
        ridged > 1.0,
        "ridged kerb shakes the wheel by only {ridged:.2} N·m"
    );
}

/// Steering torque and lateral acceleration (g) after holding `steer` for 3 s from
/// `speed` (negative: in reverse) on an open field of asphalt, or of grass.
fn steady_steering(grass: bool, speed: f64, steer: f64) -> (f64, f64) {
    // Far outside a narrow road, or on a wide one: the car circles without leaving it.
    let (track, d) = if grass {
        (circle_of_width(5000.0, 1.0), -200.0)
    } else {
        (circle_of_width(5000.0, 1000.0), 0.0)
    };
    let gear = if speed < 0.0 { -1 } else { 2 };
    let mut car = Car::new(gt3(), &track, 0.0, d, 0.0, gear);
    car.state.velocity = car.state.orientation * DVec3::X * speed;
    for wheel in &mut car.state.wheels {
        wheel.spin = speed / car.model.front_tire.p.radius;
    }
    for _ in 0..3000 {
        let hold = (car.local_velocity().x.abs() < speed.abs()) as i32 as f64;
        car.step(
            &track,
            &Controls {
                throttle: 0.3 * hold,
                steer_wheel_angle: steer,
                ..Default::default()
            },
        );
    }
    (
        car.telemetry.steering_torque,
        car.telemetry.acceleration.y / GRAVITY,
    )
}

#[test]
fn loose_ground_lightens_the_steering() {
    let (asphalt, asphalt_g) = steady_steering(false, 14.0, 0.6);
    let (grass, grass_g) = steady_steering(true, 14.0, 0.6);
    let per_g = |torque: f64, g: f64| (torque / g).abs();
    assert!(
        per_g(grass, grass_g) < 0.8 * per_g(asphalt, asphalt_g),
        "grass {grass:.2} N·m at {grass_g:.2} g, asphalt {asphalt:.2} N·m at {asphalt_g:.2} g"
    );
}

#[test]
fn reversing_leaves_the_steering_light() {
    let (forward, _) = steady_steering(false, 5.0, 0.5);
    let (reverse, _) = steady_steering(false, -5.0, 0.5);
    assert!(forward < -1.0, "forward {forward:.2} N·m does not centre");
    assert!(
        reverse.abs() < 0.3 * forward.abs(),
        "reverse {reverse:.2} N·m, forward {forward:.2} N·m"
    );
}

/// Steering torque of the parked GT3 on an open field of asphalt or grass, sampled
/// every 0.1 s while the wheel winds to 90° over a second and back by 30° over the next.
fn parked_twist(grass: bool) -> Vec<f64> {
    let (track, d) = if grass {
        (circle_of_width(5000.0, 1.0), -200.0)
    } else {
        (circle_of_width(5000.0, 1000.0), 0.0)
    };
    let mut car = Car::new(gt3(), &track, 0.0, d, 0.0, 0);
    let mut torque = Vec::new();
    for k in 0..3000 {
        let degrees = match k {
            0..1000 => 0.0,
            1000..2000 => (k - 1000) as f64 * 0.09,
            _ => 90.0 - (k - 2000) as f64 * 0.03,
        };
        car.step(
            &track,
            &Controls {
                brake: 0.3,
                steer_wheel_angle: degrees.to_radians(),
                ..Default::default()
            },
        );
        if k >= 1000 && k % 100 == 0 {
            torque.push(car.telemetry.steering_torque);
        }
    }
    torque
}

#[test]
fn a_parked_tyre_winds_up_like_rubber_and_springs_back() {
    let asphalt = parked_twist(false);
    // Winding up: resists at once, then ever more softly as the patch slips round.
    let (first, wound) = (asphalt[1] - asphalt[0], asphalt[10] - asphalt[9]);
    assert!(first < -1.0, "barely resists at first: {first:.2} N·m");
    assert!(
        wound < 0.0 && wound.abs() < 0.5 * first.abs(),
        "no softening: {first:.2} then {wound:.2} N·m per 9°"
    );
    // Turning back is stiff again: a few degrees give most of it back.
    let back = asphalt[13] - asphalt[10];
    assert!(
        back > 3.0 * wound.abs(),
        "no spring back: {back:.2} N·m over 9° back, {wound:.2} per 9° winding"
    );
    // Loose ground holds the patch less.
    let grass = parked_twist(true);
    assert!(
        grass[10].abs() < 0.7 * asphalt[10].abs(),
        "grass {:.2} N·m, asphalt {:.2} N·m",
        grass[10],
        asphalt[10]
    );
}

// ---- Engine, clutch and gearbox ------------------------------------------------------

/// Speed in km/h after `seconds` of `controls`, worked through the clutch assist if given.
fn roll(
    car: &mut Car,
    track: &Track,
    mut clutch: Option<ClutchAssist>,
    controls: Controls,
    seconds: f64,
) {
    let end = car.state.time + seconds;
    while car.state.time < end {
        let mut c = controls;
        if let Some(assist) = clutch.as_mut() {
            assist.apply(car, &mut c);
        }
        car.step(track, &c);
    }
}

/// A GT3 car with a dual-clutch gearbox instead of its sequential one.
fn gt3_dual_clutch(creep_torque: f64) -> Arc<CarModel> {
    let gt3 = CarModel::gt3();
    let mut params = gt3.params.clone();
    params.gearbox.kind = GearboxKind::DualClutch {
        shift_time: 0.15,
        creep_torque,
        launch_rpm: 4000.0,
        control: Default::default(),
    };
    params.electronics = ElectronicsParams {
        auto_blip: true,
        ..Default::default()
    };
    Arc::new(CarModel::new(params, gt3.front_tire.p.clone(), gt3.rear_tire.p.clone()).unwrap())
}

#[test]
fn clutch_assist_holds_the_car_at_rest_with_the_throttle_closed() {
    let track = circle(5000.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
    roll(
        &mut car,
        &track,
        Some(ClutchAssist::default()),
        Controls::default(),
        10.0,
    );
    assert!(
        car.speed() < 0.5 / 3.6,
        "creeps at {:.1} km/h",
        car.speed() * 3.6
    );
    assert!(!car.state.drivetrain.stalled);

    // With the clutch let in, the idling engine drives the car at its idle speed in 1st:
    // the GT3's anti-stall slips the clutch to pull away rather than stall.
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
    roll(&mut car, &track, None, Controls::default(), 10.0);
    let p = &car.model.params;
    let idle_speed = p.engine.idle_rpm / drivetrain::RPM_PER_RAD_S / drivetrain::gear_ratio(p, 1)
        * car.model.rear_tire.p.radius;
    assert!(!car.state.drivetrain.stalled);
    assert!(
        (car.speed() - idle_speed).abs() < 0.1 * idle_speed,
        "{:.1} km/h, idle speed {:.1} km/h",
        car.speed() * 3.6,
        idle_speed * 3.6
    );
}

#[test]
fn a_manual_car_stalls_unless_the_clutch_is_slipped_to_pull_away() {
    let track = circle(5000.0);
    let model = asset_car("fr_sports");
    let mut car = Car::new(model.clone(), &track, 0.0, 0.0, 0.0, 1);
    roll(
        &mut car,
        &track,
        None,
        Controls {
            throttle: 0.3,
            ..Default::default()
        },
        2.0,
    );
    assert!(
        car.state.drivetrain.stalled,
        "pulled away with the clutch dropped"
    );

    let mut car = Car::new(model, &track, 0.0, 0.0, 0.0, 1);
    let throttle = Controls {
        throttle: 0.5,
        ..Default::default()
    };
    roll(
        &mut car,
        &track,
        Some(ClutchAssist::default()),
        throttle,
        3.0,
    );
    assert!(!car.state.drivetrain.stalled);
    assert!(
        car.speed() > 15.0 / 3.6,
        "only {:.1} km/h",
        car.speed() * 3.6
    );
}

#[test]
fn braking_to_rest_in_gear_stalls_without_anti_stall() {
    let track = circle(5000.0);
    let brake = Controls {
        brake: 0.4,
        ..Default::default()
    };
    let stop = |model| {
        let mut car = Car::new(model, &track, 0.0, 0.0, 50.0 / 3.6, 2);
        roll(&mut car, &track, None, brake, 5.0);
        assert!(car.speed() < 0.1);
        car.state.drivetrain.stalled
    };
    assert!(!stop(gt3()), "the GT3's anti-stall let it stall");
    assert!(stop(asset_car("fr_sports")), "survived without anti-stall");
    // The clutch assist opens the clutch in time.
    let mut car = Car::new(asset_car("fr_sports"), &track, 0.0, 0.0, 50.0 / 3.6, 2);
    roll(&mut car, &track, Some(ClutchAssist::default()), brake, 5.0);
    assert!(!car.state.drivetrain.stalled);
}

#[test]
fn fr_sports_figures() {
    // A 150 kW, 1.3 t sports car on road tyres: 0-100 km/h in roughly 7-8 s.
    let track = circle(50_000.0);
    let mut car = Car::new(asset_car("fr_sports"), &track, 0.0, 0.0, 0.0, 1);
    let mut clutch = ClutchAssist::default();
    let mut t100 = None;
    while car.state.time < 15.0 && t100.is_none() {
        // Lifts for each shift, as a driver of a manual car does.
        let shifting = car.state.drivetrain.shifting();
        let mut c = Controls {
            throttle: if shifting { 0.0 } else { 1.0 },
            shift: AutoShift.shift(&car),
            ..Default::default()
        };
        clutch.apply(&car, &mut c);
        car.step(&track, &c);
        if car.speed() >= 100.0 / 3.6 {
            t100 = Some(car.state.time);
        }
    }
    let t100 = t100.expect("reaches 100 km/h");
    assert!((6.5..9.0).contains(&t100), "0-100 km/h in {t100:.2} s");
}

#[test]
fn an_h_pattern_gear_goes_in_only_once_the_synchroniser_matches_speeds() {
    let track = circle(5000.0);
    let model = asset_car("fr_sports");
    let shift_up = |clutch: f64| {
        let mut car = Car::new(model.clone(), &track, 0.0, 0.0, 60.0 / 3.6, 2);
        let mut engaged = None;
        for k in 0..1000 {
            car.step(
                &track,
                &Controls {
                    throttle: 0.3,
                    clutch,
                    shift: if k == 0 { Shift::Up } else { Shift::None },
                    ..Default::default()
                },
            );
            if engaged.is_none() && car.state.drivetrain.gear == 3 {
                engaged = Some(car.state.time);
            }
        }
        (engaged, car.state.drivetrain)
    };
    // With the engine driving the input shaft, the synchroniser cannot slow it: it grinds.
    let (engaged, dt) = shift_up(0.0);
    assert!(
        engaged.is_none(),
        "went into 3rd with the clutch in at {engaged:?} s"
    );
    assert!(dt.grinding && dt.target_gear == 3);
    // With the clutch down it only has the input shaft to slow: the lever's travel and a
    // fraction of a second.
    let (engaged, _) = shift_up(1.0);
    let engaged = engaged.expect("goes into 3rd with the clutch down");
    assert!(engaged < 0.4, "took {engaged:.2} s");
}

#[test]
fn an_h_shifter_selects_gates_directly() {
    let track = circle(5000.0);
    let mut car = Car::new(asset_car("fr_sports"), &track, 0.0, 0.0, 0.0, 0);
    let gate = |g| Controls {
        clutch: 1.0,
        selector: Some(g),
        ..Default::default()
    };
    roll(&mut car, &track, None, gate(1), 0.5);
    assert_eq!(car.state.drivetrain.gear, 1);
    roll(&mut car, &track, None, gate(-1), 1.0);
    assert_eq!(car.state.drivetrain.gear, -1);
    roll(&mut car, &track, None, gate(0), 0.1);
    assert_eq!(car.state.drivetrain.gear, 0);
}

/// Peak rear-wheel slip over a downshift from 4th to 3rd at 100 km/h, the throttle closed.
fn downshift_slip(model: Arc<CarModel>, blip: bool) -> f64 {
    let track = circle(5000.0);
    let mut car = Car::new(model, &track, 0.0, 0.0, 100.0 / 3.6, 4);
    let mut worst: f64 = 0.0;
    for k in 0..1000 {
        let mut c = Controls {
            shift: if k == 100 { Shift::Down } else { Shift::None },
            ..Default::default()
        };
        if blip {
            BlipAssist.apply(&car, &mut c);
        }
        car.step(&track, &c);
        worst = worst.max(-car.state.wheels[RL].kappa.min(car.state.wheels[RR].kappa));
    }
    assert_eq!(car.state.drivetrain.gear, 3);
    worst
}

#[test]
fn a_blip_smooths_a_sequential_downshift() {
    let model = asset_car("fr_coupe");
    let (dry, blipped) = (
        downshift_slip(model.clone(), false),
        downshift_slip(model, true),
    );
    assert!(
        blipped < 0.5 * dry,
        "rear slip {dry:.3} without a blip, {blipped:.3} with"
    );
}

#[test]
fn downshift_protection_refuses_an_over_rev() {
    let track = circle(5000.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 170.0 / 3.6, 3);
    let down = Controls {
        shift: Shift::Down,
        ..Default::default()
    };
    car.step(&track, &down);
    assert!(
        !car.state.drivetrain.shifting(),
        "accepted a downshift to 2nd at 170 km/h"
    );
    // Slow enough, the same request goes through.
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 120.0 / 3.6, 3);
    car.step(&track, &down);
    assert!(car.state.drivetrain.shifting());
}

#[test]
fn a_dual_clutch_upshift_keeps_the_drive() {
    let track = circle(50_000.0);
    let mut car = Car::new(gt3_dual_clutch(0.0), &track, 0.0, 0.0, 80.0 / 3.6, 2);
    let full = Controls {
        throttle: 1.0,
        ..Default::default()
    };
    roll(&mut car, &track, None, full, 0.5);
    let mut slowest = f64::INFINITY;
    let mut shifted = false;
    for k in 0..400 {
        let before = car.speed();
        car.step(
            &track,
            &Controls {
                shift: if k == 0 { Shift::Up } else { Shift::None },
                ..full
            },
        );
        slowest = slowest.min((car.speed() - before) / DT);
        shifted |= car.state.drivetrain.shifting();
    }
    assert!(shifted && car.state.drivetrain.gear == 3);
    assert!(slowest > 1.0, "the drive dropped: {slowest:.2} m/s²");
}

#[test]
fn a_dual_clutch_creeps_pulls_away_and_stops_on_its_own() {
    let track = circle(5000.0);
    // Creep with neither pedal pressed; hold still on the brake.
    let mut car = Car::new(gt3_dual_clutch(40.0), &track, 0.0, 0.0, 0.0, 1);
    roll(&mut car, &track, None, Controls::default(), 5.0);
    assert!(
        car.speed() > 1.0 / 3.6,
        "no creep: {:.2} km/h",
        car.speed() * 3.6
    );
    let mut car = Car::new(gt3_dual_clutch(40.0), &track, 0.0, 0.0, 0.0, 1);
    let brake = Controls {
        brake: 0.2,
        ..Default::default()
    };
    roll(&mut car, &track, None, brake, 5.0);
    assert!(car.speed() < 0.1 / 3.6);
    // Pulls away at full throttle without stalling, and brakes to rest in gear without
    // stalling either, dropping gears as it slows.
    let full = Controls {
        throttle: 1.0,
        ..Default::default()
    };
    roll(&mut car, &track, None, full, 3.0);
    assert!(car.speed() > 60.0 / 3.6, "{:.1} km/h", car.speed() * 3.6);
    car.step(
        &track,
        &Controls {
            shift: Shift::Up,
            ..full
        },
    );
    roll(&mut car, &track, None, full, 2.0);
    roll(
        &mut car,
        &track,
        None,
        Controls {
            brake: 0.5,
            ..Default::default()
        },
        6.0,
    );
    let dt = car.state.drivetrain;
    assert!(car.speed() < 0.1 && !dt.stalled && dt.gear == 1, "{dt:?}");
}

/// Brakes `model` from `from` to `to` km/h at the limit and floors it back up, `stops`
/// times, on a near-straight, with the gears picked for it. Returns the front-left
/// brake at the end of each stop and the distance each stop took, m.
fn brake_repeatedly(
    model: Arc<CarModel>,
    from: f64,
    to: f64,
    stops: usize,
) -> (Vec<BrakeState>, Vec<f64>) {
    let radius = 5000.0;
    let track = circle(radius);
    let mut car = Car::new(model, &track, 0.0, 0.0, from / 3.6, 4);
    let (ratio, wheelbase) = (car.model.params.steering.ratio, car.model.params.wheelbase);
    let (mut brakes, mut distances) = (Vec::new(), Vec::new());
    let mut hint = 0;
    let mut drive = |car: &mut Car, throttle: f64, brake: f64| {
        // Follows the centreline.
        let q = track.query(car.state.position, hint);
        hint = q.index;
        let fwd = car.state.orientation * DVec3::X;
        let heading_err = q.tangent.truncate().perp_dot(fwd.truncate()).asin();
        let steer = (wheelbase / radius).atan() - 0.02 * q.d - 0.5 * heading_err;
        let shift = AutoShift.shift(car);
        car.step(
            &track,
            &Controls {
                steer_wheel_angle: steer * ratio,
                throttle,
                brake,
                shift,
                ..Default::default()
            },
        );
    };
    for _ in 0..stops {
        let start = car.state.position;
        let t = car.state.time;
        while car.speed() > to / 3.6 {
            // Eases off as a wheel starts to lock, as ABS would.
            let locking = car.telemetry.wheels.iter().any(|w| w.slip_ratio < -0.1);
            drive(&mut car, 0.0, if locking { 0.4 } else { 1.0 });
            assert!(
                car.state.time < t + 30.0,
                "stuck braking at {:.1} m/s",
                car.speed()
            );
        }
        distances.push((car.state.position - start).length());
        brakes.push(car.state.wheels[0].brake);
        let t = car.state.time;
        while car.speed() < from / 3.6 {
            drive(&mut car, 1.0, 0.0);
            assert!(
                car.state.time < t + 60.0,
                "stuck at {:.1} m/s: {:?}",
                car.speed(),
                car.state.drivetrain
            );
        }
    }
    (brakes, distances)
}

#[test]
fn hard_stops_bring_racing_brakes_up_to_temperature_and_warm_the_rims() {
    let model = gt3();
    let start = model.brakes.fresh(AMBIENT_TEMPERATURE);
    let (brakes, _) = brake_repeatedly(model, 200.0, 60.0, 6);
    eprintln!("{brakes:#?}");
    let last = brakes[brakes.len() - 1];
    // Each stop leaves the discs hotter, working up to 400-700 C; the calipers stay far
    // cooler.
    assert!(last.disc > brakes[0].disc + 50.0, "{brakes:?}");
    assert!((400.0..750.0).contains(&last.disc), "{last:?}");
    assert!(
        last.effectiveness > 0.95,
        "racing pads in their window: {last:?}"
    );
    assert!(
        last.caliper > start.caliper && last.caliper < 250.0,
        "{last:?}"
    );
    assert!(
        last.rim > start.rim + 5.0,
        "the discs warm the rims: {last:?}"
    );
}

#[test]
fn road_brakes_fade_when_worked_hard() {
    let (brakes, distances) = brake_repeatedly(asset_car("hot_hatch"), 180.0, 50.0, 8);
    eprintln!(
        "stops {distances:.0?} m
{brakes:#?}"
    );
    let (first, last) = (brakes[0], brakes[brakes.len() - 1]);
    assert!(last.disc > first.disc + 150.0, "{first:?} / {last:?}");
    assert!(
        last.effectiveness < 0.9 && first.effectiveness > 0.95,
        "{first:?} / {last:?}"
    );
}

#[test]
fn damage_can_be_switched_off() {
    let track = walled_circle();
    let hit = |damage: bool| {
        let mut car = Car::new(gt3(), &track, 0.0, 0.0, 25.0, 2);
        car.realism.damage = damage;
        for _ in 0..3000 {
            car.step(&track, &Controls::default());
        }
        car.state.damage
    };
    assert!(hit(true)[0] > 0.0, "{:?}", hit(true));
    assert_eq!(hit(false), [0.0; 4]);
}

/// Holds the GT3's engine at full throttle in neutral at rest for `seconds`.
fn rev_at_rest(failures: bool, seconds: f64) -> Car {
    let track = circle(200.0);
    let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 0);
    car.realism.failures = failures;
    for _ in 0..(seconds / DT) as usize {
        car.step(
            &track,
            &Controls {
                throttle: 1.0,
                brake: 1.0,
                ..Default::default()
            },
        );
    }
    car
}

#[test]
fn an_engine_held_on_the_limiter_at_rest_overheats_and_fails() {
    let mut car = rev_at_rest(true, 600.0);
    let heat = car.state.drivetrain.engine.heat;
    eprintln!("{heat:?}");
    assert!(heat.failed(), "{heat:?}");
    assert!(car.state.drivetrain.rpm() < 100.0, "a broken engine stops");
    car.restart_engine();
    assert!(car.state.drivetrain.stalled, "and does not start again");
    // Without failures it only runs hot, down on power.
    let heat = rev_at_rest(false, 600.0).state.drivetrain.engine.heat;
    assert!(
        !heat.failed() && heat.coolant > 110.0 && heat.power < 1.0,
        "{heat:?}"
    );
}

/// Weather of `month` at `hour`, with the air `offset` K off the season's.
fn weather_of(track: &Track, month: u32, hour: f64, offset: f64) -> Weather {
    Weather::new(
        track,
        None,
        WeatherSettings {
            sky: Sky::Clear,
            dynamic: false,
            hour,
            month,
            temperature_offset: offset,
            ..Default::default()
        },
    )
}

#[test]
fn a_reset_starts_the_car_from_the_day_s_weather() {
    let track = circle(200.0);
    let winter = weather_of(&track, 1, 7.0, -5.0);
    let summer = weather_of(&track, 7, 15.0, 5.0);
    assert!(summer.air_temperature() > winter.air_temperature() + 20.0);
    let reset = |weather: &Weather| {
        let mut car = Car::new(gt3(), &track, 0.0, 0.0, 0.0, 1);
        car.reset_in(&track, weather, 0.0, 0.0, 0.0, 1);
        car.state
    };
    let (cold, hot) = (reset(&winter), reset(&summer));
    let (c, h) = (cold.wheels[0], hot.wheels[0]);
    assert!(c.brake.caliper < h.brake.caliper && c.brake.rim < h.brake.rim);
    assert!(c.tire.inflated_at < h.tire.inflated_at - 20.0);
    let (ce, he) = (cold.drivetrain.engine.heat, hot.drivetrain.engine.heat);
    assert!(
        ce.oil < he.oil && ce.gearbox < he.gearbox,
        "{ce:?} / {he:?}"
    );
    assert!(ce.friction > he.friction, "{ce:?} / {he:?}");
}

#[test]
fn a_hot_engine_bay_warms_the_intake_and_the_gearbox() {
    // Revving at rest on a hot day: the engine sheds its heat into air that barely
    // moves, and the intake draws from the bay.
    let track = circle(200.0);
    let weather = weather_of(&track, 7, 15.0, 5.0);
    let mut car = Car::new(asset_car("fr_coupe"), &track, 0.0, 0.0, 0.0, 0);
    car.reset_in(&track, &weather, 0.0, 0.0, 0.0, 0);
    let start = car.state.drivetrain.engine.heat;
    let mut evolution = TrackEvolution::UNIFORM;
    let controls = Controls {
        throttle: 0.5,
        brake: 1.0,
        ..Default::default()
    };
    for _ in 0..(120.0 / DT) as usize {
        car.step_in(&track, &mut evolution, &weather, &controls);
    }
    let heat = car.state.drivetrain.engine.heat;
    eprintln!(
        "{start:?}
{heat:?}"
    );
    assert!(heat.bay > start.bay + 15.0, "{start:?} / {heat:?}");
    assert!(heat.intake > start.intake + 5.0, "{start:?} / {heat:?}");
    assert!(heat.gearbox > start.gearbox, "{start:?} / {heat:?}");
}

// ---- Suspension ------------------------------------------------------------------------

/// `model` with `change` made to its front and rear axles.
fn with_axles(
    model: Arc<CarModel>,
    change: impl Fn(bool, &mut params::AxleParams),
) -> Arc<CarModel> {
    let mut p = model.params.clone();
    change(true, &mut p.front);
    change(false, &mut p.rear);
    Arc::new(CarModel::new(p, model.front_tire.p.clone(), model.rear_tire.p.clone()).unwrap())
}

/// Double wishbones whose arms pivot on horizontal axes and lie level: no anti-dive,
/// anti-squat or anti-lift, and the roll centre on the ground.
fn level_wishbones() -> suspension::Linkage {
    use suspension::{Link, Linkage, Wishbone};
    Linkage::DoubleWishbone {
        upper: Wishbone {
            front: [0.15, -0.45, 0.17],
            rear: [-0.15, -0.45, 0.17],
            outer: [-0.04, -0.11, 0.17],
        },
        lower: Wishbone {
            front: [0.2, -0.58, -0.15],
            rear: [-0.2, -0.58, -0.15],
            outer: [0.0, -0.06, -0.15],
        },
        tie_rod: Link {
            inner: [-0.13, -0.527, -0.02],
            outer: [-0.13, -0.095, -0.02],
        },
    }
}

#[test]
fn the_bundled_cars_have_sensible_suspension() {
    for name in [
        "gt3",
        "formula",
        "fr_coupe",
        "fr_sports",
        "mr_coupe",
        "awd_sedan",
        "hot_hatch",
    ] {
        let model = asset_car(name);
        for front in [true, false] {
            let f = model.axle_figures(front);
            let k = f.kinematics;
            let axle = format!("{name} {}: {f:?}", if front { "front" } else { "rear" });
            // Camber gain up to ~0.4°/10 mm, a few hundredths of a degree of bump steer,
            // the roll centre above the ground and below the wheel centres.
            assert!((-0.8..=0.0).contains(&k.camber_gain), "{axle}");
            assert!(k.bump_steer.abs() < 0.1f64.to_radians() * 100.0, "{axle}");
            assert!((0.0..0.2).contains(&k.roll_centre), "{axle}");
            assert!((0.0..1.0).contains(&f.anti_brake), "{axle}");
            assert!((1.2..5.0).contains(&f.ride_frequency), "{axle}");
            if front {
                assert!(k.caster > 0.0 && k.kingpin_inclination > 0.0, "{axle}");
                assert!((0.0..0.05).contains(&k.trail), "{axle}");
                assert!((0.0..0.8).contains(&f.ackermann), "{axle}");
            }
        }
    }
}

/// Mean travel of the front wheels, m, over the second half of a second of hard braking
/// from 150 km/h, eased as a wheel starts to lock.
fn front_dive(model: Arc<CarModel>) -> f64 {
    let track = circle(5000.0);
    let mut car = Car::new(model, &track, 0.0, 0.0, 150.0 / 3.6, 5);
    let (mut sum, mut n) = (0.0, 0);
    while car.state.time < 1.0 {
        let locking = car.telemetry.wheels.iter().any(|w| w.slip_ratio < -0.1);
        car.step(
            &track,
            &Controls {
                brake: if locking { 0.5 } else { 0.9 },
                ..Default::default()
            },
        );
        if car.state.time > 0.5 {
            sum += 0.5 * (car.state.wheels[FL].travel + car.state.wheels[FR].travel);
            n += 1;
        }
    }
    sum / n as f64
}

#[test]
fn anti_dive_holds_the_nose_up_under_braking() {
    let gt3 = gt3();
    let level = with_axles(gt3.clone(), |front, a| {
        if front {
            a.linkage = level_wishbones();
        }
    });
    let (anti, none) = (front_dive(gt3.clone()), front_dive(level));
    eprintln!(
        "front dive: {:.1} mm with {:.0} % anti-dive, {:.1} mm without",
        anti * 1e3,
        gt3.axle_figures(true).anti_brake * 100.0,
        none * 1e3
    );
    // The linkage takes about a quarter of the pitch off the front springs.
    assert!(anti > 0.0 && anti < 0.9 * none, "{anti} vs {none}");
}

/// Body roll, degrees, holding a 60 m circle at 0.9 g.
fn body_roll(model: Arc<CarModel>) -> f64 {
    let (_, car, _) = hold_circle(model, 60.0, 0.9);
    (car.state.orientation * DVec3::Y)
        .z
        .asin()
        .to_degrees()
        .abs()
}

#[test]
fn a_higher_roll_centre_rolls_the_body_less() {
    let gt3 = gt3();
    let level = with_axles(gt3.clone(), |_, a| a.linkage = level_wishbones());
    let (raised, ground) = (body_roll(gt3), body_roll(level));
    eprintln!(
        "roll at 0.9 g: {raised:.2}° with the GT3's roll centres, {ground:.2}° on the ground"
    );
    // The lateral forces act nearer the roll axis, which is nearer the centre of gravity.
    assert!(raised < 0.95 * ground, "{raised} vs {ground}");
}

/// Ride heights at the axles, m, holding `kmh` on a straight.
fn ride_at(model: Arc<CarModel>, kmh: f64) -> [f64; 2] {
    let track = circle(20000.0);
    let target = kmh / 3.6;
    let mut car = Car::new(model, &track, 0.0, 0.0, target, 6);
    while car.state.time < 3.0 {
        let v = car.local_velocity().x;
        car.step(
            &track,
            &Controls {
                throttle: (0.5 + 0.5 * (target - v)).clamp(0.0, 1.0),
                ..Default::default()
            },
        );
    }
    car.telemetry.ride_height
}

#[test]
fn formula_figures() {
    let model = asset_car("formula");
    // At rest the car sits at its static ride height.
    let track = circle(5000.0);
    let mut car = Car::new(model.clone(), &track, 0.0, 0.0, 0.0, 0);
    for _ in 0..3000 {
        car.step(
            &track,
            &Controls {
                brake: 0.3,
                ..Default::default()
            },
        );
    }
    let travel = car.state.wheels.map(|w| w.travel);
    assert!(travel.iter().all(|t| t.abs() < 1e-3), "{travel:?}");

    let (t100, top) = launch(model.clone(), 60.0);
    let t100 = t100.expect("reaches 100 km/h");
    let g = skidpad(model.clone(), 60.0);
    let ride = ride_at(model.clone(), 250.0);
    eprintln!(
        "formula: 0-100 km/h {t100:.2} s, {top:.0} km/h after 60 s, skidpad {g:.3} g, \
         ride {:.0} / {:.0} mm at 250 km/h",
        ride[0] * 1e3,
        ride[1] * 1e3
    );
    // Formula 3 cars: 0-100 in ~3-3.5 s, ~250-270 km/h without a tow, more lateral grip
    // than a GT3 once the wings work.
    assert!((2.6..4.0).contains(&t100), "0-100 km/h in {t100:.2} s");
    assert!((240.0..290.0).contains(&top), "top speed {top:.0} km/h");
    assert!((1.4..2.0).contains(&g), "max lateral {g:.2} g");
    // The heave springs hold the floor off the road under the downforce.
    assert!(ride.iter().all(|&h| h > 0.01), "{ride:?}");
}

#[test]
fn heave_springs_hold_the_ride_height_under_downforce() {
    let formula = asset_car("formula");
    let soft = with_axles(formula.clone(), |_, a| a.heave = None);
    let (with, without) = (ride_at(formula, 250.0), ride_at(soft, 250.0));
    eprintln!("ride at 250 km/h: {with:?} with heave springs, {without:?} without");
    assert!(with[0] > without[0] + 0.002 && with[1] > without[1] + 0.002);
}
