//! Rigid-body chassis with four suspended wheels.
//!
//! Model summary:
//! - Chassis: 6-DOF rigid body. The unsprung masses move with it except along their
//!   wheels' paths, where each moves on its own DOF, the suspension's travel.
//! - Each corner: a linkage (double wishbones, a MacPherson strut, five links or a
//!   trailing arm, see [`crate::suspension`]) guides the upright along its travel and
//!   steers it with the rack. The tyre's forces reach the body through it: what they do
//!   along the travel (anti-dive, anti-squat, the roll centre's jacking) and on the rack
//!   (the steering's feel) follows from how the linkage moves. Springs, two-stage
//!   dampers, the anti-roll bar and a heave spring work through the actuation (at the
//!   wheel, a coil-over, or a push- or pullrod and rocker) with its motion ratio; bump
//!   stops limit the travel. A tyre with vertical stiffness/damping against the road.
//! - Tyres: transient slips via relaxation length, Magic Formula combined forces,
//!   grip scaled by load, sliding speed, inflation pressure, tread temperature and wear;
//!   camber and toe follow the linkage.
//! - Wheels spin under drive, brake and road torque; brakes lock the wheel exactly. The
//!   brakes' friction follows their temperature; their heat reaches the tyres through
//!   the rims.
//! - The engine's parts heat and cool, and with failures on, wear out. The engine's bay
//!   warms the intake air, the gearbox and the tyres nearest to it.
//! - Everything starts from the weather: the brakes and the engine are warmed in the
//!   day's air, and the tyres are inflated in it. The air's temperature, density and
//!   the wind set the cooling.
//! - Aerodynamics: elements (body, wings, floor) whose coefficients follow their angle
//!   of attack (the car's pitch), the floor's ride height under them, the
//!   sideslip and the damage the car has taken.

use std::sync::Arc;

use glam::{DMat3, DQuat, DVec3};
use serde::{Deserialize, Serialize};

use crate::brakes::BrakeState;
use crate::controls::Controls;
use crate::drivetrain::{self, DriveInput, DrivetrainState};
use crate::engine::{Ambient, STANDARD_PRESSURE};
use crate::evolution::TrackEvolution;
use crate::params::{AxleParams, CarModel, DAMAGE_ZONES, lookup};
use crate::suspension::Pose;
use crate::tire::TireCondition;
use crate::track::{Surface, Track};
use crate::weather::{Air, Weather};
use crate::{Airflow, DT, FL, GRAVITY, KELVIN, RL, THERMAL_STEPS};

