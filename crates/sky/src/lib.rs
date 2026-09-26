//! Draws a simulated weather (see `open_racing_sim::weather`): the sun and a
//! physical atmosphere, volumetric clouds, the shadows the clouds cast, the daylight
//! from the sky, the exposure and the haze. The game shows the weather it simulates
//! with the car, so the clouds drawn are those that shade the road and warm or cool
//! it; the track editor shows the project's sky and light the same way.
//!
//! The weather comes from a resource of the host's (`WeatherSource`), how it is drawn
//! from `SkySettings`. The camera the clouds' shadows are made round is the one with
//! `SkyCamera` (see `camera_components`).

mod clouds;

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use bevy::asset::RenderAssetUsages;
use bevy::camera::Exposure;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::light_consts::lux;
use bevy::light::{Atmosphere, DirectionalLightTexture, GeneratedEnvironmentMapLight, SunDisk};
use bevy::pbr::generate::{
    GeneratorBindGroups, GeneratorPipelines, RenderEnvironmentMap, filtering_system,
};
use bevy::pbr::{AtmosphereSettings, DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, PipelineCache, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use bevy::render::{Render, RenderApp, RenderSystems};
use glam::DVec3;
use open_racing_sim::weather::Weather;

pub use clouds::Clouds;

/// A resource holding the weather to draw.
pub trait WeatherSource: Resource {
    fn weather(&self) -> &Weather;
}

/// How the weather is drawn.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct SkySettings {
    /// The clouds' ray march: steps, steps towards the sun, and detail (0 or 1). No
    /// steps draws no clouds.
    pub clouds: (u32, u32, f32),
    /// The clouds are marched at 1/divisor of the view's resolution.
    pub divisor: u32,
    /// Haze in the distance.
    pub haze: bool,
    /// Brightens (above 0) or darkens the picture from what the daylight calls for,
    /// EV.
    pub exposure: f64,
    /// How thick the haze is against what the weather gives: 0 none, 1 as it gives.
    pub haze_scale: f64,
}

impl Default for SkySettings {
    fn default() -> Self {
        Self {
            clouds: (64, 5, 1.0),
            divisor: 2,
            haze: true,
            exposure: 0.0,
            haze_scale: 1.0,
        }
    }
}

/// The camera the clouds' shadows are made round.
#[derive(Component, Default)]
pub struct SkyCamera;

/// Populated only during capture runs; values are total CPU work in one frame.
#[derive(Resource, Default)]
pub struct CloudCpuTimings {
    pub weather_ms: f64,
    pub upload_ms: f64,
    pub shadow_ms: f64,
}

/// Simulation is Z-up (ISO 8855), Bevy is Y-up.
fn to_bevy(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.z as f32, -v.y as f32)
}

/// Edge of the sky light's cube map faces, texels. The sky is smooth.
const SKY_MAP_SIZE: u32 = 32;
/// Share of the sky the environment light gives: it reaches every surface unoccluded,
/// while by the track grandstands, barriers, trees and the car itself hide about half.
const SKY_SEEN: f32 = 0.5;
/// Luminous efficacy of daylight, lm/W.
const DAYLIGHT_EFFICACY: f64 = 120.0;
/// Reflectance of the ground around the track.
const GROUND_ALBEDO: f32 = 0.18;
/// Optical depth of a clear atmosphere towards the zenith: Rayleigh, ozone and a little
/// aerosol, per colour channel.
const CLEAR_DEPTH: Vec3 = Vec3::new(0.061, 0.141, 0.267);
/// Exposure at midday under a clear sky, EV100, and how much of the change in daylight
/// the exposure makes up: some, as the eye adapts, but not all, so dull days look dull.
const MIDDAY_EV: f32 = 12.7;
const MIDDAY_LUX: f32 = 104_000.0;
const ADAPTATION: f32 = 0.8;
const EV_RANGE: (f32, f32) = (5.0, 15.0);
/// Cloud shadow texture: texels per edge, and the ground it covers, m.
// The moisture field has 256 m horizontal cells; supersampling its ground shadow
// at 47 m added CPU stalls without adding physical information.
const SHADOW_SIZE: usize = 64;
const SHADOW_TILE: f32 = 12_000.0;
/// Distance of the sun's entity up its rays, m (see `update_cloud_shadow`).
const DECAL_CLEARANCE: f32 = 200_000.0;
/// Changes after which the cloud shadows or the sky light are made again.
const SHADOW_MOVE: f32 = 1500.0;
const SUN_MOVE: f32 = 0.004;
const COVER_CHANGE: f64 = 0.004;
/// Shortest time between remakes of the sky light, s.
const SKY_REFRESH: f32 = 1.0;
const EARTH_RADIUS: f32 = 6_360_000.0;
/// The source cube map of the sky light the cameras share.
#[derive(Resource, Clone)]
pub struct SkyLight {
    pub image: Handle<Image>,
    /// Luminance the cube map's values are scaled by, cd/m².
    pub intensity: f32,
}

