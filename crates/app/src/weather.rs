//! Shows the simulated weather: the sun and a physical atmosphere, volumetric clouds,
//! the shadows the clouds cast, the daylight from the sky, the exposure and the haze.
//! The weather itself is simulated with the car (see `open_racing_sim::weather`), so
//! the clouds drawn are those that shade the road and warm or cool it.
//!
//! Set on the settings screen (Esc, Tab to "weather") and saved between runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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
use bevy::render::{Render, RenderApp};
use glam::{DVec2, DVec3};
use open_racing_sim::weather::CloudLayer;
use open_racing_sim::{Sky, WeatherSettings};
use serde::{Deserialize, Serialize};

use crate::Args;
use crate::bindings;
use crate::clouds::{self, CloudMaterial, Clouds};
use crate::driving::Simulation;
use crate::graphics::GraphicsSettings;
use crate::scene::to_bevy;

const SETTINGS_FILE: &str = "weather.ron";

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
const SHADOW_SIZE: usize = 256;
const SHADOW_TILE: f32 = 12_000.0;
/// Distance of the sun's entity up its rays, m (see `update_cloud_shadow`).
const DECAL_CLEARANCE: f32 = 200_000.0;
/// Changes after which the cloud shadows or the sky light are made again.
const SHADOW_MOVE: f32 = 1500.0;
const SHADOW_DRIFT: f64 = 300.0;
const SUN_MOVE: f32 = 0.004;
const COVER_CHANGE: f64 = 0.004;
const MORPH_CHANGE: f64 = 0.004;
/// Shortest time between remakes of the sky light, s.
const SKY_REFRESH: f32 = 1.0;
const EARTH_RADIUS: f32 = 6_360_000.0;
/// Drawn clouds are denser than the simulation's shading clouds by this much: the
/// simulation averages over the holes between billows that the drawing shows.
const CLOUD_DENSITY_SCALE: f32 = 6.0;
/// How far cumulus tops lean downwind, per m of their depth.
const CUMULUS_SHEAR: f32 = 0.25;

/// Weather settings kept between runs (the seed is new each run).
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherConfig(pub WeatherSettings);

impl WeatherConfig {
    /// The saved settings, with the command line's weather and time on top.
    pub fn load(args: &Args) -> Self {
        let mut config: Self = bindings::load_config(SETTINGS_FILE);
        if let Some(sky) = args.weather.as_deref().and_then(Sky::parse) {
            config.0.sky = sky;
        }
        if let Some(hour) = args.time {
            config.0.hour = hour;
        }
        config.0.seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        config
    }

    pub fn save(&self) {
        bindings::save_config(SETTINGS_FILE, self);
    }
}

/// Parses a time of day such as "14:30" or "14.5" into hours.
pub fn parse_time(s: &str) -> Result<f64, String> {
    let hours = match s.split_once(':') {
        Some((h, m)) => {
            let h: f64 = h.trim().parse().map_err(|_| format!("bad hour in {s}"))?;
            let m: f64 = m
                .trim()
                .parse()
                .map_err(|_| format!("bad minutes in {s}"))?;
            h + m / 60.0
        }
        None => s.trim().parse().map_err(|_| format!("bad time {s}"))?,
    };
    if (0.0..24.0).contains(&hours) {
        Ok(hours)
    } else {
        Err(format!("time {s} is not within the day"))
    }
}

/// Parses a sky state name.
pub fn parse_sky(s: &str) -> Result<String, String> {
    Sky::parse(s).map(|k| k.name().into()).ok_or_else(|| {
        let names: Vec<_> = Sky::ALL.iter().map(|k| k.name()).collect();
        format!("unknown weather {s}; one of: {}", names.join(", "))
    })
}

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
    shadow_covers: [f64; 4],
    shadow_weights: [f64; 2],
    shadow_offsets: [DVec2; 4],
    shadow_valid: bool,
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

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        let image = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(sky_image(&[0u16; 0]));
        let filtered = SkyFiltered::default();
        app.insert_resource(SkyLight {
            image,
            intensity: 1.0,
        })
        .insert_resource(filtered.clone())
        .init_resource::<Made>()
        .add_plugins(clouds::CloudsPlugin)
        .add_systems(Startup, spawn)
        .add_systems(
            PostUpdate,
            (
                update_sun,
                update_sky_light,
                update_exposure_and_haze,
                update_cloud_shadow,
                update_clouds,
                freeze_sky_light,
            )
                .chain()
                .before(TransformSystems::Propagate),
        );
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .insert_resource(filtered)
                .add_systems(Render, note_sky_filtered.after(filtering_system));
        }
    }
}

