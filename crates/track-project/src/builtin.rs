//! The surfaces, materials and textures a new project starts with. The textures are
//! generated, so the tool ships no image files.

use open_racing_sim::{Surface, SurfaceProps};
use open_racing_track::texture::Image;

use crate::project::{
    Alpha, BuiltinTexture, MaterialDef, NamedSurface, Profile, StripStyle, TextureSource, WallStyle,
};

/// Edge of the generated textures, texels.
const SIZE: usize = 256;

pub fn surfaces() -> Vec<NamedSurface> {
    [
        ("asphalt", Surface::Asphalt),
        ("kerb", Surface::Kerb),
        ("runoff", Surface::Runoff),
        ("grass", Surface::Grass),
        ("turf", Surface::Turf),
        ("gravel", Surface::Gravel),
        ("dirt", Surface::Dirt),
    ]
    .map(|(name, kind)| NamedSurface {
        name: name.into(),
        props: SurfaceProps::of(kind),
    })
    .to_vec()
}

/// Kerbs, gravel, run-off and grass to lay, from the built-in surfaces and materials.
pub fn strip_styles() -> Vec<StripStyle> {
    let s = |name: &str, width, profile, surface: &str, material: &str, fade| StripStyle {
        name: name.into(),
        width,
        profile,
        surface: surface.into(),
        material: material.into(),
        fade,
    };
    vec![
        s("kerb", 1.2, Profile::Crown(0.03), "kerb", "kerb", 3.0),
        s("flat kerb", 1.5, Profile::Flat, "kerb", "kerb", 2.0),
        s(
            "raised kerb",
            1.2,
            Profile::Crown(0.08),
            "kerb",
            "kerb",
            2.0,
        ),
        s(
            "sausage kerb",
            0.5,
            Profile::Crown(0.15),
            "kerb",
            "kerb",
            0.5,
        ),
        s(
            "stepped kerb",
            1.5,
            Profile::Shape(vec![
                [0.0, 0.0],
                [0.05, 0.05],
                [0.5, 0.05],
                [0.55, 0.1],
                [1.0, 0.1],
            ]),
            "kerb",
            "kerb",
            2.0,
        ),
        s("gravel", 15.0, Profile::Slope(0.3), "gravel", "gravel", 8.0),
        s("run-off", 10.0, Profile::Flat, "runoff", "asphalt", 8.0),
        s("grass", 12.0, Profile::Slope(0.2), "grass", "grass", 0.0),
    ]
}

/// Walls, rails, fences and tyre stacks to put up, from the built-in materials.
pub fn wall_styles() -> Vec<WallStyle> {
    let w = |name: &str, height, thickness, material: &str| WallStyle {
        name: name.into(),
        height,
        thickness,
        material: material.into(),
        model: None,
    };
    vec![
        w("concrete wall", 1.0, 0.5, "concrete"),
        w("guard rail", 0.75, 0.0, "armco"),
        w("tyre wall", 1.0, 1.2, "tyres"),
        w("catch fence", 4.0, 0.0, "fence"),
    ]
}

pub fn materials() -> Vec<MaterialDef> {
    let m = |name: &str, texture, tile: [f32; 2], roughness, double_sided| MaterialDef {
        name: name.into(),
        color: [1.0; 3],
        texture: TextureSource::Builtin(texture),
        tile,
        roughness,
        reflectance: 0.5,
        double_sided,
        normal: TextureSource::None,
        alpha: Alpha::Opaque,
    };
    use BuiltinTexture as T;
    vec![
        m("asphalt", T::Asphalt, [4.0, 4.0], 0.8, false),
        m("kerb", T::Kerb, [1.2, 4.0], 0.6, false),
        m("grass", T::Grass, [6.0, 6.0], 0.95, false),
        m("gravel", T::Gravel, [4.0, 4.0], 0.95, false),
        m("concrete", T::Concrete, [2.0, 4.0], 0.85, false),
        m("armco", T::Armco, [0.8, 4.0], 0.4, true),
        m("paint", T::Paint, [1.0, 1.0], 0.6, false),
        m("dirt", T::Dirt, [5.0, 5.0], 0.95, false),
        MaterialDef {
            alpha: Alpha::Mask(0.5),
            ..m("fence", T::Fence, [2.0, 2.0], 0.5, true)
        },
        m("tyres", T::Tyres, [1.0, 1.0], 0.9, false),
    ]
}

/// Deterministic value noise in [0, 1], tiling every `period` texels.
fn noise(x: usize, y: usize, period: usize, seed: u32) -> f32 {
    let hash = |x: usize, y: usize| {
        let mut h = ((x % period) as u32).wrapping_mul(0x27d4_eb2d)
            ^ ((y % period) as u32).wrapping_mul(0x1656_67b1)
            ^ seed;
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        h = h.wrapping_mul(0x297a_2d39);
        h ^= h >> 15;
        (h & 0xffff) as f32 / 65535.0
    };
    let cell = SIZE / period;
    let (cx, cy) = (x / cell, y / cell);
    let (fx, fy) = (
        (x % cell) as f32 / cell as f32,
        (y % cell) as f32 / cell as f32,
    );
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let a = hash(cx, cy) + (hash(cx + 1, cy) - hash(cx, cy)) * sx;
    let b = hash(cx, cy + 1) + (hash(cx + 1, cy + 1) - hash(cx, cy + 1)) * sx;
    a + (b - a) * sy
}

