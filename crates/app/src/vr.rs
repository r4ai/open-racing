//! VR through OpenXR (`--vr`). The headset's eye cameras sit in the cockpit at the
//! driver's eye, and head tracking moves them from there. The window keeps its own
//! camera and HUD. Without an OpenXR runtime the app renders to the window as usual.

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::render::pipelined_rendering::PipelinedRenderingPlugin;
use bevy_mod_openxr::add_xr_plugins;
use bevy_mod_openxr::reference_space::OxrReferenceSpacePlugin;
use bevy_mod_openxr::render::update_views;
use bevy_mod_xr::camera::XrCamera;
use bevy_mod_xr::session::XrTrackingRoot;

use crate::camera::{CameraMode, MainCamera, PrimaryView, body_view, driver_eye};
use crate::driving::Simulation;
use crate::input::AppRequests;
use crate::scene::{DriverEye, sky_light};

/// Replaces the renderer in `default` with the OpenXR one.
pub fn plugins(default: PluginGroupBuilder) -> PluginGroupBuilder {
    // Pipelined rendering would show each head pose a frame late.
    add_xr_plugins(default.disable::<PipelinedRenderingPlugin>())
        // Seated: the origin is where the head was when the session started.
        .set(OxrReferenceSpacePlugin {
            default_primary_ref_space: openxr::ReferenceSpaceType::LOCAL,
        })
}

/// Head pose in the tracking space that sits at the driver's eye: its position and
/// heading, so that the horizon stays level.
#[derive(Resource, Default)]
struct Seat(Transform);

pub struct VrPlugin;

impl Plugin for VrPlugin {
    fn build(&self, app: &mut App) {
        // The window shows the cockpit too, with the car visible.
        app.insert_resource(CameraMode::Cockpit)
            .init_resource::<Seat>()
            .add_observer(setup_eye)
            .add_observer(release_eye)
            .add_systems(
                PostUpdate,
                place_in_cockpit
                    .after(update_views)
                    .before(TransformSystems::Propagate),
            );
    }
}

/// Gives a new eye camera the sky light, and makes the left eye the primary view.
fn setup_eye(
    add: On<Add, XrCamera>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    eyes: Query<&XrCamera>,
    primary: Query<Entity, With<PrimaryView>>,
) {
    let mut eye = commands.entity(add.entity);
    eye.insert(sky_light(&mut images));
    if eyes.get(add.entity).is_ok_and(|e| e.0 == 0) {
        eye.insert(PrimaryView);
        for e in &primary {
            commands.entity(e).remove::<PrimaryView>();
        }
    }
}

/// Hands the primary view back to the window camera when the session ends.
fn release_eye(
    remove: On<Remove, XrCamera>,
    mut commands: Commands,
    eyes: Query<(), With<PrimaryView>>,
    main: Query<Entity, With<MainCamera>>,
) {
    if eyes.contains(remove.entity)
        && let Ok(main) = main.single()
    {
        commands.entity(main).insert(PrimaryView);
    }
}

fn place_in_cockpit(
    requests: Res<AppRequests>,
    sim: Res<Simulation>,
    driver: Option<Res<DriverEye>>,
    mut seat: ResMut<Seat>,
    eyes: Query<&Transform, (With<XrCamera>, Without<XrTrackingRoot>)>,
    mut root: Query<&mut Transform, With<XrTrackingRoot>>,
) {
    if requests.recenter_vr
        && let Some(head) = head_pose(&eyes)
    {
        seat.0 = head;
    }
    let Ok(mut root) = root.single_mut() else {
        return;
    };
    let seat_inverse = Transform::from_matrix(seat.0.to_matrix().inverse());
    *root = body_view(&sim, driver_eye(driver.as_deref())) * seat_inverse;
}

/// Between the eyes, with the head's heading only.
fn head_pose(
    eyes: &Query<&Transform, (With<XrCamera>, Without<XrTrackingRoot>)>,
) -> Option<Transform> {
    let mut eyes = eyes.iter();
    let (left, right) = (eyes.next()?, eyes.next()?);
    let (yaw, _, _) = left.rotation.to_euler(EulerRot::YXZ);
    Some(
        Transform::from_translation((left.translation + right.translation) * 0.5)
            .with_rotation(Quat::from_rotation_y(yaw)),
    )
}
