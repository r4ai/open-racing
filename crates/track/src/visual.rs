//! `visual.bin`: what the app renders. Only the app reads it.
//!
//! Meshes are batched by material and by a coarse XY tile when the package is written:
//! few enough draw calls for a whole circuit, while each batch stays small enough to be
//! frustum-culled. Models repeated many times (woods, bushes, rocks) are kept once, as
//! shapes in their own frame, with a list of where each copy stands: the renderer draws
//! the copies of a shape by instancing, so neither the file nor the memory grows with
//! the number of copies.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::Error;
use crate::bin::{Reader, Writer};

const MAGIC: &[u8; 4] = b"ORVS";
const VERSION: u32 = 7;
/// Edge of the XY tiles meshes are batched by, in m.
const BATCH_TILE: f32 = 250.0;
/// Stored for an absent texture index.
const NONE: u32 = u32::MAX;

/// A block-compressed DDS file (see `texture::prepare`), which goes to the GPU without
/// decoding.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    Opaque,
    /// Alpha-tested with this cut-off.
    Mask(f32),
    Blend,
}

/// A metallic-roughness material without metal (dielectric), as in glTF with Filament's
/// `reflectance` for the specular strength.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Linear RGBA factor, multiplied with the texture.
    pub base_color: [f32; 4],
    /// Index into `Visual::textures`; sRGB.
    pub base_color_texture: Option<u32>,
    /// Perceptual roughness, multiplied with the surface texture's G.
    pub roughness: f32,
    /// Specular reflectance at normal incidence F0 = 0.16 · reflectance², with reflectance
    /// multiplied with the surface texture's A.
    pub reflectance: f32,
    /// Scale of the specular reflection of the surroundings (sky, environment maps), with
    /// the surface texture's R: 1 is physically based, 0 leaves only the highlights of
    /// lights. Games may reflect their surroundings on some surfaces only.
    pub reflection: f32,
    /// Index into `Visual::textures`; linear data whose R, G and A scale `reflection`,
    /// `roughness` and `reflectance`.
    pub surface_texture: Option<u32>,
    /// Index into `Visual::textures`; a linear tangent-space normal map. The shading normal
    /// is normalize(x·T + y·B + z·N) with (x, y, z) = 2·rgb − 1, N the vertex normal, B the
    /// direction in which V increases (down the image) along the surface and T = B × N.
    /// The frame follows from the UVs, so meshes need no tangents.
    pub normal_texture: Option<u32>,
    pub alpha_mode: AlphaMode,
    /// Render back faces too.
    pub double_sided: bool,
    pub detail: Option<Detail>,
    /// A plant's: it moves in the wind, and its leaves take each copy's colour.
    pub plant: Option<PlantLook>,
}

/// How a plant's material moves in the wind and takes its copies' looks. The wind
/// comes from the game's weather; a copy's leaf colour is its `Instance::leaves`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlantLook {
    /// How far it bends away from a 10 m/s wind at 1 m above the copy's foot, m; it
    /// bends with the square of the height, and with the square of the wind.
    pub sway: f32,
    /// How far its leaves flutter in a 10 m/s wind, m.
    pub flutter: f32,
    /// It is leaves: they take each copy's leaf colour, and none are drawn of a bare
    /// copy.
    pub leaves: bool,
}

impl Default for Material {
    /// Plain grey, fairly rough.
    fn default() -> Self {
        Self {
            base_color: [0.5, 0.5, 0.5, 1.0],
            base_color_texture: None,
            roughness: 0.85,
            reflectance: 0.5,
            reflection: 1.0,
            surface_texture: None,
            normal_texture: None,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
            detail: None,
            plant: None,
        }
    }
}

