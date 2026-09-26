//! Pictures of models, drawn on the CPU: a model seen from three sides on crossed cards
//! (an impostor), to stand in for it far from the camera, and a thumbnail of it for the
//! editor's lists. So a model file needs no far model of its own, and a wood of
//! thousands of trees costs a few triangles a tree beyond the detail distance.
//!
//! The pictures keep the model's colours and textures with a little shading baked in
//! (darker below, where leaves shade each other); the renderer lights the cards again.
//! A plant's leaves and the rest of it are drawn on cards of their own, in the same
//! places, so that its copies' leaves take their own colours and fall far away too.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Vec2, Vec3};
use open_racing_track::texture::{self, Image, Mips};
use open_racing_track::{AlphaMode, Material, Mesh, PlantLook, Texture, VisualBuilder};

use crate::model::Model;

/// How one of a model's materials looks: its colour (linear, with alpha), its
/// texture (sRGB), the alpha below which it is cut out, and whether it is a plant's
/// leaves.
#[derive(Clone, Debug)]
pub struct Paint {
    pub colour: [f32; 4],
    pub texture: Option<Arc<Image>>,
    pub cutoff: Option<f32>,
    pub leaves: bool,
}

/// How a model's own materials look.
pub fn paints(model: &Model) -> Vec<Paint> {
    let mut decoded: HashMap<u32, Option<Arc<Image>>> = HashMap::new();
    model
        .look
        .materials
        .iter()
        .map(|m| {
            let texture = m.base_color_texture.and_then(|t| {
                decoded
                    .entry(t)
                    .or_insert_with(|| {
                        let data = &model.look.textures.get(t as usize)?.data;
                        texture::decode(data).ok().map(Arc::new)
                    })
                    .clone()
            });
            Paint {
                colour: m.base_color,
                texture,
                cutoff: match m.alpha_mode {
                    AlphaMode::Opaque => None,
                    AlphaMode::Mask(c) => Some(c),
                    AlphaMode::Blend => Some(0.5),
                },
                leaves: m.plant.is_some_and(|p| p.leaves),
            }
        })
        .collect()
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// A texture's texel at `uv`, repeating, as linear colour and alpha.
fn sample(image: &Image, uv: [f32; 2]) -> [f32; 4] {
    let (w, h) = (image.width.max(1), image.height.max(1));
    let x = ((uv[0].rem_euclid(1.0) * w as f32) as usize).min(w - 1);
    let y = ((uv[1].rem_euclid(1.0) * h as f32) as usize).min(h - 1);
    let p = &image.pixels[(y * w + x) * 4..][..4];
    [
        srgb_to_linear(p[0] as f32 / 255.0),
        srgb_to_linear(p[1] as f32 / 255.0),
        srgb_to_linear(p[2] as f32 / 255.0),
        p[3] as f32 / 255.0,
    ]
}

/// Where a picture looks from: its axes across and up the picture, and towards the
/// viewer; the part of the plane it shows; and how it is shaded.
struct View {
    right: Vec3,
    up: Vec3,
    toward: Vec3,
    /// Lowest and highest `right` and `up` coordinates shown, m.
    lo: Vec2,
    hi: Vec2,
    /// Shading: ambient plus this much of the light from `light`, and how much darker
    /// what faces down is.
    light: Vec3,
    diffuse: f32,
}

/// Linear colour and coverage of each pixel, and whether leaves cover it.
#[derive(Clone)]
struct Canvas {
    width: usize,
    height: usize,
    colour: Vec<[f32; 4]>,
    depth: Vec<f32>,
    leaves: Vec<bool>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            colour: vec![[0.0; 4]; width * height],
            depth: vec![f32::NEG_INFINITY; width * height],
            leaves: vec![false; width * height],
        }
    }

    /// Only the pixels leaves cover (`leaves`), or only those they do not and that lie
    /// two pixels or more from leaves: where the two meet, halved, the leaves' picture
    /// covers the texels and the other's does not, as the cards of both lie in the same
    /// places.
    fn only(&self, leaves: bool) -> Canvas {
        let mut out = self.clone();
        let (w, h) = (self.width as i64, self.height as i64);
        let near_leaves = |k: usize| {
            let (x, y) = ((k % self.width) as i64, (k / self.width) as i64);
            (-2..=2).any(|dy| {
                (-2..=2).any(|dx| {
                    let (u, v) = (x + dx, y + dy);
                    u >= 0 && v >= 0 && u < w && v < h && self.leaves[(v * w + u) as usize]
                })
            })
        };
        for (k, c) in out.colour.iter_mut().enumerate() {
            let keep = if leaves {
                self.leaves[k]
            } else {
                !self.leaves[k] && !near_leaves(k)
            };
            if !keep {
                *c = [0.0; 4];
            }
        }
        out
    }

    /// Draws the model onto a region of the canvas `x0` pixels in and `w` wide.
    fn draw(&mut self, model: &Model, paints: &[Paint], view: &View, x0: usize, w: usize) {
        let h = self.height;
        let size = view.hi - view.lo;
        let to_px = |p: Vec3| {
            let q = Vec2::new(p.dot(view.right), p.dot(view.up));
            Vec2::new(
                x0 as f32 + (q.x - view.lo.x) / size.x * w as f32,
                (view.hi.y - q.y) / size.y * h as f32,
            )
        };
        for mesh in &model.meshes {
            let Some(paint) = paints.get(mesh.material as usize) else {
                continue;
            };
            for t in mesh.indices.as_chunks::<3>().0 {
                let [a, b, c] = t.map(|i| i as usize);
                let p = [a, b, c].map(|i| Vec3::from(mesh.positions[i]));
                let s = p.map(to_px);
                let depth = p.map(|q| q.dot(view.toward));
                let area = (s[1] - s[0]).perp_dot(s[2] - s[0]);
                if area.abs() < 1e-9 {
                    continue;
                }
                let lo = s[0]
                    .min(s[1])
                    .min(s[2])
                    .floor()
                    .max(Vec2::new(x0 as f32, 0.0));
                let hi = s[0]
                    .max(s[1])
                    .max(s[2])
                    .ceil()
                    .min(Vec2::new((x0 + w) as f32, h as f32));
                for y in lo.y as usize..hi.y as usize {
                    for x in lo.x as usize..hi.x as usize {
                        let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                        let wa = (s[2] - s[1]).perp_dot(q - s[1]) / area;
                        let wb = (s[0] - s[2]).perp_dot(q - s[2]) / area;
                        let wc = 1.0 - wa - wb;
                        if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                            continue;
                        }
                        let k = y * self.width + x;
                        let d = wa * depth[0] + wb * depth[1] + wc * depth[2];
                        if d <= self.depth[k] {
                            continue;
                        }
                        let mut rgba = paint.colour;
                        if let Some(tex) = &paint.texture {
                            let uv = [a, b, c].map(|i| Vec2::from(mesh.uvs[i]));
                            let uv = uv[0] * wa + uv[1] * wb + uv[2] * wc;
                            let texel = sample(tex, uv.to_array());
                            for i in 0..4 {
                                rgba[i] *= texel[i];
                            }
                        }
                        if rgba[3] < paint.cutoff.unwrap_or(0.0).max(1e-3) {
                            continue;
                        }
                        let n = [a, b, c].map(|i| Vec3::from(mesh.normals[i]));
                        let mut n = (n[0] * wa + n[1] * wb + n[2] * wc).normalize_or(Vec3::Z);
                        // The back of a leaf card faces the viewer too.
                        if n.dot(view.toward) < 0.0 {
                            n = -n;
                        }
                        let shade = (1.0 - view.diffuse) * (0.75 + 0.25 * n.z)
                            + view.diffuse * n.dot(view.light).max(0.0);
                        self.depth[k] = d;
                        self.leaves[k] = paint.leaves;
                        self.colour[k] = [rgba[0] * shade, rgba[1] * shade, rgba[2] * shade, 1.0];
                    }
                }
            }
        }
    }

    /// Halved in each direction, averaging each 2 × 2 pixels: coverage becomes alpha
    /// and the colour is that of what was covered.
    fn halved(&self) -> Canvas {
        let (w, h) = (self.width / 2, self.height / 2);
        let mut out = Canvas::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let c = self.colour[(2 * y + dy) * self.width + 2 * x + dx];
                    for i in 0..3 {
                        sum[i] += c[i] * c[3];
                    }
                    sum[3] += c[3];
                }
                let a = sum[3];
                out.colour[y * w + x] = if a > 0.0 {
                    [sum[0] / a, sum[1] / a, sum[2] / a, a / 4.0]
                } else {
                    [0.0; 4]
                };
            }
        }
        out
    }

    /// Spreads the colours of covered pixels into the empty ones round them, so that
    /// filtering and smaller mip levels do not darken the edges.
    fn bleed(&mut self, passes: usize) {
        let (w, h) = (self.width, self.height);
        for _ in 0..passes {
            let before = self.colour.clone();
            let mut any = false;
            for y in 0..h {
                for x in 0..w {
                    let k = y * w + x;
                    if before[k][3] > 0.0 {
                        continue;
                    }
                    let mut sum = [0.0f32; 3];
                    let mut n = 0.0;
                    for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        let c = before[ny as usize * w + nx as usize];
                        if c[3] > 0.0 || c[0] + c[1] + c[2] > 0.0 {
                            for i in 0..3 {
                                sum[i] += c[i];
                            }
                            n += 1.0;
                        }
                    }
                    if n > 0.0 && self.colour[k][0] + self.colour[k][1] + self.colour[k][2] == 0.0 {
                        self.colour[k] = [sum[0] / n, sum[1] / n, sum[2] / n, 0.0];
                        any = true;
                    }
                }
            }
            if !any {
                break;
            }
        }
    }

    fn image(&self) -> Image {
        let pixels = self
            .colour
            .iter()
            .flat_map(|c| {
                [
                    linear_to_srgb(c[0]),
                    linear_to_srgb(c[1]),
                    linear_to_srgb(c[2]),
                    c[3],
                ]
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            })
            .collect();
        Image {
            width: self.width,
            height: self.height,
            pixels,
        }
    }
}

