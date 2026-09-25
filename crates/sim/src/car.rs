//! Rigid-body chassis with four suspended wheels.
//!
//! Model summary:
//! - Chassis: 6-DOF rigid body. Translation along body x/y carries the full vehicle
//!   mass (wheels are rigidly linked in those directions); along body z only the
//!   sprung mass, because the wheels move vertically on their own DOF.
//! - Each corner: a vertical strut (body −z) with spring, bump/rebound damping,
//!   anti-roll bar and bump stops; an unsprung mass on the strut; a tyre with
//!   vertical stiffness/damping against the road surface.
//! - Tyres: transient slips via relaxation length, Magic Formula combined forces,
//!   grip scaled by load, sliding speed, inflation pressure, tread temperature and wear;
//!   camber follows the suspension's travel.
//! - Wheels spin under drive, brake and road torque; brakes lock the wheel exactly.
//! - Aerodynamics: elements (body, wings, floor) whose coefficients follow their angle
//!   of attack (the car's pitch), the floor's ride height under them, the
//!   sideslip and the damage the car has taken.

use std::sync::Arc;

use glam::{DMat3, DQuat, DVec3};

use crate::controls::Controls;
use crate::drivetrain::{self, DriveInput, DrivetrainState};
use crate::engine::{Ambient, STANDARD_PRESSURE};
use crate::evolution::TrackEvolution;
use crate::params::{CarModel, DAMAGE_ZONES, SteeringParams, lookup};
use crate::tire::TireCondition;
use crate::track::{Surface, Track};
use crate::weather::Weather;
use crate::{DT, FL, GRAVITY, RL};

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
/// Impact speed into a wall that leaves no damage, m/s.
const DAMAGE_THRESHOLD: f64 = 2.0;
/// Pa per hPa.
const HECTOPASCAL: f64 = 100.0;
/// Standard weather for [`Car::step_evolving`], a static so that no step builds and
/// drops a copy of it.
static STANDARD_WEATHER: Weather = Weather::STANDARD;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelState {
    /// Strut extension below the hardpoint in m (larger = wheel further down).
    pub extension: f64,
    pub extension_rate: f64,
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
    pub drivetrain: DrivetrainState,
    /// Impact damage per zone of the body (front, rear, left, right): the speed of the
    /// hits into walls in that direction beyond a light touch, summed, m/s.
    pub damage: [f64; DAMAGE_ZONES],
    /// Simulated time in s.
    pub time: f64,
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
    /// Suspension force on the body, N.
    pub suspension_force: f64,
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
    pub drag: f64,
    /// Ride height of the floor at the front / rear axle, m.
    pub ride_height: [f64; 2],
    /// Speed into a wall or the run-off barrier taken away by a hit this step, m/s
    /// (0 without an impact).
    pub barrier_impact: f64,
}