/// Tiled detail textures blended by a mask and multiplied into the base colour (splat
/// mapping), for large surfaces such as roads and grass:
///
/// colour = base × multiplier × Σᵢ maskᵢ · layerᵢ(scaleᵢ · uv)
///
/// in linear space. The mask is sampled at the mesh UV; `uv` is the mesh UV, or the world
/// position's (x, y) in metres when `world_uv` is set.
#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub mask: DetailMask,
    pub layers: [Option<DetailLayer>; 4],
    pub multiplier: f32,
    pub world_uv: bool,
    /// A tiled normal map weighed like the R layer, which adds its bumps to the surface's:
    /// the tangent-space normal's (x, y) gains `strength` · weight · (x, y) of this map.
    pub normal: Option<DetailNormal>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailNormal {
    /// Index into `Visual::textures`; a linear tangent-space normal map, in the frame of
    /// `Material::normal_texture`.
    pub texture: u32,
    /// Repetitions per UV unit (or per metre with `world_uv`).
    pub scale: f32,
    pub strength: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DetailMask {
    /// Index into `Visual::textures`; linear data whose R, G, B, A weigh the layers.
    Texture(u32),
    /// The base colour's alpha keeps the base colour as it is, and its complement
    /// weighs the R layer, the only one used:
    ///
    /// colour = base × multiplier × lerp(layer_R(scale · uv), 1, base alpha)
    ///
    /// A pattern such as carbon weave or fabric then shows where an otherwise painted
    /// texture leaves it transparent. The surface is opaque.
    BaseAlpha,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailLayer {
    /// Index into `Visual::textures`; sRGB.
    pub texture: u32,
    /// Repetitions per UV unit (or per metre with `world_uv`).
    pub scale: f32,
}

/// Triangles with one material, in world coordinates (m, Z up). Front faces wind
/// counter-clockwise.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// Index into `Visual::materials`.
    pub material: u32,
    pub cast_shadows: bool,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Meshes in a model's own frame (m, Z up, its foot at the origin), drawn wherever
/// `Instances` put copies of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub meshes: Vec<Mesh>,
}

/// Copies of a model: the shape of each of its levels of detail, nearest first, and
/// where each copy stands. Each copy shows the levels whose distances from the camera
/// it is at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instances {
    pub levels: Vec<Level>,
    pub copies: Vec<Instance>,
}

/// A level of detail of copies: its shape, drawn from `fade_in` to `fade_out` from the
/// camera to each copy, fading in and out over those ranges, m. One level's `fade_out`
/// is the next one's `fade_in`, so that they cross-fade.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level {
    /// Index into `Visual::shapes`.
    pub shape: u32,
    pub fade_in: [f32; 2],
    pub fade_out: [f32; 2],
}

/// A distance standing for "however far", m.
pub const FAR_AWAY: f32 = 1e9;

/// Where a copy stands: the shape's points `p` go to `pos + rotation · (scale · p)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Instance {
    pub pos: [f32; 3],
    /// A unit quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    pub scale: f32,
    /// Its leaves' look (for materials of plants' leaves): an sRGB colour and how much
    /// of it (0 to 254 for 0 to 1) is mixed into their own, keeping their brightness
    /// as the colour's is to a luminance of 0.2; or `BARE` (255) for none at all.
    pub leaves: [u8; 4],
}

/// `Instance::leaves` of a copy with its leaves fallen.
pub const BARE: [u8; 4] = [0, 0, 0, 255];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Visual {
    pub textures: Vec<Texture>,
    pub materials: Vec<Material>,
    pub meshes: Vec<Mesh>,
    pub shapes: Vec<Shape>,
    pub instances: Vec<Instances>,
}

/// Collects textures, materials and meshes, removing duplicate textures and batching
/// meshes.
#[derive(Default)]
pub struct VisualBuilder {
    visual: Visual,
    textures: HashMap<u64, Vec<u32>>,
    batches: HashMap<(u32, bool, i32, i32), usize>,
}