/// How many cards an impostor has, crossing at the model's upright axis.
pub const CARDS: usize = 3;
/// Height of each card's picture, texels.
const CARD_HEIGHT: usize = 256;

/// The corners of the box round the model's positions, and how far it reaches from
/// its upright axis.
fn extent(model: &Model) -> (Vec3, Vec3, f32) {
    let mut radius = 0.0f32;
    let (mut lo, mut hi) = (Vec3::MAX, Vec3::MIN);
    for p in model.meshes.iter().flat_map(|m| &m.positions) {
        let p = Vec3::from(*p);
        lo = lo.min(p);
        hi = hi.max(p);
        radius = radius.max(p.truncate().length());
    }
    (lo, hi, radius.max(0.05))
}

/// The pictures of a model on `CARDS` crossed upright cards, as a model of its own
/// that stands in for it: each card shows the model as seen square to it, on both of
/// its faces. A plant's leaves and the rest of it are on cards of their own, in the
/// same places, whose pictures cover none of the same pixels.
pub fn cards(model: &Model, paints: &[Paint]) -> Model {
    let (lo, hi, radius) = extent(model);
    let height = (hi.z - lo.z).max(0.05);
    let cell_h = CARD_HEIGHT;
    let cell_w = ((cell_h as f32 * 2.0 * radius / height).round() as usize).clamp(16, 256) / 4 * 4;
    let atlas_w = cell_w * CARDS;
    // Drawn at twice the size and halved, for smooth edges.
    let mut canvas = Canvas::new(2 * atlas_w, 2 * cell_h);
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for i in 0..CARDS {
        let a = i as f32 / CARDS as f32 * std::f32::consts::PI;
        let across = Vec3::new(a.cos(), a.sin(), 0.0);
        let toward = Vec3::new(-a.sin(), a.cos(), 0.0);
        let view = View {
            right: across,
            up: Vec3::Z,
            toward,
            lo: Vec2::new(-radius, lo.z),
            hi: Vec2::new(radius, hi.z),
            light: (Vec3::Z * 0.8 + toward * 0.6).normalize(),
            diffuse: 0.25,
        };
        canvas.draw(model, paints, &view, 2 * i * cell_w, 2 * cell_w);
        // Both faces of the card, their normals leaning up, as the canopy's are on
        // the whole.
        for side in [1.0f32, -1.0] {
            let base = positions.len() as u32;
            let n = (toward * side * 0.4 + Vec3::Z * 0.9).normalize();
            for (u, z) in [(-1.0, lo.z), (1.0, lo.z), (1.0, hi.z), (-1.0, hi.z)] {
                positions.push((across * u * radius + Vec3::Z * z).to_array());
                normals.push(n.to_array());
                uvs.push([
                    (i as f32 + 0.5 * (u + 1.0)) / CARDS as f32,
                    (hi.z - z) / height,
                ]);
            }
            let quad = if side > 0.0 {
                [0, 1, 2, 0, 2, 3]
            } else {
                [0, 2, 1, 0, 3, 2]
            };
            indices.extend(quad.map(|k| base + k));
        }
    }
    let used = |leaves: bool| canvas.leaves.iter().zip(&canvas.colour).any(|(l, c)| *l == leaves && c[3] > 0.0);
    let layers: Vec<bool> = [false, true].into_iter().filter(|&l| used(l)).collect();
    let mut look = VisualBuilder::new();
    let mut meshes = Vec::new();
    for (i, leaves) in layers.iter().copied().enumerate() {
        let mut picture = if layers.len() > 1 {
            canvas.only(leaves)
        } else {
            canvas.clone()
        }
        .halved();
        picture.bleed(16);
        let texture = look.add_texture(Texture {
            data: texture::encode_with(picture.image(), Mips::AlphaTest(0.5)),
        });
        look.add_material(Material {
            base_color: [1.0; 4],
            base_color_texture: Some(texture),
            roughness: 0.9,
            reflectance: 0.3,
            alpha_mode: AlphaMode::Mask(0.5),
            plant: leaves.then_some(PlantLook {
                leaves: true,
                ..Default::default()
            }),
            ..Default::default()
        });
        meshes.push(Mesh {
            material: i as u32,
            cast_shadows: true,
            positions: positions.clone(),
            normals: normals.clone(),
            uvs: uvs.clone(),
            indices: indices.clone(),
        });
    }
    Model {
        triangles: meshes.iter().map(|m| m.indices.len() / 3).sum(),
        bounds: [
            Vec3::new(-radius, -radius, lo.z),
            Vec3::new(radius, radius, hi.z),
        ],
        meshes,
        look: look.build(),
    }
}

