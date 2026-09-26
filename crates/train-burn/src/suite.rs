//! A fixed set of conditions to drive every checkpoint in, and the choice of the best.

use std::path::{Path, PathBuf};

use open_racing_api::{DefaultTermination, EnvConfig, TrackEvolution};
use open_racing_sim::{Sky, WeatherSettings};
use open_racing_train_burn::BurnPolicy;

use crate::longrun::{self, Setup, Summary};

/// One condition of the suite.
pub struct Condition {
    pub name: &'static str,
    drive: Drive,
    conditions: EnvConfig,
}

/// How the cars of a condition drive.
#[derive(Clone, Copy)]
enum Drive {
    /// The suite's laps from a standing start.
    Stint,
    /// [`APP_LAPS`] with half the cars: the app's weather model costs far more than
    /// the training weather, and stints are measured in the other conditions.
    Short,
    /// One lap after a spin.
    AfterSpin,
}

const APP_LAPS: u32 = 3;

/// Stints from a standing start in the training conditions and on drawn tracks in
/// drawn weather; a few laps in the app's weather on its tracks from the grid; then
/// spins.
pub fn conditions() -> Vec<Condition> {
    let app = |sky, hour, temperature_offset| {
        Some(WeatherSettings {
            sky,
            hour,
            month: 6,
            temperature_offset,
            ..WeatherSettings::default()
        })
    };
    let track = |lo: f64, hi: f64| EnvConfig {
        track_grip: Some((lo, hi)),
        grip_gain_per_lap: TrackEvolution::DEFAULT_GAIN_PER_LAP,
        ..EnvConfig::default()
    };
    let stint = |name, conditions| Condition {
        name,
        drive: Drive::Stint,
        conditions,
    };
    let short = |name, conditions| Condition {
        name,
        drive: Drive::Short,
        conditions,
    };
    vec![
        stint("standard", EnvConfig::default()),
        stint(
            "drawn",
            EnvConfig {
                air_temperature: (5.0, 40.0),
                road_heat: (0.0, 30.0),
                wind_speed: (0.0, 9.0),
                ..track(0.90, 1.0)
            },
        ),
        short(
            "app hot",
            EnvConfig {
                weather_model: app(Sky::Fair, 12.0, 10.0),
                grid_start: true,
                ..track(1.0, 1.0)
            },
        ),
        short(
            "app cold dusty",
            EnvConfig {
                weather_model: app(Sky::Fair, 8.0, -12.0),
                grid_start: true,
                ..track(0.90, 0.90)
            },
        ),
        short(
            "app overcast green",
            EnvConfig {
                weather_model: app(Sky::Overcast, 12.0, 0.0),
                grid_start: true,
                ..track(0.94, 0.94)
            },
        ),
        Condition {
            name: "spin recovery",
            drive: Drive::AfterSpin,
            conditions: EnvConfig::default(),
        },
    ]
}

/// Longest the recovery condition lets a car be off the track, face the wrong way or
/// stand still, s: long enough to turn round and drive back on.
const RECOVERY_OFF_TRACK: f64 = 10.0;
const RECOVERY_WRONG_WAY: f64 = 10.0;
const RECOVERY_STUCK: f64 = 6.0;

fn setup(policy: &BurnPolicy, condition: &Condition, laps: u32, cars: usize) -> Setup {
    let conditions = &condition.conditions;
    match condition.drive {
        Drive::Stint => Setup::standing_start(policy, laps, cars, conditions),
        Drive::Short => Setup::standing_start(policy, APP_LAPS, cars.div_ceil(2), conditions),
        Drive::AfterSpin => after_spin(Setup::standing_start(policy, 1, cars, conditions)),
    }
}

/// `setup` from a spin somewhere on the track at 10 to 40 m/s instead of the start
/// line, with time to turn round and drive back onto the track.
pub fn after_spin(mut setup: Setup) -> Setup {
    setup.config = EnvConfig {
        random_start: true,
        safe_start: true,
        start_speed: (10.0, 40.0),
        start_offset: (-2.0, 2.0),
        spin_start_fraction: 1.0,
        ..setup.config
    };
    setup.termination = DefaultTermination {
        max_off_track_time: RECOVERY_OFF_TRACK,
        max_wrong_way_time: RECOVERY_WRONG_WAY,
        max_stuck_time: RECOVERY_STUCK,
        ..DefaultTermination::default()
    };
    setup
}

/// Drives `policy` in every condition.
pub fn evaluate(policy: &mut BurnPolicy, laps: u32, cars: usize) -> Vec<Summary> {
    conditions()
        .iter()
        .map(|c| longrun::run(policy, &setup(policy, c, laps, cars)))
        .collect()
}

pub fn print_header() {
    let names: Vec<String> = conditions()
        .iter()
        .map(|c| format!("{:>20}", c.name))
        .collect();
    println!("{:<24}{}", "", names.join(""));
}

pub fn print_row(label: &str, results: &[Summary]) {
    let cells: Vec<String> = results
        .iter()
        .map(|r| {
            let time = r.mean_time.map_or("-".into(), |t| format!("{t:.0}s"));
            format!(
                "{:>20}",
                format!("{}/{} {:.1} {time}", r.finished, r.cars, r.off_per_lap)
            )
        })
        .collect();
    println!("{label:<24}{}", cells.join(""));
}

/// Cars that did not finish over all conditions, then whether it left the track
/// limits, then the mean time in the standard conditions: lower is better.
fn score(results: &[Summary]) -> (usize, bool, f64) {
    let failures = results.iter().map(|r| r.cars - r.finished).sum();
    let off_course = results.iter().any(|r| r.off_course_per_lap > 0.5);
    let standard = results[0].mean_time.unwrap_or(f64::INFINITY);
    (failures, off_course, standard)
}

/// Drives every checkpoint of the run in `dir` (its `checkpoints/*`, then the final
/// policy) and copies the best to `dir/best`.
pub fn select(dir: &Path, laps: u32, cars: usize) {
    let mut candidates: Vec<(String, PathBuf)> = std::fs::read_dir(dir.join("checkpoints"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("meta.ron").exists())
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
        .collect();
    candidates.sort();
    if dir.join("meta.ron").exists() {
        candidates.push(("final".into(), dir.to_path_buf()));
    }
    assert!(!candidates.is_empty(), "no policies in {}", dir.display());
    println!("per condition: finished/cars, steps per lap with a wheel off, mean time");
    print_header();
    let mut best: Option<((usize, bool, f64), PathBuf, String)> = None;
    for (label, path) in candidates {
        let mut policy =
            BurnPolicy::load(&path).unwrap_or_else(|e| panic!("loading {}: {e}", path.display()));
        let results = evaluate(&mut policy, laps, cars);
        print_row(&label, &results);
        let key = score(&results);
        if best
            .as_ref()
            .is_none_or(|(b, _, _)| key.partial_cmp(b).is_some_and(|o| o.is_lt()))
        {
            best = Some((key, path, label));
        }
    }
    let (_, path, label) = best.expect("a candidate");
    let out = dir.join("best");
    std::fs::create_dir_all(&out).expect("create best");
    for file in ["policy.bin", "meta.ron"] {
        std::fs::copy(path.join(file), out.join(file)).expect("copy best");
    }
    println!("best: {label}, copied to {}", out.display());
}
