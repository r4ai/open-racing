//! Headless-friendly verification: with `OPEN_RACING_SCREENSHOT=<path>` the app saves a
//! screenshot after a few seconds of driving and exits. Optional
//! `OPEN_RACING_SCREENSHOT_CAMERA=<n>` cycles the camera `n` times first.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use crate::input::AppRequests;

const CAPTURE_AT: f32 = 8.0;
const EXIT_AT: f32 = 10.0;

#[derive(Resource)]
struct Capture {
    path: String,
    camera_cycles: u32,
    taken: bool,
}

pub struct CapturePlugin;

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        let Ok(path) = std::env::var("OPEN_RACING_SCREENSHOT") else { return };
        let camera_cycles = std::env::var("OPEN_RACING_SCREENSHOT_CAMERA").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        app.insert_resource(Capture { path, camera_cycles, taken: false })
            .add_systems(PreUpdate, capture.after(crate::input::InputSystemsSet));
    }
}

fn capture(mut commands: Commands, time: Res<Time>, mut cap: ResMut<Capture>, mut requests: ResMut<AppRequests>, mut exit: MessageWriter<AppExit>) {
    if cap.camera_cycles > 0 {
        cap.camera_cycles -= 1;
        requests.cycle_camera = true;
    }
    let t = time.elapsed_secs();
    if t > CAPTURE_AT && !cap.taken {
        cap.taken = true;
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(cap.path.clone()));
    }
    if t > EXIT_AT {
        exit.write(AppExit::Success);
    }
}
