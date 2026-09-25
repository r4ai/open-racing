//! Cloud data, noise volumes and the low-resolution volumetric renderer.
mod render;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, ShaderType, TextureDimension, TextureFormat};
use open_racing_sim::weather::{CLOUD_MAP_SIZE, CloudMap, CloudSnapshot, VOLUME_X, VOLUME_Z};

const SHAPE_SIZE: usize = 64;
const DETAIL_SIZE: usize = 32;

#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct GpuCirrus {
    pub altitude: f32,
    pub threshold: f32,
    pub cover: f32,
    pub scale: f32,
    pub offset: Vec2,
    pub weights: Vec2,
}

#[derive(ShaderType, Clone, Copy, Debug, Default)]
pub struct CloudParams {
    pub sun_direction: Vec3,
    pub steps: u32,
    pub sun_illuminance: Vec3,
    pub light_steps: u32,
    pub sky_radiance: Vec3,
    pub detail: f32,
    pub ground_radiance: Vec3,
    pub base_height: f32,
    pub horizon_radiance: Vec3,
    pub blend: f32,
    pub shape_offset: Vec2,
    pub detail_offset: Vec2,
    pub frame_ages: Vec2,
    pub streaks: Vec2,
    pub cirrus_phase: Vec2,
    pub noise_mean: Vec2,
    pub upper_wind: Vec2,
    pub occupied_bands: u32,
    pub march_base: f32,
    pub winds: [Vec4; 32],
    pub cirrus: GpuCirrus,
}

#[derive(Resource, Clone, Debug)]
pub struct Clouds {
    pub params: CloudParams,
    pub cloud_map: Handle<Image>,
    pub shape: Handle<Image>,
    pub detail: Handle<Image>,
    pub field: Handle<Image>,
    pub time: f64,
    pub generation: u64,
    pub divisor: u32,
    pub uploaded: Option<(u64, f64)>,
}

pub struct CloudsPlugin;
impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "clouds.wgsl");
        app.add_plugins(render::CloudRenderPlugin);
    }
}

