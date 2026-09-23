use std::sync::Arc;
use open_racing_sim::*;

fn circle(r: f64) -> Track {
    let points = (0..32).map(|i| {
        let a = i as f64 / 32.0 * std::f64::consts::TAU;
        TrackPoint { pos: (r * a.cos(), r * a.sin(), 0.0), width_left: 30.0, width_right: 30.0, bank: 0.0 }
    }).collect();
    Track::new(&TrackDef { name: "c".into(), points, kerb_width: 1.0, kerb_height: 0.0, spacing: 1.0 }).unwrap()
}

fn main() {
    let model = Arc::new(CarModel::gt3());
    let track = circle(5000.0);
    let mut car = Car::new(model.clone(), &track, 0.0, 0.0, 0.0, 1);
    for k in 0..3000 {
        car.step(&track, &Controls::default());
        if k % 500 == 0 {
            let s = &car.state;
            println!("t={:.2} z={:.4} v={:.4} ext={:.4?} load={:.0?} rpm={:.0}", s.time, s.position.z, car.speed(),
                s.wheels.map(|w| w.extension), car.telemetry.wheels.map(|w| w.load), s.drivetrain.rpm());
        }
    }
    // acceleration run
    let mut t100 = None;
    for k in 0..60000 {
        let rpm = car.state.drivetrain.rpm();
        let shift = if rpm > 8900.0 && car.state.drivetrain.gear < 6 && car.state.drivetrain.shift_timer == 0.0 { Shift::Up } else { Shift::None };
        car.step(&track, &Controls { throttle: 1.0, shift, ..Default::default() });
        let v = car.speed() * 3.6;
        if t100.is_none() && v >= 100.0 { t100 = Some(car.state.time - 3.0); }
        if k % 2000 == 0 {
            let s = &car.state;
            println!("t={:.1} v={:.1}km/h gear={} rpm={:.0} kappa_r={:.3} load={:.0?} pitch={:.4}", s.time, v, s.drivetrain.gear, s.drivetrain.rpm(),
                s.wheels[2].kappa, car.telemetry.wheels.map(|w| w.load), s.orientation.to_euler(glam::EulerRot::ZYX).1);
        }
    }
    println!("0-100: {:?}  top: {:.1} km/h", t100, car.speed() * 3.6);
    // braking from 100
    car.reset(&track, 0.0, 0.0, 100.0 / 3.6, 3);
    let start = car.state.position;
    while car.speed() > 0.1 && car.state.time < 20.0 {
        car.step(&track, &Controls { brake: 0.75, clutch: 1.0, ..Default::default() });
    }
    println!("100-0 dist {:.1} m in {:.2}s", (car.state.position - start).length(), car.state.time);
}
