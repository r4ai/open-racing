//! Chase, cockpit, first-person, bumper, trackside TV and overhead cameras.

use bevy::prelude::*;
use glam::DVec3;

use crate::debug_view::DebugSettings;
use crate::driving::Simulation;
use crate::input::AppRequests;
use crate::scene::{CarNose, CarVisualRoot, DriverEye, quat_to_bevy, to_bevy};
use open_racing_sky::{self as weather, SkyLight};

#[derive(Component)]
pub struct MainCamera;

/// The camera whose viewpoint effects such as smoke billboards face: the window camera,
/// or one eye of the headset in VR.
#[derive(Component)]
pub struct PrimaryView;

#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    #[default]
    Chase,
    /// Chase camera held at a fixed distance behind the car.
    FixedChase,
    Cockpit,
    /// From the driver's eye with the car hidden.
    FirstPerson,
    Bumper,
    Tv,
    Top,
}

impl CameraMode {
    fn next(self) -> Self {
        match self {
            Self::Chase => Self::FixedChase,
            Self::FixedChase => Self::Cockpit,
            Self::Cockpit => Self::FirstPerson,
            Self::FirstPerson => Self::Bumper,
            Self::Bumper => Self::Tv,
            Self::Tv => Self::Top,
            Self::Top => Self::Chase,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Chase => "chase",
            Self::FixedChase => "fixed chase",
            Self::Cockpit => "cockpit",
            Self::FirstPerson => "first person",
            Self::Bumper => "bumper",
            Self::Tv => "tv",
            Self::Top => "top",
        }
    }
}

/// Chase camera offsets behind and above the centre of gravity, m.
const CHASE_BACK: f64 = 6.5;
const CHASE_UP: f64 = 2.0;
/// Height of the bumper camera above the ground, m.
const BUMPER_HEIGHT: f64 = 0.45;

/// Spacing of trackside TV cameras along the track, m.
const TV_SPACING: f64 = 180.0;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraMode>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                PostUpdate,
                (follow, hide_car).before(TransformSystems::Propagate),
            );
    }
}

fn spawn_camera(mut commands: Commands, sky: Res<SkyLight>) {
    commands.spawn((
        MainCamera,
        PrimaryView,
        Camera3d::default(),
        weather::camera_components(&sky),
        Projection::Perspective(PerspectiveProjection {
            fov: 60f32.to_radians(),
            near: 0.05,
            far: 8000.0,
            ..default()
        }),
        Transform::from_xyz(-10.0, 5.0, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn follow(
    time: Res<Time>,
    requests: Res<AppRequests>,
    sim: Res<Simulation>,
    eye: Option<Res<DriverEye>>,
    nose: Option<Res<CarNose>>,
    mut mode: ResMut<CameraMode>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
) {
    if requests.cycle_camera {
        *mode = mode.next();
    }
    let Ok((mut cam, mut projection)) = cameras.single_mut() else {
        return;
    };
    let (pos, rot) = sim.body_pose();
    let forward = rot * DVec3::X;
    let flat = DVec3::new(forward.x, forward.y, 0.0).normalize_or(DVec3::X);
    let fov = |deg: f32| deg.to_radians();
    let mut set_fov = |deg: f32| {
        if let Projection::Perspective(p) = &mut *projection {
            p.fov = fov(deg);
        }
    };

    let chase = to_bevy(pos - flat * CHASE_BACK + DVec3::Z * CHASE_UP);
    let chase_target = to_bevy(pos + flat * 4.0 + DVec3::Z * 0.6);
    let mut ride = |local: DVec3| {
        let view = body_view(&sim, local);
        cam.translation = view.translation;
        cam.rotation = view.rotation;
    };

    match *mode {
        CameraMode::Chase => {
            set_fov(60.0);
            let desired = chase;
            let k = 1.0 - (-8.0 * time.delta_secs()).exp();
            // Snap when far away (reset, mode change).
            cam.translation = if cam.translation.distance(desired) > 30.0 {
                desired
            } else {
                cam.translation.lerp(desired, k)
            };
            cam.look_at(chase_target, Vec3::Y);
        }
        CameraMode::FixedChase => {
            set_fov(60.0);
            cam.translation = chase;
            cam.look_at(chase_target, Vec3::Y);
        }
        CameraMode::Cockpit | CameraMode::FirstPerson => {
            set_fov(75.0);
            ride(driver_eye(eye.as_deref()));
        }
        CameraMode::Bumper => {
            set_fov(70.0);
            let p = &sim.car.model.params;
            let nose = nose.map_or(p.wheelbase * (1.0 - p.front_weight) + 0.9, |n| n.0);
            ride(DVec3::new(nose, 0.0, BUMPER_HEIGHT - p.cg_height));
        }
        CameraMode::Tv => {
            let track = &sim.track;
            let s = track.locate(pos, sim.lap.hint()).s;
            let slot = ((s + 0.5 * TV_SPACING) / TV_SPACING).floor() * TV_SPACING;
            let smp = track.sample_at(slot);
            let side = if smp.curvature >= 0.0 { -1.0 } else { 1.0 }; // outside of the corner
            let offset = side
                * ((if side > 0.0 {
                    smp.width_left
                } else {
                    smp.width_right
                }) + 20.0);
            cam.translation = to_bevy(smp.pos + smp.lateral * offset + DVec3::Z * 6.0);
            cam.look_at(to_bevy(pos), Vec3::Y);
            let distance = cam.translation.distance(to_bevy(pos));
            set_fov(
                (2.0 * (6.0 / distance.max(1.0)).atan())
                    .to_degrees()
                    .clamp(4.0, 60.0),
            );
        }
        CameraMode::Top => {
            set_fov(50.0);
            cam.translation = to_bevy(pos + DVec3::Z * 120.0);
            cam.look_at(to_bevy(pos), to_bevy(flat));
        }
    }
}

/// The driver's eye in body coordinates, m.
pub fn driver_eye(eye: Option<&DriverEye>) -> DVec3 {
    eye.map_or(DVec3::new(-0.3, 0.38, 0.45), |e| e.0)
}

/// A view fixed to the body at `local`, looking along it.
pub fn body_view(sim: &Simulation, local: DVec3) -> Transform {
    let (pos, rot) = sim.body_pose();
    Transform::from_translation(to_bevy(pos + rot * local))
        .with_rotation(quat_to_bevy(rot) * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2))
}

/// Hides the car in views from inside or on it, where its model would block the view,
/// and while the car's debug view draws what the simulation models in its place.
fn hide_car(
    mode: Res<CameraMode>,
    debug: Res<DebugSettings>,
    mut roots: Query<&mut Visibility, With<CarVisualRoot>>,
) {
    if !mode.is_changed() && !debug.is_changed() {
        return;
    }
    let hidden = debug.car || matches!(*mode, CameraMode::FirstPerson | CameraMode::Bumper);
    let visibility = if hidden {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut v in &mut roots {
        v.set_if_neq(visibility);
    }
}