pub fn cloud_map_image(map: &CloudMap) -> Image {
    let data: Vec<u8> = map.texels().iter().flatten().copied().collect();
    Image::new(
        Extent3d {
            width: CLOUD_MAP_SIZE as u32,
            height: CLOUD_MAP_SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    )
}

fn volume(size: usize, data: Vec<u8>) -> Image {
    let (data, levels) = mip_chain(size, data);
    // `Image::new` asserts the data is the base level alone; this holds the mip chain.
    let mut image = Image::new_uninit(
        Extent3d {
            width: size as u32,
            height: size as u32,
            depth_or_array_layers: size as u32,
        },
        TextureDimension::D3,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image
}

fn mip_chain(mut size: usize, mut level: Vec<u8>) -> (Vec<u8>, u32) {
    let mut data = level.clone();
    let mut levels = 1;
    while size > 1 {
        let next = size / 2;
        let mut out = vec![0u8; next * next * next];
        for z in 0..next {
            for y in 0..next {
                for x in 0..next {
                    let mut sum = 0u32;
                    for dz in 0..2 {
                        for dy in 0..2 {
                            for dx in 0..2 {
                                sum += level[((z * 2 + dz) * size + y * 2 + dy) * size + x * 2 + dx]
                                    as u32;
                            }
                        }
                    }
                    out[(z * next + y) * next + x] = ((sum + 4) / 8) as u8;
                }
            }
        }
        data.extend_from_slice(&out);
        level = out;
        size = next;
        levels += 1;
    }
    (data, levels)
}

pub fn field_image(snapshot: Option<CloudSnapshot<'_>>) -> Image {
    // Texture coordinates are X, height, north, matching shader X, Y, -Z.
    let mut data = Vec::with_capacity(VOLUME_X * VOLUME_Z * VOLUME_X * 4);
    for y in 0..VOLUME_X {
        for z in 0..VOLUME_Z {
            for x in 0..VOLUME_X {
                let i = (z * VOLUME_X + y) * VOLUME_X + x;
                for previous in [true, false] {
                    let v = snapshot.map_or(0.0, |s| {
                        if previous {
                            s.previous.extinction[i]
                        } else {
                            s.current.extinction[i]
                        }
                    });
                    data.extend_from_slice(&half::f16::from_f32(v).to_le_bytes());
                }
            }
        }
    }
    Image::new(
        Extent3d {
            width: VOLUME_X as u32,
            height: VOLUME_Z as u32,
            depth_or_array_layers: VOLUME_X as u32,
        },
        TextureDimension::D3,
        data,
        TextureFormat::Rg16Float,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// A conservative vertical occupancy mask. The extra neighbours cover linear
/// filtering between cell centres; horizontal wind cannot invalidate this mask.
pub fn occupied_bands(snapshot: CloudSnapshot<'_>) -> u32 {
    let mut mask = 0u32;
    for z in 0..VOLUME_Z {
        let start = z * VOLUME_X * VOLUME_X;
        if snapshot.previous.extinction[start..start + VOLUME_X * VOLUME_X]
            .iter()
            .chain(&snapshot.current.extinction[start..start + VOLUME_X * VOLUME_X])
            .any(|&v| v > 0.0)
        {
            mask |= 1 << z;
        }
    }
    mask | (mask << 1) | (mask >> 1)
}

pub fn spawn(map: &CloudMap, images: &mut Assets<Image>) -> Clouds {
    let (shape, detail) = noise_volumes();
    let mean = |data: &[u8]| {
        data.iter().map(|&v| v as f64 / 255.0).sum::<f64>() as f32 / data.len() as f32
    };
    let noise_mean = Vec2::new(mean(&shape), mean(&detail));
    Clouds {
        params: CloudParams {
            noise_mean,
            ..default()
        },
        cloud_map: images.add(cloud_map_image(map)),
        shape: images.add(volume(SHAPE_SIZE, shape)),
        detail: images.add(volume(DETAIL_SIZE, detail)),
        field: images.add(field_image(None)),
        time: 0.0,
        generation: 0,
        divisor: 2,
        uploaded: None,
    }
}

/// Tileable scalar shape and detail signals. Only the sampled channel is built.
fn noise_volumes() -> (Vec<u8>, Vec<u8>) {
    std::thread::scope(|scope| {
        let shape = scope.spawn(|| {
            volume_data(SHAPE_SIZE, 11, [4, 8, 16], |p, w| {
                let perlin = fbm_perlin(p, 4, 7);
                let worley = w[0] * 0.625 + w[1] * 0.25 + w[2] * 0.125;
                let pw = worley + perlin * (1.0 - worley);
                // Erode before mip filtering, preserving the original red signal.
                ((pw - 0.6) / 0.4).max(0.0)
            })
        });
        let detail = volume_data(DETAIL_SIZE, 23, [2, 4, 8], |_, w| {
            w[0] * 0.625 + w[1] * 0.25 + w[2] * 0.125
        });
        (shape.join().unwrap(), detail)
    })
}

/// R8 data; each texel receives its position and three inverted Worley octaves.
fn volume_data(
    size: usize,
    seed: u64,
    frequencies: [u32; 3],
    texel: impl Fn(Vec3, [f32; 3]) -> f32 + Sync,
) -> Vec<u8> {
    let points = frequencies.map(|f| feature_points(f, seed ^ f as u64));
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(16);
    let slices = size.div_ceil(threads);
    let mut data = vec![0u8; size * size * size];
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in data.chunks_mut(slices * size * size).enumerate() {
            let (points, texel) = (&points, &texel);
            scope.spawn(move || {
                for (k, out) in chunk.iter_mut().enumerate() {
                    let i = chunk_index * slices * size * size + k;
                    let (x, y, z) = (i % size, (i / size) % size, i / (size * size));
                    let p = (Vec3::new(x as f32, y as f32, z as f32) + 0.5) / size as f32;
                    let octaves =
                        std::array::from_fn(|j| 1.0 - worley(p, frequencies[j], &points[j]));
                    *out = (texel(p, octaves).clamp(0.0, 1.0) * 255.0).round() as u8;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_preserve_constant_density_and_include_all_levels() {
        let (data, levels) = mip_chain(8, vec![117; 8 * 8 * 8]);
        assert_eq!(levels, 4);
        assert_eq!(data.len(), 512 + 64 + 8 + 1);
        assert!(data.iter().all(|&v| v == 117));
    }

    #[test]
    fn eroded_noise_mips_preserve_calibrated_mean_extinction() {
        let (shape, _) = noise_volumes();
        let mean = |data: &[u8]| data.iter().map(|&v| v as f64).sum::<f64>() / data.len() as f64;
        let reference = mean(&shape);
        assert!(reference > 1.0);
        let (mips, _) = mip_chain(SHAPE_SIZE, shape);
        let mut start = 0;
        let mut size = SHAPE_SIZE;
        loop {
            let end = start + size * size * size;
            // R8 rounding accumulates by at most half a quantisation step/level.
            assert!((mean(&mips[start..end]) / reference - 1.0).abs() < 0.06);
            if size == 1 {
                break;
            }
            start = end;
            size /= 2;
        }
    }

    #[test]
    fn perlin_tiles_without_a_position_jump() {
        let p = Vec3::new(0.37, 0.22, 0.81);
        assert!((fbm_perlin(p, 4, 7) - fbm_perlin(p + Vec3::X, 4, 7)).abs() < 1e-5);
    }
}
