//! Observation encoding.
//!
//! The default observation only uses quantities that common racing sims expose
//! through their telemetry (velocities, accelerations, wheel speeds, rpm, gear,
//! inputs) plus track geometry relative to the car. Ground-truth tyre state is
//! available separately as "privileged" features, so a policy can be trained with or
//! without them depending on the transfer target.

use glam::DVec3;
use open_racing_sim::{Car, GRAVITY, Track};

/// Names every dimension of a flat feature vector.
#[derive(Clone, Debug, PartialEq)]
pub struct ObsSpec {
    pub names: Vec<String>,
}

impl ObsSpec {
    pub fn dim(&self) -> usize {
        self.names.len()
    }
}

/// Scales that bring raw quantities to roughly unit range.
const SPEED_SCALE: f64 = 50.0;
const ACCEL_SCALE: f64 = 3.0 * GRAVITY;
const DISTANCE_SCALE: f64 = 100.0;
const LOAD_SCALE: f64 = 5000.0;
const TEMPERATURE_SCALE: f64 = 100.0;
const WIDTH_SCALE: f64 = 10.0;

#[derive(Clone, Copy, Debug)]
pub struct ObsLayout {
    pub lookahead_points: usize,
    pub lookahead_spacing: f64,
    pub privileged: bool,
    pub tyres: bool,
    pub edges: bool,
}

impl ObsLayout {
    pub fn spec(&self) -> ObsSpec {
        let mut names: Vec<String> = [
            "vel_long",
            "vel_lat",
            "yaw_rate",
            "accel_long",
            "accel_lat",
            "wheel_speed_fl",
            "wheel_speed_fr",
            "wheel_speed_rl",
            "wheel_speed_rr",
            "rpm",
            "gear",
            "steer",
            "throttle",
            "brake",
            "track_offset",
            "heading_sin",
            "heading_cos",
        ]
        .map(String::from)
        .to_vec();
        for k in 1..=self.lookahead_points {
            names.push(format!("ahead_{k}_x"));
            names.push(format!("ahead_{k}_y"));
        }
        if self.edges {
            for k in 1..=self.lookahead_points {
                names.push(format!("ahead_{k}_width_left"));
                names.push(format!("ahead_{k}_width_right"));
            }
        }
        if self.tyres {
            for w in ["fl", "fr", "rl", "rr"] {
                names.push(format!("tread_temp_{w}"));
                names.push(format!("pressure_{w}"));
            }
        }
        if self.privileged {
            for w in ["fl", "fr", "rl", "rr"] {
                names.push(format!("slip_angle_{w}"));
                names.push(format!("slip_ratio_{w}"));
                names.push(format!("load_{w}"));
            }
        }
        ObsSpec { names }
    }
}

/// Inputs the car actually received, normalised (steer −1..1, pedals 0..1).
#[derive(Clone, Copy, Debug, Default)]
pub struct AppliedInput {
    pub steer: f64,
    pub throttle: f64,
    pub brake: f64,
}

/// Writes the observation of `car` into `out` (length = `layout.spec().dim()`).
pub fn encode(
    layout: &ObsLayout,
    car: &Car,
    track: &Track,
    hint: usize,
    input: &AppliedInput,
    out: &mut [f32],
) {
    let st = &car.state;
    let p = &car.model.params;
    let inv = st.orientation.inverse();
    let v = inv * st.velocity;
    let tel = &car.telemetry;
    let q = track.query(st.position, hint);
    let forward = st.orientation * DVec3::X;
    let heading_sin = q.tangent.truncate().perp_dot(forward.truncate());
    let heading_cos = q.tangent.truncate().dot(forward.truncate());
    let half_width = if q.d >= 0.0 {
        q.width_left
    } else {
        q.width_right
    };

    let mut o = Writer { out, i: 0 };
    o.push(v.x / SPEED_SCALE);
    o.push(v.y / SPEED_SCALE);
    o.push(st.angular_velocity.z);
    o.push(tel.acceleration.x / ACCEL_SCALE);
    o.push(tel.acceleration.y / ACCEL_SCALE);
    for (i, w) in st.wheels.iter().enumerate() {
        o.push(w.spin * car.model.tire(i).p.radius / SPEED_SCALE);
    }
    o.push(st.drivetrain.rpm() / p.engine.limiter_rpm);
    o.push(st.drivetrain.gear as f64 / p.gearbox.ratios.len() as f64);
    o.push(input.steer);
    o.push(input.throttle);
    o.push(input.brake);
    o.push(q.d / half_width);
    o.push(heading_sin);
    o.push(heading_cos);

    for k in 1..=layout.lookahead_points {
        let smp = track.sample_at(q.s + k as f64 * layout.lookahead_spacing);
        let rel = inv * (smp.pos - st.position);
        o.push(rel.x / DISTANCE_SCALE);
        o.push(rel.y / DISTANCE_SCALE);
    }
    if layout.edges {
        for k in 1..=layout.lookahead_points {
            let smp = track.sample_at(q.s + k as f64 * layout.lookahead_spacing);
            o.push(smp.width_left / WIDTH_SCALE);
            o.push(smp.width_right / WIDTH_SCALE);
        }
    }
    if layout.tyres {
        for (w, t) in st.wheels.iter().zip(&tel.wheels) {
            o.push(w.tire.surface_temperature(&t.tread_load) / TEMPERATURE_SCALE);
            o.push(t.pressure);
        }
    }
    if layout.privileged {
        for w in &tel.wheels {
            o.push(w.slip_angle);
            o.push(w.slip_ratio);
            o.push(w.load / LOAD_SCALE);
        }
    }
    debug_assert_eq!(o.i, o.out.len());
}

struct Writer<'a> {
    out: &'a mut [f32],
    i: usize,
}

impl Writer<'_> {
    #[inline]
    fn push(&mut self, x: f64) {
        self.out[self.i] = x as f32;
        self.i += 1;
    }
}