impl SkyLight {
    /// The component that filters the sky light into a camera's environment light.
    pub fn component(&self) -> GeneratedEnvironmentMapLight {
        GeneratedEnvironmentMapLight {
            environment_map: self.image.clone(),
            intensity: self.intensity,
            ..default()
        }
    }
}

/// Counts the render frames in which the sky light's filter passes ran.
#[derive(Resource, Clone, Default)]
struct SkyFiltered(Arc<AtomicU32>);

#[derive(Resource, Clone, Default)]
struct AtmosphereReady(Arc<AtomicBool>);

#[derive(Component)]
struct PendingAtmosphere;

fn note_atmosphere_ready(
    views: Query<&bevy::pbr::resources::GpuAtmosphere, With<Camera3d>>,
    ready: Res<AtmosphereReady>,
) {
    // Bevy 0.19's shared atmosphere buffer writer uses Query::single(). Give it
    // one bootstrap frame before enabling additional views of the same planet.
    if views.single().is_ok() {
        ready.0.store(true, Ordering::Relaxed);
    }
}

fn activate_additional_atmospheres(
    mut commands: Commands,
    ready: Res<AtmosphereReady>,
    cameras: Query<Entity, With<PendingAtmosphere>>,
) {
    if ready.0.load(Ordering::Relaxed) {
        for camera in &cameras {
            commands
                .entity(camera)
                .remove::<PendingAtmosphere>()
                .insert(AtmosphereSettings::default());
        }
    }
}

/// What the sky light and the cloud shadows were last made for.
#[derive(Resource, Default)]
struct Made {
    sky_sun: Vec3,
    sky_cover: f64,
    sky_age: f32,
    /// Filter count when the sky light was last handed to the cameras.
    sky_pending: Option<u32>,
    shadow_centre: Vec3,
    shadow_sun: Vec3,
    shadow_previous: Vec<u8>,
    shadow_current: Vec<u8>,
    shadow_rotation: Quat,
    shadow_valid: bool,
    shadow_time: f64,
    shadow_generation: u64,
    /// Current exposure, EV100, eased towards its target.
    ev: Option<f32>,
}

#[derive(Component)]
pub struct Sun;

#[derive(Resource)]
struct CloudShadow(Handle<Image>);

/// Light at the track as the renderer needs it.
#[derive(Clone, Copy, Debug)]
pub struct Daylight {
    /// Towards the sun, world (Bevy) axes.
    pub sun: Vec3,
    /// Sunlight on the cloud layer, lux per channel.
    pub sun_lux: Vec3,
    /// Radiance of the sky at the zenith and horizon, and of the ground, cd/m².
    pub zenith: Vec3,
    pub horizon: Vec3,
    pub ground: Vec3,
    /// Global illuminance on level ground, lux.
    pub global_lux: f32,
}

/// Draws the weather of the resource `S` (see `WeatherSource`).
pub struct SkyPlugin<S>(PhantomData<S>);

impl<S> Default for SkyPlugin<S> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<S: WeatherSource> Plugin for SkyPlugin<S> {
    fn build(&self, app: &mut App) {
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(sky_image(&[0u16; 0]));
        let filtered = SkyFiltered::default();
        let atmosphere_ready = AtmosphereReady::default();
        app.insert_resource(SkyLight {
            image,
            intensity: 1.0,
        })
        .insert_resource(filtered.clone())
        .insert_resource(atmosphere_ready.clone())
        .init_resource::<Made>()
        .init_resource::<SkySettings>()
        .add_plugins(clouds::CloudsPlugin)
        .add_systems(
            PostUpdate,
            (
                spawn::<S>,
                activate_additional_atmospheres,
                update_sun::<S>,
                update_sky_light::<S>,
                update_exposure_and_haze::<S>,
                update_cloud_shadow::<S>,
                update_clouds::<S>,
                freeze_sky_light,
            )
                .chain()
                .before(TransformSystems::Propagate),
        );
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .insert_resource(filtered)
                .insert_resource(atmosphere_ready)
                .add_systems(Render, note_atmosphere_ready.in_set(RenderSystems::Cleanup))
                .add_systems(Render, note_sky_filtered.after(filtering_system));
        }
    }
}

