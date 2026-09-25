//! Tyre marks and tyre smoke, driven by the simulated slip and tread temperature at
//! each contact patch.
//!
//! Both are dynamic meshes written on the CPU: skid marks are a ring buffer of quads
//! laid on the road, split into chunks so that a new segment re-uploads only its chunk;
//! smoke is a pool of camera-facing billboards. Nothing here feeds back into the
//! simulation.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::Image;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use open_racing_sim::{Coat, GRAVITY, Surface, TireCondition, WheelTelemetry};

use crate::camera::PrimaryView;
use crate::driving::{self, Simulation};
use crate::scene::to_bevy;

/// Skid mark quads kept before the oldest are overwritten.
const MARK_CAPACITY: usize = 8192;
/// Skid mark quads per mesh.
const MARK_CHUNK: usize = 512;
/// Minimum distance a contact patch travels before a new mark segment is laid, m.
const MARK_SEGMENT: f32 = 0.25;
/// A contact patch jumping further than this in one frame (reset, replay) starts a new trail, m.
const MARK_BREAK: f32 = 3.0;
const MARK_HALF_WIDTH: f32 = 0.14;
/// Height of the marks above the road, above the kerb strips (0.01 m), m.
const MARK_LIFT: f64 = 0.015;
/// Slide intensity below which no mark is laid.
const MARK_THRESHOLD: f32 = 0.08;
const MARK_OPACITY: f32 = 0.55;

const SMOKE_CAPACITY: usize = 1024;
/// Particles per second from one tyre at full smoke.
const SMOKE_RATE: f32 = 70.0;
/// Contact patch temperatures over which rubber starts to vaporise and smoke fully, °C.
const SMOKE_TEMPERATURE: (f64, f64) = (130.0, 230.0);
/// Flash heating of the contact patch above the tread surface per m/s of sliding, K.
const FLASH_PER_SLIDE_SPEED: f64 = 4.0;
/// Particles per second from one tyre on grass at speed.
const DUST_RATE: f32 = 35.0;
/// Particles per second from one fully dirt-coated tyre flinging it off on the road.
const DEBRIS_RATE: f32 = 25.0;

/// How much a tyre on asphalt or a kerb is sliding, 0..1. Shared with the tyre squeal.
pub fn slide(w: &WheelTelemetry) -> f64 {
    smoothstep(0.06, 0.15, w.slip_angle.abs()).max(smoothstep(0.1, 0.3, w.slip_ratio.abs()))
}

pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A mesh of skid marks, hidden under the road's debug view.
#[derive(Component)]
pub struct SkidMarkMesh;

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn).add_systems(
            Update,
            (update_marks, update_smoke).after(driving::step_simulation),
        );
    }
}

/// End of the last laid segment of one wheel's trail.
#[derive(Clone, Copy)]
struct TrailEnd {
    center: Vec3,
    left: Vec3,
    right: Vec3,
    alpha: f32,
}

#[derive(Resource)]
struct SkidMarks {
    /// `MARK_CAPACITY / MARK_CHUNK` meshes of `MARK_CHUNK` quads each.
    chunks: Vec<Handle<Mesh>>,
    /// Next quad to write.
    next: usize,
    trails: [Option<TrailEnd>; 4],
}