/// Several octaves of noise, in [0, 1].
fn fbm(x: usize, y: usize, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut weight = 0.5;
    let mut total = 0.0;
    for (i, period) in [4, 8, 16, 32, 64].into_iter().enumerate() {
        sum += weight * noise(x, y, period, seed.wrapping_add(i as u32));
        total += weight;
        weight *= 0.6;
    }
    sum / total
}

/// Generates a built-in texture, sRGB.
pub fn image(texture: BuiltinTexture) -> Image {
    let mut pixels = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let n = fbm(x, y, texture as u32 * 101);
            let grain = noise(x, y, 128, 7 + texture as u32);
            let mut alpha = 1.0;
            let rgb: [f32; 3] = match texture {
                BuiltinTexture::Asphalt => {
                    let v = 0.16 + 0.1 * n + 0.08 * grain;
                    [v, v, v * 1.03]
                }
                BuiltinTexture::Kerb => {
                    // Red and white halves along the texture, as blocks along a kerb.
                    let wear = 0.9 + 0.1 * n;
                    if y < SIZE / 2 {
                        [0.75 * wear, 0.1 * wear, 0.08 * wear]
                    } else {
                        [0.9 * wear, 0.9 * wear, 0.88 * wear]
                    }
                }
                BuiltinTexture::Grass => {
                    let v = 0.7 + 0.3 * n + 0.15 * grain;
                    [0.2 * v, 0.38 * v, 0.12 * v]
                }
                BuiltinTexture::Gravel => {
                    let v = 0.75 + 0.35 * grain + 0.1 * n;
                    [0.62 * v, 0.56 * v, 0.46 * v]
                }
                BuiltinTexture::Concrete => {
                    let v = 0.6 + 0.12 * n + 0.05 * grain;
                    [v, v, v * 0.98]
                }
                BuiltinTexture::Armco => {
                    // Corrugations across the rail.
                    let wave = (std::f32::consts::TAU * 3.0 * x as f32 / SIZE as f32).cos();
                    let v = 0.55 + 0.12 * wave + 0.05 * n;
                    [v, v * 1.01, v * 1.04]
                }
                BuiltinTexture::Paint => {
                    let v = 0.85 + 0.1 * n;
                    [v, v, v]
                }
                BuiltinTexture::Dirt => {
                    let v = 0.7 + 0.3 * n + 0.1 * grain;
                    [0.4 * v, 0.3 * v, 0.2 * v]
                }
                BuiltinTexture::Fence => {
                    // Diagonal wires, eight diamonds across.
                    let cell = (SIZE / 8) as f32;
                    let wire = |a: f32| {
                        let t = (a / cell).fract();
                        t.min(1.0 - t) * cell
                    };
                    let (u, v) = (x as f32 + y as f32, x as f32 + SIZE as f32 - y as f32);
                    let near = wire(u).min(wire(v));
                    alpha = if near < 1.6 { 1.0 } else { 0.0 };
                    let g = 0.6 + 0.1 * n;
                    [g, g * 1.02, g * 1.05]
                }
                BuiltinTexture::Tyres => {
                    // Four tyres high, with treads and a lighter sidewall band.
                    let row = (y * 4 / SIZE) as f32;
                    let t = (y as f32 * 4.0 / SIZE as f32 - row - 0.5).abs() * 2.0;
                    let edge = if t > 0.85 { 0.03 } else { 0.0 };
                    let tread = if (x / 8 + y / 8) % 2 == 0 { 0.012 } else { 0.0 };
                    let v = 0.055 + 0.02 * n + tread - edge;
                    [v, v, v]
                }
            };
            pixels.extend(rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
            pixels.push((alpha * 255.0f32).round() as u8);
        }
    }
    Image {
        width: SIZE,
        height: SIZE,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_tile() {
        let img = image(BuiltinTexture::Grass);
        let px = |x: usize, y: usize| &img.pixels[(y * SIZE + x) * 4..][..3];
        // Opposite edges meet smoothly.
        for i in (0..SIZE).step_by(17) {
            let d = |a: &[u8], b: &[u8]| {
                a.iter()
                    .zip(b)
                    .map(|(x, y)| (*x as i32 - *y as i32).abs())
                    .max()
                    .unwrap()
            };
            assert!(d(px(0, i), px(SIZE - 1, i)) < 40);
            assert!(d(px(i, 0), px(i, SIZE - 1)) < 40);
        }
    }
}
