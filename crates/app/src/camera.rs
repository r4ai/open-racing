//! Chase, cockpit, trackside TV and overhead cameras.

use bevy::prelude::*;
use glam::DVec3;

use crate::driving::Simulation;
use crate::input::AppRequests;
use crate::scene::{quat_to_bevy, to_bevy};

#[derive(Component)]
struct MainCamera;

#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    #[default]
    Chase,
    Cockpit,
    Tv,
    Top,
}

impl CameraMode {
    fn next(self) -> Self {
        match self {
            Self::Chase => Self::Cockpit,
            Self::Cockpit => Self::Tv,
            Self::Tv => Self::Top,
            Self::Top => Self::Chase,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Chase => "chase",
            Self::Cockpit => "cockpit",
            Self::Tv => "tv",
            Self::Top => "top",
        }
    }
}

/// Spacing of trackside TV cameras along the track, m.
const TV_SPACING: f64 = 180.0;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraMode>()
            .add_systems(Startup, spawn_camera)
            .add_systems(PostUpdate, follow.before(TransformSystems::Propagate));
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        MainCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 60f32.to_radians(), near: 0.05, far: 8000.0, ..default() }),
        Transform::from_xyz(-10.0, 5.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn follow(
    time: Res<Time>,
    requests: Res<AppRequests>,
    sim: Res<Simulation>,
    mut mode: ResMut<CameraMode>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
) {
    if requests.cycle_camera {
        *mode = mode.next();
    }
    let Ok((mut cam, mut projection)) = cameras.single_mut() else { return };
    let (pos, rot) = sim.body_pose();
    let forward = rot * DVec3::X;
    let flat = DVec3::new(forward.x, forward.y, 0.0).normalize_or(DVec3::X);
    let fov = |deg: f32| deg.to_radians();
    let mut set_fov = |deg: f32| {
        if let Projection::Perspective(p) = &mut *projection {
            p.fov = fov(deg);
        }
    };

    match *mode {
        CameraMode::Chase => {
            set_fov(60.0);
            let desired = to_bevy(pos - flat * 6.5 + DVec3::Z * 2.0);
            let k = 1.0 - (-8.0 * time.delta_secs()).exp();
            // Snap when far away (reset, mode change).
            cam.translation = if cam.translation.distance(desired) > 30.0 { desired } else { cam.translation.lerp(desired, k) };
            cam.look_at(to_bevy(pos + flat * 4.0 + DVec3::Z * 0.6), Vec3::Y);
        }
        CameraMode::Cockpit => {
            set_fov(75.0);
            cam.translation = to_bevy(pos + rot * DVec3::new(-0.3, 0.38, 0.45));
            cam.rotation = quat_to_bevy(rot) * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        }
        CameraMode::Tv => {
            let track = &sim.track;
            let s = track.query(pos, sim.lap.hint()).s;
            let slot = ((s + 0.5 * TV_SPACING) / TV_SPACING).floor() * TV_SPACING;
            let smp = track.sample_at(slot);
            let side = if smp.curvature >= 0.0 { -1.0 } else { 1.0 }; // outside of the corner
            let offset = side * ((if side > 0.0 { smp.width_left } else { smp.width_right }) + 20.0);
            cam.translation = to_bevy(smp.pos + smp.lateral * offset + DVec3::Z * 6.0);
            cam.look_at(to_bevy(pos), Vec3::Y);
            let distance = cam.translation.distance(to_bevy(pos));
            set_fov((2.0 * (6.0 / distance.max(1.0)).atan()).to_degrees().clamp(4.0, 60.0));
        }
        CameraMode::Top => {
            set_fov(50.0);
            cam.translation = to_bevy(pos + DVec3::Z * 120.0);
            cam.look_at(to_bevy(pos), to_bevy(flat));
        }
    }
}