impl VisualBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the index of the texture, reusing an identical one already added.
    pub fn add_texture(&mut self, texture: Texture) -> u32 {
        let mut h = DefaultHasher::new();
        texture.data.hash(&mut h);
        let same = self.textures.entry(h.finish()).or_default();
        if let Some(&i) = same
            .iter()
            .find(|&&i| self.visual.textures[i as usize] == texture)
        {
            return i;
        }
        let i = self.visual.textures.len() as u32;
        self.visual.textures.push(texture);
        same.push(i);
        i
    }

    pub fn add_material(&mut self, material: Material) -> u32 {
        self.visual.materials.push(material);
        (self.visual.materials.len() - 1) as u32
    }

    /// A material added before.
    pub fn material(&self, i: u32) -> &Material {
        &self.visual.materials[i as usize]
    }

    /// Adds a model's shape, to draw copies of; returns its index.
    pub fn add_shape(&mut self, shape: Shape) -> u32 {
        self.visual.shapes.push(shape);
        (self.visual.shapes.len() - 1) as u32
    }

    /// Adds copies of shapes, unless there are none.
    pub fn add_instances(&mut self, instances: Instances) {
        if !instances.copies.is_empty() && !instances.levels.is_empty() {
            self.visual.instances.push(instances);
        }
    }

    /// Adds a mesh to the batch of its material, shadow flag and tile. `normals` and
    /// `uvs` have one entry per position.
    pub fn add_mesh(
        &mut self,
        material: u32,
        cast_shadows: bool,
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        uvs: &[[f32; 2]],
        indices: &[u32],
    ) {
        if indices.is_empty() {
            return;
        }
        let (lo, hi) = positions.iter().fold(
            ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]),
            |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )
            },
        );
        let tile = |a: f32, b: f32| ((a + b) * 0.5 / BATCH_TILE).floor() as i32;
        let key = (
            material,
            cast_shadows,
            tile(lo[0], hi[0]),
            tile(lo[1], hi[1]),
        );
        let meshes = &mut self.visual.meshes;
        let i = *self.batches.entry(key).or_insert_with(|| {
            meshes.push(Mesh {
                material,
                cast_shadows,
                ..Default::default()
            });
            meshes.len() - 1
        });
        let m = &mut meshes[i];
        let base = m.positions.len() as u32;
        m.positions.extend_from_slice(positions);
        m.normals.extend_from_slice(normals);
        m.uvs.extend_from_slice(uvs);
        m.indices.extend(indices.iter().map(|i| base + i));
    }

    pub fn build(self) -> Visual {
        self.visual
    }
}