fn spawn(
    mut commands: Commands,
    sim: Res<Simulation>,
    mut media: ResMut<Assets<ScatteringMedium>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<CloudMaterial>>,
) {
    // The planet under the track's lowest point.
    let base = sim.weather.road().map_or(0.0, |r| r.base_height()) as f32;
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
    if let Some(map) = sim.weather.cloud_map() {
        let clouds = clouds::spawn(&mut commands, map, &mut meshes, &mut images, &mut materials);
        commands.insert_resource(clouds);
    }
}

/// Camera components for the weather: the atmosphere and a first exposure.
pub fn camera_components(sky: &SkyLight) -> impl Bundle {
    (
        AtmosphereSettings::default(),
        Exposure { ev100: MIDDAY_EV },
        sky.component(),
    )
}

/// Light at the track now, from the simulation's weather.
pub fn daylight(sim: &Simulation) -> Daylight {
    let w = &sim.weather;
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

fn update_sun(
    sim: Res<Simulation>,
    mut suns: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
) {
    let sun = to_bevy(sim.weather.sun_direction()).normalize_or(Vec3::Y);
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
fn update_sky_light(
    mut commands: Commands,
    time: Res<Time>,
    sim: Res<Simulation>,
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
    let light = daylight(&sim);
    let cover = sim.weather.cloud_cover();
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
fn update_exposure_and_haze(
    time: Res<Time>,
    sim: Res<Simulation>,
    graphics: Res<GraphicsSettings>,
    mut made: ResMut<Made>,
    mut exposures: Query<&mut Exposure>,
    mut fogs: Query<&mut DistanceFog>,
) {
    let light = daylight(&sim);
    let target = (MIDDAY_EV + ADAPTATION * (light.global_lux.max(1.0) / MIDDAY_LUX).log2())
        .clamp(EV_RANGE.0, EV_RANGE.1);
    let ev = match made.ev {
        Some(ev) => ev + (target - ev) * (1.0 - (-time.delta_secs() / 1.5).exp()),
        None => target,
    };
    made.ev = Some(ev);
    for mut e in &mut exposures {
        e.ev100 = ev;
    }
    if !graphics.haze {
        return;
    }
    let exposure = Exposure { ev100: ev }.exposure();
    let w = &sim.weather;
    // Visibility: long in dry clear air, shorter in humid, grey weather.
    let visibility = 30_000.0
        * (1.0 - 0.6 * w.relative_humidity() as f32)
        * (1.0 - 0.3 * w.cloud_cover() as f32);
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
fn update_cloud_shadow(
    sim: Res<Simulation>,
    shadow: Option<Res<CloudShadow>>,
    mut made: ResMut<Made>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut suns: Query<&mut Transform, With<Sun>>,
) {
    let (Some(shadow), Ok(camera), Ok(mut sun_t)) = (shadow, cameras.single(), suns.single_mut())
    else {
        return;
    };
    let w = &sim.weather;
    let Some(road) = w.road() else { return };
    let layers = w.cloud_layers();
    let sun = to_bevy(w.sun_direction()).normalize_or(Vec3::Y);
    let base = road.base_height() as f32;
    let here = camera.translation().with_y(base);
    // The texture moves with the layer that casts most of the shadow; it is remade once
    // the others have drifted apart from it.
    let reference = (0..4)
        .max_by(|&a, &b| {
            let weight = |i: usize| {
                layers[i].cover * layers[i].extinction * (layers[i].top - layers[i].base)
            };
            weight(a).total_cmp(&weight(b))
        })
        .unwrap_or(0);
    let offsets = layers.map(|l| l.offset);
    let period = open_racing_sim::weather::CLOUD_MAP_PERIOD;
    let shortest = |d: DVec2, scale: f64| {
        let p = period * scale;
        d - (d / p).round() * p
    };
    let drifts: [DVec2; 4] =
        std::array::from_fn(|i| shortest(offsets[i] - made.shadow_offsets[i], layers[i].scale));
    let drift = drifts[reference];
    let apart = drifts.iter().any(|d| (*d - drift).length() > SHADOW_DRIFT);
    let covers = layers.map(|l| l.cover);
    let remake = !made.shadow_valid
        || apart
        || made.shadow_centre.distance(here) > SHADOW_MOVE
        || made.shadow_sun.distance(sun) > SUN_MOVE * 0.25
        || (0..4).any(|i| (made.shadow_covers[i] - covers[i]).abs() > COVER_CHANGE)
        || (made.shadow_weights[0] - layers[0].weights[0]).abs() > MORPH_CHANGE
        || (made.shadow_weights[1] - layers[0].weights[1]).abs() > MORPH_CHANGE;
    let rotation = sun_t.rotation;
    let drift = if remake && sun.y > 0.0 {
        made.shadow_centre = (here / 50.0).round() * 50.0;
        made.shadow_sun = sun;
        made.shadow_covers = covers;
        made.shadow_weights = layers[0].weights;
        made.shadow_offsets = offsets;
        made.shadow_valid = true;
        let data = shadow_texels(&sim, made.shadow_centre, rotation, base);
        if let Some(mut image) = images.get_mut(&shadow.0) {
            *image = shadow_image(data);
        }
        DVec2::ZERO
    } else {
        drift
    };
    let drift = DVec3::new(drift.x, drift.y, 0.0);
    // Bevy also treats the light's texture as a decal over a box of the light's size; keep
    // that box far up the sun's rays, clear of the scene. Only the position across the
    // rays places the texture.
    sun_t.translation = made.shadow_centre + to_bevy(drift) + rotation * Vec3::Z * DECAL_CLEARANCE;
    sun_t.scale = Vec3::splat(SHADOW_TILE * 0.5);
}

/// Sunlight let through by the clouds over a tile centred on `centre`, in the frame of
/// the sun at `rotation`: each texel is a sun ray, found where it meets the ground.
fn shadow_texels(sim: &Simulation, centre: Vec3, rotation: Quat, base: f32) -> Vec<u8> {
    let w = &sim.weather;
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
                    *out = (w.sun_transmittance(sim_point) * 255.0).round() as u8;
                }
            });
        }
    });
    data
}