/// Below this speed a slip-velocity damping term is blended in so the relaxation
/// length model does not oscillate at standstill.
const LOW_SPEED: f64 = 3.0;
/// Low-speed damping per newton of load, N·s/m per N.
const LOW_SPEED_DAMPING: f64 = 3.0;
/// Fraction of the normal speed returned when hitting the run-off barrier.
const BARRIER_RESTITUTION: f64 = 0.2;
/// Coulomb friction coefficient between the car and the run-off barrier.
const BARRIER_FRICTION: f64 = 0.3;
/// Wheels this far inside the run-off barrier cannot reach it within one step, m.
const BARRIER_SKIN: f64 = 1.0;
/// Rolling speed over which the pneumatic trail swaps ends when the wheel reverses, m/s.
const TRAIL_REVERSAL_SPEED: f64 = 0.5;
/// How far the air around a tyre is from the air temperature towards the road's: the
/// air in the first centimetres over sunlit asphalt is well above the air temperature.
const NEAR_ROAD_AIR: f64 = 0.2;
/// How far the air around the tyres nearest the engine is from the air temperature
/// towards the engine bay's, whose air flows out past them.
const NEAR_BAY_AIR: f64 = 0.15;
/// Impact speed into a wall that leaves no damage, m/s.
const DAMAGE_THRESHOLD: f64 = 2.0;
/// Pa per hPa.
const HECTOPASCAL: f64 = 100.0;
/// Standard weather for [`Car::step_evolving`], a static so that no step builds and
/// drops a copy of it.
static STANDARD_WEATHER: Weather = Weather::STANDARD;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelState {
    /// Suspension travel from static ride height in m: the wheel centre's height in the
    /// body (larger = wheel further up, in bump).
    pub travel: f64,
    pub travel_rate: f64,
    /// Spin rate in rad/s, positive = rolling forward.
    pub spin: f64,
    /// Accumulated rotation angle in rad, wrapped to [0, 2π). For rendering.
    pub angle: f64,
    /// Transient longitudinal slip ratio.
    pub kappa: f64,
    /// Transient slip angle (tan α).
    pub alpha: f64,
    /// Road wheel steer angle at the last step, rad.
    pub steer: f64,
    /// Torque with which the twisted contact patch resists the wheel having turned
    /// over it, N·m, positive after a turn to the left.
    pub twist: f64,
    /// Track query hint.
    pub hint: usize,
    pub tire: TireCondition,
    pub brake: BrakeState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarState {
    /// CG position in world coordinates.
    pub position: DVec3,
    /// Body-to-world rotation.
    pub orientation: DQuat,
    /// CG velocity in world coordinates.
    pub velocity: DVec3,
    /// Angular velocity in body coordinates.
    pub angular_velocity: DVec3,
    pub wheels: [WheelState; 4],
    /// Steering rack's travel to the left, m.
    pub rack: f64,
    pub drivetrain: DrivetrainState,
    /// Impact damage per zone of the body (front, rear, left, right): the speed of the
    /// hits into walls in that direction beyond a light touch, summed, m/s.
    pub damage: [f64; DAMAGE_ZONES],
    /// Simulated time in s.
    pub time: f64,
    /// Physics steps taken since the reset.
    pub steps: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct WheelTelemetry {
    pub load: f64,
    pub slip_ratio: f64,
    pub slip_angle: f64,
    /// Speed at which the contact patch slides over the road, m/s.
    pub slide_speed: f64,
    /// Inflation pressure, bar (gauge).
    pub pressure: f64,
    /// Share of the load on the inner, middle and outer tread zone.
    pub tread_load: [f64; 3],
    /// Tyre forces in the contact frame, N.
    pub fx: f64,
    pub fy: f64,
    /// Self-aligning torque, N·m.
    pub mz: f64,
    /// Road wheel steer angle, rad.
    pub steer: f64,
    /// Inclination to the road, rad (positive = top leaning right).
    pub inclination: f64,
    pub surface: Surface,
    /// Contact point in world coordinates.
    pub contact: DVec3,
    /// Force of the springs, dampers, bars and bump stops against the wheel's travel,
    /// N (at the wheel).
    pub suspension_force: f64,
    /// Camber against the body, rad, negative = top leaning inwards.
    pub camber: f64,
    /// Force of the road on the tyre in world coordinates, N.
    pub force: DVec3,
    /// Friction multiplier of the tyre's nominal μ: the surface, its rubber and dirt,
    /// camber, sliding and the tyre's condition.
    pub grip: f64,
    /// Temperature of the air around the tyre and of the road under it, °C.
    pub air_temperature: f64,
    pub road_temperature: f64,
}

impl Default for WheelTelemetry {
    fn default() -> Self {
        Self {
            load: 0.0,
            slip_ratio: 0.0,
            slip_angle: 0.0,
            slide_speed: 0.0,
            pressure: 0.0,
            tread_load: [0.0; 3],
            fx: 0.0,
            fy: 0.0,
            mz: 0.0,
            steer: 0.0,
            inclination: 0.0,
            surface: Surface::Asphalt,
            contact: DVec3::ZERO,
            suspension_force: 0.0,
            camber: 0.0,
            force: DVec3::ZERO,
            grip: 0.0,
            air_temperature: 0.0,
            road_temperature: 0.0,
        }
    }
}

/// Derived quantities from the last step, for HUD, observations and force feedback.
#[derive(Clone, Copy, Debug, Default)]
pub struct Telemetry {
    pub wheels: [WheelTelemetry; 4],
    /// Specific force at the CG in body coordinates (what an accelerometer measures), m/s².
    pub acceleration: DVec3,
    /// Torque at the steering wheel from the front contact patches' forces about the
    /// steering axes (aligning moment, trail, scrub radius, caster, kingpin), N·m.
    /// Positive turns the steering wheel left. This is the force-feedback source.
    pub steering_torque: f64,
    /// Aerodynamic downforce, front / rear, N.
    pub downforce: [f64; 2],
    /// The first [`MAX_AERO_TELEMETRY`] aero elements' forces, in the order of the car's.
    pub aero: [AeroTelemetry; MAX_AERO_TELEMETRY],
    /// The air where the car is, and its velocity through it in body coordinates, m/s.
    pub air: Air,
    pub airspeed: DVec3,
    pub drag: f64,
    /// Ride height of the floor at the front / rear axle, m.
    pub ride_height: [f64; 2],
    /// Speed into a wall or the run-off barrier taken away by a hit this step, m/s
    /// (0 without an impact).
    pub barrier_impact: f64,
}

/// Aero elements reported in [`Telemetry::aero`].
pub const MAX_AERO_TELEMETRY: usize = 16;

/// One aero element in the last step.
#[derive(Clone, Copy, Debug, Default)]
pub struct AeroTelemetry {
    /// Downforce, N.
    pub lift: f64,
    /// Drag in body coordinates, N.
    pub drag: DVec3,
    /// Angle of attack, rad, and the floor's height under the element, m.
    pub angle_of_attack: f64,
    pub ride_height: f64,
}

/// What the car can come to harm from, chosen by the driver as in other sims. Heat
/// always changes what the brakes and the engine give; these decide whether anything
/// is lasting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Realism {
    /// Hits into walls damage the body: its aerodynamics, and the radiator and oil
    /// cooler behind the nose.
    pub damage: bool,
    /// The engine's parts wear when overheated or over-revved, and a broken one stops
    /// the engine for good.
    pub failures: bool,
}

impl Default for Realism {
    fn default() -> Self {
        Self {
            damage: true,
            failures: true,
        }
    }
}

/// A car driving on a track.
#[derive(Clone, Debug)]
pub struct Car {
    pub model: Arc<CarModel>,
    pub state: CarState,
    pub telemetry: Telemetry,
    pub realism: Realism,
}

impl Car {
    /// Places the car on the track at distance `s`, lateral offset `d`, pointing along
    /// the track, rolling at `speed` m/s in `gear`, with suspension at static ride height.
    pub fn new(model: Arc<CarModel>, track: &Track, s: f64, d: f64, speed: f64, gear: i32) -> Self {
        let mut car = Self {
            state: CarState {
                position: DVec3::ZERO,
                orientation: DQuat::IDENTITY,
                velocity: DVec3::ZERO,
                angular_velocity: DVec3::ZERO,
                wheels: [WheelState::default(); 4],
                rack: 0.0,
                drivetrain: DrivetrainState::new(&model.params, gear),
                damage: [0.0; DAMAGE_ZONES],
                time: 0.0,
                steps: 0,
            },
            telemetry: Telemetry::default(),
            realism: Realism::default(),
            model,
        };
        car.reset(track, s, d, speed, gear);
        car
    }

    /// Places the car as [`Self::new`] does, in standard weather.
    pub fn reset(&mut self, track: &Track, s: f64, d: f64, speed: f64, gear: i32) {
        self.reset_in(track, &STANDARD_WEATHER, s, d, speed, gear);
    }

    /// Places the car as [`Self::new`] does, in `weather`: the brakes and the engine
    /// warmed on the way out in the air there, the tyres inflated in it.
    pub fn reset_in(
        &mut self,
        track: &Track,
        weather: &Weather,
        s: f64,
        d: f64,
        speed: f64,
        gear: i32,
    ) {
        let m = &*self.model;
        let (surface, tangent, normal) = track.pose_at(s, d);
        let x = (tangent - normal * tangent.dot(normal)).normalize();
        let y = normal.cross(x);
        let orientation = DQuat::from_mat3(&DMat3::from_cols(x, y, normal)).normalize();
        let hint = track.nearest_index(surface);
        let air = weather.air_at(surface + normal * m.params.cg_height);

        let mut wheels = [WheelState::default(); 4];
        for (i, w) in wheels.iter_mut().enumerate() {
            let tire = m.tire(i);
            *w = WheelState {
                spin: speed / tire.p.radius,
                hint,
                tire: tire.fresh(air.temperature),
                brake: m.brakes.fresh(air.temperature),
                ..Default::default()
            };
        }
        let mut drivetrain = DrivetrainState::new(&m.params, gear);
        let ratio = drivetrain.ratio(&m.params);
        let share = m.params.drive.front_share();
        let wheel_spin = share * wheels[FL].spin + (1.0 - share) * wheels[RL].spin;
        drivetrain.engine_speed = drivetrain.engine_speed.max(wheel_spin * ratio);
        if ratio != 0.0 {
            drivetrain.input_speed = wheel_spin * ratio;
        }
        drivetrain.engine = m.engine.settled(drivetrain.rpm(), 0.0);
        drivetrain.engine.heat = m.engine.thermal.warmed(air.temperature, air.density);

        self.state = CarState {
            position: surface + normal * m.params.cg_height,
            orientation,
            velocity: x * speed,
            angular_velocity: DVec3::ZERO,
            wheels,
            rack: 0.0,
            drivetrain,
            damage: [0.0; DAMAGE_ZONES],
            time: 0.0,
            steps: 0,
        };
        self.telemetry = Telemetry::default();
    }

    /// Restarts a stalled engine (starter button).
    pub fn restart_engine(&mut self) {
        self.state.drivetrain.restart(&self.model.params);
    }

    /// Speed of the CG in m/s.
    pub fn speed(&self) -> f64 {
        self.state.velocity.length()
    }

    /// CG velocity in body coordinates.
    pub fn local_velocity(&self) -> DVec3 {
        self.state.orientation.inverse() * self.state.velocity
    }

    /// The upright's pose of wheel `i` (relative to its static wheel centre).
    pub fn pose(&self, i: usize) -> Pose {
        self.model
            .pose(i, self.state.wheels[i].travel, self.state.rack)
    }

    /// Wheel centre of wheel `i` in body coordinates.
    pub fn wheel_center_body(&self, i: usize) -> DVec3 {
        self.model.corners[i].origin + self.pose(i).centre
    }

    /// Wheel centre in world coordinates (for rendering).
    pub fn wheel_center(&self, i: usize) -> DVec3 {
        self.state.position + self.state.orientation * self.wheel_center_body(i)
    }

    /// Lines between the joints of wheel `i`'s suspension in body coordinates, appended
    /// to `out` (for rendering).
    pub fn linkage_segments(&self, i: usize, out: &mut Vec<(DVec3, DVec3)>) {
        self.model
            .linkage_segments(i, self.state.wheels[i].travel, self.state.rack, out);
    }

    /// Advances the simulation by one fixed step of `DT`, with the same grip over the
    /// whole asphalt and nothing left on it (see [`Self::step_evolving`]).
    pub fn step(&mut self, track: &Track, controls: &Controls) {
        let mut uniform = TrackEvolution::UNIFORM;
        self.step_evolving(track, &mut uniform, controls);
    }

    /// Advances the simulation by one fixed step of `DT` on a track with the rubber and
    /// dirt of `evolution`, in standard weather (see [`Self::step_in`]).
    pub fn step_evolving(
        &mut self,
        track: &Track,
        evolution: &mut TrackEvolution,
        controls: &Controls,
    ) {
        self.step_in(track, evolution, &STANDARD_WEATHER, controls);
    }

    /// Advances the simulation by one fixed step of `DT` on a track with the rubber and
    /// dirt of `evolution`, in `weather`. The tyres lay rubber where they roll on the
    /// asphalt, pick up dirt off the road and leave it where they rejoin. The air's
    /// density sets the aerodynamic forces and the engine's power, the wind adds to the
    /// airspeed, and the air and the road under each tyre heat or cool its tread.
    pub fn step_in(
        &mut self,
        track: &Track,
        evolution: &mut TrackEvolution,
        weather: &Weather,
        controls: &Controls,
    ) {
        let model = &*self.model;
        let p = &model.params;
        let st = &mut self.state;
        let tel = &mut self.telemetry;
        let dt = DT;
        let c = controls.sanitized(p.steering.lock);

        let rot = DMat3::from_quat(st.orientation);
        let rot_t = rot.transpose();
        let omega = st.angular_velocity;
        st.rack = c.steer_wheel_angle * model.front_kinematics.rack_gain;
        let poses: [Pose; 4] = std::array::from_fn(|i| model.pose(i, st.wheels[i].travel, st.rack));
        let air = weather.air_at(st.position);
        let (engine_bay, intake) = {
            let h = &st.drivetrain.engine.heat;
            (h.bay, h.intake)
        };

        // ---- Tyres ---------------------------------------------------------------
        // Per wheel: the wheel centre, the tyre's force and moment in body coordinates,
        // the contact patch relative to the wheel centre, and the road torque.
        let mut center_body = [DVec3::ZERO; 4];
        let mut tire_force_body = [DVec3::ZERO; 4];
        let mut tire_moment_body = [DVec3::ZERO; 4];
        let mut lever = [DVec3::ZERO; 4];
        let mut road_torque = [0.0; 4];
        // Generalised forces of the tyres along each wheel's travel (bump) and on the
        // rack (to the left), N: the work they do as the linkage moves.
        let mut travel_force = [0.0; 4];
        let mut rack_force = 0.0;
        // Tyre deflection, m (negative: the tyre is off the ground by that much).
        let mut deflection = [0.0; 4];
        // Closest any wheel comes to the run-off barrier, m.
        let mut barrier_clearance = f64::INFINITY;

        for i in 0..4 {
            let corner = &model.corners[i];
            let axle = model.axle(i);
            let tire = model.tire(i);
            let tp = &tire.p;
            let w = &mut st.wheels[i];
            let pose = &poses[i];

            center_body[i] = corner.origin + pose.centre;
            let center = st.position + rot * center_body[i];
            let center_vel = st.velocity
                + rot * (omega.cross(center_body[i]) + pose.centre_travel * w.travel_rate);

            let q = track.query(center, w.hint);
            w.hint = q.index;
            barrier_clearance = barrier_clearance.min(-q.beyond_barrier(track));
            let n = q.normal;
            let height = (center - q.surface_point).dot(n);
            let penetration = tp.radius - height;
            deflection[i] = penetration;
            let pressure = w.tire.pressure(axle.pressure);
            let fz = if penetration > 0.0 {
                (tire.vertical_stiffness(pressure) * penetration
                    - tp.vertical_damping * center_vel.dot(n))
                .max(0.0)
            } else {
                0.0
            };

            // Wheel frame projected onto the road: the spin axis (pointing left) as the
            // linkage holds it, and the direction it rolls in.
            let delta = pose.steer();
            let axis = rot * pose.axis;
            let long = axis.cross(n).normalize();
            let lat = n.cross(long);
            let inclination = axis.dot(n).clamp(-1.0, 1.0).asin();

            let vx = center_vel.dot(long);
            let vy = center_vel.dot(lat);
            let radius = tp.radius - (penetration.max(0.0) / 3.0);
            let slip_vel = w.spin * radius - vx;
            let speed = vx.abs();

            // Transient slips: σ dκ/dt + |vx| κ = ωr − vx (exact integration of the linear ODE).
            if speed > 1e-3 {
                let kss = slip_vel / speed;
                let ass = -vy / speed;
                w.kappa = kss + (w.kappa - kss) * (-speed * dt / tp.relaxation_x).exp();
                w.alpha = ass + (w.alpha - ass) * (-speed * dt / tp.relaxation_y).exp();
            } else {
                w.kappa += dt * slip_vel / tp.relaxation_x;
                w.alpha += dt * -vy / tp.relaxation_y;
            }
            w.kappa = w.kappa.clamp(-2.0, 2.0);
            w.alpha = w.alpha.clamp(-1.5, 1.5);

            let optimal = -corner.side * tp.optimal_camber;
            let camber_grip =
                (1.0 - tp.camber_grip_loss * (inclination - optimal).powi(2)).max(0.5);
            let alpha_eff = w.alpha - tp.camber_thrust * inclination;
            // Speed at which the contact patch slides over the road.
            let slide_speed = (slip_vel * slip_vel + vy * vy).sqrt();
            // Lateral force towards the centreline per unit load, from the pure-slip curve.
            let inward_force = -corner.side * tp.mu_y * tire.lat.eval(alpha_eff);
            let tread_load = tire.tread_load(corner.side * inclination, inward_force, pressure);
            let mu = q.grip
                * evolution.grip_at(q.surface, q.s, q.d)
                * camber_grip
                * tire.slide_grip(slide_speed)
                * tire.condition_grip(&w.tire, &tread_load, pressure);
            let mut f = tire.forces(w.kappa, alpha_eff, fz, mu, pressure);
            // The pneumatic trail lies behind the patch's middle in the direction the
            // wheel rolls, so it swaps ends in reverse; on loose ground the tyre ploughs
            // and the lateral force acts nearer the middle.
            let firmness = q.surface.firmness();
            f.mz *= (vx / TRAIL_REVERSAL_SPEED).clamp(-1.0, 1.0) * firmness;
            // Turning the wheel over the road (steering, the car yawing) twists the patch
            // until it slips round; rolling unwinds it once fresh tread has come through
            // half the patch, whose length follows from the tyre's deflection.
            let turn = delta - w.steer + omega.dot(rot_t * n) * dt;
            let half_patch = (2.0 * tp.radius * penetration.max(0.0)).sqrt().max(0.01);
            let unwound = w.twist * (-speed * dt / half_patch).exp();
            w.steer = delta;
            w.twist = tire.twist(unwound, turn, fz, tp.mu_y * mu, pressure, firmness);
            f.mz -= w.twist;

            let blend = (1.0 - speed / LOW_SPEED).max(0.0);
            if blend > 0.0 {
                let limit_x = tp.mu_x * mu * fz;
                let limit_y = tp.mu_y * mu * fz;
                let damping = blend * LOW_SPEED_DAMPING * fz;
                f.fx = (f.fx + damping * slip_vel).clamp(-limit_x, limit_x);
                f.fy = (f.fy - damping * vy).clamp(-limit_y, limit_y);
            }

            if fz > 0.0 {
                let shed = w
                    .tire
                    .roll_dirt(q.surface, q.dirt, speed * dt, slide_speed * dt);
                if q.surface == Surface::Asphalt {
                    let grip_use =
                        (f.fx * f.fx + f.fy * f.fy).sqrt() / (tp.mu_y * mu * fz).max(1e-9);
                    let picked = evolution.roll(q.s, q.d, speed * dt, grip_use, shed);
                    w.tire.add_coat(picked);
                }
            }

            let rolling_resistance = tire.rolling_resistance(pressure);
            let slide_power = (f.fx * slip_vel - f.fy * vy).max(0.0);
            let road = weather.road_temperature(q.s, q.d);
            let bay = if corner.front == p.engine.position.front() {
                NEAR_BAY_AIR * (engine_bay - air.temperature)
            } else {
                0.0
            };
            let tyre_air = air.temperature + NEAR_ROAD_AIR * (road - air.temperature) + bay;
            tire.update_condition(
                &mut w.tire,
                &tread_load,
                slide_power,
                rolling_resistance * fz * speed,
                speed,
                fz > 0.0,
                tyre_air,
                road,
                dt,
            );

            let rolling =
                (rolling_resistance + q.drag) * fz * radius * (w.spin * radius / 0.5).tanh();
            road_torque[i] = -f.fx * radius - rolling;

            let force = long * f.fx + lat * f.fy + n * fz;
            let contact = center - n * height;
            let (f_body, m_body) = (rot_t * force, rot_t * (n * f.mz));
            tire_force_body[i] = f_body;
            tire_moment_body[i] = m_body;
            lever[i] = rot_t * (contact - center);
            // How far the contact patch (fixed to the upright) moves per unit of travel
            // and of rack, and how far the upright turns under the aligning moment.
            let work = |centre: DVec3, spin: DVec3| {
                f_body.dot(centre + spin.cross(lever[i])) + m_body.dot(spin)
            };
            travel_force[i] = work(pose.centre_travel, pose.spin_travel);
            if corner.front {
                rack_force += work(pose.centre_rack, pose.spin_rack);
            }

            tel.wheels[i] = WheelTelemetry {
                load: fz,
                slip_ratio: w.kappa,
                slip_angle: w.alpha.atan(),
                slide_speed,
                pressure,
                tread_load,
                fx: f.fx,
                fy: f.fy,
                mz: f.mz,
                steer: delta,
                inclination,
                surface: q.surface,
                contact,
                suspension_force: 0.0,
                camber: pose.camber(corner.side),
                force,
                grip: mu,
                air_temperature: tyre_air,
                road_temperature: road,
            };
        }

        // ---- Drivetrain and wheel spin --------------------------------------------
        let drive = drivetrain::step(
            &mut st.drivetrain,
            p,
            &model.engine,
            &DriveInput {
                throttle: c.throttle,
                brake: c.brake,
                clutch_pedal: c.clutch,
                shift: c.shift,
                selector: c.selector,
                air: intake_air(&air, intake),
                wheel_speed: st.wheels.map(|w| w.spin),
                wheel_torque: road_torque,
                wheel_inertia: [0, 1, 2, 3].map(|i| model.axle(i).wheel_inertia),
                dt,
            },
        );

        for (i, &road_torque) in road_torque.iter().enumerate() {
            let axle = model.axle(i);
            let w = &mut st.wheels[i];
            let drive_torque = drive[i];
            let brake_torque = model.brakes.torque(i, c.brake, &w.brake);
            let free = w.spin + dt * (road_torque + drive_torque) / axle.wheel_inertia;
            let brake_dv = dt * brake_torque / axle.wheel_inertia;
            w.spin = if free.abs() <= brake_dv {
                0.0
            } else {
                free - brake_dv * free.signum()
            };
            // The kinetic energy the brake took from the wheel heats it.
            model.brakes.heat(
                i,
                &mut w.brake,
                0.5 * axle.wheel_inertia * (free * free - w.spin * w.spin),
            );
            w.angle = (w.angle + w.spin * dt).rem_euclid(std::f64::consts::TAU);
            // The driveshaft turns the wheel against the body, so its torque also does
            // work as the upright turns about the axle (squat) or steers (torque steer).
            let pose = &poses[i];
            travel_force[i] += drive_torque * pose.axis.dot(pose.spin_travel);
            if i < 2 {
                rack_force += drive_torque * pose.axis.dot(pose.spin_rack);
            }
        }

        // ---- Suspension -------------------------------------------------------------
        // Force of the coil-over, bar and heave spring on each actuation (compression
        // positive), and what they and the bump stops hold against the travel.
        let compression = poses.map(|p| p.actuation);
        let compression_rate: [f64; 4] =
            std::array::from_fn(|i| poses[i].motion_ratio * st.wheels[i].travel_rate);
        let mut susp = [0.0; 4];
        for (i, suspension) in susp.iter_mut().enumerate() {
            let corner = &model.corners[i];
            let axle = model.axle(i);
            let w = &st.wheels[i];
            let spring = corner.preload + axle.spring_rate * compression[i];
            let arb = axle.anti_roll_rate * (compression[i] - compression[i ^ 1]);
            let heave = axle.heave.as_ref().map_or(0.0, |h| {
                let (a, b) = (i & !1, i | 1);
                let mean = 0.5 * (compression[a] + compression[b]);
                let rate = 0.5 * (compression_rate[a] + compression_rate[b]);
                0.5 * (h.rate * (mean - h.gap).max(0.0) + h.damping * rate)
            });
            let element = spring + arb + heave + damper(axle, compression_rate[i]);
            let stop = if w.travel > corner.bump_stop {
                axle.bump_stop_rate * (w.travel - corner.bump_stop)
            } else if w.travel < corner.droop_stop {
                -axle.bump_stop_rate * (corner.droop_stop - w.travel)
            } else {
                0.0
            };
            *suspension = element * poses[i].motion_ratio + stop;
            if i < 2 {
                rack_force -= element * poses[i].actuation_rack;
            }
            tel.wheels[i].suspension_force = *suspension;
        }
        tel.steering_torque = rack_force * model.front_kinematics.rack_gain;

        // ---- Body forces --------------------------------------------------------------
        // The linkage passes the tyre's force and moment to the body, except what the
        // travel takes up: the unsprung mass moves along its path, on which the
        // suspension's force acts instead. The body then moves with the unsprung masses
        // across their paths, and without them along.
        let gravity = rot_t * DVec3::new(0.0, 0.0, -GRAVITY);
        let mut force = DVec3::ZERO;
        let mut torque = DVec3::ZERO;
        let mut mass = DMat3::from_diagonal(DVec3::splat(p.mass));
        for i in 0..4 {
            let path = poses[i].centre_travel;
            let along = path.length_squared();
            let f = tire_force_body[i] - path * ((travel_force[i] - susp[i]) / along);
            force += f;
            torque +=
                center_body[i].cross(f) + lever[i].cross(tire_force_body[i]) + tire_moment_body[i];
            let m_u = model.axle(i).unsprung_mass / along;
            mass -= DMat3::from_cols(
                path * (m_u * path.x),
                path * (m_u * path.y),
                path * (m_u * path.z),
            );
        }

        // Airspeed: the car's velocity through the moving air.
        let v_body = rot_t * (st.velocity - air.wind);
        let q_dyn = 0.5 * air.density * v_body.x * v_body.x;
        let mut downforce = [0.0; 2];
        // Drag per m² of drag area.
        let drag_unit = -0.5 * air.density * v_body.length() * v_body;
        let mut drag = DVec3::ZERO;
        let ride = ride_heights(model, st, &deflection);
        {
            let (front_x, rear_x) = (model.corners[FL].origin.x, model.corners[RL].origin.x);
            let wheelbase = front_x - rear_x;
            let [front_static, rear_static] = p.aero.ride_height;
            // Nose down positive, from the attitude at rest.
            let pitch = ((ride[1] - ride[0]) - (rear_static - front_static)) / wheelbase;
            let sideslip = if v_body.x.abs() > 1.0 {
                (v_body.y / v_body.x.abs()).atan().abs()
            } else {
                0.0
            };
            let damage = st.damage;
            let intact = |loss: &[f64; DAMAGE_ZONES]| {
                (1.0 - (0..DAMAGE_ZONES).map(|z| loss[z] * damage[z]).sum::<f64>()).max(0.0)
            };
            for (k, e) in p.aero.elements.iter().enumerate() {
                // Where the element sits between the axles, 0 at the front, 1 at the rear.
                let t = (front_x - e.position[0]) / wheelbase;
                let height = (ride[0] + (ride[1] - ride[0]) * t).max(0.0);
                let map = |table: &[(f64, f64)]| {
                    if table.is_empty() {
                        1.0
                    } else {
                        lookup(table, height)
                    }
                };
                let aoa = e.angle + pitch;
                let lift = q_dyn
                    * e.area
                    * lookup(&e.lift, aoa)
                    * map(&e.height_lift)
                    * (1.0 + e.yaw_lift * sideslip)
                    * intact(&e.damage_lift);
                let element_drag = drag_unit
                    * (e.area * lookup(&e.drag, aoa) * map(&e.height_drag))
                    * intact(&e.damage_drag);
                let f = element_drag - DVec3::Z * lift;
                force += f;
                torque += DVec3::new(e.position[0], 0.0, e.position[1]).cross(f);
                drag += element_drag;
                downforce[0] += lift * (1.0 - t);
                downforce[1] += lift * t;
                if let Some(a) = tel.aero.get_mut(k) {
                    *a = AeroTelemetry {
                        lift,
                        drag: element_drag,
                        angle_of_attack: aoa,
                        ride_height: height,
                    };
                }
            }
        }
        tel.downforce = downforce;
        tel.air = air;
        tel.airspeed = v_body;
        tel.drag = drag.length();
        tel.ride_height = ride;

        // Specific force (what an accelerometer measures).
        let accel = mass.inverse() * force;
        tel.acceleration = accel;
        let inertia = DVec3::from_array(p.inertia);
        let ang_accel = (torque - omega.cross(inertia * omega)) / inertia;

        // ---- Unsprung masses ----------------------------------------------------------
        // Each moves along its path under the tyre's and the suspension's forces, and
        // with the body's acceleration where it is.
        for i in 0..4 {
            let m_u = model.axle(i).unsprung_mass;
            let path = poses[i].centre_travel;
            let c = center_body[i];
            let body_accel = accel + ang_accel.cross(c) + omega.cross(omega.cross(c));
            let travel_accel = (travel_force[i] - susp[i] - m_u * path.dot(body_accel))
                / (m_u * path.length_squared());
            let w = &mut st.wheels[i];
            w.travel_rate += travel_accel * dt;
            w.travel += w.travel_rate * dt;
            // Hard mechanical limits beyond the bump stops.
            let (lo, hi) = model.kinematics(i).travel;
            if w.travel < lo || w.travel > hi {
                w.travel = w.travel.clamp(lo, hi);
                w.travel_rate = 0.0;
            }
        }
        let accel = accel + gravity;

        // ---- Integrate chassis (semi-implicit Euler) ----------------------------------
        st.velocity += rot * accel * dt;
        st.angular_velocity += ang_accel * dt;
        st.position += st.velocity * dt;
        let w = st.angular_velocity * dt;
        st.orientation = (st.orientation * DQuat::from_scaled_axis(w)).normalize();
        st.time += dt;

        // ---- Walls and the barrier at the edge of the run-off --------------------------
        let (impact, push) = collide(model, track, st, barrier_clearance);
        tel.barrier_impact = impact;
        if impact > DAMAGE_THRESHOLD && self.realism.damage {
            // The wall is on the side the car was pushed away from.
            let away = st.orientation.inverse() * push;
            let zone = if away.x.abs() >= away.y.abs() {
                if away.x < 0.0 { 0 } else { 1 }
            } else if away.y < 0.0 {
                2
            } else {
                3
            };
            st.damage[zone] += impact - DAMAGE_THRESHOLD;
        }

        // ---- Heat among the brakes' and the engine's parts and the air ----------------
        if st.steps.is_multiple_of(THERMAL_STEPS) {
            let dt = THERMAL_STEPS as f64 * dt;
            let airflow = Airflow {
                speed: v_body.x,
                temperature: air.temperature,
                density: air.density,
            };
            for (i, w) in st.wheels.iter_mut().enumerate() {
                let tire = model.tire(i);
                let to_tyre = model.brakes.exchange(
                    i,
                    &mut w.brake,
                    w.spin * tire.p.radius,
                    &airflow,
                    w.tire.core_temperature,
                    dt,
                );
                tire.heat_core(&mut w.tire, to_tyre * dt);
            }
            let d = &mut st.drivetrain;
            model.engine.thermal.exchange(
                &mut d.engine.heat,
                d.engine_speed,
                &airflow,
                st.damage[0],
                self.realism.failures,
                dt,
            );
        }
        st.steps += 1;
    }
}

/// The air the engine breathes: the weather's `air` at the pressure there, warmed to
/// `intake` °C in the intake, which thins the charge (SAE J1349's temperature term).
fn intake_air(air: &Air, intake: f64) -> Ambient {
    let pressure = air.pressure * HECTOPASCAL;
    let warming = ((air.temperature + KELVIN) / (intake + KELVIN)).sqrt();
    Ambient {
        pressure,
        temperature: intake,
        charge: air.engine * STANDARD_PRESSURE / pressure * warming,
    }
}

/// Ride height of the floor at the front / rear axle: the static ride height lowered by
/// how far the wheels have risen into the body and by how far the tyres are deflected,
/// from rest.
fn ride_heights(model: &CarModel, st: &CarState, deflection: &[f64; 4]) -> [f64; 2] {
    let lift = |i: usize| {
        let c = &model.corners[i];
        -st.wheels[i].travel - (deflection[i] - c.static_deflection)
    };
    let [front, rear] = model.params.aero.ride_height;
    [
        front + 0.5 * (lift(0) + lift(1)),
        rear + 0.5 * (lift(2) + lift(3)),
    ]
}

/// Pushes the car back so no wheel is inside a wall or beyond the run-off barrier, and
/// takes away the speed into it. Returns that speed, m/s (0 without an impact), and the
/// direction the car was pushed (world).
/// `barrier_clearance` is how far inside the barrier the wheels were before the step.
fn collide(
    model: &CarModel,
    track: &Track,
    st: &mut CarState,
    barrier_clearance: f64,
) -> (f64, DVec3) {
    let rot = DMat3::from_quat(st.orientation);
    let mut push = DVec3::ZERO;
    let mut depth = 0.0;
    for i in 0..4 {
        let local = model.corners[i].origin + model.pose(i, st.wheels[i].travel, st.rack).centre;
        let center = st.position + rot * local;
        let contacts = [
            if barrier_clearance < BARRIER_SKIN {
                track.barrier_contact(center, st.wheels[i].hint)
            } else {
                None
            },
            track.wall_contact(center, model.tire(i).p.radius),
        ];
        for (dir, excess) in contacts.into_iter().flatten() {
            if excess > depth {
                depth = excess;
                push = dir;
            }
        }
    }
    if depth <= 0.0 {
        return (0.0, push);
    }
    st.position += push * depth;
    let outward = -st.velocity.dot(push);
    if outward <= 0.0 {
        return (0.0, push);
    }
    // Inelastic hit; friction against the barrier scrubs speed along it.
    let impulse = (1.0 + BARRIER_RESTITUTION) * outward;
    let along = st.velocity + push * outward;
    let scrub = (BARRIER_FRICTION * impulse / along.length().max(1e-9)).min(1.0);
    st.velocity = push * (BARRIER_RESTITUTION * outward) + along * (1.0 - scrub);
    (outward, push)
}

/// Force of `axle`'s damper at `rate` of compression (negative in rebound), N: each
/// direction's low-speed damping up to the knee, its high-speed damping beyond.
fn damper(axle: &AxleParams, rate: f64) -> f64 {
    let (slow, fast) = if rate > 0.0 {
        (axle.bump_damping, axle.fast_bump_damping)
    } else {
        (axle.rebound_damping, axle.fast_rebound_damping)
    };
    let speed = rate.abs();
    let knee = axle.damper_knee;
    let force = if speed <= knee {
        slow * speed
    } else {
        slow * knee + fast.unwrap_or(slow) * (speed - knee)
    };
    force.copysign(rate)
}