impl SkidMarks {
    fn push(&mut self, meshes: &mut Assets<Mesh>, from: TrailEnd, to: TrailEnd, normal: Vec3) {
        let (chunk, quad) = (self.next / MARK_CHUNK, self.next % MARK_CHUNK);
        self.next = (self.next + 1) % MARK_CAPACITY;
        let Some(mut mesh) = meshes.get_mut(&self.chunks[chunk]) else {
            return;
        };
        let v = quad * 4;
        let corners = [
            (from.left, from.alpha),
            (from.right, from.alpha),
            (to.left, to.alpha),
            (to.right, to.alpha),
        ];
        let (positions, colors) = attributes(&mut mesh);
        for (k, (p, a)) in corners.into_iter().enumerate() {
            positions[v + k] = p.to_array();
            colors[v + k] = [0.02, 0.02, 0.02, a * MARK_OPACITY];
        }
        if let Some(VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
        {
            normals[v..v + 4].fill(normal.to_array());
        }
    }
}

/// The positions and colours of a mesh made by `dynamic_mesh`, to write in place.
fn attributes(mesh: &mut Mesh) -> (&mut [[f32; 3]], &mut [[f32; 4]]) {
    let mut positions = None;
    let mut colors = None;
    for (attribute, values) in mesh.attributes_mut() {
        match values {
            VertexAttributeValues::Float32x3(v) if attribute.id == Mesh::ATTRIBUTE_POSITION.id => {
                positions = Some(v.as_mut_slice())
            }
            VertexAttributeValues::Float32x4(v) if attribute.id == Mesh::ATTRIBUTE_COLOR.id => {
                colors = Some(v.as_mut_slice())
            }
            _ => {}
        }
    }
    (
        positions.expect("dynamic mesh has positions"),
        colors.expect("dynamic mesh has colours"),
    )
}

struct Particle {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    size: (f32, f32),
    spin: f32,
    color: [f32; 3],
    opacity: f32,
}

#[derive(Resource)]
struct Smoke {
    mesh: Handle<Mesh>,
    particles: Vec<Particle>,
    /// Particles written to the mesh last frame.
    drawn: usize,
    /// Fractional particles owed per wheel.
    owed: [f32; 4],
    rng: u32,
}

impl Smoke {
    /// Uniform in [-1, 1) (xorshift32).
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

/// Quad indices for `quads` independent quads laid out as (a-left, a-right, b-left, b-right).
fn quad_indices(quads: usize) -> Indices {
    Indices::U32(
        (0..quads as u32)
            .flat_map(|q| [0, 1, 2, 1, 3, 2].map(|i| 4 * q + i))
            .collect(),
    )
}

/// `quads` empty quads, each with UVs over the whole texture.
fn dynamic_mesh(quads: usize) -> Mesh {
    let n = quads * 4;
    let uvs = [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]].repeat(quads);
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; n])
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; n])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; n])
    .with_inserted_indices(quad_indices(quads))
}

/// Soft round puff: white with alpha falling off towards the edge.
fn puff_texture() -> Image {
    const SIZE: u32 = 64;
    let data = (0..SIZE * SIZE)
        .flat_map(|i| {
            let (x, y) = ((i % SIZE) as f32 + 0.5, (i / SIZE) as f32 + 0.5);
            let r = Vec2::new(x, y).distance(Vec2::splat(SIZE as f32 * 0.5)) / (SIZE as f32 * 0.5);
            let a = (1.0 - r * r).max(0.0).powi(2);
            [255, 255, 255, (a * 255.0) as u8]
        })
        .collect();
    Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mark_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    let chunks = (0..MARK_CAPACITY / MARK_CHUNK)
        .map(|_| {
            let mesh = meshes.add(dynamic_mesh(MARK_CHUNK));
            commands.spawn((
                SkidMarkMesh,
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mark_material.clone()),
                NoFrustumCulling,
                NotShadowCaster,
            ));
            mesh
        })
        .collect();
    commands.insert_resource(SkidMarks {
        chunks,
        next: 0,
        trails: [None; 4],
    });

    let smoke = meshes.add(dynamic_mesh(SMOKE_CAPACITY));
    commands.spawn((
        Mesh3d(smoke.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(puff_texture())),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            ..default()
        })),
        NoFrustumCulling,
        NotShadowCaster,
    ));
    commands.insert_resource(Smoke {
        mesh: smoke,
        particles: Vec::with_capacity(SMOKE_CAPACITY),
        drawn: 0,
        owed: [0.0; 4],
        rng: 0x2545_F491,
    });
}

/// Slide intensity of a wheel weighted by how hard it is pressed into the road, 0..1.
fn wheel_slide(w: &WheelTelemetry, static_load: f64, moving: f64) -> f32 {
    if w.load <= 0.0 || !w.surface.paved() {
        return 0.0;
    }
    (slide(w) * (w.load / static_load).min(1.5) * moving).min(1.0) as f32
}

