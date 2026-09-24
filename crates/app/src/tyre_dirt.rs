//! Grass, earth and gravel caked on the tyres after a trip off the track: a coat over
//! each tread in the colour of what it picked up. Each spot of the coat has its own
//! noise threshold, so it spreads over the tread as dirt builds up and breaks up into
//! splotches as it wears off.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::driving::{self, Simulation};
use crate::scene::{CarWheel, WheelAxles, to_bevy};

/// Side of the splatter texture, texels.
const SPLATTER_SIZE: u32 = 256;

/// Colour of each kind of coat (`Coat` order, sRGB): grass clippings, earth, sand and
/// gravel.
const COAT_COLORS: [[f32; 3]; 3] = [[0.30, 0.31, 0.17], [0.36, 0.27, 0.17], [0.62, 0.57, 0.47]];

/// Colour of a tyre coat, mixed by kind.
pub fn coat_color(coat: &[f64; 3]) -> [f32; 3] {
    let total: f64 = coat.iter().sum();
    if total <= 0.0 {
        return COAT_COLORS[1];
    }
    std::array::from_fn(|c| {
        (0..3)
            .map(|k| COAT_COLORS[k][c] * (coat[k] / total) as f32)
            .sum()
    })
}

pub struct TyreDirtPlugin;

impl Plugin for TyreDirtPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (attach_coats, update_coats).after(driving::step_simulation),
        );
    }
}

/// Marks a wheel that has its coat.
#[derive(Component)]
struct Coated;

/// The coat over the tread of wheel `.0`.
#[derive(Component)]
struct TyreCoat(usize, Handle<StandardMaterial>);

/// Smooth value noise in 0..1 with features about one unit across, repeating every
/// `period` units in x and y.
fn tiled_noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let hash = |i: i32, j: i32| {
        let (i, j) = (i.rem_euclid(period), j.rem_euclid(period));
        let mut h = (i as u32).wrapping_mul(0x8da6_b343)
            ^ (j as u32).wrapping_mul(0xd816_3841)
            ^ seed.wrapping_mul(0xcb1a_b31f);
        h ^= h >> 13;
        h = h.wrapping_mul(0x5bd1_e995);
        h ^= h >> 15;
        (h & 0xffff) as f32 / 65535.0
    };
    let (i, j) = (x.floor() as i32, y.floor() as i32);
    let (u, v) = (x - x.floor(), y - y.floor());
    let (u, v) = (u * u * (3.0 - 2.0 * u), v * v * (3.0 - 2.0 * v));
    let a = hash(i, j) + (hash(i + 1, j) - hash(i, j)) * u;
    let b = hash(i, j + 1) + (hash(i + 1, j + 1) - hash(i, j + 1)) * u;
    a + (b - a) * v
}

/// Octaves of tiling noise with `cells` features across the tile at the coarsest.
fn fractal(x: f32, y: f32, cells: i32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut weight, mut total) = (0.0, 1.0, 0.0);
    for o in 0..octaves {
        let f = cells << o;
        let scale = f as f32 / SPLATTER_SIZE as f32;
        sum += weight * tiled_noise(x * scale, y * scale, f, seed + o);
        total += weight;
        weight *= 0.5;
    }
    sum / total
}

/// Mud splatter: clumps and grain in the alpha channel. It tiles, and the tread tube's
/// UVs wrap it round the tyre.
fn splatter_texture() -> Image {
    let n = SPLATTER_SIZE;
    let data = (0..n * n)
        .flat_map(|i| {
            let (x, y) = ((i % n) as f32, (i / n) as f32);
            let v = 0.7 * fractal(x, y, 8, 3, 40) + 0.3 * fractal(x, y, 32, 2, 50);
            // Stretch the noise's middle out so the threshold sweeps evenly through it.
            let v = ((v - 0.5) * 1.8 + 0.5).clamp(0.0, 0.999);
            [255, 255, 255, (v * 255.0) as u8]
        })
        .collect();
    let mut image = Image::new(
        Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

/// Open tube around the tread, with its axis along +Y.
fn tread_tube(radius: f32, width: f32) -> Mesh {
    const SEGMENTS: u32 = 64;
    let (mut positions, mut normals, mut uvs) = (Vec::new(), Vec::new(), Vec::new());
    for k in 0..=SEGMENTS {
        let a = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let n = Vec3::new(a.cos(), 0.0, a.sin());
        for side in [-0.5, 0.5] {
            positions.push((n * radius + Vec3::Y * side * width).to_array());
            normals.push(n.to_array());
            uvs.push([k as f32 / SEGMENTS as f32 * 4.0, side + 0.5]);
        }
    }
    let indices = (0..SEGMENTS)
        .flat_map(|k| {
            let v = 2 * k;
            [v, v + 1, v + 2, v + 1, v + 3, v + 2]
        })
        .collect();
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

/// Gives each wheel a coat over its tread, hidden while the tyre is clean.
#[allow(clippy::too_many_arguments)]
fn attach_coats(
    mut commands: Commands,
    sim: Res<Simulation>,
    axles: Option<Res<WheelAxles>>,
    wheels: Query<(Entity, &CarWheel), Without<Coated>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut splatter: Local<Option<Handle<Image>>>,
) {
    if wheels.is_empty() {
        return;
    }
    let texture = splatter
        .get_or_insert_with(|| images.add(splatter_texture()))
        .clone();
    for (entity, wheel) in &wheels {
        let i = wheel.0;
        let tire = &sim.car.model.tire(i).p;
        let axis = axles.as_ref().map_or(glam::DVec3::Y, |a| a.0[i]);
        let axis = to_bevy(axis).normalize_or(Vec3::Z);
        let material = materials.add(StandardMaterial {
            base_color_texture: Some(texture.clone()),
            perceptual_roughness: 1.0,
            alpha_mode: AlphaMode::Mask(1.0),
            ..default()
        });
        commands.entity(entity).insert(Coated);
        commands.spawn((
            TyreCoat(i, material.clone()),
            Mesh3d(meshes.add(tread_tube(
                tire.radius as f32 + 0.004,
                tire.width as f32 * 0.92,
            ))),
            MeshMaterial3d(material),
            Transform::from_rotation(Quat::from_rotation_arc(Vec3::Y, axis)),
            Visibility::Hidden,
            NotShadowCaster,
            ChildOf(entity),
        ));
    }
}

/// Shows as much of each tyre's coat as it carries, in the colour of what it picked
/// up, by lowering the mask threshold as the coat builds up.
fn update_coats(
    sim: Res<Simulation>,
    mut coats: Query<(&TyreCoat, &mut Visibility)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (coat, mut visibility) in &mut coats {
        let tire = &sim.car.state.wheels[coat.0].tire;
        let dirt = tire.dirt() as f32;
        let shown = dirt > 0.02;
        visibility.set_if_neq(if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
        if !shown {
            continue;
        }
        // Quantised, so the material only changes when the coat visibly does.
        let q = |v: f32| (v * 48.0).round() / 48.0;
        let cutoff = q(1.0 - 0.92 * dirt.sqrt());
        let [r, g, b] = coat_color(&tire.coat).map(q);
        let color = Color::srgb(r, g, b);
        if materials
            .get(&coat.1)
            .is_some_and(|m| m.alpha_mode != AlphaMode::Mask(cutoff) || m.base_color != color)
            && let Some(mut m) = materials.get_mut(&coat.1)
        {
            m.alpha_mode = AlphaMode::Mask(cutoff);
            m.base_color = color;
        }
    }
}
