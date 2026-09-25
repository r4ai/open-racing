//! Headless-friendly verification: with `OPEN_RACING_SCREENSHOT=<path>` the app saves a
//! screenshot after a few seconds of driving and exits. Optional
//! `OPEN_RACING_SCREENSHOT_CAMERA=<n>` cycles the camera `n` times first.

mod report;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use report::Measurements;

use crate::input::AppRequests;
use bevy::diagnostic::DiagnosticsStore;
use bevy::render::{Render, RenderApp, RenderSystems, render_resource::PipelineCache};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const CAPTURE_AT: f32 = 8.0;
const EXIT_AT: f32 = 10.0;

/// Populated only during capture runs; values are total CPU work in one frame.
#[derive(Resource, Default)]
pub struct CloudCpuTimings {
    pub weather_ms: f64,
    pub upload_ms: f64,
    pub shadow_ms: f64,
}

#[derive(Resource, Clone, Default)]
struct CaptureReady(Arc<AtomicBool>);

#[derive(Component)]
struct CameraOffset(Vec3);

fn follow_camera_offsets(
    main: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut followers: Query<
        (&CameraOffset, &mut Transform, &mut GlobalTransform),
        Without<crate::camera::MainCamera>,
    >,
) {
    let Ok(main) = main.single() else { return };
    for (offset, mut local, mut global) in &mut followers {
        *local = main.compute_transform();
        let displacement = local.rotation * offset.0;
        local.translation += displacement;
        *global = GlobalTransform::from(*local);
    }
}

fn note_pipelines_ready(cache: Res<PipelineCache>, ready: Res<CaptureReady>) {
    ready.0.store(
        cache.waiting_pipelines().next().is_none(),
        Ordering::Relaxed,
    );
}

#[derive(Resource)]
struct Capture {
    path: String,
    camera_cycles: u32,
    duration: f32,
    warmup: f32,
    interval: f32,
    next: f32,
    index: u32,
    measurements: Measurements,
    target: Option<Handle<Image>>,
    weather_speed: f64,
    elapsed: f32,
    stamps: String,
    scenario: String,
    stage: u32,
    saved_weather: Option<open_racing_sim::Weather>,
    eyes: Vec<Handle<Image>>,
}

pub struct CapturePlugin;

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        let Ok(path) = std::env::var("OPEN_RACING_SCREENSHOT") else {
            return;
        };
        // Bevy otherwise limits an unfocused window to 60 Hz, which measures
        // event-loop sleep instead of rendering cost during an unattended run.
        app.insert_resource(bevy::winit::WinitSettings::continuous());
        let camera_cycles = std::env::var("OPEN_RACING_SCREENSHOT_CAMERA")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let weather_speed = app
            .world()
            .resource::<crate::driving::Simulation>()
            .weather
            .settings
            .time_scale;
        let ready = CaptureReady::default();
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .insert_resource(ready.clone())
                .add_systems(Render, note_pipelines_ready.in_set(RenderSystems::Cleanup));
        }
        app.insert_resource(ready);
        app.insert_resource(Capture {
            path,
            camera_cycles,
            duration: number("OPEN_RACING_CAPTURE_SECONDS", EXIT_AT),
            warmup: number("OPEN_RACING_CAPTURE_WARMUP", CAPTURE_AT),
            interval: number("OPEN_RACING_CAPTURE_INTERVAL", 0.0),
            next: number("OPEN_RACING_CAPTURE_WARMUP", CAPTURE_AT),
            index: 0,
            measurements: default(),
            target: None,
            weather_speed,
            elapsed: 0.0,
            stamps: "frame,real_seconds,weather_seconds,generation\n".into(),
            scenario: std::env::var("OPEN_RACING_CAPTURE_SCENARIO").unwrap_or_default(),
            stage: 0,
            saved_weather: None,
            eyes: Vec::new(),
        })
        .init_resource::<CloudCpuTimings>()
        .add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin)
        .add_systems(PostStartup, setup_target)
        .add_systems(Last, follow_camera_offsets)
        .add_systems(PreUpdate, capture.after(crate::input::InputSystemsSet));
    }
}

#[allow(clippy::too_many_arguments)]
fn setup_target(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut cap: ResMut<Capture>,
    cameras: Query<Entity, With<crate::camera::MainCamera>>,
    sky: Res<crate::weather::SkyLight>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if std::env::var_os("OPEN_RACING_CAPTURE_QUALITY").is_none() {
        return;
    }
    let target = images.add(target_texture(2560, 1440));
    for camera in &cameras {
        commands.entity(camera).insert((
            bevy::camera::RenderTarget::Image(target.clone().into()),
            IsDefaultUiCamera,
        ));
    }
    cap.target = Some(target);
    if cap.scenario == "transparent" {
        commands.spawn((
            CameraOffset(Vec3::new(0.0, 3.0, -8.0)),
            Mesh3d(meshes.add(Rectangle::new(5.0, 3.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgba(0.05, 0.5, 0.8, 0.3),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                cull_mode: None,
                ..default()
            })),
            Transform::default(),
            bevy::light::NotShadowCaster,
        ));
    }
    if cap.scenario == "stereo" {
        for side in [-0.032, 0.032] {
            let target = images.add(target_texture(1280, 720));
            commands.spawn((
                Camera3d::default(),
                CameraOffset(Vec3::X * side),
                Transform::default(),
                bevy::camera::RenderTarget::Image(target.clone().into()),
                crate::weather::additional_camera_components(&sky),
            ));
            cap.eyes.push(target);
        }
    }
}

