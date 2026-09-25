//! Debug views: what the simulation computes, drawn over the scene in false colour with
//! the scene's own colours set aside. Chosen on the settings screen (Esc, Tab to
//! "debug") or with F1–F4, and kept between runs.
//! - Road: the grip, rubber, dirt or surface temperature of the road, as an overlay.
//! - Environment: the sun, the cloud shadows and the wind around the car.
//! - Aero: each aero element's downforce and drag at its centre of pressure, the ride
//!   heights, the air flowing at the car and the temperature of the air around it.
//! - Car: the car's model hidden and what the simulation models drawn in its place: the
//!   body, struts, tyres, brakes, engine and gearbox coloured by temperature, and the
//!   forces at the contact patches.
//!
//! A panel at the bottom right gives the numbers and the colour scales. False colours run from
//! blue (cold, little) through green (working temperature) to red (hot, much); grip runs
//! from red (poor) through yellow to green (full).

use std::fmt::Write;

use bevy::asset::RenderAssetUsages;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use glam::{DQuat, DVec3};
use serde::{Deserialize, Serialize};

use open_racing_sim::{EngineHeat, EnginePosition, FL, MAX_AERO_TELEMETRY, RL, Surface, Track};

use crate::bindings;
use crate::driving::{self, Simulation};
use crate::effects::SkidMarkMesh;
use crate::scene::{quat_to_bevy, to_bevy};

const SETTINGS_FILE: &str = "debug.ron";

/// Road overlay: spacing of its rows along the track and at most that of its columns
/// across the road, m, and its height over the road, m.
const OVERLAY_ROW: f64 = 2.0;
const OVERLAY_COLUMN: f64 = 0.5;
const OVERLAY_LIFT: f64 = 0.03;
/// Seconds between refreshes of the road overlay's colours and of the cloud shadows.
const REFRESH: f32 = 0.25;
/// Grip shown from red at this much of the tyre's nominal μ to green at all of it.
const GRIP_SCALE: f64 = 0.85;
/// Cover on the road shown fully coloured.
const DIRT_SCALE: f64 = 0.3;
/// Smallest span of the road temperature scale, K.
const ROAD_SPAN: f64 = 4.0;

/// Cloud shadows: samples on each side of the car and their spacing, m.
const SHADOW_SAMPLES: i32 = 12;
const SHADOW_SPACING: f64 = 5.0;
/// Wind arrows: their spacing, m, and length per m/s of wind, m.
const WIND_SPACING: f64 = 6.0;
const WIND_SCALE: f64 = 0.6;
/// Length of the ray towards the sun, m.
const SUN_RAY: f64 = 12.0;

/// Arrow length per newton of aerodynamic or tyre force, m.
const AERO_SCALE: f64 = 1.0 / 2000.0;
const TYRE_SCALE: f64 = 1.0 / 5000.0;
/// Arrow length per m/s of airspeed and of the car's velocity, and per m/s² of its
/// acceleration, m.
const AIRSPEED_SCALE: f64 = 0.05;
const VELOCITY_SCALE: f64 = 0.1;
const ACCELERATION_SCALE: f64 = 0.1;
/// Ride height shown from blue this far below its static value to red this far above, m.
const RIDE_SPAN: f64 = 0.03;
/// Air temperatures shown from blue this far below the ambient to red this far above, K.
const AIR_BELOW: f64 = 5.0;
const AIR_ABOVE: f64 = 25.0;
/// Tyre temperatures shown from blue this far below the grip curve's peak to red this far
/// above, K.
const TYRE_BELOW: f64 = 50.0;
const TYRE_ABOVE: f64 = 40.0;
/// The engine's temperatures shown green at their working values and red this far above, K.
const ENGINE_ABOVE: f64 = 50.0;

/// What the road overlay shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoadView {
    #[default]
    Off,
    Grip,
    Rubber,
    Dirt,
    Temperature,
}