/// A car driving on a track.
#[derive(Clone, Debug)]
pub struct Car {
    pub model: Arc<CarModel>,
    pub state: CarState,
    pub telemetry: Telemetry,
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
                drivetrain: DrivetrainState::new(&model.params, gear),
                damage: [0.0; DAMAGE_ZONES],
                time: 0.0,
            },
            telemetry: Telemetry::default(),
            model,
        };
        car.reset(track, s, d, speed, gear);
        car
    }

    pub fn reset(&mut self, track: &Track, s: f64, d: f64, speed: f64, gear: i32) {
        let m = &*self.model;
        let (surface, tangent, normal) = track.pose_at(s, d);
        let x = (tangent - normal * tangent.dot(normal)).normalize();
        let y = normal.cross(x);
        let orientation = DQuat::from_mat3(&DMat3::from_cols(x, y, normal)).normalize();
        let hint = track.nearest_index(surface);

        let mut wheels = [WheelState::default(); 4];
        for (i, (w, corner)) in wheels.iter_mut().zip(&m.corners).enumerate() {
            let tire = m.tire(i);
            *w = WheelState {
                extension: corner.static_extension,
                spin: speed / tire.p.radius,
                hint,
                tire: tire.fresh(),
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

        self.state = CarState {
            position: surface + normal * m.params.cg_height,
            orientation,
            velocity: x * speed,
            angular_velocity: DVec3::ZERO,
            wheels,
            drivetrain,
            damage: [0.0; DAMAGE_ZONES],
            time: 0.0,
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

    /// Wheel centre in world coordinates (for rendering).
    pub fn wheel_center(&self, i: usize) -> DVec3 {
        let c = &self.model.corners[i];
        let local = c.hardpoint - DVec3::Z * self.state.wheels[i].extension;
        self.state.position + self.state.orientation * local
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
        let steer = steer_angles(model, c.steer_wheel_angle);
        let air = weather.air_at(st.position);

        // ---- Tyres ---------------------------------------------------------------
        // Per wheel: tyre force in body coordinates, application point, road torque.
        let mut tire_force_body = [DVec3::ZERO; 4];
        let mut contact_body = [DVec3::ZERO; 4];
        let mut road_torque = [0.0; 4];
        let mut steering_torque = 0.0;
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

            let center_body = corner.hardpoint - DVec3::Z * w.extension;
            let center = st.position + rot * center_body;
            let center_vel =
                st.velocity + rot * (omega.cross(center_body) - DVec3::Z * w.extension_rate);

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

            // Wheel frame projected onto the road.
            let delta = steer[i];
            let (sd, cd) = delta.sin_cos();
            let camber =
                axle.static_camber + axle.camber_gain * (corner.static_extension - w.extension);
            let (sc, cc) = camber.sin_cos();
            let forward = rot * DVec3::new(cd, sd, 0.0);
            let axis = rot * DVec3::new(-sd * cc, cd * cc, -corner.side * sc);
            let long = (forward - n * forward.dot(n)).normalize();
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
            tire.update_condition(
                &mut w.tire,
                &tread_load,
                slide_power,
                rolling_resistance * fz * speed,
                speed,
                fz > 0.0,
                air.temperature + NEAR_ROAD_AIR * (road - air.temperature),
                road,
                dt,
            );

            let rolling =
                (rolling_resistance + q.drag) * fz * radius * (w.spin * radius / 0.5).tanh();
            road_torque[i] = -f.fx * radius - rolling;

            let force = long * f.fx + lat * f.fy + n * fz;
            let contact = center - n * height;
            tire_force_body[i] = rot_t * force;
            contact_body[i] = rot_t * (contact - st.position);
            if corner.front {
                steering_torque +=
                    kingpin_torque(&p.steering, corner.side, delta, tire_force_body[i], f.mz);
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
            };
        }
        tel.steering_torque = steering_torque / p.steering.ratio;

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
                air: Ambient {
                    pressure: air.pressure * HECTOPASCAL,
                    temperature: air.temperature,
                    charge: air.engine * STANDARD_PRESSURE / (air.pressure * HECTOPASCAL),
                },
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
            let share = if i < 2 {
                p.brakes.front_bias
            } else {
                1.0 - p.brakes.front_bias
            };
            let brake_torque = c.brake * p.brakes.max_torque * share * 0.5;
            let free = w.spin + dt * (road_torque + drive_torque) / axle.wheel_inertia;
            let brake_dv = dt * brake_torque / axle.wheel_inertia;
            w.spin = if free.abs() <= brake_dv {
                0.0
            } else {
                free - brake_dv * free.signum()
            };
            w.angle = (w.angle + w.spin * dt).rem_euclid(std::f64::consts::TAU);
        }

        // ---- Suspension -------------------------------------------------------------
        let mut susp = [0.0; 4];
        for (i, suspension) in susp.iter_mut().enumerate() {
            let corner = &model.corners[i];
            let axle = model.axle(i);
            let w = &st.wheels[i];
            let other = &st.wheels[i ^ 1];
            let spring = axle.spring_rate * (corner.spring_free_extension - w.extension);
            let arb = axle.anti_roll_rate * (other.extension - w.extension);
            let compression_rate = -w.extension_rate;
            let damper = compression_rate
                * if compression_rate > 0.0 {
                    axle.bump_damping
                } else {
                    axle.rebound_damping
                };
            let stop = if w.extension < corner.min_extension {
                axle.bump_stop_rate * (corner.min_extension - w.extension)
            } else if w.extension > corner.max_extension {
                -axle.bump_stop_rate * (w.extension - corner.max_extension)
            } else {
                0.0
            };
            *suspension = spring + arb + damper + stop;
            tel.wheels[i].suspension_force = *suspension;
        }

        // ---- Body forces --------------------------------------------------------------
        let gravity = rot_t * DVec3::new(0.0, 0.0, -GRAVITY);
        let mut force = DVec3::ZERO;
        let mut torque = DVec3::ZERO;
        for i in 0..4 {
            let f_planar = DVec3::new(tire_force_body[i].x, tire_force_body[i].y, 0.0);
            force += f_planar;
            torque += contact_body[i].cross(f_planar);
            let f_susp = DVec3::new(0.0, 0.0, susp[i]);
            force += f_susp;
            torque += model.corners[i].hardpoint.cross(f_susp);
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
            let (front_x, rear_x) = (model.corners[FL].hardpoint.x, model.corners[RL].hardpoint.x);
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
            for e in &p.aero.elements {
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
            }
        }
        tel.downforce = downforce;
        tel.drag = drag.length();
        tel.ride_height = ride;

        let accel = DVec3::new(
            force.x / p.mass,
            force.y / p.mass,
            force.z / model.sprung_mass,
        );
        tel.acceleration = accel;
        let accel = accel + DVec3::new(gravity.x, gravity.y, gravity.z);
        let inertia = DVec3::from_array(p.inertia);
        let ang_accel = (torque - omega.cross(inertia * omega)) / inertia;

        // ---- Unsprung masses ----------------------------------------------------------
        for i in 0..4 {
            let corner = &model.corners[i];
            let m_u = model.axle(i).unsprung_mass;
            let hp = corner.hardpoint;
            let hp_accel = accel + ang_accel.cross(hp) + omega.cross(omega.cross(hp));
            let w = &mut st.wheels[i];
            let net = susp[i] - tire_force_body[i].z - m_u * gravity.z;
            let ext_accel = net / m_u + hp_accel.z;
            w.extension_rate += ext_accel * dt;
            w.extension += w.extension_rate * dt;
            // Hard mechanical limits beyond the bump stops.
            let (lo, hi) = (corner.min_extension - 0.03, corner.max_extension + 0.03);
            if w.extension < lo || w.extension > hi {
                w.extension = w.extension.clamp(lo, hi);
                w.extension_rate = 0.0;
            }
        }

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
        if impact > DAMAGE_THRESHOLD {
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
    }
}

/// Ride height of the floor at the front / rear axle: the static ride height raised by
/// how far the springs have extended and lowered by how far the tyres are deflected,
/// from rest.
fn ride_heights(model: &CarModel, st: &CarState, deflection: &[f64; 4]) -> [f64; 2] {
    let lift = |i: usize| {
        let c = &model.corners[i];
        (st.wheels[i].extension - c.static_extension) - (deflection[i] - c.static_deflection)
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
        let center =
            st.position + rot * (model.corners[i].hardpoint - DVec3::Z * st.wheels[i].extension);
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

/// Torque about a front wheel's steering axis, positive turning it left, from the
/// force on its contact patch (`force`, body coordinates, including the road's normal
/// force) and the tyre's aligning torque `mz`. The patch trails the axis by the
/// mechanical trail and lies outboard of it by the scrub radius, so a lateral force
/// (cornering, a kerb's slope) acts on the trail, a longitudinal one (braking, the edge
/// of a bump) on the scrub radius, and the load on the leaning axis.
fn kingpin_torque(s: &SteeringParams, side: f64, steer: f64, force: DVec3, mz: f64) -> f64 {
    let (sd, cd) = steer.sin_cos();
    let along = force.x * cd + force.y * sd;
    let across = force.y * cd - force.x * sd;
    let load = force.z;
    let (caster, kingpin) = (s.caster.sin(), s.kingpin_inclination.sin());
    mz - s.trail * across - side * s.scrub_radius * along
        // Opposite on the two sides, so it cancels until a bump or kerb loads one
        // wheel more than the other.
        - side * load * (s.scrub_radius * caster + s.trail * kingpin)
        // Steering lifts the car on the kingpin inclination, which centres the wheel.
        - load * s.scrub_radius * kingpin * sd
}

/// Road wheel angles for a steering wheel angle, with partial Ackermann.
fn steer_angles(model: &CarModel, steering_wheel: f64) -> [f64; 4] {
    let p = &model.params;
    let delta = steering_wheel / p.steering.ratio;
    let t = delta.tan();
    let half_track = 0.5 * p.track_front * p.steering.ackermann;
    let l = p.wheelbase;
    let left = (l * t / (l - half_track * t)).atan();
    let right = (l * t / (l + half_track * t)).atan();
    [left, right, 0.0, 0.0]
}
