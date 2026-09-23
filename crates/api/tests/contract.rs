use open_racing_api::*;

fn spec(config: EnvConfig) -> EnvSpec {
    EnvSpec::new(Track::default_circuit(), CarModel::gt3(), config)
}

/// Pure-pursuit driver working purely from the observation vector.
struct PurePursuit {
    ix: usize,
    iy: usize,
    iv: usize,
    obs_dim: usize,
}

impl PurePursuit {
    fn new(space: &BoxSpace) -> Self {
        let idx = |n: &str| space.names.iter().position(|x| x == n).unwrap();
        Self {
            ix: idx("ahead_2_x"),
            iy: idx("ahead_2_y"),
            iv: idx("vel_long"),
            obs_dim: space.dim(),
        }
    }
}

impl Policy for PurePursuit {
    fn act(&mut self, obs: &[f32], actions: &mut [f32]) {
        for (o, a) in obs.chunks(self.obs_dim).zip(actions.chunks_mut(3)) {
            let (x, y) = (o[self.ix] * 100.0, o[self.iy] * 100.0);
            let curvature = 2.0 * y / (x * x + y * y);
            // road angle = atan(L κ); steering wheel = 13 × road angle; action = wheel / lock.
            a[0] = ((2.65 * curvature).atan() * 13.0 / 4.712).clamp(-1.0, 1.0);
            let v = o[self.iv] * 50.0;
            a[1] = if v < 22.0 { 0.6 } else { 0.0 };
            a[2] = if v > 26.0 { 0.3 } else { 0.0 };
        }
    }
}

#[test]
fn buffers_match_spaces() {
    let spec = spec(EnvConfig::default());
    let mut env = spec.make_vec_env(8);
    let d = env.observation_space().dim();
    let a = env.action_space().dim();
    assert_eq!(a, 3);
    assert_eq!(env.reset(1).len(), 8 * d);
    let actions = vec![0.0; 8 * a];
    let r = env.step(&actions);
    assert_eq!(r.obs.len(), 8 * d);
    assert_eq!(r.final_obs.len(), 8 * d);
    assert_eq!(r.rewards.len(), 8);
    assert!(r.obs.iter().all(|x| x.is_finite()));
}

#[test]
fn privileged_obs_extends_space() {
    let base = spec(EnvConfig::default()).observation_space().dim();
    let priv_ = spec(EnvConfig {
        privileged_obs: true,
        ..Default::default()
    })
    .observation_space()
    .dim();
    assert_eq!(priv_, base + 12);
}

#[test]
fn manual_shift_adds_a_gear_action() {
    let spec = spec(EnvConfig {
        auto_shift: false,
        random_start: false,
        start_speed: (0.0, 0.0),
        ..Default::default()
    });
    let mut env = spec.make_vec_env(1);
    assert_eq!(
        env.action_space().names,
        ["steer", "throttle", "brake", "shift"]
    );
    env.reset(0);
    let mut run = |shifts: &[f32]| {
        for &shift in shifts {
            env.step(&[0.0, 0.0, 1.0, shift]);
        }
        env.cars().next().unwrap().state.drivetrain.gear
    };
    let once = |shift: f32| [[shift].as_slice(), &[0.0; 9]].concat();
    assert_eq!(run(&once(1.0)), 2, "one request shifts exactly one gear");
    assert_eq!(
        run(&[1.0; 50]),
        6,
        "a held request keeps shifting up to the top gear"
    );
    assert_eq!(run(&[-1.0; 50]), 1, "never shifts below first gear");
}

#[test]
fn manual_downshift_never_over_revs() {
    let spec = spec(EnvConfig {
        auto_shift: false,
        random_start: false,
        start_speed: (60.0, 60.0),
        ..Default::default()
    });
    let mut env = spec.make_vec_env(1);
    env.reset(0);
    for _ in 0..50 {
        env.step(&[0.0, 0.0, 0.0, -1.0]);
    }
    let car = env.cars().next().unwrap();
    let dt = &car.state.drivetrain;
    assert!(dt.gear > 1, "no gear below the limiter at speed");
    assert!(dt.rpm() <= car.model.params.engine.limiter_rpm * 1.02);
}

#[test]
fn reckless_driving_terminates_and_autoresets() {
    let spec = spec(EnvConfig::default());
    let mut env = spec.make_vec_env(4);
    env.reset(3);
    let actions = [1.0, 1.0, 0.0].repeat(4);
    let mut episodes = 0;
    for _ in 0..3000 {
        let r = env.step(&actions);
        assert!(r.obs.iter().all(|x| x.is_finite()));
        episodes += r.terminated.iter().filter(|&&t| t != 0).count();
    }
    assert!(
        episodes > 0,
        "full lock + full throttle should leave the track"
    );
}

#[test]
fn same_seed_same_rollout() {
    let spec = spec(EnvConfig::default());
    let run = || {
        let mut env = spec.make_vec_env(4);
        env.reset(42);
        let actions = [0.1, 0.5, 0.0].repeat(4);
        let mut sum = 0.0;
        for _ in 0..500 {
            sum += env
                .step(&actions)
                .rewards
                .iter()
                .map(|&r| r as f64)
                .sum::<f64>();
        }
        (sum, env.reset(0).to_vec())
    };
    assert_eq!(run().0, run().0);
}

#[test]
fn observation_supports_a_simple_driver() {
    let spec = spec(EnvConfig {
        random_start: false,
        start_speed: (20.0, 20.0),
        start_offset: (0.0, 0.0),
        ..Default::default()
    });
    let mut env = spec.make_vec_env(1);
    let mut policy = PurePursuit::new(env.observation_space());
    let mut obs = env.reset(0).to_vec();
    let mut actions = [0.0; 3];
    for _ in 0..(50 * 60) {
        policy.act(&obs, &mut actions);
        let r = env.step(&actions);
        assert_eq!(r.terminated[0], 0, "pure pursuit left the track");
        obs.copy_from_slice(r.obs);
    }
    let progress = env.episode_stats().next().unwrap().progress;
    assert!(progress > 1000.0, "progress {progress}");
}