impl RoadView {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Grip,
        Self::Rubber,
        Self::Dirt,
        Self::Temperature,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Grip => "grip",
            Self::Rubber => "rubber",
            Self::Dirt => "dirt",
            Self::Temperature => "temperature",
        }
    }

    /// The view `step` places on in `ALL`, wrapping round.
    pub fn step(self, step: isize) -> Self {
        let n = Self::ALL.len() as isize;
        let i = Self::ALL.iter().position(|&v| v == self).unwrap_or(0) as isize;
        Self::ALL[(i + step).rem_euclid(n) as usize]
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugSettings {
    pub road: RoadView,
    pub environment: bool,
    pub aero: bool,
    /// Also hides the car's model.
    pub car: bool,
}

impl DebugSettings {
    pub fn load() -> Self {
        bindings::load_config(SETTINGS_FILE)
    }

    pub fn save(&self) {
        bindings::save_config(SETTINGS_FILE, self);
    }
}

/// Gizmos drawn over everything, so the car's parts show through its body and the scene.
#[derive(Default, Reflect, GizmoConfigGroup)]
struct OnTop;

#[derive(Component)]
struct DebugPanel;

/// The road overlay, and the track coordinates of each of its vertices.
#[derive(Component)]
struct RoadOverlay {
    mesh: Handle<Mesh>,
    coords: Vec<(f64, f64)>,
}

/// Cloud shadows sampled round the car: points in the world and the share of direct
/// sunlight reaching them.
#[derive(Default)]
struct Shadows(Vec<(DVec3, f64)>);

pub struct DebugViewPlugin;

impl Plugin for DebugViewPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DebugSettings::load())
            .init_gizmo_group::<OnTop>()
            .add_systems(Startup, (spawn_panel, configure_gizmos))
            .add_systems(
                Update,
                (
                    hotkeys,
                    (road_overlay, hide_marks, environment, aero, car, panel)
                        .after(driving::step_simulation),
                ),
            );
    }
}

fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<OnTop>();
    config.depth_bias = -1.0;
    config.line.width = 2.5;
}