/// The planet, the sun and the clouds, once there is weather over a track to show.
fn spawn<S: WeatherSource>(
    mut commands: Commands,
    source: Option<Res<S>>,
    mut done: Local<bool>,
    mut media: ResMut<Assets<ScatteringMedium>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(source) = source else { return };
    let w = source.weather();
    let Some(road) = w.road() else { return };
    if *done {
        return;
    }
    *done = true;
    // The planet under the track's lowest point.
    let base = road.base_height() as f32;
    commands.spawn((
        Atmosphere::earth(media.add(ScatteringMedium::default())),
        Transform::from_xyz(0.0, base - EARTH_RADIUS, 0.0),
    ));
    let shadow = images.add(shadow_image(vec![255; SHADOW_SIZE * SHADOW_SIZE]));
    commands.insert_resource(CloudShadow(shadow.clone()));
    commands.spawn((
        Sun,
        DirectionalLight {
            illuminance: lux::RAW_SUNLIGHT,
            shadow_maps_enabled: true,
            ..default()
        },
        SunDisk::EARTH,
        DirectionalLightTexture {
            image: shadow,
            tiled: true,
        },
        Transform::from_xyz(0.0, 0.0, 0.0).looking_to(-Vec3::Y, Vec3::X),
    ));
    if let Some(map) = w.cloud_map() {
        let clouds = clouds::spawn(map, &mut images);
        commands.insert_resource(clouds);
    }
}

/// Camera components for the weather: the atmosphere and a first exposure.
pub fn camera_components(sky: &SkyLight) -> impl Bundle {
    (
        SkyCamera,
        AtmosphereSettings::default(),
        Exposure { ev100: MIDDAY_EV },
        sky.component(),
    )
}

/// Additional views share the main camera's planet, after its GPU buffer exists.
pub fn additional_camera_components(sky: &SkyLight) -> impl Bundle {
    (
        PendingAtmosphere,
        bevy::camera::Hdr,
        Exposure { ev100: MIDDAY_EV },
        sky.component(),
    )
}

