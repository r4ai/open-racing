//! Owns the simulated car and advances it at a fixed 1 kHz, independent of the frame
//! rate. Controls come from the human, a replay or an AI policy.

use std::sync::Arc;

use bevy::prelude::*;
use glam::{DQuat, DVec3};
use open_racing_api::{AgentDriver, EnvSpec, LapTimer, Policy};
use open_racing_sim::weather::{OccluderBuilder, Occluders};
use open_racing_sim::{
    AutoShift, BlipAssist, Car, CarState, ClutchAssist, Controls, DT, Realism, RubberMap, Shift,
    Track, TrackEvolution, Weather, WeatherSettings,
};

use crate::Args;
use crate::assists::AssistSettings;
use crate::input::{AppRequests, DriverInput};
use crate::settings::settings_closed;

/// Physics steps allowed per frame; beyond this the sim runs slower than real time
/// instead of spiralling.
const MAX_STEPS_PER_FRAME: usize = 250;
/// Scenery farther than this from the centreline does not shade the road, m.
const SHADE_REACH: f64 = 150.0;
/// Share of the light let through by alpha-tested or blended scenery: foliage, fences.
const SEE_THROUGH: f64 = 0.5;

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
    /// Rubber and dirt on the track, and the weather, at the start.
    track: TrackEvolution,
    weather: Weather,
    controls: Vec<Controls>,
    cursor: usize,
}

impl Recording {
    fn new(start: CarState, track: TrackEvolution, weather: Weather) -> Self {
        Self {
            start,
            track,
            weather,
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
    /// Rubber and dirt on the track.
    pub evolution: TrackEvolution,
    /// Sun, air, wind, clouds and road temperature.
    pub weather: Weather,
    /// Racing-line grip the track starts from, and what it gains per lap.
    pub track_grip: f64,
    pub grip_gain: f64,
    /// State before the last physics step, for render interpolation.
    pub previous: CarState,
    /// Fraction of a physics step accumulated but not simulated yet.
    pub alpha: f64,
    accumulator: f64,
    pub mode: Mode,
    pub lap: LapTimer,
    /// Clutch assist state for the human driver.
    clutch_assist: ClutchAssist,
    pub controls: Controls,
    /// Steering torque averaged over the physics steps of the last frame, for force feedback.
    pub ffb_torque: f64,
    recording: Recording,
    spec: EnvSpec,
}

impl Simulation {
    /// Also returns the track's and the car's 3D models, where they have one, for the
    /// scene to spawn.
    pub fn new(
        args: &Args,
        weather: WeatherSettings,
    ) -> Result<(Self, TrackModel, CarModelVisual), open_racing_api::Error> {
        let (track, model) =
            open_racing_api::load_track_with_visual(args.track.as_deref().unwrap_or("lakeside"))?;
        let (car_model, car_visual) = open_racing_api::load_car_with_visual(&args.car)?;
        let spec = EnvSpec::new(track, car_model, Default::default());
        let car = Car::new(spec.car.clone(), &spec.track, 0.0, 0.0, 0.0, 1);
        let rubber = Arc::new(RubberMap::new(&spec.track));
        let evolution = TrackEvolution::new(rubber, args.track_grip, args.grip_gain);
        let scenery = model.as_ref().map(|m| Arc::new(scenery(&spec.track, m)));
        let weather = Weather::new(&spec.track, scenery, weather);
        let sim = Self {
            lap: LapTimer::new(&spec.track, car.state.position),
            previous: car.state,
            recording: Recording::new(car.state, evolution.clone(), weather.clone()),
            evolution,
            weather,
            track_grip: args.track_grip,
            grip_gain: args.grip_gain,
            track: spec.track.clone(),
            car,
            alpha: 0.0,
            accumulator: 0.0,
            mode: Mode::Human,
            clutch_assist: ClutchAssist::default(),
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
        let s = self
            .track
            .locate(self.car.state.position, self.lap.hint())
            .s;
        self.car.reset(&self.track, s, 0.0, 0.0, 1);
        self.clutch_assist = ClutchAssist::default();
        self.previous = self.car.state;
        self.lap = LapTimer::new(&self.track, self.car.state.position);
        self.recording = self.new_recording();
    }

    fn new_recording(&self) -> Recording {
        Recording::new(self.car.state, self.evolution.clone(), self.weather.clone())
    }

    /// Starts the weather over from `settings`.
    pub fn restart_weather(&mut self, settings: WeatherSettings) {
        self.weather.restart(settings);
        if self.mode == Mode::Replay {
            self.mode = Mode::Human;
        }
        self.recording = self.new_recording();
    }

    /// Sets what the car can come to harm from. The recording starts over, as a replay
    /// has to run under the same rules.
    pub fn set_realism(&mut self, realism: Realism) {
        self.car.realism = realism;
        if self.mode == Mode::Replay {
            self.mode = Mode::Human;
        }
        self.recording = self.new_recording();
    }

    /// Starts the track over from `track_grip` and `grip_gain`: the rubber and dirt the
    /// car left are gone.
    pub fn restart_track(&mut self) {
        self.evolution.restart(self.track_grip, self.grip_gain);
        if self.mode == Mode::Replay {
            self.mode = Mode::Human;
        }
        self.recording = self.new_recording();
    }

    fn start_replay(&mut self) {
        self.car.state = self.recording.start;
        self.evolution = self.recording.track.clone();
        self.weather = self.recording.weather.clone();
        self.previous = self.car.state;
        self.lap = LapTimer::new(&self.track, self.car.state.position);
        self.recording.cursor = 0;
        self.mode = Mode::Replay;
    }
}

/// The scenery of a track's model that shades its road: meshes that cast shadows, the
/// see-through ones letting part of the light through.
fn scenery(track: &Track, model: &open_racing_track::Visual) -> Occluders {
    let road: Vec<DVec3> = track.samples.iter().map(|s| s.pos).collect();
    let mut builder = OccluderBuilder::new(&road, SHADE_REACH);
    for mesh in model.meshes.iter().filter(|m| m.cast_shadows) {
        let see_through = model
            .materials
            .get(mesh.material as usize)
            .is_some_and(|m| !matches!(m.alpha_mode, open_racing_track::AlphaMode::Opaque));
        let transmission = if see_through { SEE_THROUGH } else { 0.0 };
        builder.add(&mesh.positions, &mesh.indices, transmission);
    }
    builder.build()
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
    assists: Res<AssistSettings>,
    mut timings: Option<ResMut<crate::capture::CloudCpuTimings>>,
) {
    if let Some(t) = &mut timings {
        t.weather_ms = 0.0;
    }
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
                if assists.auto_shift && c.shift == Shift::None {
                    c.shift = AutoShift.shift(&sim.car);
                }
                if assists.clutch {
                    sim.clutch_assist.apply(&sim.car, &mut c);
                }
                if assists.blip {
                    BlipAssist.apply(&sim.car, &mut c);
                }
                c
            }
        };
        if sim.mode != Mode::Replay {
            sim.recording.controls.push(controls);
        }
        sim.controls = controls;
        sim.previous = sim.car.state;
        let started = timings.as_ref().map(|_| std::time::Instant::now());
        sim.weather.step(DT);
        if let (Some(t), Some(started)) = (&mut timings, started) {
            t.weather_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        sim.car
            .step_in(&sim.track, &mut sim.evolution, &sim.weather, &controls);
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