/// A square picture of the model seen from above one side, `size` pixels, on a clear
/// background.
pub fn thumbnail(model: &Model, paints: &[Paint], size: usize) -> Image {
    let (yaw, pitch) = (-0.6f32, 0.35f32);
    let toward = Vec3::new(
        yaw.sin() * pitch.cos(),
        -yaw.cos() * pitch.cos(),
        pitch.sin(),
    );
    let right = Vec3::Z.cross(toward).normalize();
    let up = toward.cross(right);
    let (mut lo, mut hi) = (Vec2::MAX, Vec2::MIN);
    for p in model.meshes.iter().flat_map(|m| &m.positions) {
        let q = Vec2::new(Vec3::from(*p).dot(right), Vec3::from(*p).dot(up));
        lo = lo.min(q);
        hi = hi.max(q);
    }
    // Square, with a margin.
    let middle = 0.5 * (lo + hi);
    let half = 0.5 * (hi - lo).max_element().max(0.01) * 1.08;
    let view = View {
        right,
        up,
        toward,
        lo: middle - half,
        hi: middle + half,
        light: Vec3::new(-0.4, -0.6, 0.7).normalize(),
        diffuse: 0.55,
    };
    let mut canvas = Canvas::new(2 * size, 2 * size);
    canvas.draw(model, paints, &view, 0, 2 * size);
    let mut picture = canvas.halved();
    // Brightened a little: the models' colours are dark as leaves are.
    for c in &mut picture.colour {
        for v in &mut c[..3] {
            *v = (*v * 2.2).min(1.0);
        }
    }
    picture.image()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_s_cards_show_it_from_three_sides() {
        let pine = crate::shapes::model("pine").unwrap();
        let mut paint = paints(&pine);
        paint.iter_mut().for_each(|p| p.leaves = false);
        let far = cards(&pine, &paint);
        assert_eq!(far.triangles, CARDS * 4);
        assert_eq!(far.look.textures.len(), 1);
        let image = texture::decode(&far.look.textures[0].data).unwrap();
        assert_eq!(image.width, (image.width / 4) * 4);
        // Each card's picture: covered along its middle (the trunk and the needles),
        // clear in its top corners.
        let cell = image.width / CARDS;
        let alpha = |x: usize, y: usize| image.pixels[(y * image.width + x) * 4 + 3];
        for i in 0..CARDS {
            let middle = i * cell + cell / 2;
            assert!(alpha(middle, image.height * 3 / 4) > 200, "card {i}");
            assert!(alpha(middle, image.height / 2) > 200, "card {i}");
            assert!(alpha(i * cell + 1, 1) < 30, "card {i}");
        }
        // As tall and as wide as the tree.
        assert!((far.bounds[1].z - pine.bounds[1].z).abs() < 1e-3);
    }

    #[test]
    fn a_plant_s_leaves_and_trunk_are_on_cards_of_their_own() {
        let tree = crate::shapes::model("tree").unwrap();
        let far = cards(&tree, &paints(&tree));
        assert_eq!(far.meshes.len(), 2);
        assert_eq!(far.meshes[0].positions, far.meshes[1].positions);
        let [trunk, leaves] = [0, 1].map(|i| &far.look.materials[i]);
        assert!(trunk.plant.is_none() && leaves.plant.is_some_and(|p| p.leaves));
        // No texel is covered in both pictures.
        let [a, b] = [0, 1].map(|i| texture::decode(&far.look.textures[i].data).unwrap());
        let alpha = |im: &Image, k: usize| im.pixels[k * 4 + 3];
        let n = a.width * a.height;
        assert!((0..n).all(|k| alpha(&a, k) < 128 || alpha(&b, k) < 128));
        assert!((0..n).any(|k| alpha(&a, k) > 200) && (0..n).any(|k| alpha(&b, k) > 200));
    }

    #[test]
    fn a_thumbnail_is_the_model_on_a_clear_background() {
        let rock = crate::shapes::model("rock").unwrap();
        let t = thumbnail(&rock, &paints(&rock), 48);
        assert_eq!((t.width, t.height), (48, 48));
        let alpha = |x: usize, y: usize| t.pixels[(y * 48 + x) * 4 + 3];
        assert!(alpha(24, 24) == 255);
        assert_eq!(alpha(0, 0), 0);
    }
}
