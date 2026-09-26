//! Stint evaluation: can a policy drive lap after lap as its tyres wear and heat up?

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use open_racing_api::{
    Car, DefaultTermination, Ending, EnvConfig, EnvSpec, OFF_COURSE_WHEELS, Policy, Track, VecEnv,
};
use open_racing_train_burn::BurnPolicy;

/// Where, when and why a car's stint ended early.
#[derive(Clone, Copy)]
struct Crash {
    lap: usize,
    /// Distance along the lap, m, and time since the start, s.
    s: f64,
    time: f64,
    ending: Option<Ending>,
}

/// One car's stint so far.
#[derive(Clone, Default)]
struct Stint {
    laps: Vec<Lap>,
    /// Lap and distance along the lap where the car crashed.
    crash: Option<Crash>,
    /// Time when the last lap was completed, s.
    finished: Option<f64>,
    current: LapAccumulator,
}

#[derive(Clone, Copy, Default)]
struct Lap {
    time: f64,
    /// Hottest load-weighted tread temperature of any tyre during the lap, °C.
    max_tread: f64,
    /// Mean grip left by the tyres' condition (temperature, pressure, wear, dirt).
    mean_grip: f64,
    /// Mean tread worn at the end of the lap.
    wear: f64,
    /// Mean hot pressure at the end of the lap, bar.
    pressure: f64,
    damage: f64,
    /// Steps with a wheel off the track, and off course (three or more wheels:
    /// beyond the track limits).
    off_steps: usize,
    off_course_steps: usize,
}

#[derive(Clone, Copy, Default)]
struct LapAccumulator {
    max_tread: f64,
    grip_sum: f64,
    steps: usize,
    off_steps: usize,
    off_course_steps: usize,
}

impl LapAccumulator {
    fn add(&mut self, car: &Car) {
        for i in 0..4 {
            let (w, t) = (&car.state.wheels[i].tire, &car.telemetry.wheels[i]);
            self.max_tread = self.max_tread.max(w.surface_temperature(&t.tread_load));
            self.grip_sum += car
                .model
                .tire(i)
                .condition_grip(w, &t.tread_load, t.pressure)
                / 4.0;
        }
        let off = car
            .telemetry
            .wheels
            .iter()
            .filter(|w| w.surface.off_track())
            .count();
        self.off_steps += (off >= 1) as usize;
        self.off_course_steps += (off >= OFF_COURSE_WHEELS) as usize;
        self.steps += 1;
    }

    fn finish(&mut self, car: &Car, time: f64) -> Lap {
        let lap = Lap {
            time,
            max_tread: self.max_tread,
            mean_grip: self.grip_sum / self.steps.max(1) as f64,
            wear: car.state.wheels.iter().map(|w| w.tire.wear).sum::<f64>() / 4.0,
            pressure: car.telemetry.wheels.iter().map(|w| w.pressure).sum::<f64>() / 4.0,
            damage: car.state.damage.iter().sum(),
            off_steps: self.off_steps,
            off_course_steps: self.off_course_steps,
        };
        *self = Self::default();
        lap
    }
}

/// How a stint is driven and measured.
pub struct Setup {
    pub laps: u32,
    pub cars: usize,
    /// Standard deviation of the noise added to every car's actions but the first.
    pub noise: f32,
    /// The environment, including where and in what conditions the cars start.
    pub config: EnvConfig,
    /// What ends a stint early; its time limit is set from the laps.
    pub termination: DefaultTermination,
    /// Write this car's stint step by step to this CSV file.
    pub trace: Option<(PathBuf, usize)>,
    /// Print each lap and crash, not only return the summary.
    pub verbose: bool,
}

impl Setup {
    /// A stint of `laps` from a standing start on the start line, in the policy's
    /// own environment with the track and weather of `conditions` (without a
    /// `track_grip`, the same grip everywhere).
    pub fn standing_start(
        policy: &BurnPolicy,
        laps: u32,
        cars: usize,
        conditions: &EnvConfig,
    ) -> Self {
        Self {
            laps,
            cars,
            noise: 0.02,
            config: EnvConfig {
                random_start: false,
                start_speed: (0.0, 0.0),
                start_offset: (-0.3, 0.3),
                track_grip: conditions.track_grip,
                grip_gain_per_lap: conditions.grip_gain_per_lap,
                air_temperature: conditions.air_temperature,
                road_heat: conditions.road_heat,
                wind_speed: conditions.wind_speed,
                weather_model: conditions.weather_model,
                grid_start: conditions.grid_start,
                ..policy.meta.env_config()
            },
            termination: DefaultTermination::default(),
            trace: None,
            verbose: false,
        }
    }
}

/// What a stint's cars achieved.
#[derive(Clone, Copy, Debug, Default)]
pub struct Summary {
    pub cars: usize,
    pub finished: usize,
    pub crashes: usize,
    /// Laps completed by all cars.
    pub laps: usize,
    /// Steps per lap with a wheel off the track, and with three or more.
    pub off_per_lap: f64,
    pub off_course_per_lap: f64,
    /// Mean and best time of the cars that finished, s.
    pub mean_time: Option<f64>,
    pub best_time: Option<f64>,
}