/// How much a tyre smokes, 0..1: rubber vaporises once the sliding contact patch is hot
/// enough, so a tyre at the grip limit stays clean while lock-ups, wheelspin and long
/// slides that cook the tread smoke more and more.
fn tire_smoke(w: &WheelTelemetry, tire: &TireCondition) -> f64 {
    if w.load <= 0.0 {
        return 0.0;
    }
    let contact = tire.surface_temperature(&w.tread_load) + FLASH_PER_SLIDE_SPEED * w.slide_speed;
    smoothstep(SMOKE_TEMPERATURE.0, SMOKE_TEMPERATURE.1, contact)
}

fn update_marks(
    sim: Res<Simulation>,
    mut marks: ResMut<SkidMarks>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let car = &sim.car;
    let static_load = car.model.params.mass * GRAVITY / 4.0;
    let moving = smoothstep(0.5, 3.0, car.speed());
    for i in 0..4 {
        let w = &car.telemetry.wheels[i];
        let alpha = wheel_slide(w, static_load, moving);
        if alpha < MARK_THRESHOLD {
            marks.trails[i] = None;
            continue;
        }
        let q = sim.track.query(w.contact, car.state.wheels[i].hint);
        let normal = to_bevy(q.normal);
        let center = to_bevy(w.contact + q.normal * MARK_LIFT);
        let Some(last) = marks.trails[i] else {
            marks.trails[i] = Some(TrailEnd {
                center,
                left: center,
                right: center,
                alpha,
            });
            continue;
        };
        let step = center - last.center;
        let length = step.length();
        if length > MARK_BREAK {
            marks.trails[i] = Some(TrailEnd {
                center,
                left: center,
                right: center,
                alpha,
            });
        } else if length >= MARK_SEGMENT {
            let side = normal.cross(step / length).normalize_or_zero() * MARK_HALF_WIDTH;
            let end = TrailEnd {
                center,
                left: center + side,
                right: center - side,
                alpha,
            };
            // A fresh trail has no width yet; give its first edge this segment's direction.
            let start = if last.left == last.right {
                TrailEnd {
                    left: last.center + side,
                    right: last.center - side,
                    ..last
                }
            } else {
                last
            };
            marks.push(&mut meshes, start, end, normal);
            marks.trails[i] = Some(end);
        }
    }
}

