//! `visual.bin`: what the app renders. Only the app reads it.
//!
//! Meshes are batched by material and by a coarse XY tile when the package is written:
//! few enough draw calls for a whole circuit, while each batch stays small enough to be
//! frustum-culled.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::Error;
use crate::bin::{Reader, Writer};

const MAGIC: &[u8; 4] = b"ORVS";
const VERSION: u32 = 3;
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
    /// Index into `Visual::textures`; linear data whose R, G, B, A weigh the layers.
    pub mask: u32,
    pub layers: [Option<DetailLayer>; 4],
    pub multiplier: f32,
    pub world_uv: bool,
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Visual {
    pub textures: Vec<Texture>,
    pub materials: Vec<Material>,
    pub meshes: Vec<Mesh>,
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
    pub(crate) fn validate(&self) -> Result<(), Error> {
        let bad = |what: &str| Err(Error::Format(format!("visual: {what}")));
        let missing = |t: u32| t as usize >= self.textures.len();
        for m in &self.materials {
            let detail = m.detail.iter().flat_map(|d| {
                std::iter::once(d.mask).chain(d.layers.iter().flatten().map(|l| l.texture))
            });
            let own = [m.base_color_texture, m.surface_texture, m.normal_texture];
            if own.into_iter().flatten().chain(detail).any(missing) {
                return bad("material refers to a missing texture");
            }
        }
        for m in &self.meshes {
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
        Ok(())
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
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
            match &m.detail {
                None => w.u8(0),
                Some(d) => {
                    w.u8(1);
                    w.u32(d.mask);
                    for layer in &d.layers {
                        w.u32(layer.map_or(NONE, |l| l.texture));
                        w.f32(layer.map_or(0.0, |l| l.scale));
                    }
                    w.f32(d.multiplier);
                    w.u8(d.world_uv.into());
                }
            }
        }
        w.u32(self.meshes.len() as u32);
        for m in &self.meshes {
            w.u32(m.material);
            w.u8(m.cast_shadows.into());
            w.vecs(&m.positions);
            w.vecs(&m.normals);
            w.vecs(&m.uvs);
            w.u32s(&m.indices);
        }
        w.finish()
    }

    pub(crate) fn decode(buf: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(buf, MAGIC, VERSION, "visual.bin")?;
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
            let detail = match r.u8()? {
                0 => None,
                _ => {
                    let mask = r.u32()?;
                    let mut layers = [None; 4];
                    for layer in &mut layers {
                        let (texture, scale) = (r.u32()?, r.f32()?);
                        *layer = (texture != NONE).then_some(DetailLayer { texture, scale });
                    }
                    Some(Detail {
                        mask,
                        layers,
                        multiplier: r.f32()?,
                        world_uv: r.u8()? != 0,
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
            });
        }
        for _ in 0..r.u32()? {
            v.meshes.push(Mesh {
                material: r.u32()?,
                cast_shadows: r.u8()? != 0,
                positions: r.vecs()?,
                normals: r.vecs()?,
                uvs: r.vecs()?,
                indices: r.u32s()?,
            });
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
}
