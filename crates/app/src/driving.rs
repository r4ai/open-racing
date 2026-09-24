//! Owns the simulated car and advances it at a fixed 1 kHz, independent of the frame
//! rate. Controls come from the human, a replay or an AI policy.

use std::sync::Arc;

use bevy::prelude::*;
use glam::{DQuat, DVec3};
use open_racing_api::{AgentDriver, EnvSpec, LapTimer, Policy};
use open_racing_sim::{AutoShift, Car, CarState, Controls, DT, Shift, Track};

use crate::Args;
use crate::input::{AppRequests, DriverInput};
use crate::settings::settings_closed;

/// Physics steps allowed per frame; beyond this the sim runs slower than real time
/// instead of spiralling.
const MAX_STEPS_PER_FRAME: usize = 250;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Human,
    Ai,
    Replay,
}

/// Inputs recorded since the last reset. Replaying them reproduces the drive exactly
/// because the simulation is deterministic.
pub struct Recording {
    start: CarState,
    controls: Vec<Controls>,
    cursor: usize,
}

impl Recording {
    fn new(start: CarState) -> Self {
        Self {
            start,
            controls: Vec::new(),
            cursor: 0,
        }
    }
}

/// 3D model of a track package, taken by the scene when it spawns.
#[derive(Resource, Default)]
pub struct TrackModel(pub Option<open_racing_track::Visual>);

/// 3D model of a car package, taken by the scene when it spawns.
#[derive(Resource, Default)]
pub struct CarModelVisual(pub Option<open_racing_car::CarVisual>);

#[derive(Resource)]
pub struct Simulation {
    pub track: Arc<Track>,
    pub car: Car,
    /// State before the last physics step, for render interpolation.
    pub previous: CarState,
    /// Fraction of a physics step accumulated but not simulated yet.
    pub alpha: f64,
    accumulator: f64,
    pub mode: Mode,
    pub lap: LapTimer,
    pub auto_shift: bool,
    pub controls: Controls,
    /// Steering torque averaged over the physics steps of the last frame, for force feedback.
    pub ffb_torque: f64,
    recording: Recording,
    spec: EnvSpec,
}

impl Simulation {
    /// Also returns the track's and the car's 3D models, where they have one, for the
    /// scene to spawn.
    pub fn new(args: &Args) -> Result<(Self, TrackModel, CarModelVisual), open_racing_api::Error> {
        let (track, model) =
            open_racing_api::load_track_with_visual(args.track.as_deref().unwrap_or("lakeside"))?;
        let (car_model, car_visual) = open_racing_api::load_car_with_visual(&args.car)?;
        let spec = EnvSpec::new(track, car_model, Default::default());
        let car = Car::new(spec.car.clone(), &spec.track, 0.0, 0.0, 0.0, 1);
        let sim = Self {
            lap: LapTimer::new(&spec.track, car.state.position),
            previous: car.state,
            recording: Recording::new(car.state),
            track: spec.track.clone(),
            car,
            alpha: 0.0,
            accumulator: 0.0,
            mode: Mode::Human,
            auto_shift: args.auto_shift,
            controls: Controls::default(),
            ffb_torque: 0.0,
            spec,
        };
        Ok((sim, TrackModel(model), CarModelVisual(car_visual)))
    }

    /// Body pose interpolated between the last two physics states.
    pub fn body_pose(&self) -> (DVec3, DQuat) {
        let (a, b) = (&self.previous, &self.car.state);
        (
            a.position.lerp(b.position, self.alpha),
            a.orientation.slerp(b.orientation, self.alpha),
        )
    }

    /// Puts the car back on the centreline at the nearest point, at rest.
    fn reset_car(&mut self) {
        let s = self.track.query(self.car.state.position, self.lap.hint()).s;
        self.car.reset(&self.track, s, 0.0, 0.0, 1);
        self.previous = self.car.state;
        self.lap = LapTimer::new(&self.track, self.car.state.position);
        self.recording = Recording::new(self.car.state);
    }

    fn start_replay(&mut self) {
        self.car.state = self.recording.start;
        self.previous = self.car.state;
        self.lap = LapTimer::new(&self.track, self.car.state.position);
        self.recording.cursor = 0;
        self.mode = Mode::Replay;
    }
}

/// AI driver, kept on the main thread since inference backends may not be `Send`.
pub struct AiDriver {
    pub policy: Box<dyn Policy>,
    pub driver: AgentDriver,
}

