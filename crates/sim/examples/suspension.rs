//! Prints what each car's suspension does at static ride height: the figures its
//! hardpoints and actuation give, for setting up a car's linkage.
//!
//! ```text
//! cargo run -p open-racing-sim --example suspension [car.ron ...]
//! ```
//!
//! Without arguments it prints every car in `assets/cars/`.

use std::path::PathBuf;

use open_racing_sim::CarModel;

fn main() {
    let mut paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/cars");
        paths = std::fs::read_dir(dir)
            .expect("assets/cars")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "ron"))
            .collect();
        paths.sort();
    }
    for path in paths {
        let model = match CarModel::load(&path) {
            Ok(m) => m,
            Err(e) => {
                println!("{}: {e}\n", path.display());
                continue;
            }
        };
        println!("{} ({})", model.params.name, path.display());
        for front in [true, false] {
            let f = model.axle_figures(front);
            let k = f.kinematics;
            println!(
                "  {}: camber gain {:+.2}°/10 mm, bump steer {:+.3}°/10 mm toe-in, roll centre {:.0} mm, \
                 motion ratio {:.2}",
                if front { "front" } else { "rear " },
                k.camber_gain.to_degrees() / 100.0,
                k.bump_steer.to_degrees() / 100.0,
                k.roll_centre * 1e3,
                k.motion_ratio,
            );
            println!(
                "         anti-{} {:.0} %, anti-{} {:.0} %; wheel rates: spring {:.0}, bar {:.0}, heave {:.0} N/mm; \
                 ride {:.2} Hz",
                if front { "dive" } else { "lift" },
                f.anti_brake * 100.0,
                if front { "lift" } else { "squat" },
                f.anti_drive * 100.0,
                f.spring_rate / 1e3,
                f.anti_roll_rate / 1e3,
                f.heave_rate / 1e3,
                f.ride_frequency,
            );
            if front {
                println!(
                    "         caster {:.1}°, kingpin {:.1}°, trail {:.0} mm, scrub {:.0} mm, Ackermann {:.0} %",
                    k.caster.to_degrees(),
                    k.kingpin_inclination.to_degrees(),
                    k.trail * 1e3,
                    k.scrub_radius * 1e3,
                    f.ackermann * 100.0,
                );
            }
        }
        println!();
    }
}