/// Light at the track now, from the simulation's weather.
pub fn daylight(w: &Weather) -> Daylight {
    let light = w.sunlight();
    let sun = to_bevy(light.direction).normalize_or(Vec3::Y);
    let cover = w.cloud_cover() as f32;
    let humidity = w.relative_humidity() as f32;
    let elevation = sun.y;
    // Air mass (Kasten–Young), and the atmosphere's colour filter along it; humid air
    // adds grey haze.
    let degrees = elevation.clamp(-1.0, 1.0).asin().to_degrees();
    let air_mass = if degrees > -1.0 {
        1.0 / (elevation.max(0.0) + 0.50572 * (degrees + 6.07995).max(0.1).powf(-1.6364))
    } else {
        40.0
    };
    let depth = CLEAR_DEPTH + Vec3::splat(0.04 + 0.12 * humidity);
    let filter = (-depth * air_mass.min(40.0)).exp();
    let sun_lux = filter * lux::RAW_SUNLIGHT * smooth(-1.0, 1.0, degrees);
    let diffuse_lux = (light.diffuse * DAYLIGHT_EFFICACY) as f32;
    let global_lux = (light.global * DAYLIGHT_EFFICACY) as f32;
    // Clear-sky colours: deep blue overhead, paler at the horizon, which warms with the
    // low sun; cloud greys both.
    let warm = filter / filter.max_element().max(1e-6);
    let zenith = Vec3::new(0.32, 0.5, 0.95).lerp(Vec3::splat(0.78), cover);
    let horizon_clear =
        Vec3::new(0.78, 0.85, 0.95) * warm.lerp(Vec3::ONE, smooth(2.0, 25.0, degrees));
    let horizon = horizon_clear.lerp(Vec3::new(0.8, 0.81, 0.83), cover);
    // Scale so that the sky gives the diffuse illuminance: a sky of radiance L from the
    // zenith to the horizon gives about π (0.4 L_zenith + 0.6 L_horizon).
    let mean = 0.4 * zenith.dot(Vec3::splat(1.0 / 3.0)) + 0.6 * horizon.dot(Vec3::splat(1.0 / 3.0));
    let scale = diffuse_lux / (std::f32::consts::PI * mean.max(1e-3));
    let ground = Vec3::new(0.9, 0.95, 0.8) * GROUND_ALBEDO * global_lux / std::f32::consts::PI;
    Daylight {
        sun,
        sun_lux,
        zenith: zenith * scale,
        horizon: horizon * scale,
        ground,
        global_lux,
    }
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn update_sun<S: WeatherSource>(
    source: Res<S>,
    mut suns: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
) {
    let sun = to_bevy(source.weather().sun_direction()).normalize_or(Vec3::Y);
    for (mut t, mut light) in &mut suns {
        let up = if sun.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
        t.rotation = Transform::default().looking_to(-sun, up).rotation;
        // Down to nothing when the sun has set; the atmosphere dims and reddens it before.
        let degrees = sun.y.clamp(-1.0, 1.0).asin().to_degrees();
        light.illuminance = lux::RAW_SUNLIGHT * smooth(-3.0, 0.0, degrees);
    }
}

/// Makes the sky light again when the sun or the clouds have moved on, and hands it to
/// the cameras, whose environment light filters it once.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_sky_light<S: WeatherSource>(
    mut commands: Commands,
    time: Res<Time>,
    source: Res<S>,
    filtered: Res<SkyFiltered>,
    mut sky: ResMut<SkyLight>,
    mut made: ResMut<Made>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(
        Entity,
        &mut EnvironmentMapLight,
        Has<GeneratedEnvironmentMapLight>,
    )>,
) {
    made.sky_age += time.delta_secs();
    let light = daylight(source.weather());
    let cover = source.weather().cloud_cover();
    let moved = made.sky_sun.distance(light.sun) > SUN_MOVE
        || (made.sky_cover - cover).abs() > COVER_CHANGE;
    let first = made.sky_sun == Vec3::ZERO;
    if first || (moved && made.sky_age > SKY_REFRESH && made.sky_pending.is_none()) {
        let (data, intensity) = sky_texels(&light);
        if let Some(mut image) = images.get_mut(&sky.image) {
            *image = sky_image(&data);
        }
        sky.intensity = intensity * SKY_SEEN;
        made.sky_sun = light.sun;
        made.sky_cover = cover;
        made.sky_age = 0.0;
        made.sky_pending = Some(filtered.0.load(Ordering::Relaxed));
        for (camera, _, generating) in &cameras {
            if !generating {
                commands.entity(camera).insert(sky.component());
            }
        }
    }
    for (_, mut env, _) in &mut cameras {
        env.intensity = sky.intensity;
    }
}

/// Stops filtering once the new sky light has been filtered: it stays as it is until
/// the next remake.
fn freeze_sky_light(
    mut commands: Commands,
    filtered: Res<SkyFiltered>,
    mut made: ResMut<Made>,
    cameras: Query<
        Entity,
        (
            With<GeneratedEnvironmentMapLight>,
            With<EnvironmentMapLight>,
        ),
    >,
) {
    let Some(since) = made.sky_pending else {
        return;
    };
    // Two runs: the first may still have filtered the old texels.
    if filtered.0.load(Ordering::Relaxed).wrapping_sub(since) >= 2 {
        for camera in &cameras {
            commands
                .entity(camera)
                .remove::<GeneratedEnvironmentMapLight>();
        }
        made.sky_pending = None;
    }
}

