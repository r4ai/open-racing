//! Track-aware reference controller used to bootstrap Watkins Glen Formula training.
//!
//! It combines geometric preview, a braking envelope and tyre-slip feedback. The
//! measured rough section has its own speed cap; that cap is explicit so a later
//! policy cannot quietly learn an unsafe speed there from progress reward alone.

use open_racing_sim::{Car, Track};

#[derive(Clone, Copy, Debug)]
pub struct ReferenceConfig {
    pub max_speed: f64,
    pub lateral_accel: f64,
    pub rough_speed: f64,
}

impl Default for ReferenceConfig {
    fn default() -> Self {
        Self {
            max_speed: 40.0,
            lateral_accel: 4.0,
            rough_speed: 15.0,
        }
    }
}

impl ReferenceConfig {
    /// Action in the standard steer/throttle/brake space.
    pub fn action(self, car: &Car, track: &Track, hint: &mut usize) -> [f32; 3] {
        if car.state.time == 0.0 {
            *hint = track.nearest_index(car.state.position);
        }
        let q = track.locate(car.state.position, *hint);
        *hint = q.index;
        let speed = car.speed();
        let lookahead = (12.0 + 0.9 * speed).clamp(15.0, 55.0);
        let target = track.sample_at(q.s + lookahead);
        let rel = car.state.orientation.inverse() * (target.pos - car.state.position);
        let road_angle = (2.0 * car.model.params.wheelbase * rel.y
            / (rel.x * rel.x + rel.y * rel.y).max(25.0))
        .atan();
        let steer = (road_angle * car.model.params.steering.ratio / car.model.params.steering.lock)
            .clamp(-1.0, 1.0);

        let mut target_speed = self.max_speed;
        for distance in (0..=150).step_by(5) {
            let ahead_s = (q.s + distance as f64).rem_euclid(track.length);
            let curve = track.sample_at(ahead_s).curvature.abs();
            let corner_speed = (self.lateral_accel / curve.max(1e-5)).sqrt();
            let section_cap = if (3900.0..4600.0).contains(&ahead_s) {
                self.rough_speed
            } else {
                self.max_speed
            };
            let allowed = (corner_speed.min(section_cap).powi(2) + 8.0 * distance as f64).sqrt();
            target_speed = target_speed.min(allowed);
        }
        let error = target_speed - speed;
        let spin = (0..4)
            .filter(|&i| car.model.corners[i].driven)
            .map(|i| car.state.wheels[i].kappa)
            .fold(0.0_f64, f64::max);
        let throttle = (0.2 * error)
            .clamp(0.0, 1.0)
            .min((1.0 - 8.0 * (spin - 0.1)).clamp(0.0, 1.0));
        let brake = (-0.2 * error).clamp(0.0, 0.7);
        [steer as f32, throttle as f32, brake as f32]
    }
}
