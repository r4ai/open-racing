//! Pictures of models, drawn on the CPU: a model seen from many sides (an octahedral
//! impostor), to stand in for it far from the camera, and a thumbnail of it for the
//! editor's lists. So a model file needs no far model of its own, and a wood of
//! thousands of trees costs two triangles a tree beyond the detail distance.
//!
//! The impostor is a quad the renderer turns to the viewer, showing the picture drawn
//! from the side nearest the viewer's, of `IMPOSTOR_FRAMES` × `IMPOSTOR_FRAMES` over
//! the half of a sphere above the model. The pictures keep the model's colours and
//! textures, a little darker below where leaves shade each other, and its normals, by
//! which the renderer lights them. A plant's leaves and the rest of it are pictures of
//! their own, in the same places, so that its copies' leaves take their own colours and
//! fall far away too.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Vec2, Vec3};
use open_racing_track::texture::{self, Image, Mips};
use open_racing_track::{
    AlphaMode, IMPOSTOR_FRAMES, Material, Mesh, Texture, Varies, VisualBuilder,
};

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
                leaves: m.varies.is_some_and(|p| p.tinted),
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

/// Linear colour and coverage of each pixel, the normal of what covers it (turned to
/// the viewer), and whether leaves cover it.
#[derive(Clone)]
struct Canvas {
    width: usize,
    height: usize,
    colour: Vec<[f32; 4]>,
    normal: Vec<Vec3>,
    depth: Vec<f32>,
    leaves: Vec<bool>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            colour: vec![[0.0; 4]; width * height],
            normal: vec![Vec3::ZERO; width * height],
            depth: vec![f32::NEG_INFINITY; width * height],
            leaves: vec![false; width * height],
        }
    }

    /// Only the pixels leaves cover (`leaves`), or only those they do not.
    fn only(&self, leaves: bool) -> Canvas {
        let mut out = self.clone();
        for (c, l) in out.colour.iter_mut().zip(&self.leaves) {
            if *l != leaves {
                *c = [0.0; 4];
            }
        }
        out
    }

    /// The picture halved, as two: of the rest of the plant and of its leaves. The
    /// cards of both lie in the same places, so no texel may show in both (they would
    /// fight over it) and together they show what the one picture would: where both
    /// would show, only the leaves do, and a texel half covered by each goes to the one
    /// covering more of it, or to the leaves. Their coverage is kept well clear of the
    /// cut-off, which block compression blurs.
    fn split(&self) -> [Canvas; 2] {
        let [mut wood, mut leaves] = [false, true].map(|l| self.only(l).halved());
        for (w, l) in wood.colour.iter_mut().zip(&mut leaves.colour) {
            let (a, b) = (w[3], l[3]);
            if b >= 0.5 {
                w[3] = w[3].min(0.3);
            } else if a < 0.5 && a + b >= 0.5 {
                if b >= a {
                    l[3] = 0.7;
                } else {
                    w[3] = 0.7;
                }
            }
        }
        [wood, leaves]
    }

    /// Draws the model onto the region of the canvas from pixel `at`, `size` pixels.
    fn draw(
        &mut self,
        model: &Model,
        paints: &[Paint],
        view: &View,
        at: [usize; 2],
        size: [usize; 2],
    ) {
        let [x0, y0] = at;
        let [w, h] = size;
        let span = view.hi - view.lo;
        let to_px = |p: Vec3| {
            let q = Vec2::new(p.dot(view.right), p.dot(view.up));
            Vec2::new(
                x0 as f32 + (q.x - view.lo.x) / span.x * w as f32,
                y0 as f32 + (view.hi.y - q.y) / span.y * h as f32,
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
                    .max(Vec2::new(x0 as f32, y0 as f32));
                let hi = s[0]
                    .max(s[1])
                    .max(s[2])
                    .ceil()
                    .min(Vec2::new((x0 + w) as f32, (y0 + h) as f32));
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
                        self.normal[k] = n;
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
                let mut normal = Vec3::ZERO;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let k = (2 * y + dy) * self.width + 2 * x + dx;
                    let c = self.colour[k];
                    for i in 0..3 {
                        sum[i] += c[i] * c[3];
                    }
                    sum[3] += c[3];
                    normal += self.normal[k] * c[3];
                }
                out.normal[y * w + x] = normal.normalize_or_zero();
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

/// Edge of each picture of an impostor, texels.
const FRAME: usize = 64;

/// The axes of picture (i, j) of an impostor: across it and up it, and towards the side
/// it is drawn from. See `open_racing_track::IMPOSTOR_FRAMES`, and `impostor` in
/// `track_plant.wgsl`, which must agree.
fn frame_axes(i: usize, j: usize) -> [Vec3; 3] {
    let n = IMPOSTOR_FRAMES as f32;
    let h = Vec2::new(i as f32 + 0.5, j as f32 + 0.5) / n;
    let o = Vec2::new(h.x + h.y - 1.0, h.x - h.y);
    let side = Vec3::new(o.x, o.y, (1.0 - o.x.abs() - o.y.abs()).max(0.0)).normalize();
    let right = Vec3::new(-side.y, side.x, 0.0)
        .try_normalize()
        .unwrap_or(Vec3::X);
    let up = side.cross(right);
    [right, up, side]
}

/// The middle of the box round the model's positions, and the radius of the sphere
/// round them about it.
fn sphere(model: &Model) -> (Vec3, f32) {
    let (mut lo, mut hi) = (Vec3::MAX, Vec3::MIN);
    for p in model.meshes.iter().flat_map(|m| &m.positions) {
        lo = lo.min(Vec3::from(*p));
        hi = hi.max(Vec3::from(*p));
    }
    let middle = 0.5 * (lo + hi);
    let radius = model
        .meshes
        .iter()
        .flat_map(|m| &m.positions)
        .map(|p| (Vec3::from(*p) - middle).length())
        .fold(0.05, f32::max);
    (middle, radius)
}

/// An octahedral impostor of a model, as a model of its own that stands in for it: a
/// quad whose corners lie at the model's middle (see `Material::impostor`), with the
/// pictures of it from each side and their normals. A plant's leaves and the rest of it
/// are pictures of their own, in the same places, which cover none of the same pixels.
pub fn impostor(model: &Model, paints: &[Paint]) -> Model {
    let (middle, radius) = sphere(model);
    let frames = IMPOSTOR_FRAMES as usize;
    let edge = frames * FRAME;
    // Drawn at twice the size and halved, for smooth edges.
    let mut canvas = Canvas::new(2 * edge, 2 * edge);
    for j in 0..frames {
        for i in 0..frames {
            let [right, up, side] = frame_axes(i, j);
            let c = Vec2::new(middle.dot(right), middle.dot(up));
            let view = View {
                right,
                up,
                toward: side,
                lo: c - radius,
                hi: c + radius,
                light: Vec3::Z,
                diffuse: 0.0,
            };
            let at = [2 * i * FRAME, 2 * j * FRAME];
            canvas.draw(model, paints, &view, at, [2 * FRAME; 2]);
        }
    }
    let normals = normal_map(&canvas.halved());
    let used = |leaves: bool| {
        canvas
            .leaves
            .iter()
            .zip(&canvas.colour)
            .any(|(l, c)| *l == leaves && c[3] > 0.0)
    };
    let layers: Vec<bool> = [false, true].into_iter().filter(|&l| used(l)).collect();
    let mut look = VisualBuilder::new();
    let normal_texture = look.add_texture(Texture {
        data: texture::encode(normals),
    });
    let mut pictures = if layers.len() > 1 {
        canvas.split().to_vec()
    } else {
        vec![canvas.halved()]
    };
    let mut meshes = Vec::new();
    for (i, leaves) in layers.iter().copied().enumerate() {
        let picture = &mut pictures[i];
        picture.bleed(8);
        let texture = look.add_texture(Texture {
            data: texture::encode_with(picture.image(), Mips::AlphaTest(0.5)),
        });
        look.add_material(Material {
            base_color: [1.0; 4],
            base_color_texture: Some(texture),
            normal_texture: Some(normal_texture),
            roughness: 0.9,
            reflectance: 0.3,
            alpha_mode: AlphaMode::Mask(0.5),
            double_sided: true,
            varies: leaves.then_some(Varies {
                tinted: true,
                ..Default::default()
            }),
            impostor: true,
            ..Default::default()
        });
        meshes.push(Mesh {
            material: i as u32,
            cast_shadows: true,
            positions: vec![middle.to_array(); 4],
            normals: vec![[radius, 0.0, 0.0]; 4],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            indices: vec![0, 2, 1, 0, 3, 2],
        });
    }
    Model {
        triangles: meshes.iter().map(|m| m.indices.len() / 3).sum(),
        bounds: [middle - radius, middle + radius],
        meshes,
        lods: vec![],
        look: look.build(),
    }
}

/// The impostor's normals as a tangent-space normal map: each picture's in the frame
/// the renderer gives its quad (T = B × N, B down the picture: see
/// `open_racing_track::Material::normal_texture`), facing the viewer where nothing is.
fn normal_map(canvas: &Canvas) -> Image {
    let frame = canvas.width / IMPOSTOR_FRAMES as usize;
    let mut pixels = Vec::with_capacity(canvas.width * canvas.height * 4);
    for y in 0..canvas.height {
        for x in 0..canvas.width {
            let [right, up, side] = frame_axes(x / frame, y / frame);
            let n = canvas.normal[y * canvas.width + x];
            let t = if n == Vec3::ZERO {
                Vec3::Z
            } else {
                Vec3::new(-n.dot(right), -n.dot(up), n.dot(side).max(0.05)).normalize()
            };
            let byte = |v: f32| ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
            pixels.extend([byte(t.x), byte(t.y), byte(t.z), 255]);
        }
    }
    Image {
        width: canvas.width,
        height: canvas.height,
        pixels,
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
    canvas.draw(model, paints, &view, [0, 0], [2 * size; 2]);
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
    fn a_tree_s_impostor_shows_it_from_every_side() {
        let pine = crate::shapes::model("pine").unwrap();
        let mut paint = paints(&pine);
        paint.iter_mut().for_each(|p| p.leaves = false);
        let far = impostor(&pine, &paint);
        assert_eq!(far.triangles, 2);
        assert!(far.look.materials[0].impostor);
        // Its colour and its normals.
        assert_eq!(far.look.textures.len(), 2);
        let colour = far.look.materials[0].base_color_texture.unwrap() as usize;
        let image = texture::decode(&far.look.textures[colour].data).unwrap();
        let frames = IMPOSTOR_FRAMES as usize;
        assert_eq!(image.width, frames * FRAME);
        // Each picture: covered in its middle (the trunk and the needles), clear in its
        // corners.
        let alpha = |x: usize, y: usize| image.pixels[(y * image.width + x) * 4 + 3];
        for j in 0..frames {
            for i in 0..frames {
                let (x, y) = (i * FRAME, j * FRAME);
                assert!(
                    alpha(x + FRAME / 2, y + FRAME / 2) > 200,
                    "picture {i}, {j}"
                );
                assert!(alpha(x + 1, y + 1) < 30, "picture {i}, {j}");
            }
        }
        // The quad's corners at the middle, its normal as long as the sphere's radius.
        let (middle, radius) = sphere(&pine);
        let quad = &far.meshes[0];
        assert!(quad.positions.iter().all(|p| Vec3::from(*p) == middle));
        assert!((Vec3::from(quad.normals[0]).length() - radius).abs() < 1e-5);
        assert!(radius >= 0.5 * (pine.bounds[1].z - pine.bounds[0].z));
    }

    #[test]
    fn the_pictures_look_from_above_the_model() {
        let n = IMPOSTOR_FRAMES as usize;
        for j in 0..n {
            for i in 0..n {
                let [right, up, side] = frame_axes(i, j);
                assert!(side.z >= 0.0 && (side.length() - 1.0).abs() < 1e-5);
                assert!(right.cross(up).abs_diff_eq(side, 1e-5));
            }
        }
        // The middle pictures look down from overhead, the corners from the horizon.
        assert!(frame_axes(n / 2, n / 2)[2].z > 0.8);
        assert!(frame_axes(0, 0)[2].z < 0.3);
    }

    #[test]
    fn a_plant_s_leaves_and_trunk_are_pictures_of_their_own() {
        let tree = crate::shapes::model("tree").unwrap();
        let far = impostor(&tree, &paints(&tree));
        assert_eq!(far.meshes.len(), 2);
        assert_eq!(far.meshes[0].positions, far.meshes[1].positions);
        let [trunk, leaves] = [0, 1].map(|i| &far.look.materials[i]);
        assert!(trunk.varies.is_none() && leaves.varies.is_some_and(|p| p.tinted));
        // No texel is covered in both pictures.
        let [a, b] = [trunk, leaves].map(|m| {
            let t = m.base_color_texture.unwrap() as usize;
            texture::decode(&far.look.textures[t].data).unwrap()
        });
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