impl Visual {
    /// Checks that every index refers to something that exists.
    pub fn validate(&self) -> Result<(), Error> {
        let bad = |what: &str| Err(Error::Format(format!("visual: {what}")));
        let missing = |t: u32| t as usize >= self.textures.len();
        for m in &self.materials {
            let detail = m.detail.iter().flat_map(|d| {
                let mask = match d.mask {
                    DetailMask::Texture(t) => Some(t),
                    DetailMask::BaseAlpha => None,
                };
                mask.into_iter()
                    .chain(d.layers.iter().flatten().map(|l| l.texture))
                    .chain(d.normal.map(|n| n.texture))
            });
            let own = [m.base_color_texture, m.surface_texture, m.normal_texture];
            if own.into_iter().flatten().chain(detail).any(missing) {
                return bad("material refers to a missing texture");
            }
        }
        let meshes = self.meshes.iter();
        for m in meshes.chain(self.shapes.iter().flat_map(|s| &s.meshes)) {
            if m.material as usize >= self.materials.len() {
                return bad("mesh refers to a missing material");
            }
            let n = m.positions.len();
            if m.normals.len() != n || m.uvs.len() != n {
                return bad("vertex attribute counts differ");
            }
            if m.indices.len() % 3 != 0 || m.indices.iter().any(|&i| i as usize >= n) {
                return bad("invalid triangle indices");
            }
        }
        for l in self.instances.iter().flat_map(|i| &i.levels) {
            if l.shape as usize >= self.shapes.len() {
                return bad("copies of a missing shape");
            }
            if !(l.fade_in[0] <= l.fade_in[1]
                && l.fade_in[1] <= l.fade_out[0]
                && l.fade_out[0] <= l.fade_out[1])
            {
                return bad("a level of detail's distances out of order");
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(MAGIC, VERSION);
        w.u32(self.textures.len() as u32);
        for t in &self.textures {
            w.bytes(&t.data);
        }
        w.u32(self.materials.len() as u32);
        for m in &self.materials {
            m.base_color.iter().for_each(|&c| w.f32(c));
            w.u32(m.base_color_texture.unwrap_or(NONE));
            w.f32(m.roughness);
            w.f32(m.reflectance);
            w.f32(m.reflection);
            w.u32(m.surface_texture.unwrap_or(NONE));
            w.u32(m.normal_texture.unwrap_or(NONE));
            match m.alpha_mode {
                AlphaMode::Opaque => (w.u8(0), w.f32(0.0)),
                AlphaMode::Mask(c) => (w.u8(1), w.f32(c)),
                AlphaMode::Blend => (w.u8(2), w.f32(0.0)),
            };
            w.u8(m.double_sided.into());
            match &m.plant {
                None => w.u8(0),
                Some(p) => {
                    w.u8(1);
                    w.f32(p.sway);
                    w.f32(p.flutter);
                    w.u8(p.leaves.into());
                }
            }
            match &m.detail {
                None => w.u8(0),
                Some(d) => {
                    match d.mask {
                        DetailMask::Texture(t) => (w.u8(1), w.u32(t)),
                        DetailMask::BaseAlpha => (w.u8(2), w.u32(NONE)),
                    };
                    for layer in &d.layers {
                        w.u32(layer.map_or(NONE, |l| l.texture));
                        w.f32(layer.map_or(0.0, |l| l.scale));
                    }
                    w.f32(d.multiplier);
                    w.u8(d.world_uv.into());
                    w.u32(d.normal.map_or(NONE, |n| n.texture));
                    w.f32(d.normal.map_or(0.0, |n| n.scale));
                    w.f32(d.normal.map_or(0.0, |n| n.strength));
                }
            }
        }
        let meshes = |w: &mut Writer, meshes: &[Mesh]| {
            w.u32(meshes.len() as u32);
            for m in meshes {
                w.u32(m.material);
                w.u8(m.cast_shadows.into());
                w.vecs(&m.positions);
                w.vecs(&m.normals);
                w.vecs(&m.uvs);
                w.u32s(&m.indices);
            }
        };
        meshes(&mut w, &self.meshes);
        w.u32(self.shapes.len() as u32);
        for s in &self.shapes {
            meshes(&mut w, &s.meshes);
        }
        w.u32(self.instances.len() as u32);
        for i in &self.instances {
            w.u32(i.levels.len() as u32);
            for l in &i.levels {
                w.u32(l.shape);
                let [a, b] = l.fade_in;
                let [c, d] = l.fade_out;
                [a, b, c, d].into_iter().for_each(|v| w.f32(v));
            }
            let copies: Vec<[f32; 8]> = i
                .copies
                .iter()
                .map(|c| {
                    let [x, y, z] = c.pos;
                    let [a, b, d, e] = c.rotation;
                    [x, y, z, a, b, d, e, c.scale]
                })
                .collect();
            w.vecs(&copies);
            let leaves: Vec<u32> = i
                .copies
                .iter()
                .map(|c| u32::from_le_bytes(c.leaves))
                .collect();
            w.u32s(&leaves);
        }
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(buf, MAGIC, VERSION..=VERSION, "visual.bin")?;
        let mut v = Visual::default();
        for _ in 0..r.u32()? {
            v.textures.push(Texture {
                data: r.bytes()?.to_vec(),
            });
        }
        for _ in 0..r.u32()? {
            let base_color = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
            let base_color_texture = Some(r.u32()?).filter(|&t| t != NONE);
            let (roughness, reflectance, reflection) = (r.f32()?, r.f32()?, r.f32()?);
            let surface_texture = Some(r.u32()?).filter(|&t| t != NONE);
            let normal_texture = Some(r.u32()?).filter(|&t| t != NONE);
            let alpha_mode = match (r.u8()?, r.f32()?) {
                (0, _) => AlphaMode::Opaque,
                (1, c) => AlphaMode::Mask(c),
                (2, _) => AlphaMode::Blend,
                (a, _) => return Err(Error::Format(format!("visual: unknown alpha mode {a}"))),
            };
            let double_sided = r.u8()? != 0;
            let plant = match r.u8()? {
                0 => None,
                _ => Some(PlantLook {
                    sway: r.f32()?,
                    flutter: r.f32()?,
                    leaves: r.u8()? != 0,
                }),
            };
            let detail = match r.u8()? {
                0 => None,
                kind => {
                    let mask = match (kind, r.u32()?) {
                        (2, _) => DetailMask::BaseAlpha,
                        (_, t) => DetailMask::Texture(t),
                    };
                    let mut layers = [None; 4];
                    for layer in &mut layers {
                        let (texture, scale) = (r.u32()?, r.f32()?);
                        *layer = (texture != NONE).then_some(DetailLayer { texture, scale });
                    }
                    let (multiplier, world_uv) = (r.f32()?, r.u8()? != 0);
                    let (texture, scale, strength) = (r.u32()?, r.f32()?, r.f32()?);
                    let normal = (texture != NONE).then_some(DetailNormal {
                        texture,
                        scale,
                        strength,
                    });
                    Some(Detail {
                        mask,
                        layers,
                        multiplier,
                        world_uv,
                        normal,
                    })
                }
            };
            v.materials.push(Material {
                base_color,
                base_color_texture,
                roughness,
                reflectance,
                reflection,
                surface_texture,
                normal_texture,
                alpha_mode,
                double_sided,
                detail,
                plant,
            });
        }
        let meshes = |r: &mut Reader| -> Result<Vec<Mesh>, Error> {
            (0..r.u32()?)
                .map(|_| {
                    Ok(Mesh {
                        material: r.u32()?,
                        cast_shadows: r.u8()? != 0,
                        positions: r.vecs()?,
                        normals: r.vecs()?,
                        uvs: r.vecs()?,
                        indices: r.u32s()?,
                    })
                })
                .collect()
        };
        v.meshes = meshes(&mut r)?;
        for _ in 0..r.u32()? {
            v.shapes.push(Shape {
                meshes: meshes(&mut r)?,
            });
        }
        for _ in 0..r.u32()? {
            let mut levels = Vec::new();
            for _ in 0..r.u32()? {
                let shape = r.u32()?;
                let mut f = [0.0; 4];
                for x in &mut f {
                    *x = r.f32()?;
                }
                levels.push(Level {
                    shape,
                    fade_in: [f[0], f[1]],
                    fade_out: [f[2], f[3]],
                });
            }
            let places = r.vecs::<8>()?;
            let leaves = r.u32s()?;
            if leaves.len() != places.len() {
                return Err(Error::Format("visual: copies' looks miscounted".into()));
            }
            let copies = places
                .into_iter()
                .zip(leaves)
                .map(|([x, y, z, a, b, d, e, scale], leaves)| Instance {
                    pos: [x, y, z],
                    rotation: [a, b, d, e],
                    scale,
                    leaves: leaves.to_le_bytes(),
                })
                .collect();
            v.instances.push(Instances { levels, copies });
        }
        r.finish()?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRI: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];

    fn add(b: &mut VisualBuilder, material: u32, x: f32) {
        let p = TRI.map(|v| [v[0] + x, v[1], v[2]]);
        b.add_mesh(
            material,
            true,
            &p,
            &[[0.0, 0.0, 1.0]; 3],
            &[[0.0; 2]; 3],
            &[0, 1, 2],
        );
    }

    #[test]
    fn meshes_batch_by_material_and_tile() {
        let mut b = VisualBuilder::new();
        let tex = Texture {
            data: vec![1, 2, 3],
        };
        assert_eq!(b.add_texture(tex.clone()), 0);
        assert_eq!(b.add_texture(Texture { data: vec![4] }), 1);
        assert_eq!(b.add_texture(tex), 0);
        let mat = Material {
            base_color: [1.0; 4],
            base_color_texture: Some(0),
            ..Default::default()
        };
        let (m0, m1) = (b.add_material(mat.clone()), b.add_material(mat));
        add(&mut b, m0, 0.0);
        add(&mut b, m0, 10.0);
        add(&mut b, m1, 10.0);
        add(&mut b, m0, 1000.0);
        let v = b.build();
        assert_eq!(v.meshes.len(), 3);
        assert_eq!(v.meshes[0].indices, [0, 1, 2, 3, 4, 5]);
        assert_eq!(v.meshes[0].positions[3], [10.0, 0.0, 0.0]);
    }

    #[test]
    fn copies_of_shapes_round_trip() {
        let mut b = VisualBuilder::new();
        let m = b.add_material(Material {
            plant: Some(PlantLook {
                sway: 0.01,
                flutter: 0.02,
                leaves: true,
            }),
            ..Default::default()
        });
        add(&mut b, m, 0.0);
        let shape = b.add_shape(Shape {
            meshes: vec![Mesh {
                material: m,
                cast_shadows: false,
                positions: TRI.to_vec(),
                normals: vec![[0.0, 0.0, 1.0]; 3],
                uvs: vec![[0.0; 2]; 3],
                indices: vec![0, 1, 2],
            }],
        });
        let copy = Instance {
            pos: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.6, 0.8],
            scale: 1.5,
            leaves: [200, 120, 30, 180],
        };
        b.add_instances(Instances {
            levels: vec![Level {
                shape,
                fade_in: [0.0; 2],
                fade_out: [900.0, 940.0],
            }],
            copies: vec![copy; 3],
        });
        let v = b.build();
        v.validate().unwrap();
        let back = Visual::decode(&v.encode()).unwrap();
        assert_eq!(back, v);
        assert_eq!(back.instances[0].copies[2], copy);
        let mut bad = v;
        bad.instances[0].levels[0].shape = 7;
        assert!(bad.validate().is_err());
    }
}