/// Hands the weather and the graphics settings to the cloud material.
fn update_clouds(
    sim: Res<Simulation>,
    graphics: Res<GraphicsSettings>,
    clouds: Option<Res<Clouds>>,
    mut materials: ResMut<Assets<CloudMaterial>>,
    mut visibility: Query<&mut Visibility>,
) {
    let Some(clouds) = clouds else { return };
    let (steps, light_steps, detail) = graphics.clouds.march();
    if let Ok(mut v) = visibility.get_mut(clouds.entity) {
        v.set_if_neq(if steps > 0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    if steps == 0 {
        return;
    }
    let Some(mut material) = materials.get_mut(&clouds.material) else {
        return;
    };
    let light = daylight(&sim);
    let w = &sim.weather;
    let base = w.road().map_or(0.0, |r| r.base_height()) as f32;
    let layers = w.cloud_layers();
    let (_, from) = w.wind();
    // The cirrus streaks run along the upper wind, which blows from `from` veered.
    let heading = (90.0 - from + 180.0).to_radians() as f32 - 0.7;
    material.params = clouds::CloudParams {
        sun_direction: light.sun,
        steps,
        sun_illuminance: light.sun_lux,
        light_steps,
        sky_radiance: light.zenith.lerp(light.horizon, 0.3),
        detail,
        ground_radiance: light.ground,
        planet_centre: base - EARTH_RADIUS,
        horizon_radiance: light.horizon,
        temporal: if graphics.anti_aliasing == crate::graphics::AntiAliasing::Taa {
            1.0
        } else {
            0.0
        },
        // World X, Z of a direction at `heading` from the simulation's +X.
        streaks: Vec2::new(heading.cos(), -heading.sin()),
        density_scale: CLOUD_DENSITY_SCALE,
        _pad: 0.0,
        // The wind at the cumulus tops outruns that at their bases.
        shear: {
            let d = (layers[0].top - layers[0].base) as f32 * CUMULUS_SHEAR;
            Vec2::new(heading.cos(), -heading.sin()) * d
        },
        _pad2: Vec2::ZERO,
        layers: layers.map(|l: CloudLayer| clouds::GpuLayer {
            base: base + l.base as f32,
            top: base + l.top as f32,
            threshold: l.threshold as f32,
            cover: l.cover as f32,
            offset: Vec2::new(l.offset.x as f32, l.offset.y as f32),
            weights: Vec2::new(l.weights[0] as f32, l.weights[1] as f32),
            scale: l.scale as f32,
            extinction: l.extinction as f32,
            cell_threshold: l.cell_threshold as f32,
            _pad: Vec3::ZERO,
        }),
    };
}