fn update_smoke(
    time: Res<Time>,
    sim: Res<Simulation>,
    mut smoke: ResMut<Smoke>,
    mut meshes: ResMut<Assets<Mesh>>,
    cameras: Query<&GlobalTransform, With<PrimaryView>>,
) {
    let dt = time.delta_secs();
    let car = &sim.car;
    let speed = car.speed();
    let moving = smoothstep(0.5, 3.0, speed);
    let car_vel = to_bevy(car.state.velocity);

    // Age and move the existing particles: they drag to a stop in the air and rise.
    smoke.particles.retain_mut(|p| {
        p.age += dt;
        p.vel *= (-1.8 * dt).exp();
        p.vel.y += 0.4 * dt;
        p.pos += p.vel * dt;
        p.age < p.life
    });

    for i in 0..4 {
        let w = &car.telemetry.wheels[i];
        let (rate, color, opacity, life, size) = match w.surface.coat() {
            Some(coat) if w.load > 0.0 => {
                // Thrown up by speed or by a spinning / sliding tyre; gravel and dry
                // earth raise the most dust, turf hardly any.
                let dust = smoothstep(3.0, 25.0, speed).max(slide(w) * moving) as f32;
                let (color, amount) = match (w.surface, coat) {
                    (Surface::Turf, _) => ([0.45, 0.45, 0.35], 0.2),
                    (_, Coat::Grass) => ([0.45, 0.40, 0.28], 1.0),
                    (_, Coat::Soil) => ([0.42, 0.33, 0.22], 1.3),
                    (_, Coat::Grit) => ([0.62, 0.56, 0.45], 1.6),
                };
                (
                    DUST_RATE * dust * amount,
                    color,
                    0.35 * dust * amount.min(1.2),
                    1.4 * amount.max(0.8),
                    (0.4, 2.2 * amount.max(0.8)),
                )
            }
            _ => {
                let tire = &car.state.wheels[i].tire;
                let s = tire_smoke(w, tire) as f32;
                // A dirty tyre flings its coat off as it gets back up to speed.
                let debris = if w.load > 0.0 {
                    (tire.dirt() * smoothstep(3.0, 20.0, speed)) as f32
                } else {
                    0.0
                };
                if DEBRIS_RATE * debris > SMOKE_RATE * s {
                    (
                        DEBRIS_RATE * debris,
                        crate::tyre_dirt::coat_color(&tire.coat),
                        0.3 * debris.sqrt(),
                        1.0,
                        (0.3, 1.4),
                    )
                } else {
                    (
                        SMOKE_RATE * s,
                        [0.82, 0.82, 0.84],
                        0.45 * s.sqrt(),
                        2.6,
                        (0.5, 2.0 + 1.5 * s),
                    )
                }
            }
        };
        smoke.owed[i] += rate * dt;
        let contact = to_bevy(w.contact);
        while smoke.owed[i] >= 1.0 {
            smoke.owed[i] -= 1.0;
            if smoke.particles.len() == SMOKE_CAPACITY {
                break;
            }
            let jitter = Vec3::new(smoke.rand(), smoke.rand(), smoke.rand());
            let p = Particle {
                pos: contact + Vec3::Y * 0.25 + jitter * 0.2,
                // Mostly left behind by the car, kicked out a little in random directions.
                vel: car_vel * 0.25 + jitter * 1.2 + Vec3::Y * 0.5,
                age: 0.0,
                life: life * (1.0 + 0.25 * smoke.rand()),
                size: (size.0, size.1 * (1.0 + 0.2 * smoke.rand())),
                spin: smoke.rand() * std::f32::consts::PI,
                color,
                opacity,
            };
            smoke.particles.push(p);
        }
        smoke.owed[i] = smoke.owed[i].min(1.0);
    }

    // Nothing to draw and nothing left to clear: leave the mesh as it is.
    if smoke.particles.is_empty() && smoke.drawn == 0 {
        return;
    }
    let Ok(camera) = cameras.single() else { return };
    let smoke = &mut *smoke;
    let Some(mut mesh) = meshes.get_mut(&smoke.mesh) else {
        return;
    };
    let eye = camera.translation();
    let (right, up) = (camera.right().as_vec3(), camera.up().as_vec3());
    // Blended without depth writes, so draw back to front.
    smoke.particles.sort_by(|a, b| {
        b.pos
            .distance_squared(eye)
            .total_cmp(&a.pos.distance_squared(eye))
    });

    let (positions, colors) = attributes(&mut mesh);
    for (q, p) in smoke.particles.iter().enumerate() {
        let t = p.age / p.life;
        let radius = 0.5 * (p.size.0 + (p.size.1 - p.size.0) * (1.0 - (1.0 - t).powi(2)));
        let alpha = p.opacity * (p.age / 0.12).min(1.0) * (1.0 - t).powf(1.5);
        let (sin, cos) = (p.spin + 0.3 * p.age).sin_cos();
        let (r, u) = (
            (right * cos + up * sin) * radius,
            (up * cos - right * sin) * radius,
        );
        // In the order of `dynamic_mesh`'s UVs.
        for (k, corner) in [-r + u, r + u, -r - u, r - u].into_iter().enumerate() {
            positions[4 * q + k] = (p.pos + corner).to_array();
            colors[4 * q + k] = [p.color[0], p.color[1], p.color[2], alpha];
        }
    }
    // Collapse the quads of particles that have died since the last frame.
    let live = smoke.particles.len();
    if smoke.drawn > live {
        positions[4 * live..4 * smoke.drawn].fill([0.0; 3]);
    }
    smoke.drawn = live;
}
