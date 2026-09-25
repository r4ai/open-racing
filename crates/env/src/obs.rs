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
    let q = track.locate(st.position, hint);
    let forward = st.orientation * DVec3::X;
    let tangent = q.sample.tangent.truncate();
    let heading_sin = tangent.perp_dot(forward.truncate());
    let heading_cos = tangent.dot(forward.truncate());
    let half_width = if q.d >= 0.0 {
        q.sample.width_left
    } else {
        q.sample.width_right
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

    // Widths follow every lookahead coordinate in the existing observation layout.
    let edge_start = o.i + 2 * layout.lookahead_points;
    for k in 1..=layout.lookahead_points {
        let smp = track.sample_at(q.s + k as f64 * layout.lookahead_spacing);
        let rel = inv * (smp.pos - st.position);
        o.push(rel.x / DISTANCE_SCALE);
        o.push(rel.y / DISTANCE_SCALE);
        if layout.edges {
            let edge = edge_start + 2 * (k - 1);
            o.out[edge] = (smp.width_left / WIDTH_SCALE) as f32;
            o.out[edge + 1] = (smp.width_right / WIDTH_SCALE) as f32;
        }
    }
    if layout.edges {
        o.i = edge_start + 2 * layout.lookahead_points;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use open_racing_sim::{CarModel, Track};

    use super::{AppliedInput, encode};
    use crate::{Env, EnvConfig, EnvShared};

    #[test]
    fn edge_observations_keep_the_existing_layout() {
        let config = EnvConfig {
            random_start: false,
            start_speed: (0.0, 0.0),
            tyre_obs: true,
            edge_obs: true,
            ..EnvConfig::default()
        };
        let shared = EnvShared::new(
            config.clone(),
            Arc::new(Track::default_circuit()),
            Arc::new(CarModel::gt3()),
        );
        let env = Env::new(&shared, 7);
        let layout = config.obs_layout();
        let mut with_edges = vec![0.0; layout.spec().dim()];
        encode(
            &layout,
            &env.car,
            &shared.track,
            0,
            &AppliedInput::default(),
            &mut with_edges,
        );

        let mut without_layout = layout;
        without_layout.edges = false;
        let mut without_edges = vec![0.0; without_layout.spec().dim()];
        encode(
            &without_layout,
            &env.car,
            &shared.track,
            0,
            &AppliedInput::default(),
            &mut without_edges,
        );

        let edge_start = 17 + 2 * layout.lookahead_points;
        assert_eq!(&with_edges[..edge_start], &without_edges[..edge_start]);
        assert_eq!(
            &with_edges[edge_start + 2 * layout.lookahead_points..],
            &without_edges[edge_start..]
        );
        let s = shared.track.locate(env.car.state.position, 0).s;
        for k in 1..=layout.lookahead_points {
            let sample = shared
                .track
                .sample_at(s + k as f64 * layout.lookahead_spacing);
            assert_eq!(
                with_edges[edge_start + 2 * (k - 1)],
                (sample.width_left / 10.0) as f32
            );
            assert_eq!(
                with_edges[edge_start + 2 * (k - 1) + 1],
                (sample.width_right / 10.0) as f32
            );
        }
    }
}
