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
//!   grip scaled by inflation pressure, tread temperature and wear.
//! - Wheels spin under drive, brake and road torque; brakes lock the wheel exactly.

use std::sync::Arc;

use glam::{DMat3, DQuat, DVec3};

use crate::controls::{Controls, Shift};
use crate::drivetrain::{self, DriveInput, DrivetrainState};
use crate::params::CarModel;
use crate::tire::TireCondition;
use crate::track::{Surface, Track};
use crate::{AIR_DENSITY, DT, GRAVITY, RL, RR};

/// Below this speed a slip-velocity damping term is blended in so the relaxation
/// length model does not oscillate at standstill.
const LOW_SPEED: f64 = 3.0;
/// Low-speed damping per newton of load, N·s/m per N.
const LOW_SPEED_DAMPING: f64 = 3.0;
/// Fraction of the normal speed returned when hitting the run-off barrier.
const BARRIER_RESTITUTION: f64 = 0.2;
/// Coulomb friction coefficient between the car and the run-off barrier.
const BARRIER_FRICTION: f64 = 0.3;

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
    /// Torque at the steering wheel from the tyres' aligning moments, N·m.
    /// Positive turns the steering wheel left. This is the force-feedback source.
    pub steering_torque: f64,
    /// Aerodynamic downforce, front / rear, N.
    pub downforce: [f64; 2],
    pub drag: f64,
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
        let wheel_spin = wheels[RL].spin;
        drivetrain.engine_speed = drivetrain.engine_speed.max(wheel_spin * ratio);

        self.state = CarState {
            position: surface + normal * m.params.cg_height,
            orientation,
            velocity: x * speed,
            angular_velocity: DVec3::ZERO,
            wheels,
            drivetrain,
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

    /// Advances the simulation by one fixed step of `DT`.
    pub fn step(&mut self, track: &Track, controls: &Controls) {
        let model = &*self.model;
        let p = &model.params;
        let st = &mut self.state;
        let tel = &mut self.telemetry;
        let dt = DT;
        let c = controls.sanitized(p.steering.lock);

        match c.shift {
            Shift::None => {}
            Shift::Up => st.drivetrain.request_shift(p, true),
            Shift::Down => st.drivetrain.request_shift(p, false),
        }

        let rot = DMat3::from_quat(st.orientation);
        let rot_t = rot.transpose();
        let omega = st.angular_velocity;
        let steer = steer_angles(model, c.steer_wheel_angle);

        // ---- Tyres ---------------------------------------------------------------
        // Per wheel: tyre force in body coordinates, application point, road torque.
        let mut tire_force_body = [DVec3::ZERO; 4];
        let mut contact_body = [DVec3::ZERO; 4];
        let mut road_torque = [0.0; 4];
        let mut steering_torque = 0.0;

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
            let n = q.normal;
            let height = (center - q.surface_point).dot(n);
            let penetration = tp.radius - height;
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
            let (sc, cc) = axle.static_camber.sin_cos();
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
            // Lateral force towards the centreline per unit load, from the pure-slip curve.
            let inward_force = -corner.side * tp.mu_y * tire.lat.eval(alpha_eff);
            let tread_load = tire.tread_load(corner.side * inclination, inward_force, pressure);
            let mu = q.grip * camber_grip * tire.condition_grip(&w.tire, &tread_load, pressure);
            let mut f = tire.forces(w.kappa, alpha_eff, fz, mu, pressure);

            let blend = (1.0 - speed / LOW_SPEED).max(0.0);
            if blend > 0.0 {
                let limit_x = tp.mu_x * mu * fz;
                let limit_y = tp.mu_y * mu * fz;
                let damping = blend * LOW_SPEED_DAMPING * fz;
                f.fx = (f.fx + damping * slip_vel).clamp(-limit_x, limit_x);
                f.fy = (f.fy - damping * vy).clamp(-limit_y, limit_y);
            }

            let rolling_resistance = tire.rolling_resistance(pressure);
            let slide_power = (f.fx * slip_vel - f.fy * vy).max(0.0);
            tire.update_condition(
                &mut w.tire,
                &tread_load,
                slide_power,
                rolling_resistance * fz * speed,
                speed,
                fz > 0.0,
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
                steering_torque += f.mz;
            }

            tel.wheels[i] = WheelTelemetry {
                load: fz,
                slip_ratio: w.kappa,
                slip_angle: w.alpha.atan(),
                slide_speed: slip_vel.hypot(vy),
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
            &DriveInput {
                throttle: c.throttle,
                clutch_pedal: c.clutch,
                wheel_speed: [st.wheels[RL].spin, st.wheels[RR].spin],
                wheel_torque: [road_torque[RL], road_torque[RR]],
                wheel_inertia: p.rear.wheel_inertia,
                dt,
            },
        );

        for (i, &road_torque) in road_torque.iter().enumerate() {
            let axle = model.axle(i);
            let w = &mut st.wheels[i];
            let drive_torque = match i {
                RL => drive[0],
                RR => drive[1],
                _ => 0.0,
            };
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

        let v_body = rot_t * st.velocity;
        let q_dyn = 0.5 * AIR_DENSITY * v_body.x * v_body.x;
        let downforce = [
            q_dyn * p.aero.downforce_area_front,
            q_dyn * p.aero.downforce_area_rear,
        ];
        let drag = -0.5 * AIR_DENSITY * p.aero.drag_area * v_body.length() * v_body;
        let drag_point = DVec3::new(0.0, 0.0, p.aero.drag_height);
        force += drag;
        torque += drag_point.cross(drag);
        for (k, &fd) in downforce.iter().enumerate() {
            let f = DVec3::new(0.0, 0.0, -fd);
            force += f;
            torque += model.corners[k * 2].hardpoint.with_y(0.0).cross(f);
        }
        tel.downforce = downforce;
        tel.drag = drag.length();

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
        // Push the car back so no wheel is inside them and remove the outward velocity.
        let rot = DMat3::from_quat(st.orientation);
        let mut push = DVec3::ZERO;
        let mut depth = 0.0;
        for i in 0..4 {
            let center = st.position
                + rot * (model.corners[i].hardpoint - DVec3::Z * st.wheels[i].extension);
            let q = track.query(center, st.wheels[i].hint);
            let excess = q.beyond_barrier(track);
            if excess > depth {
                depth = excess;
                push = q.lateral * -q.d.signum();
            }
            if let Some((dir, excess)) = track.wall_contact(center, model.tire(i).p.radius)
                && excess > depth
            {
                depth = excess;
                push = dir;
            }
        }
        if depth > 0.0 {
            st.position += push * depth;
            let outward = -st.velocity.dot(push);
            if outward > 0.0 {
                // Inelastic hit; friction against the barrier scrubs speed along it.
                let impulse = (1.0 + BARRIER_RESTITUTION) * outward;
                let along = st.velocity + push * outward;
                let scrub = (BARRIER_FRICTION * impulse / along.length().max(1e-9)).min(1.0);
                st.velocity = push * (BARRIER_RESTITUTION * outward) + along * (1.0 - scrub);
            }
        }
    }
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