/// Counts the frames in which the sky light's filter passes ran: their pipelines are
/// compiled and a map had its bind groups.
fn note_sky_filtered(
    filtered: Res<SkyFiltered>,
    pipelines: Option<Res<GeneratorPipelines>>,
    cache: Res<PipelineCache>,
    maps: Query<(), (With<GeneratorBindGroups>, With<RenderEnvironmentMap>)>,
) {
    let Some(p) = pipelines else { return };
    let ready = [
        p.copy,
        p.downsample_first,
        p.downsample_second,
        p.radiance,
        p.irradiance,
    ]
    .into_iter()
    .all(|id| cache.get_compute_pipeline(id).is_some());
    if ready && !maps.is_empty() {
        filtered.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// The sky light's cube map: sky above the horizon, brighter around the sun, the
/// ground below. Half floats, and the luminance they are scaled by.
fn sky_texels(light: &Daylight) -> (Vec<u16>, f32) {
    let n = SKY_MAP_SIZE as usize;
    let mut radiance = Vec::with_capacity(6 * n * n);
    for face in 0..6 {
        for row in 0..n {
            for col in 0..n {
                let [u, v] = [col, row].map(|i| (i as f32 + 0.5) / n as f32 * 2.0 - 1.0);
                // Cube map faces +X, −X, +Y, −Y, +Z, −Z.
                let dir = match face {
                    0 => Vec3::new(1.0, -v, -u),
                    1 => Vec3::new(-1.0, -v, u),
                    2 => Vec3::new(u, 1.0, v),
                    3 => Vec3::new(u, -1.0, -v),
                    4 => Vec3::new(u, -v, 1.0),
                    _ => Vec3::new(-u, -v, -1.0),
                }
                .normalize();
                let up = dir.y;
                let c = if up >= 0.0 {
                    let sky = light.horizon.lerp(light.zenith, up.sqrt());
                    // Circumsolar glow, weaker under cloud.
                    let near_sun = dir.dot(light.sun).max(0.0).powi(8);
                    let glow = light.sun_lux / lux::RAW_SUNLIGHT;
                    sky + light.horizon * glow * (2.0 * near_sun)
                } else {
                    light.ground.lerp(light.horizon, (1.0 + 8.0 * up).max(0.0))
                };
                radiance.push(c);
            }
        }
    }
    let scale = radiance
        .iter()
        .map(|c| c.max_element())
        .fold(1e-3, f32::max);
    let mut data = Vec::with_capacity(radiance.len() * 4);
    for c in radiance {
        let c = c / scale;
        data.extend([c.x, c.y, c.z, 1.0].map(f16_bits));
    }
    (data, scale)
}

fn sky_image(data: &[u16]) -> Image {
    let n = SKY_MAP_SIZE as usize;
    let bytes: Vec<u8> = if data.is_empty() {
        [0x3c00u16, 0x3c00, 0x3c00, 0x3c00]
            .repeat(6 * n * n)
            .iter()
            .flat_map(|h| h.to_le_bytes())
            .collect()
    } else {
        data.iter().flat_map(|h| h.to_le_bytes()).collect()
    };
    let mut image = Image::new(
        Extent3d {
            width: SKY_MAP_SIZE,
            height: SKY_MAP_SIZE,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        bytes,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image
}

/// IEEE half-precision bits of a small non-negative float.
fn f16_bits(x: f32) -> u16 {
    let bits = x.max(0.0).to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mantissa = (bits >> 13) & 0x3ff;
    if x <= 0.0 || exp <= 0 {
        0
    } else if exp >= 31 {
        0x7bff
    } else {
        ((exp as u32) << 10 | mantissa) as u16
    }
}

/// Eases the exposure towards what the daylight calls for, and sets the haze.
fn update_exposure_and_haze<S: WeatherSource>(
    time: Res<Time>,
    source: Res<S>,
    settings: Res<SkySettings>,
    mut made: ResMut<Made>,
    mut exposures: Query<&mut Exposure>,
    mut fogs: Query<&mut DistanceFog>,
) {
    let light = daylight(source.weather());
    let target = (MIDDAY_EV + ADAPTATION * (light.global_lux.max(1.0) / MIDDAY_LUX).log2())
        .clamp(EV_RANGE.0, EV_RANGE.1);
    let ev = match made.ev {
        Some(ev) => ev + (target - ev) * (1.0 - (-time.delta_secs() / 1.5).exp()),
        None => target,
    };
    made.ev = Some(ev);
    // The track's own exposure brightens or darkens what the daylight calls for.
    for mut e in &mut exposures {
        e.ev100 = ev - settings.exposure as f32;
    }
    if !settings.haze {
        return;
    }
    let exposure = Exposure { ev100: ev }.exposure();
    let w = source.weather();
    // Visibility: long in dry clear air, shorter in humid, grey weather.
    let visibility = 30_000.0
        * (1.0 - 0.6 * w.relative_humidity() as f32)
        * (1.0 - 0.3 * w.cloud_cover() as f32)
        / (settings.haze_scale as f32).max(0.02);
    let haze = light.horizon * exposure;
    let glare = light.sun_lux * exposure * 0.02 * (1.0 - w.cloud_cover() as f32);
    for mut fog in &mut fogs {
        fog.color = Color::linear_rgb(haze.x, haze.y, haze.z);
        fog.directional_light_color = Color::linear_rgb(glare.x, glare.y, glare.z);
        fog.directional_light_exponent = 20.0;
        fog.falloff = FogFalloff::from_visibility(visibility);
    }
}

fn shadow_image(data: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width: SHADOW_SIZE as u32,
            height: SHADOW_SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
}

/// The clouds' shadows, as a texture the sun projects along its rays: remade around the
/// camera when the sun, the cover or the clouds' shapes have changed, and otherwise
/// moved with the wind.
fn update_cloud_shadow<S: WeatherSource>(
    source: Res<S>,
    shadow: Option<Res<CloudShadow>>,
    mut made: ResMut<Made>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<&GlobalTransform, With<SkyCamera>>,
    mut suns: Query<&mut Transform, With<Sun>>,
    mut timings: Option<ResMut<CloudCpuTimings>>,
) {
    if let Some(t) = &mut timings {
        t.shadow_ms = 0.0;
    }
    let started = timings.as_ref().map(|_| std::time::Instant::now());
    let (Some(shadow), Ok(camera), Ok(mut sun_t)) = (shadow, cameras.single(), suns.single_mut())
    else {
        return;
    };
    let w = source.weather();
    let Some(road) = w.road() else { return };
    let Some(snapshot) = w.cloud_snapshot() else {
        return;
    };
    let sun = to_bevy(w.sun_direction()).normalize_or(Vec3::Y);
    let base = road.base_height() as f32;
    let here = camera.translation().with_y(base);
    let rotation = sun_t.rotation;
    let reframe = !made.shadow_valid
        || made.shadow_generation != snapshot.generation
        || snapshot.current.time < made.shadow_time
        || made.shadow_centre.distance(here) > SHADOW_MOVE
        || made.shadow_sun.distance(sun) > SUN_MOVE * 0.25;
    let new_tick = snapshot.current.time != made.shadow_time;
    if reframe || new_tick {
        if reframe {
            made.shadow_centre = (here / 50.0).round() * 50.0;
            made.shadow_sun = sun;
            made.shadow_rotation = rotation;
        }
        let sampler = w.cloud_shadow_sampler();
        // Both maps use the same projection. Reuse the previous tick's current
        // map only when there was no skip, rewind, or projection change.
        made.shadow_previous = if !reframe && made.shadow_time == snapshot.previous.time {
            std::mem::take(&mut made.shadow_current)
        } else {
            shadow_texels(
                sampler.with_snapshot(open_racing_sim::weather::CloudSnapshot {
                    previous: snapshot.previous,
                    current: snapshot.previous,
                    time: snapshot.previous.time,
                    generation: snapshot.generation,
                }),
                made.shadow_centre,
                made.shadow_rotation,
                base,
            )
        };
        made.shadow_current = shadow_texels(
            sampler.with_snapshot(open_racing_sim::weather::CloudSnapshot {
                previous: snapshot.current,
                current: snapshot.current,
                time: snapshot.current.time,
                generation: snapshot.generation,
            }),
            made.shadow_centre,
            made.shadow_rotation,
            base,
        );
        made.shadow_time = snapshot.current.time;
        made.shadow_generation = snapshot.generation;
        made.shadow_valid = true;
    }
    // Match the cloud interpolation clock. Backtrace each shadow map with the
    // dominant layer wind; secondary layers can drift by at most one grid tick.
    let layers = w.cloud_layers();
    let reference = (0..4)
        .max_by(|&a, &b| {
            let weight = |i: usize| {
                layers[i].cover * layers[i].extinction * (layers[i].top - layers[i].base)
            };
            weight(a).total_cmp(&weight(b))
        })
        .unwrap_or(0);
    let velocity = to_bevy(w.cloud_winds()[reference].extend(0.0));
    let flow = Vec2::new(
        -(made.shadow_rotation * Vec3::X).dot(velocity),
        (made.shadow_rotation * Vec3::Y).dot(velocity),
    ) / SHADOW_TILE;
    let ages = Vec2::new(
        (snapshot.time - snapshot.previous.time) as f32,
        (snapshot.time - snapshot.current.time) as f32,
    );
    let data: Vec<u8> = (0..SHADOW_SIZE * SHADOW_SIZE)
        .map(|i| {
            let uv = (Vec2::new((i % SHADOW_SIZE) as f32, (i / SHADOW_SIZE) as f32) + 0.5)
                / SHADOW_SIZE as f32;
            let a = shadow_sample(&made.shadow_previous, uv - flow * ages.x);
            let b = shadow_sample(&made.shadow_current, uv - flow * ages.y);
            (a + (b - a) * snapshot.blend()).round() as u8
        })
        .collect();
    if let Some(mut image) = images.get_mut(&shadow.0) {
        image.data = Some(data);
    }
    // Keep the light decal box clear of geometry, centred on this projection.
    sun_t.translation = made.shadow_centre + rotation * Vec3::Z * DECAL_CLEARANCE;
    sun_t.scale = Vec3::splat(SHADOW_TILE * 0.5);
    if let (Some(t), Some(started)) = (&mut timings, started) {
        t.shadow_ms = started.elapsed().as_secs_f64() * 1000.0;
    }
}

fn shadow_sample(data: &[u8], uv: Vec2) -> f32 {
    let p =
        (uv * SHADOW_SIZE as f32 - 0.5).clamp(Vec2::ZERO, Vec2::splat((SHADOW_SIZE - 1) as f32));
    let at = p.as_uvec2();
    let f = p - p.floor();
    let row = |y: usize| {
        let a = data[y * SHADOW_SIZE + at.x as usize] as f32;
        let b = data[y * SHADOW_SIZE + (at.x as usize + 1).min(SHADOW_SIZE - 1)] as f32;
        a + (b - a) * f.x
    };
    let a = row(at.y as usize);
    a + (row((at.y as usize + 1).min(SHADOW_SIZE - 1)) - a) * f.y
}

/// Sunlight let through by the clouds over a tile centred on `centre`, in the frame of
/// the sun at `rotation`: each texel is a sun ray, found where it meets the ground.
fn shadow_texels(
    sampler: open_racing_sim::weather::CloudShadowSampler<'_>,
    centre: Vec3,
    rotation: Quat,
    base: f32,
) -> Vec<u8> {
    let n = SHADOW_SIZE;
    let back = rotation * Vec3::Z;
    let (right, up) = (rotation * Vec3::X, rotation * Vec3::Y);
    let half = SHADOW_TILE * 0.5;
    let mut data = vec![255u8; n * n];
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(8);
    let rows = n.div_ceil(threads);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in data.chunks_mut(rows * n).enumerate() {
            scope.spawn(move || {
                for (k, out) in chunk.iter_mut().enumerate() {
                    let i = chunk_index * rows * n + k;
                    let (col, row) = (i % n, i / n);
                    // The light texture's u runs against the sun's x axis.
                    let (u, v) = ((col as f32 + 0.5) / n as f32, (row as f32 + 0.5) / n as f32);
                    let (x, y) = (1.0 - 2.0 * u, 2.0 * v - 1.0);
                    let p = centre + (right * x + up * y) * half;
                    let t = (base - p.y) / back.y.max(1e-3);
                    let ground = p + back * t;
                    let sim_point = DVec3::new(ground.x as f64, -ground.z as f64, base as f64);
                    *out = (sampler.transmittance(sim_point) * 255.0).round() as u8;
                }
            });
        }
    });
    data
}

/// Hands the weather and graphics settings to the render snapshot.
fn update_clouds<S: WeatherSource>(
    source: Res<S>,
    settings: Res<SkySettings>,
    clouds: Option<ResMut<Clouds>>,
    mut images: ResMut<Assets<Image>>,
    mut timings: Option<ResMut<CloudCpuTimings>>,
) {
    if let Some(t) = &mut timings {
        t.upload_ms = 0.0;
    }
    let Some(mut clouds) = clouds else { return };
    let (steps, light_steps, detail) = settings.clouds;
    clouds.params.steps = steps;
    if steps == 0 {
        return;
    }
    let w = source.weather();
    let Some(snapshot) = w.cloud_snapshot() else {
        return;
    };
    let stamp = (snapshot.generation, snapshot.current.time);
    if clouds.uploaded != Some(stamp) {
        let started = timings.as_ref().map(|_| std::time::Instant::now());
        let new_generation = clouds.uploaded.is_none_or(|s| s.0 != snapshot.generation)
            || clouds.uploaded.is_some_and(|s| s.1 > snapshot.current.time);
        if new_generation && let Some(map) = w.cloud_map() {
            images
                .insert(clouds.cloud_map.id(), clouds::cloud_map_image(map))
                .unwrap();
        }
        images
            .insert(clouds.field.id(), clouds::field_image(Some(snapshot)))
            .unwrap();
        clouds.uploaded = Some(stamp);
        clouds.params.occupied_bands = clouds::occupied_bands(snapshot);
        if let (Some(t), Some(started)) = (&mut timings, started) {
            t.upload_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
    }
    let light = daylight(source.weather());
    let base = w.road().map_or(0.0, |r| r.base_height()) as f32;
    let layers = w.cloud_layers();
    let upper = w.cloud_winds()[3].normalize_or(glam::DVec2::X);
    let delay = w.weather_time() - snapshot.time;
    let shape_offset = layers[0].offset - w.cloud_winds()[0] * delay;
    let cirrus_offset = layers[3].offset - w.cloud_winds()[3] * delay;
    clouds.time = snapshot.time;
    clouds.generation = snapshot.generation;
    clouds.divisor = settings.divisor;
    clouds.params = clouds::CloudParams {
        sun_direction: light.sun,
        steps,
        sun_illuminance: light.sun_lux,
        light_steps,
        sky_radiance: light.zenith.lerp(light.horizon, 0.3),
        detail,
        ground_radiance: light.ground,
        base_height: base,
        horizon_radiance: light.horizon,
        blend: snapshot.blend(),
        // Independently wrap each noise field in f64, not the shared displacement.
        shape_offset: Vec2::from_array(
            shape_offset
                .rem_euclid(glam::DVec2::splat(2600.0))
                .as_vec2()
                .to_array(),
        ),
        detail_offset: Vec2::from_array(
            shape_offset
                .rem_euclid(glam::DVec2::splat(320.0))
                .as_vec2()
                .to_array(),
        ),
        frame_ages: Vec2::new(
            (snapshot.time - snapshot.previous.time) as f32,
            (snapshot.time - snapshot.current.time) as f32,
        ),
        streaks: Vec2::new(upper.x as f32, -upper.y as f32),
        cirrus_phase: Vec2::new(
            (cirrus_offset.dot(upper) / 7000.0).rem_euclid(1.0) as f32,
            (-cirrus_offset.dot(upper.perp()) / 1200.0).rem_euclid(1.0) as f32,
        ),
        noise_mean: clouds.params.noise_mean,
        upper_wind: Vec2::from_array(w.cloud_winds()[3].as_vec2().to_array()),
        occupied_bands: clouds.params.occupied_bands,
        march_base: (layers[0].base.min(layers[1].base) as f32 - 250.0).max(0.0),
        winds: std::array::from_fn(|z| {
            Vec4::new(
                snapshot.previous.winds[z].x as f32,
                snapshot.previous.winds[z].y as f32,
                snapshot.current.winds[z].x as f32,
                snapshot.current.winds[z].y as f32,
            )
        }),
        cirrus: clouds::GpuCirrus {
            altitude: 0.5 * (layers[3].base as f32 + layers[3].top as f32),
            threshold: layers[3].threshold as f32,
            cover: layers[3].cover as f32,
            scale: layers[3].scale as f32,
            offset: Vec2::from_array(
                cirrus_offset
                    .rem_euclid(glam::DVec2::splat(
                        open_racing_sim::weather::CLOUD_MAP_PERIOD * layers[3].scale,
                    ))
                    .as_vec2()
                    .to_array(),
            ),
            weights: Vec2::new(layers[3].weights[0] as f32, layers[3].weights[1] as f32),
        },
    };
}