fn target_texture(width: u32, height: u32) -> Image {
    use bevy::render::render_resource::{TextureFormat, TextureUsages};
    let mut image = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    image
}

fn number(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(default)
}

#[allow(clippy::too_many_arguments)]
fn capture(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut cap: ResMut<Capture>,
    mut requests: ResMut<AppRequests>,
    mut exit: MessageWriter<AppExit>,
    diagnostics: Res<DiagnosticsStore>,
    mut driver: ResMut<crate::input::DriverInput>,
    mut sim: ResMut<crate::driving::Simulation>,
    cpu: Res<CloudCpuTimings>,
    mut graphics: ResMut<crate::graphics::GraphicsSettings>,
    mut images: ResMut<Assets<Image>>,
    ready: Res<CaptureReady>,
) {
    // Physical pedals/controllers must not change a visual comparison run.
    driver.controls = open_racing_sim::Controls {
        brake: 1.0,
        ..default()
    };
    if cap.camera_cycles > 0 {
        cap.camera_cycles -= 1;
        requests.cycle_camera = true;
    }
    // Initial synchronous pipeline compilation can take longer than the whole
    // capture. Count warm-up only while frames are actually being rendered.
    cap.elapsed += if cap.elapsed < cap.warmup {
        if ready.0.load(Ordering::Relaxed) {
            time.delta_secs().min(0.1)
        } else {
            0.0
        }
    } else {
        time.delta_secs()
    };
    let t = cap.elapsed;
    let active = (t - cap.warmup).max(0.0);
    if cap.scenario == "drive" && active > 0.0 {
        driver.controls.brake = 0.0;
        driver.controls.throttle = 0.65;
    }
    if cap.scenario == "cuts" && (active / 2.0) as u32 > cap.stage {
        cap.stage = (active / 2.0) as u32;
        requests.cycle_camera = true;
    }
    if cap.scenario == "transitions" && (active / 3.0) as u32 > cap.stage {
        use crate::graphics::Level;
        cap.stage = (active / 3.0) as u32;
        match cap.stage {
            1 => graphics.clouds = Level::Low,
            2 => graphics.clouds = Level::Off,
            3 => graphics.clouds = Level::High,
            4 => graphics.clouds = Level::Medium,
            5 => graphics.clouds = Level::Ultra,
            6 => {
                cap.saved_weather = Some(sim.weather.clone());
                let mut settings = sim.weather.settings;
                settings.seed += 1;
                sim.restart_weather(settings);
            }
            7 => requests.cycle_camera = true,
            8 | 9 => {
                if let Some(mut image) = cap.target.as_ref().and_then(|h| images.get_mut(h)) {
                    let divisor = if cap.stage == 8 { 2 } else { 1 };
                    image.resize(bevy::render::render_resource::Extent3d {
                        width: 2560 / divisor,
                        height: 1440 / divisor,
                        depth_or_array_layers: 1,
                    });
                }
            }
            10 => {
                if let Some(weather) = cap.saved_weather.take() {
                    sim.weather = weather;
                }
            }
            _ => (),
        }
    }
    // Shader warm-up must not consume the beginning of a weather comparison.
    sim.weather.settings.time_scale = if t < cap.warmup {
        0.0
    } else {
        cap.weather_speed
    };
    if t > cap.warmup && t <= cap.duration {
        cap.measurements
            .record(time.delta_secs_f64() * 1000.0, &cpu, &diagnostics);
    }

    if t >= cap.next && t < cap.duration - 0.1 && (cap.index == 0 || cap.interval > 0.0) {
        let path = if cap.interval > 0.0 {
            format!("{}-{:04}.png", cap.path, cap.index)
        } else {
            cap.path.clone()
        };
        cap.index += 1;
        let stamp = format!(
            "{},{:.6},{:.6},{}\n",
            cap.index - 1,
            t - cap.warmup,
            sim.weather.weather_time(),
            sim.weather.cloud_generation()
        );
        cap.stamps.push_str(&stamp);
        cap.next = t + cap.interval;
        commands
            .spawn(
                cap.target
                    .clone()
                    .map_or_else(Screenshot::primary_window, Screenshot::image),
            )
            .observe(save_to_disk(path));
        if cap.index == 1 {
            for (i, target) in cap.eyes.iter().enumerate() {
                commands
                    .spawn(Screenshot::image(target.clone()))
                    .observe(save_to_disk(format!("{}-eye-{i}.png", cap.path)));
            }
        }
    }
    // Allow outstanding GPU readbacks/PNG writes to finish after measurement.
    if t > cap.duration + 0.5 {
        let cap = &mut *cap;
        let result = cap.measurements.write(&cap.path, &cap.stamps);
        exit.write(match result {
            Ok(()) => AppExit::Success,
            Err(error) => {
                error!("Failed to write capture report: {error}");
                AppExit::error()
            }
        });
    }
}