impl Summary {
    fn of(stints: &[Stint]) -> Self {
        let finishers: Vec<f64> = stints.iter().filter_map(|s| s.finished).collect();
        let laps: usize = stints.iter().map(|s| s.laps.len()).sum();
        let per_lap = |f: fn(&Lap) -> usize| {
            stints.iter().flat_map(|s| &s.laps).map(f).sum::<usize>() as f64 / laps.max(1) as f64
        };
        Self {
            cars: stints.len(),
            finished: finishers.len(),
            crashes: stints.iter().filter(|s| s.crash.is_some()).count(),
            laps,
            off_per_lap: per_lap(|l| l.off_steps),
            off_course_per_lap: per_lap(|l| l.off_course_steps),
            mean_time: mean(&finishers),
            best_time: finishers.iter().copied().reduce(f64::min),
        }
    }
}

pub fn run(policy: &mut BurnPolicy, setup: &Setup) -> Summary {
    let (laps, cars, noise) = (setup.laps, setup.cars, setup.noise);
    let trace_car = setup.trace.as_ref().map_or(0, |(_, car)| *car);
    assert!(cars > 0 && laps > 0 && trace_car < cars);
    let config = setup.config.clone();
    let mut spec = EnvSpec::from_names(&policy.meta.track, &policy.meta.car, config.clone())
        .unwrap_or_else(|e| panic!("{e}"));
    // Room to reach the start line first, from a grid slot or a spun start.
    let max_time = 200.0 * f64::from(laps) + 150.0;
    spec.termination = Arc::new(DefaultTermination {
        max_time,
        ..setup.termination.clone()
    });
    let mut env = spec.make_vec_env(cars);
    policy
        .check_compatible(env.observation_space())
        .unwrap_or_else(|e| panic!("{e}"));
    let track = &*spec.track;
    let act_dim = config.action_dim();
    let mut obs = env.reset(7).to_vec();
    let mut actions = vec![0.0; cars * act_dim];
    let mut stints = vec![Stint::default(); cars];
    let mut done = vec![false; cars];
    let mut rng = 0x2545_F491_4F6C_DD1D_u64;
    let mut normal = move || {
        let mut u = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            ((rng >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (u1, u2) = (u(), u());
        ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
    };
    let mut trace = setup.trace.as_ref().map(|(path, _)| {
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        );
        writeln!(
            file,
            "time,lap,s,offset,speed,steer,throttle,brake,gear,tread_fl,tread_fr,tread_rl,tread_rr,grip_fl,grip_fr,grip_rl,grip_rr,pressure_fl,pressure_fr,pressure_rl,pressure_rr,wear,dirt,wheels_off"
        )
        .unwrap();
        file
    });
    let steps = (max_time * config.control_hz) as usize + 1;
    let mut hints = vec![0_usize; cars];
    for _ in 0..steps {
        policy.act(&obs, &mut actions);
        for row in actions.chunks_mut(act_dim).skip(1) {
            for (a, x) in row.iter_mut().enumerate() {
                let (low, high) = (policy.meta.act_low[a], policy.meta.act_high[a]);
                *x = (*x + noise * (high - low) / 2.0 * normal()).clamp(low, high);
            }
        }
        let before: Vec<_> = env.cars().map(|c| c.state.position).collect();
        let r = env.step(&actions);
        obs.copy_from_slice(r.obs);
        let terminated: Vec<bool> = r.terminated.iter().map(|&t| t != 0).collect();
        let truncated: Vec<bool> = r.truncated.iter().map(|&t| t != 0).collect();
        let finished: Vec<_> = env.finished_episodes().to_vec();
        for (e, car) in env.cars().enumerate() {
            if done[e] {
                continue;
            }
            let stint = &mut stints[e];
            if terminated[e] || truncated[e] {
                done[e] = true;
                if terminated[e] {
                    let q = track.locate(before[e], track.nearest_index(before[e]));
                    let stats = finished.iter().find(|(i, _)| *i == e).map(|(_, s)| s);
                    stint.crash = Some(Crash {
                        lap: stint.laps.len() + 1,
                        s: q.s,
                        time: stats.map_or(0.0, |s| s.time),
                        ending: stats.and_then(|s| s.ending),
                    });
                }
                // The episode's final lap, if the last step completed one.
                if let Some((_, stats)) = finished.iter().find(|(i, _)| *i == e)
                    && stats.laps as usize > stint.laps.len()
                {
                    let lap = stint
                        .current
                        .finish(car, stats.last_lap_time.unwrap_or(0.0));
                    stint.laps.push(lap);
                }
                continue;
            }
            stint.current.add(car);
            let stats = env.episode_stats().nth(e).copied().unwrap_or_default();
            if stats.laps as usize > stint.laps.len() {
                let lap = stint
                    .current
                    .finish(car, stats.last_lap_time.unwrap_or(0.0));
                stint.laps.push(lap);
                if stint.laps.len() as u32 >= laps {
                    stint.finished = Some(stats.time);
                    done[e] = true;
                }
            }
            if e == trace_car
                && let Some(file) = &mut trace
            {
                hints[e] = track.locate(car.state.position, hints[e]).index;
                let action = &actions[e * act_dim..(e + 1) * act_dim];
                write_trace(
                    file,
                    car,
                    track,
                    hints[e],
                    stats.time,
                    stint.laps.len() + 1,
                    action,
                );
            }
        }
        if done.iter().all(|&d| d) {
            break;
        }
    }
    if setup.verbose {
        report(&stints, laps, track);
    }
    Summary::of(&stints)
}

fn write_trace(
    file: &mut impl Write,
    car: &Car,
    track: &Track,
    hint: usize,
    time: f64,
    lap: usize,
    action: &[f32],
) {
    let q = track.locate(car.state.position, hint);
    let mut temps = [0.0; 4];
    let mut grips = [0.0; 4];
    for i in 0..4 {
        let (w, t) = (&car.state.wheels[i].tire, &car.telemetry.wheels[i]);
        temps[i] = w.surface_temperature(&t.tread_load);
        grips[i] = car
            .model
            .tire(i)
            .condition_grip(w, &t.tread_load, t.pressure);
    }
    let wear = car.state.wheels.iter().map(|w| w.tire.wear).sum::<f64>() / 4.0;
    let dirt = car.state.wheels.iter().map(|w| w.tire.dirt()).sum::<f64>() / 4.0;
    let p: Vec<f64> = car.telemetry.wheels.iter().map(|w| w.pressure).collect();
    let off = car
        .telemetry
        .wheels
        .iter()
        .filter(|w| w.surface.off_track())
        .count();
    writeln!(
        file,
        "{time:.2},{lap},{:.1},{:.2},{:.2},{:.3},{:.3},{:.3},{},{:.0},{:.0},{:.0},{:.0},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{wear:.4},{dirt:.3},{off}",
        q.s,
        q.d,
        car.speed(),
        action[0],
        action[1],
        action[2],
        car.state.drivetrain.gear,
        temps[0],
        temps[1],
        temps[2],
        temps[3],
        grips[0],
        grips[1],
        grips[2],
        grips[3],
        p[0],
        p[1],
        p[2],
        p[3],
    )
    .unwrap();
}

fn report(stints: &[Stint], laps: u32, track: &Track) {
    let first = &stints[0];
    println!("car 0 (as the policy drives):");
    println!("  lap   time   max tread  grip   wear   p_hot  damage  off  off course");
    for (k, l) in first.laps.iter().enumerate() {
        println!(
            "  {:>3} {:>7.3} {:>8.0}°C {:>6.3} {:>6.4} {:>6.2} {:>6.1} {:>5} {:>5}",
            k + 1,
            l.time,
            l.max_tread,
            l.mean_grip,
            l.wear,
            l.pressure,
            l.damage,
            l.off_steps,
            l.off_course_steps
        );
    }
    match (first.finished, first.crash) {
        (Some(t), _) => println!("  finished {laps} laps in {t:.3} s"),
        (_, Some(c)) => println!(
            "  crashed on lap {} at {:.0} m ({:?})",
            c.lap, c.s, c.ending
        ),
        _ => println!("  ran out of time after {} laps", first.laps.len()),
    }
    let sum = Summary::of(stints);
    let n = sum.cars;
    println!(
        "all {n} cars: {}/{n} finished {laps} laps, {} crashes in {} laps, {:.2} steps per lap with a wheel off, {:.2} off course, total time mean {} best {}",
        sum.finished,
        sum.crashes,
        sum.laps,
        sum.off_per_lap,
        sum.off_course_per_lap,
        sum.mean_time.map_or("-".into(), |t| format!("{t:.2}s")),
        sum.best_time.map_or("-".into(), |t| format!("{t:.2}s")),
    );
    let crashes: Vec<(usize, Crash)> = stints
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.crash.map(|c| (i, c)))
        .collect();
    for k in 0..laps as usize {
        let times: Vec<f64> = stints
            .iter()
            .filter_map(|s| s.laps.get(k).map(|l| l.time))
            .collect();
        let grips: Vec<f64> = stints
            .iter()
            .filter_map(|s| s.laps.get(k).map(|l| l.mean_grip))
            .collect();
        if times.is_empty() {
            break;
        }
        println!(
            "  lap {:>2}: {:>2} cars, mean {:.3} s, best {:.3} s, grip {:.3}",
            k + 1,
            times.len(),
            mean(&times).unwrap(),
            times.iter().copied().fold(f64::INFINITY, f64::min),
            mean(&grips).unwrap()
        );
    }
    let mut crashes = crashes;
    crashes.sort_by(|a, b| a.1.s.total_cmp(&b.1.s));
    for (
        car,
        Crash {
            lap,
            s,
            time,
            ending,
        },
    ) in crashes
    {
        let why = ending.map_or(String::new(), |e| format!(" ({e:?})"));
        println!(
            "  crash on lap {lap} at {s:.0} m of {:.0}{why}, car {car} after {time:.1} s",
            track.length
        );
    }
}

fn mean(x: &[f64]) -> Option<f64> {
    (!x.is_empty()).then(|| x.iter().sum::<f64>() / x.len() as f64)
}
