//! Stint evaluation: can a policy drive lap after lap as its tyres wear and heat up?

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use open_racing_api::{
    Car, DefaultTermination, EnvConfig, EnvSpec, OFF_COURSE_WHEELS, Policy, Track, VecEnv,
};
use open_racing_train_burn::BurnPolicy;

/// One car's stint so far.
#[derive(Clone, Default)]
struct Stint {
    laps: Vec<Lap>,
    /// Lap and distance along the lap where the car crashed.
    crash: Option<(usize, f64)>,
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
    /// Steps with two or more wheels off the track, and off course (three or more:
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
        self.off_steps += (off >= 2) as usize;
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

pub fn run(
    policy: &mut BurnPolicy,
    laps: u32,
    cars: usize,
    noise: f32,
    weather: &EnvConfig,
    trace: Option<&Path>,
    trace_car: usize,
) {
    assert!(cars > 0 && laps > 0 && trace_car < cars);
    let config = EnvConfig {
        random_start: false,
        start_speed: (0.0, 0.0),
        start_offset: (-0.3, 0.3),
        air_temperature: weather.air_temperature,
        road_heat: weather.road_heat,
        wind_speed: weather.wind_speed,
        ..policy.meta.env_config()
    };
    let mut spec = EnvSpec::from_names(&policy.meta.track, &policy.meta.car, config.clone())
        .unwrap_or_else(|e| panic!("{e}"));
    let max_time = 200.0 * f64::from(laps);
    spec.termination = Arc::new(DefaultTermination {
        max_time,
        ..DefaultTermination::default()
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
    let mut trace = trace.map(|path| {
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
                    stint.crash = Some((stint.laps.len() + 1, q.s));
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
    report(&stints, laps, track);
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
        (_, Some((lap, s))) => println!("  crashed on lap {lap} at {s:.0} m"),
        _ => println!("  ran out of time after {} laps", first.laps.len()),
    }
    let finishers: Vec<f64> = stints.iter().filter_map(|s| s.finished).collect();
    let n = stints.len();
    let completed_laps: usize = stints.iter().map(|s| s.laps.len()).sum();
    let crashes: Vec<(usize, f64)> = stints.iter().filter_map(|s| s.crash).collect();
    let off_course: usize = stints
        .iter()
        .flat_map(|s| &s.laps)
        .map(|l| l.off_course_steps)
        .sum();
    println!(
        "all {n} cars: {}/{n} finished {laps} laps, {} crashes in {completed_laps} laps, {:.2} off-course steps per lap, total time mean {} best {}",
        finishers.len(),
        crashes.len(),
        off_course as f64 / completed_laps.max(1) as f64,
        mean(&finishers).map_or("-".into(), |t| format!("{t:.2}s")),
        finishers
            .iter()
            .copied()
            .reduce(f64::min)
            .map_or("-".into(), |t| format!("{t:.2}s")),
    );
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
    crashes.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (lap, s) in crashes {
        println!("  crash on lap {lap} at {s:.0} m of {:.0}", track.length);
    }
}

fn mean(x: &[f64]) -> Option<f64> {
    (!x.is_empty()).then(|| x.iter().sum::<f64>() / x.len() as f64)
}