pub struct DrivingPlugin;

impl Plugin for DrivingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (handle_requests, step_simulation.run_if(settings_closed)).chain(),
        );
    }
}

/// The track the `--ai` policy was trained on, if it can be read.
pub fn policy_track(args: &Args) -> Option<String> {
    #[cfg(feature = "burn-policy")]
    if let Some(dir) = &args.ai {
        return open_racing_train_burn::load_meta(dir)
            .ok()
            .map(|meta| meta.track);
    }
    let _ = args;
    None
}

/// Loads the policy given on the command line, if any.
pub fn install_policy(app: &mut App, args: &Args) {
    let Some(dir) = &args.ai else { return };
    #[cfg(feature = "burn-policy")]
    {
        let policy = match open_racing_train_burn::BurnPolicy::load(dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("could not load policy from {}: {e}", dir.display());
                return;
            }
        };
        let sim = app.world().resource::<Simulation>();
        let spec = EnvSpec {
            config: policy.meta.env_config(),
            ..sim.spec.clone()
        };
        if let Err(e) = policy.check_compatible(&spec.observation_space()) {
            eprintln!("{e}");
            return;
        }
        let driver = spec.agent_driver();
        app.insert_non_send(AiDriver {
            policy: Box::new(policy),
            driver,
        });
        app.world_mut().resource_mut::<Simulation>().mode = Mode::Ai;
    }
    #[cfg(not(feature = "burn-policy"))]
    eprintln!(
        "built without the `burn-policy` feature; ignoring --ai {}",
        dir.display()
    );
}

fn handle_requests(
    requests: Res<AppRequests>,
    mut sim: ResMut<Simulation>,
    ai: Option<NonSendMut<AiDriver>>,
) {
    if requests.reset_car {
        sim.reset_car();
        if sim.mode == Mode::Replay {
            sim.mode = Mode::Human;
        }
    }
    if requests.restart_engine {
        sim.car.restart_engine();
    }
    if requests.toggle_replay {
        match sim.mode {
            Mode::Replay => {
                // Continue driving from here; drop the part of the recording not replayed.
                let cursor = sim.recording.cursor;
                sim.recording.controls.truncate(cursor);
                sim.mode = Mode::Human;
            }
            _ => sim.start_replay(),
        }
    }
    if requests.toggle_ai
        && let Some(mut ai) = ai
    {
        sim.mode = if sim.mode == Mode::Ai {
            Mode::Human
        } else {
            Mode::Ai
        };
        let (car, track) = (&sim.car, &sim.track);
        ai.driver.reset(car, track);
    }
}

pub fn step_simulation(
    time: Res<Time>,
    mut sim: ResMut<Simulation>,
    mut input: ResMut<DriverInput>,
    mut ai: Option<NonSendMut<AiDriver>>,
) {
    let sim = &mut *sim;
    sim.accumulator += time.delta_secs_f64().min(0.25);
    let mut steps = 0;
    let mut torque = 0.0;
    while sim.accumulator >= DT && steps < MAX_STEPS_PER_FRAME {
        let controls = match (sim.mode, ai.as_deref_mut()) {
            (Mode::Ai, Some(ai)) => ai.driver.controls(ai.policy.as_mut(), &sim.car, &sim.track),
            (Mode::Replay, _) => match sim.recording.controls.get(sim.recording.cursor) {
                Some(&c) => {
                    sim.recording.cursor += 1;
                    c
                }
                None => {
                    sim.mode = Mode::Human;
                    input.controls
                }
            },
            _ => {
                let mut c = input.controls;
                // A gear request applies to one physics step only.
                input.controls.shift = Shift::None;
                if sim.auto_shift && c.shift == Shift::None {
                    c.shift = AutoShift.shift(&sim.car);
                }
                c
            }
        };
        if sim.mode != Mode::Replay {
            sim.recording.controls.push(controls);
        }
        sim.controls = controls;
        sim.previous = sim.car.state;
        sim.car.step(&sim.track, &controls);
        torque += sim.car.telemetry.steering_torque;
        let (pos, t) = (sim.car.state.position, sim.car.state.time);
        sim.lap.update(&sim.track, pos, t);
        sim.accumulator -= DT;
        steps += 1;
    }
    if steps == MAX_STEPS_PER_FRAME {
        sim.accumulator = 0.0;
    }
    if steps > 0 {
        sim.ffb_torque = torque / steps as f64;
    }
    sim.alpha = sim.accumulator / DT;
}
