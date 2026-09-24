//! Volumetric clouds (see `clouds.wgsl`): a material on a sphere around the scene, drawn
//! where the sky shows, fed with the simulation's cloud map and tileable 3D noise made at
//! start-up.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use open_racing_sim::weather::{CLOUD_MAP_SIZE, CloudMap};

/// Radius of the sphere the clouds are drawn on, m; it only needs to enclose the views.
const DOME_RADIUS: f32 = 50_000.0;
/// Texels along each edge of the shape and detail noise.
const SHAPE_SIZE: usize = 64;
const DETAIL_SIZE: usize = 32;

/// One cloud layer, see `Layer` in `clouds.wgsl`.
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct GpuLayer {
    pub base: f32,
    pub top: f32,
    pub threshold: f32,
    pub cover: f32,
    pub offset: Vec2,
    pub weights: Vec2,
    pub scale: f32,
    pub extinction: f32,
    pub cell_threshold: f32,
    /// Uniform arrays step in 16 bytes.
    pub _pad: Vec3,
}

/// See `CloudParams` in `clouds.wgsl`.
#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct CloudParams {
    pub sun_direction: Vec3,
    pub steps: u32,
    pub sun_illuminance: Vec3,
    pub light_steps: u32,
    pub sky_radiance: Vec3,
    pub detail: f32,
    pub ground_radiance: Vec3,
    pub planet_centre: f32,
    pub horizon_radiance: Vec3,
    pub temporal: f32,
    pub streaks: Vec2,
    pub density_scale: f32,
    pub _pad: f32,
    pub shear: Vec2,
    pub _pad2: Vec2,
    pub layers: [GpuLayer; 4],
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct CloudMaterial {
    #[uniform(0)]
    pub params: CloudParams,
    #[texture(1)]
    #[sampler(2)]
    pub cloud_map: Handle<Image>,
    #[texture(3, dimension = "3d")]
    #[sampler(4)]
    pub shape: Handle<Image>,
    #[texture(5, dimension = "3d")]
    #[sampler(6)]
    pub detail: Handle<Image>,
}

impl Material for CloudMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://open_racing_app/clouds.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://open_racing_app/clouds.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }

    /// Behind every other transparent object.
    fn depth_bias(&self) -> f32 {
        -1.0e9
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Seen from inside.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// The cloud material, updated each frame by the weather.
#[derive(Resource)]
pub struct Clouds {
    pub material: Handle<CloudMaterial>,
    pub entity: Entity,
}

pub struct CloudsPlugin;

impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "clouds.wgsl");
        app.add_plugins(MaterialPlugin::<CloudMaterial>::default());
    }
}

fn repeating(image: &mut Image) {
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
}