fn spawn_panel(mut commands: Commands) {
    commands.spawn((
        DebugPanel,
        Text::new(""),
        TextFont::from_font_size(12.0),
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(12.0),
            bottom: Val::Px(10.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        Visibility::Hidden,
    ));
}

/// F1 steps through the road views; F2, F3 and F4 toggle the environment, aero and car.
fn hotkeys(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<DebugSettings>) {
    let mut s = *settings;
    if keys.just_pressed(KeyCode::F1) {
        s.road = s.road.step(1);
    }
    if keys.just_pressed(KeyCode::F2) {
        s.environment = !s.environment;
    }
    if keys.just_pressed(KeyCode::F3) {
        s.aero = !s.aero;
    }
    if keys.just_pressed(KeyCode::F4) {
        s.car = !s.car;
    }
    if s != *settings {
        *settings = s;
        settings.save();
    }
}

/// False colour of `t` in 0..1: blue, cyan, green, yellow, red.
pub fn heat(t: f64) -> Color {
    const STOPS: [[f32; 3]; 5] = [
        [0.10, 0.25, 1.00],
        [0.00, 0.85, 0.95],
        [0.10, 0.90, 0.20],
        [1.00, 0.90, 0.10],
        [1.00, 0.10, 0.05],
    ];
    let x = t.clamp(0.0, 1.0) as f32 * 4.0;
    let i = (x as usize).min(3);
    let u = x - i as f32;
    let [a, b] = [STOPS[i], STOPS[i + 1]];
    Color::srgb(
        a[0] + (b[0] - a[0]) * u,
        a[1] + (b[1] - a[1]) * u,
        a[2] + (b[2] - a[2]) * u,
    )
}

/// Colour of grip `t` in 0..1: red, yellow, green.
fn quality(t: f64) -> Color {
    let t = t.clamp(0.0, 1.0) as f32;
    if t < 0.5 {
        Color::srgb(1.0, 0.1 + 1.6 * t, 0.05)
    } else {
        Color::srgb(1.0 - 1.8 * (t - 0.5), 0.9, 0.05 + 0.3 * (t - 0.5))
    }
}

/// `value` on a scale that is 0 at `lo`, ½ at `mid` and 1 at `hi`.
fn centred(value: f64, lo: f64, mid: f64, hi: f64) -> f64 {
    if value < mid {
        0.5 * (value - lo) / (mid - lo).max(1e-6)
    } else {
        0.5 + 0.5 * (value - mid) / (hi - mid).max(1e-6)
    }
    .clamp(0.0, 1.0)
}

/// Colour of an air temperature against the ambient.
fn air_color(t: f64, ambient: f64) -> Color {
    heat(centred(
        t,
        ambient - AIR_BELOW,
        ambient,
        ambient + AIR_ABOVE,
    ))
}

// ---- Road ------------------------------------------------------------------------------

/// A grid over the road following its surface, with the track coordinates of its
/// vertices. Rows run along the track, closing the loop; columns across the road.
fn overlay_mesh(track: &Track) -> (Mesh, Vec<(f64, f64)>) {
    let rows = ((track.length / OVERLAY_ROW).round() as usize).max(3);
    let widest = track
        .samples
        .iter()
        .map(|s| s.width_left + s.width_right)
        .fold(0.0, f64::max);
    let cols = ((widest / OVERLAY_COLUMN).ceil() as usize).max(1);
    let (mut positions, mut normals, mut coords) = (Vec::new(), Vec::new(), Vec::new());
    for r in 0..rows {
        let s = r as f64 * track.length / rows as f64;
        let smp = track.sample_at(s);
        for c in 0..=cols {
            let d = -smp.width_right + (smp.width_left + smp.width_right) * c as f64 / cols as f64;
            let (p, _, n) = track.pose_at(s, d);
            positions.push(to_bevy(p + n * OVERLAY_LIFT).to_array());
            normals.push(to_bevy(n).to_array());
            coords.push((s, d));
        }
    }
    let width = cols as u32 + 1;
    let mut indices = Vec::with_capacity(rows * cols * 6);
    for r in 0..rows as u32 {
        let next = (r + 1) % rows as u32;
        for c in 0..cols as u32 {
            let (a, b) = (r * width + c, next * width + c);
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    let colors = vec![[0.0f32; 4]; positions.len()];
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices));
    (mesh, coords)
}

/// Range of the road temperature scale, °C.
fn road_range(sim: &Simulation) -> (f64, f64) {
    let (lo, hi) = sim.weather.road().map_or_else(
        || {
            let t = sim.weather.air_temperature();
            (t, t)
        },
        |r| {
            let (_, lo, hi) = r.stats();
            (lo, hi)
        },
    );
    let (mid, half) = (0.5 * (lo + hi), (0.5 * ROAD_SPAN).max(0.5 * (hi - lo)));
    (mid - half, mid + half)
}

/// Colour of the road at (s, d) in `view`.
fn road_color(view: RoadView, sim: &Simulation, range: (f64, f64), s: f64, d: f64) -> Color {
    let e = &sim.evolution;
    match view {
        RoadView::Off => Color::BLACK,
        RoadView::Grip => {
            let g = e.grip_at(Surface::Asphalt, s, d);
            quality((g - GRIP_SCALE) / (1.0 - GRIP_SCALE))
        }
        RoadView::Rubber => heat(e.rubber_at(s, d)),
        RoadView::Dirt => {
            let cover = e.cover_at(s, d);
            let total: f64 = cover.iter().sum();
            let t = (total / DIRT_SCALE).sqrt().min(1.0) as f32;
            // Clean asphalt dark grey; covered, the colour of the dirt, brightened.
            let [r, g, b] = crate::tyre_dirt::coat_color(&cover).map(|c| (c * 2.2).min(1.0));
            let clean = 0.12;
            Color::srgb(
                clean + (r - clean) * t,
                clean + (g - clean) * t,
                clean + (b - clean) * t,
            )
        }
        RoadView::Temperature => {
            let t = sim.weather.road_temperature(s, d);
            heat((t - range.0) / (range.1 - range.0))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn road_overlay(
    mut commands: Commands,
    settings: Res<DebugSettings>,
    sim: Res<Simulation>,
    time: Res<Time>,
    mut overlay: Query<(&RoadOverlay, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut since: Local<f32>,
) {
    let view = settings.road;
    let Ok((overlay, mut visibility)) = overlay.single_mut() else {
        if view != RoadView::Off {
            // Built on first use: it follows the road's surface, which takes a while
            // on tracks with a 3D model.
            let (mesh, coords) = overlay_mesh(&sim.track);
            let mesh = meshes.add(mesh);
            commands.spawn((
                RoadOverlay {
                    mesh: mesh.clone(),
                    coords,
                },
                Mesh3d(mesh),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    unlit: true,
                    fog_enabled: false,
                    cull_mode: None,
                    depth_bias: 50.0,
                    ..default()
                })),
                Transform::default(),
                NotShadowCaster,
                NotShadowReceiver,
            ));
            *since = REFRESH;
        }
        return;
    };
    visibility.set_if_neq(if view == RoadView::Off {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    });
    *since += time.delta_secs();
    if view == RoadView::Off || (*since < REFRESH && !settings.is_changed()) {
        return;
    }
    *since = 0.0;
    let range = road_range(&sim);
    let colors: Vec<[f32; 4]> = overlay
        .coords
        .iter()
        .map(|&(s, d)| {
            road_color(view, &sim, range, s, d)
                .to_linear()
                .to_f32_array()
        })
        .collect();
    if let Some(mut mesh) = meshes.get_mut(&overlay.mesh) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    }
}

/// Hides the skid marks while the road overlay shows, as they would draw over it.
fn hide_marks(settings: Res<DebugSettings>, mut marks: Query<&mut Visibility, With<SkidMarkMesh>>) {
    if !settings.is_changed() {
        return;
    }
    let visibility = if settings.road == RoadView::Off {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut v in &mut marks {
        v.set_if_neq(visibility);
    }
}

// ---- Environment -----------------------------------------------------------------------

/// Horizontal unit vector the 10 m wind blows towards, from the compass direction it
/// blows from (0 = from north, +Y).
fn wind_towards(from: f64) -> DVec3 {
    let a = from.to_radians();
    -DVec3::new(a.sin(), a.cos(), 0.0)
}

fn environment(
    settings: Res<DebugSettings>,
    sim: Res<Simulation>,
    time: Res<Time>,
    mut gizmos: Gizmos,
    mut shadows: Local<Shadows>,
    mut since: Local<f32>,
) {
    if !settings.environment {
        shadows.0.clear();
        return;
    }
    let w = &sim.weather;
    let (pos, _) = sim.body_pose();
    let ground = pos - DVec3::Z * sim.car.model.params.cg_height;

    // Cloud shadows on the ground round the car: sunlit yellow, shaded dark blue.
    *since += time.delta_secs();
    if shadows.0.is_empty() || *since >= REFRESH {
        *since = 0.0;
        let sampler = w.cloud_shadow_sampler();
        let n = SHADOW_SAMPLES;
        shadows.0 = (-n..=n)
            .flat_map(|i| (-n..=n).map(move |j| (i, j)))
            .map(|(i, j)| {
                let p = ground + DVec3::new(i as f64, j as f64, 0.0) * SHADOW_SPACING;
                (p, sampler.transmittance(p))
            })
            .collect();
    }
    let sun = w.sun_direction();
    let daylight = sun.z > 0.0;
    let flat = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    for &(p, t) in &shadows.0 {
        let t = if daylight { t } else { 0.0 } as f32;
        let color = Color::srgb(0.1 + 0.9 * t, 0.15 + 0.75 * t, 0.45 - 0.35 * t);
        let size = Vec2::splat(SHADOW_SPACING as f32 * 0.8);
        gizmos.rect(
            Isometry3d::new(to_bevy(p + DVec3::Z * 0.1), flat),
            size,
            color,
        );
    }

    // The sun: yellow where it shines on the car, grey behind cloud or below the horizon.
    let lit = if daylight {
        w.sun_transmittance(pos)
    } else {
        0.0
    } as f32;
    gizmos.arrow(
        to_bevy(pos + sun * SUN_RAY),
        to_bevy(pos),
        Color::srgb(0.4 + 0.6 * lit, 0.4 + 0.5 * lit, 0.4 - 0.3 * lit),
    );

    // The wind at the car's height with its gusts, on a grid round it, coloured by
    // speed; and the 10 m wind above the car.
    let air = w.air_at(pos);
    let wind = air.wind;
    let color = heat(wind.length() / 15.0);
    for i in -1..=1 {
        for j in -1..=1 {
            let p = pos + DVec3::new(i as f64, j as f64, 1.0) * WIND_SPACING;
            gizmos.arrow(to_bevy(p), to_bevy(p + wind * WIND_SCALE), color);
        }
    }
    let (speed, from) = w.wind();
    let high = ground + DVec3::Z * 10.0;
    gizmos.arrow(
        to_bevy(high),
        to_bevy(high + wind_towards(from) * speed * WIND_SCALE),
        heat(speed / 15.0),
    );
}

// ---- Aero ------------------------------------------------------------------------------

/// Where the engine sits along the body's x axis, m from the CG.
fn engine_x(sim: &Simulation) -> f64 {
    let m = &sim.car.model;
    let (front, rear) = (m.corners[FL].origin.x, m.corners[RL].origin.x);
    match m.params.engine.position {
        EnginePosition::Front => front - 0.6,
        EnginePosition::Mid => rear + 0.9,
        EnginePosition::Rear => rear - 0.4,
    }
}

/// Height of the floor in body coordinates at rest, m.
fn floor_z(sim: &Simulation) -> f64 {
    let p = &sim.car.model.params;
    0.5 * (p.aero.ride_height[0] + p.aero.ride_height[1]) - p.cg_height
}

fn aero(settings: Res<DebugSettings>, sim: Res<Simulation>, mut gizmos: Gizmos<OnTop>) {
    if !settings.aero {
        return;
    }
    let car = &sim.car;
    let p = &car.model.params;
    let tel = &car.telemetry;
    let (pos, rot) = sim.body_pose();
    let world = |local: DVec3| to_bevy(pos + rot * local);
    let up = rot * DVec3::Z;
    let half_width = 0.5 * p.track_front.min(p.track_rear);

    // Each element: a bar across the car at its centre of pressure, its downforce
    // pressing on it (green; red if it lifts) and its drag (orange).
    for (e, a) in p.aero.elements.iter().zip(&tel.aero) {
        let cop = DVec3::new(e.position[0], 0.0, e.position[1]);
        gizmos.line(
            world(cop + DVec3::Y * half_width),
            world(cop - DVec3::Y * half_width),
            Color::WHITE,
        );
        let at = pos + rot * cop;
        let lift = up * a.lift * AERO_SCALE;
        let (start, end, color) = if a.lift >= 0.0 {
            (at + lift, at, Color::srgb(0.2, 1.0, 0.3))
        } else {
            (at, at - lift, Color::srgb(1.0, 0.2, 0.2))
        };
        gizmos.arrow(to_bevy(start), to_bevy(end), color);
        gizmos.arrow(
            to_bevy(at),
            to_bevy(at + rot * a.drag * AERO_SCALE),
            Color::srgb(1.0, 0.55, 0.1),
        );
    }

    // Ride heights at the axles: the floor down to the road, coloured against their
    // static values.
    let floor = floor_z(&sim);
    for k in 0..2 {
        let x = car.model.corners[2 * k].origin.x;
        let (ride, rest) = (tel.ride_height[k], p.aero.ride_height[k]);
        let top = DVec3::new(x, 0.0, floor);
        let color = heat(centred(ride, rest - RIDE_SPAN, rest, rest + RIDE_SPAN));
        gizmos.line(world(top), world(top - DVec3::Z * ride.max(0.0)), color);
        gizmos.line(
            world(top + DVec3::Y * 0.3),
            world(top - DVec3::Y * 0.3),
            color,
        );
    }

    // The air coming at the car, ahead of the nose, coloured by its speed.
    let front = car.model.corners[FL].origin.x + 1.5;
    let flow = -tel.airspeed;
    let length = flow.length();
    if length > 0.5 {
        let color = heat(length / 80.0);
        for y in [-0.6, 0.0, 0.6] {
            let end = DVec3::new(front, y, floor + 0.3);
            let start = end - flow * AIRSPEED_SCALE;
            gizmos.arrow(world(start), world(end), color);
        }
    }

    // The air's temperature where the car meets it: round each tyre (warmed by the road
    // and the engine bay), in the bay and in the intake.
    let ambient = tel.air.temperature;
    for (i, wt) in tel.wheels.iter().enumerate() {
        let hp = car.model.corners[i].origin * DVec3::new(1.0, 1.0, 0.0);
        gizmos.sphere(
            Isometry3d::from_translation(world(hp + DVec3::Z * 0.35)),
            0.1,
            air_color(wt.air_temperature, ambient),
        );
    }
    let heat_state = &car.state.drivetrain.engine.heat;
    let bay = DVec3::new(engine_x(&sim), 0.0, floor + 0.9);
    gizmos.sphere(
        Isometry3d::from_translation(world(bay)),
        0.16,
        air_color(heat_state.bay, ambient),
    );
    gizmos.sphere(
        Isometry3d::from_translation(world(bay + DVec3::Z * 0.3)),
        0.08,
        air_color(heat_state.intake, ambient),
    );
}

// ---- Car -------------------------------------------------------------------------------

/// Temperature of the brake pads' peak friction, °C.
fn pad_peak(sim: &Simulation) -> f64 {
    sim.car
        .model
        .params
        .brakes
        .pad_friction
        .iter()
        .copied()
        .fold((0.0, 0.0), |best, f| if f.1 > best.1 { f } else { best })
        .0
}

fn car(settings: Res<DebugSettings>, sim: Res<Simulation>, mut gizmos: Gizmos<OnTop>) {
    if !settings.car {
        return;
    }
    let car = &sim.car;
    let m = &*car.model;
    let p = &m.params;
    let st = &car.state;
    let tel = &car.telemetry;
    let (pos, rot) = sim.body_pose();
    let world = |local: DVec3| to_bevy(pos + rot * local);
    let body_rot = quat_to_bevy(rot);
    let ambient = tel.air.temperature;

    // The body between the axles, and the centre of gravity.
    let (front, rear) = (m.corners[FL].origin.x, m.corners[RL].origin.x);
    let floor = floor_z(&sim);
    let height = (2.0 * p.cg_height).max(0.8);
    let width = p.track_front.min(p.track_rear) - 0.3;
    let length = front - rear + 1.4;
    let centre = DVec3::new(0.5 * (front + rear), 0.0, floor + 0.5 * height);
    // Local Bevy axes of the body: x forward, y up, z right.
    gizmos.cube(
        Transform::from_translation(world(centre))
            .with_rotation(body_rot)
            .with_scale(Vec3::new(length as f32, height as f32, width as f32)),
        Color::srgba(0.8, 0.8, 0.85, 0.6),
    );
    gizmos.axes(
        Transform::from_translation(world(DVec3::ZERO)).with_rotation(body_rot),
        0.4,
    );
    let cg = pos;
    gizmos.arrow(
        to_bevy(cg),
        to_bevy(cg + st.velocity * VELOCITY_SCALE),
        Color::WHITE,
    );
    gizmos.arrow(
        to_bevy(cg),
        to_bevy(cg + rot * tel.acceleration * ACCELERATION_SCALE),
        Color::srgb(1.0, 0.3, 1.0),
    );

    let peak = pad_peak(&sim);
    let fluid = p.brakes.fluid_boiling_point;
    let mut links = Vec::new();
    for i in 0..4 {
        let corner = &m.corners[i];
        let w = &st.wheels[i];
        let wt = &tel.wheels[i];
        let tire = m.tire(i);
        let (r, tyre_width) = (tire.p.radius, tire.p.width);
        let hub = car.wheel_center_body(i);
        // The spin axis as the linkage holds it: steered, cambered.
        let pose = car.pose(i);
        let axle = rot * pose.axis;
        let steered = rot * DQuat::from_rotation_arc(DVec3::Y, pose.axis);
        let at = pos + rot * hub;
        let facing = Quat::from_rotation_arc(Vec3::Z, to_bevy(axle).normalize_or(Vec3::Z));
        let circle = |offset: f64| Isometry3d::new(to_bevy(at + axle * offset), facing);

        // The linkage and its actuation, blue at full droop to red on the bump stops.
        let travel = (w.travel - corner.droop_stop) / (corner.bump_stop - corner.droop_stop);
        links.clear();
        car.linkage_segments(i, &mut links);
        for &(a, b) in &links {
            gizmos.line(world(a), world(b), heat(travel));
        }

        // The tread's inner, middle and outer zones, and the carcass inside them.
        let optimal = tire.optimal_temperature();
        let tyre_color = |t: f64| {
            heat(centred(
                t,
                optimal - TYRE_BELOW,
                optimal,
                optimal + TYRE_ABOVE,
            ))
        };
        let side = corner.side;
        for (zone, &t) in w.tire.tread_temperature.iter().enumerate() {
            let offset = -side * tyre_width / 3.0 * (1.0 - zone as f64);
            gizmos
                .circle(circle(offset), r as f32, tyre_color(t))
                .resolution(32);
        }
        gizmos
            .circle(
                circle(0.0),
                (0.8 * r) as f32,
                tyre_color(w.tire.core_temperature),
            )
            .resolution(32);
        // A spoke turning with the wheel, red as the tread wears.
        let spoke = steered * DQuat::from_rotation_y(w.angle) * DVec3::X * (0.8 * r);
        gizmos.line(to_bevy(at), to_bevy(at + spoke), quality(1.0 - w.tire.wear));

        // The brake disc inboard, and its caliper above it.
        let inboard = -side * 0.6 * tyre_width;
        gizmos
            .circle(
                circle(inboard),
                (0.5 * r) as f32,
                heat(centred(w.brake.disc, ambient, peak, peak + 300.0)),
            )
            .resolution(24);
        gizmos.sphere(
            Isometry3d::from_translation(to_bevy(
                at + axle * inboard + steered * DVec3::new(-0.2 * r, 0.0, 0.45 * r),
            )),
            0.05,
            heat((w.brake.caliper - ambient) / (fluid - ambient).max(1.0)),
        );

        // The road's force on the tyre: its load (grey) and its grip force coloured by
        // how much of the grip it uses.
        if wt.load > 0.0 {
            let n = rot * DVec3::Z;
            let planar = wt.force - n * wt.force.dot(n);
            let limit = tire.p.mu_y * wt.grip * wt.load;
            gizmos.arrow(
                to_bevy(wt.contact),
                to_bevy(wt.contact + n * wt.load * TYRE_SCALE),
                Color::srgb(0.7, 0.7, 0.7),
            );
            gizmos.arrow(
                to_bevy(wt.contact),
                to_bevy(wt.contact + planar * TYRE_SCALE),
                heat(planar.length() / limit.max(1.0)),
            );
        }
    }

    // The engine's block, sump and gearbox, and the radiator behind the nose.
    let h = &st.drivetrain.engine.heat;
    let thermal = &m.engine.thermal;
    let warm = EngineHeat::WARM;
    let engine = |t: f64, mid: f64| heat(centred(t, ambient, mid, mid + ENGINE_ABOVE));
    let x = engine_x(&sim);
    let part = |local: DVec3, size: Vec3| {
        Transform::from_translation(world(local))
            .with_rotation(body_rot)
            .with_scale(size)
    };
    let block = DVec3::new(x, 0.0, floor + 0.4);
    gizmos.cube(
        part(block, Vec3::new(0.55, 0.45, 0.5)),
        engine(h.cylinder, warm.cylinder),
    );
    gizmos.cube(
        part(block - DVec3::Z * 0.3, Vec3::new(0.5, 0.12, 0.45)),
        engine(h.oil, warm.oil),
    );
    let gearbox = match p.engine.position {
        EnginePosition::Rear => block + DVec3::X * 0.55,
        _ => block - DVec3::X * 0.55,
    };
    gizmos.cube(
        part(gearbox, Vec3::new(0.45, 0.3, 0.3)),
        engine(h.gearbox, warm.gearbox),
    );
    gizmos.cube(
        part(
            DVec3::new(front + 0.7, 0.0, floor + 0.3),
            Vec3::new(0.06, 0.35, (width * 0.6) as f32),
        ),
        heat(centred(
            h.coolant,
            ambient,
            thermal.thermostat,
            thermal.boiling_point,
        )),
    );
}

// ---- Panel -----------------------------------------------------------------------------

fn panel(
    settings: Res<DebugSettings>,
    sim: Res<Simulation>,
    mut panel: Query<(&mut Text, &mut Visibility), With<DebugPanel>>,
) {
    let Ok((mut text, mut visibility)) = panel.single_mut() else {
        return;
    };
    let s = *settings;
    let shown = s.road != RoadView::Off || s.environment || s.aero || s.car;
    visibility.set_if_neq(if shown {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    });
    if !shown {
        return;
    }
    let mut out = String::from("DEBUG (F1 road, F2 environment, F3 aero, F4 car)\n");
    if s.road != RoadView::Off {
        road_text(&mut out, s.road, &sim);
    }
    if s.environment {
        environment_text(&mut out, &sim);
    }
    if s.aero {
        aero_text(&mut out, &sim);
    }
    if s.car {
        car_text(&mut out, &sim);
    }
    text.0 = out.trim_end().to_string();
}

fn road_text(out: &mut String, view: RoadView, sim: &Simulation) {
    let q = sim.track.query(sim.car.state.position, sim.lap.hint());
    let e = &sim.evolution;
    let _ = writeln!(out, "\nROAD: {}", view.name());
    let _ = match view {
        RoadView::Off => Ok(()),
        RoadView::Grip => writeln!(
            out,
            "red {:.0} % .. yellow .. green 100 % of the tyre's grip",
            GRIP_SCALE * 100.0
        ),
        RoadView::Rubber => writeln!(out, "blue dusty .. red rubbered in"),
        RoadView::Dirt => writeln!(
            out,
            "grey clean .. coloured by kind (grass, earth, gravel), full at {:.0} % cover",
            DIRT_SCALE * 100.0
        ),
        RoadView::Temperature => {
            let (lo, hi) = road_range(sim);
            writeln!(out, "blue {lo:.1} C .. green .. red {hi:.1} C")
        }
    };
    let cover: f64 = e.cover_at(q.s, q.d).iter().sum();
    let _ = writeln!(
        out,
        "here: s {:.0} m, d {:+.1} m, {:?}: grip {:.1} %, rubber {:.2}, dirt {:.3}, {:.1} C",
        q.s,
        q.d,
        q.surface,
        e.grip_at(q.surface, q.s, q.d) * 100.0,
        e.rubber_at(q.s, q.d),
        cover,
        sim.weather.road_temperature(q.s, q.d)
    );
}

fn environment_text(out: &mut String, sim: &Simulation) {
    let w = &sim.weather;
    let (pos, _) = sim.body_pose();
    let air = w.air_at(pos);
    let sun = w.sun_direction();
    let light = w.sunlight();
    let (speed, from) = w.wind();
    let _ = writeln!(
        out,
        "\nENVIRONMENT {} {}",
        crate::settings::clock(w.hour()),
        w.regime().name()
    );
    let _ = writeln!(
        out,
        "air {:.1} C here ({:.1} C at the track's lowest), {:.0} % humidity, {:.1} hPa",
        air.temperature,
        w.air_temperature(),
        w.relative_humidity() * 100.0,
        air.pressure
    );
    let _ = writeln!(
        out,
        "density {:.3} kg/m3, engine power {:.1} % of standard",
        air.density,
        air.engine * 100.0
    );
    let _ = writeln!(
        out,
        "wind {:.1} m/s from {} at 10 m; here {:.1} m/s with gusts",
        speed,
        crate::settings::compass(from),
        air.wind.length()
    );
    let _ = writeln!(
        out,
        "sun elevation {:.0} deg, azimuth {:.0} deg; direct {:.0}, diffuse {:.0}, global {:.0} W/m2",
        sun.z.clamp(-1.0, 1.0).asin().to_degrees(),
        sun.x.atan2(sun.y).to_degrees().rem_euclid(360.0),
        light.direct,
        light.diffuse,
        light.global
    );
    let cover = w.layer_cover();
    let _ = writeln!(
        out,
        "cloud {:.0} %: cumulus {:.0}, stratus {:.0}, middle {:.0}, cirrus {:.0} %; base {:.0} m",
        w.cloud_cover() * 100.0,
        cover[0] * 100.0,
        cover[1] * 100.0,
        cover[2] * 100.0,
        cover[3] * 100.0,
        w.condensation_level()
    );
    let _ = writeln!(
        out,
        "sunlight on the car {:.0} %; ground: yellow sunlit .. blue in cloud shadow",
        w.sun_transmittance(pos) * 100.0
    );
    if let Some((mean, lo, hi)) = w.road().map(|r| r.stats()) {
        let _ = writeln!(out, "road {mean:.1} C mean, {lo:.1} .. {hi:.1} C");
    }
}

fn aero_text(out: &mut String, sim: &Simulation) {
    let car = &sim.car;
    let p = &car.model.params;
    let tel = &car.telemetry;
    let v = tel.airspeed;
    let yaw = if v.x.abs() > 5.0 {
        v.y.atan2(v.x.abs()).to_degrees()
    } else {
        0.0
    };
    let total = tel.downforce[0] + tel.downforce[1];
    let _ = writeln!(
        out,
        "\nAERO airspeed {:.1} m/s, yaw {:+.1} deg, density {:.3} kg/m3",
        v.length(),
        yaw,
        tel.air.density
    );
    let _ = writeln!(
        out,
        "downforce {:.0} N ({:.1} % front), drag {:.0} N, L/D {:.2}",
        total,
        100.0 * tel.downforce[0] / total.abs().max(1.0),
        tel.drag,
        total / tel.drag.max(1.0)
    );
    let _ = writeln!(
        out,
        "ride height front {:.1} mm (static {:.1}), rear {:.1} mm (static {:.1})",
        tel.ride_height[0] * 1e3,
        p.aero.ride_height[0] * 1e3,
        tel.ride_height[1] * 1e3,
        p.aero.ride_height[1] * 1e3
    );
    let _ = writeln!(
        out,
        "element        downforce N   drag N   AoA deg   height mm"
    );
    for (e, a) in p.aero.elements.iter().zip(&tel.aero) {
        let _ = writeln!(
            out,
            "{:<14} {:>11.0}   {:>6.0}   {:>7.2}   {:>9.1}",
            e.name,
            a.lift,
            a.drag.length(),
            a.angle_of_attack.to_degrees(),
            a.ride_height * 1e3
        );
    }
    if p.aero.elements.len() > MAX_AERO_TELEMETRY {
        let _ = writeln!(
            out,
            "(only the first {MAX_AERO_TELEMETRY} of {} elements)",
            p.aero.elements.len()
        );
    }
    let h = &car.state.drivetrain.engine.heat;
    let _ = writeln!(
        out,
        "air C: ambient {:.1}, tyres {}, bay {:.1}, intake {:.1}",
        tel.air.temperature,
        tel.wheels
            .map(|w| format!("{:.1}", w.air_temperature))
            .join(" "),
        h.bay,
        h.intake
    );
    let _ = writeln!(
        out,
        "arrows: green downforce, orange drag, air ahead of the nose; spheres: air C,\n\
         blue {:.0} below .. green ambient .. red {:.0} above; axle bars: ride height",
        AIR_BELOW, AIR_ABOVE
    );
}

fn car_text(out: &mut String, sim: &Simulation) {
    let car = &sim.car;
    let m = &*car.model;
    let st = &car.state;
    let tel = &car.telemetry;
    let _ = writeln!(
        out,
        "\nCAR tyre  tread in/mid/out C  core C   bar  wear %  disc C  caliper C  travel mm  camber °  grip %"
    );
    for (i, name) in ["FL", "FR", "RL", "RR"].iter().enumerate() {
        let w = &st.wheels[i];
        let wt = &tel.wheels[i];
        let [a, b, c] = w.tire.tread_temperature;
        let travel = w.travel * 1e3;
        let _ = writeln!(
            out,
            "    {name}    {a:5.0} {b:5.0} {c:5.0}     {:5.0}  {:5.2}  {:5.1}  {:6.0}  {:9.0}  {:+9.1}  {:+8.2}  {:6.1}",
            w.tire.core_temperature,
            wt.pressure,
            w.tire.wear * 100.0,
            w.brake.disc,
            w.brake.caliper,
            travel,
            wt.camber.to_degrees(),
            wt.grip * 100.0
        );
    }
    let h = &st.drivetrain.engine.heat;
    let _ = writeln!(
        out,
        "engine: cylinders {:.0}, coolant {:.0} (thermostat {:.0}), oil {:.0}, gearbox {:.0} C",
        h.cylinder, h.coolant, m.engine.thermal.thermostat, h.oil, h.gearbox
    );
    let optimal = [m.tire(0), m.tire(2)].map(|t| t.optimal_temperature());
    let _ = writeln!(
        out,
        "tyres green at {:.0} / {:.0} C (front / rear), brakes at {:.0} C, calipers red at {:.0} C",
        optimal[0],
        optimal[1],
        pad_peak(sim),
        m.params.brakes.fluid_boiling_point
    );
    let _ = writeln!(
        out,
        "rings: tread zones and core; spoke: wear; linkage: blue at full droop .. red on the stops;\n\
         arrows: grey load, grip force blue unused .. red at the limit, white velocity,\n\
         magenta acceleration"
    );
}