/// The simulation's cloud map as a texture.
pub fn cloud_map_image(map: &CloudMap) -> Image {
    let data: Vec<u8> = map.texels().iter().flatten().copied().collect();
    let mut image = Image::new(
        Extent3d {
            width: CLOUD_MAP_SIZE as u32,
            height: CLOUD_MAP_SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    repeating(&mut image);
    image
}

fn volume(size: usize, data: Vec<u8>) -> Image {
    let s = size as u32;
    let mut image = Image::new(
        Extent3d {
            width: s,
            height: s,
            depth_or_array_layers: s,
        },
        TextureDimension::D3,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    repeating(&mut image);
    image
}

/// Spawns the cloud dome, drawing the clouds of `map`.
pub fn spawn(
    commands: &mut Commands,
    map: &CloudMap,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<CloudMaterial>,
) -> Clouds {
    let (shape, detail) = noise_volumes();
    let material = materials.add(CloudMaterial {
        params: CloudParams::default(),
        cloud_map: images.add(cloud_map_image(map)),
        shape: images.add(volume(SHAPE_SIZE, shape)),
        detail: images.add(volume(DETAIL_SIZE, detail)),
    });
    let entity = commands
        .spawn((
            Mesh3d(meshes.add(Sphere::new(DOME_RADIUS).mesh().ico(3).unwrap())),
            MeshMaterial3d(material.clone()),
            NoFrustumCulling,
            NotShadowCaster,
            NotShadowReceiver,
            Transform::default(),
        ))
        .id();
    Clouds { material, entity }
}

/// The shape noise (Perlin–Worley, then Worley at three frequencies) and the detail
/// noise (Worley at three frequencies), tileable, made on all cores.
fn noise_volumes() -> (Vec<u8>, Vec<u8>) {
    std::thread::scope(|scope| {
        let shape = scope.spawn(|| {
            volume_data(SHAPE_SIZE, 11, |p, w| {
                let perlin = fbm_perlin(p, 4, 7);
                let worley = w(4) * 0.625 + w(8) * 0.25 + w(16) * 0.125;
                // Perlin billows on a floor of Worley cells (Schneider).
                let pw = worley + perlin * (1.0 - worley);
                [
                    pw,
                    w(4) * 0.625 + w(8) * 0.25 + w(16) * 0.125,
                    w(8) * 0.625 + w(16) * 0.25 + w(32) * 0.125,
                    w(16) * 0.625 + w(32) * 0.25 + w(32) * 0.125,
                ]
            })
        });
        let detail = volume_data(DETAIL_SIZE, 23, |_, w| {
            [
                w(2) * 0.625 + w(4) * 0.25 + w(8) * 0.125,
                w(4) * 0.625 + w(8) * 0.25 + w(16) * 0.125,
                w(8) * 0.625 + w(16) * 0.25 + w(16) * 0.125,
                1.0,
            ]
        });
        (shape.join().unwrap(), detail)
    })
}

/// RGBA8 data of a `size`³ volume, each texel from `texel(position in 0..1, worley)`,
/// where `worley(f)` is inverted Worley noise with f cells per edge.
fn volume_data(
    size: usize,
    seed: u64,
    texel: impl Fn(Vec3, &dyn Fn(u32) -> f32) -> [f32; 4] + Sync,
) -> Vec<u8> {
    let frequencies = [2, 4, 8, 16, 32];
    let points: Vec<Vec<Vec3>> = frequencies
        .iter()
        .map(|&f| feature_points(f, seed ^ f as u64))
        .collect();
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(16);
    let slices = size.div_ceil(threads);
    let mut data = vec![0u8; size * size * size * 4];
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in data.chunks_mut(slices * size * size * 4).enumerate() {
            let (points, texel) = (&points, &texel);
            scope.spawn(move || {
                for (k, out) in chunk.chunks_mut(4).enumerate() {
                    let i = chunk_index * slices * size * size + k;
                    let (x, y, z) = (i % size, (i / size) % size, i / (size * size));
                    let p = (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / size as f32;
                    let worley = |f: u32| {
                        let slot = frequencies.iter().position(|&g| g == f).unwrap();
                        1.0 - worley(p, f, &points[slot])
                    };
                    let v = texel(p, &worley);
                    for c in 0..4 {
                        out[c] = (v[c].clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
            });
        }
    });
    data
}

fn hash(x: u64) -> u64 {
    let mut x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// One random point in each of the f³ cells.
fn feature_points(f: u32, seed: u64) -> Vec<Vec3> {
    (0..f * f * f)
        .map(|i| {
            let h = hash(seed.wrapping_mul(31).wrapping_add(i as u64));
            Vec3::new(unit(h), unit(hash(h)), unit(hash(h ^ 1)))
        })
        .collect()
}

/// Distance to the nearest feature point, in cells, repeating every f cells (0..~1).
fn worley(p: Vec3, f: u32, points: &[Vec3]) -> f32 {
    let q = p * f as f32;
    let cell = q.floor();
    let fi = f as i32;
    let mut best = f32::INFINITY;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = cell + Vec3::new(dx as f32, dy as f32, dz as f32);
                let w = |v: f32| (v as i32).rem_euclid(fi) as u32;
                let index = (w(c.z) * f + w(c.y)) * f + w(c.x);
                let d = (c + points[index as usize] - q).length_squared();
                best = best.min(d);
            }
        }
    }
    best.sqrt().min(1.0)
}

/// Tileable gradient noise FBM starting at `f` cells per edge, 0..1.
fn fbm_perlin(p: Vec3, f: u32, seed: u64) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 0.5;
    let mut total = 0.0;
    for octave in 0..3 {
        let g = f << octave;
        sum += amplitude * perlin3(p * g as f32, g, seed + octave);
        total += amplitude;
        amplitude *= 0.5;
    }
    (sum / total * 0.7 + 0.5).clamp(0.0, 1.0)
}

fn perlin3(p: Vec3, period: u32, seed: u64) -> f32 {
    let i = p.floor();
    let f = p - i;
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let u = Vec3::new(fade(f.x), fade(f.y), fade(f.z));
    let pi = period as i32;
    let grad = |dx: i32, dy: i32, dz: i32| {
        let w = |v: f32, d: i32| ((v as i32 + d).rem_euclid(pi)) as u64;
        let h = hash(seed ^ (w(i.x, dx) | w(i.y, dy) << 20 | w(i.z, dz) << 40));
        // One of the twelve cube-edge directions.
        let g = match h % 12 {
            0 => Vec3::new(1.0, 1.0, 0.0),
            1 => Vec3::new(-1.0, 1.0, 0.0),
            2 => Vec3::new(1.0, -1.0, 0.0),
            3 => Vec3::new(-1.0, -1.0, 0.0),
            4 => Vec3::new(1.0, 0.0, 1.0),
            5 => Vec3::new(-1.0, 0.0, 1.0),
            6 => Vec3::new(1.0, 0.0, -1.0),
            7 => Vec3::new(-1.0, 0.0, -1.0),
            8 => Vec3::new(0.0, 1.0, 1.0),
            9 => Vec3::new(0.0, -1.0, 1.0),
            10 => Vec3::new(0.0, 1.0, -1.0),
            _ => Vec3::new(0.0, -1.0, -1.0),
        };
        g.dot(f - Vec3::new(dx as f32, dy as f32, dz as f32))
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(grad(0, 0, 0), grad(1, 0, 0), u.x);
    let x10 = lerp(grad(0, 1, 0), grad(1, 1, 0), u.x);
    let x01 = lerp(grad(0, 0, 1), grad(1, 0, 1), u.x);
    let x11 = lerp(grad(0, 1, 1), grad(1, 1, 1), u.x);
    lerp(lerp(x00, x10, u.y), lerp(x01, x11, u.y), u.z)
}
