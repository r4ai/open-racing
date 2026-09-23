//! KN5 materials and the textures they use, as package materials.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use open_racing_track::texture::{self, Mips};
use open_racing_track::{AlphaMode, Detail, DetailLayer, Material, Texture, VisualBuilder};
use rayon::prelude::*;

use crate::kn5;

/// Stand-in for the detail layers of multi-layer materials that lack a mask.
const DETAIL_TINT: f32 = 0.25;
/// Detail samplers of the multi-layer shaders, with the property holding their tiling.
const DETAIL_LAYERS: [(&str, &str); 4] = [("txDetailR", "multR"), ("txDetailG", "multG"), ("txDetailB", "multB"), ("txDetailA", "multA")];
/// The files' shading runs on gamma-encoded values; multipliers apply to linear ones.
const GAMMA: f32 = 2.2;

fn is_multilayer(m: &kn5::Material) -> bool {
    m.shader.starts_with("ksMultilayer")
}

fn alpha_mode(m: &kn5::Material) -> AlphaMode {
    match (m.blend_mode, m.alpha_tested) {
        (1, _) => AlphaMode::Blend,
        (_, true) => AlphaMode::Mask(m.property("ksAlphaRef").filter(|&r| r > 0.0).unwrap_or(0.5)),
        _ => AlphaMode::Opaque,
    }
}

/// Mip levels for the diffuse texture, following how the material uses its alpha.
fn diffuse_mips(m: &kn5::Material) -> Mips {
    match alpha_mode(m) {
        AlphaMode::Opaque => Mips::Complete,
        AlphaMode::Mask(c) => Mips::AlphaTest(c),
        AlphaMode::Blend => Mips::Source,
    }
}

/// Textures a material samples, with the mip levels each needs.
fn texture_uses(m: &kn5::Material) -> Vec<(&str, Mips)> {
    let mut uses: Vec<(&str, Mips)> = m.texture("txDiffuse").map(|t| (t, diffuse_mips(m))).into_iter().collect();
    if is_multilayer(m) {
        let detail = std::iter::once("txMask").chain(DETAIL_LAYERS.iter().map(|(s, _)| *s));
        uses.extend(detail.filter_map(|s| m.texture(s)).map(|t| (t, Mips::Complete)));
    }
    uses
}

/// Hashable form of `Mips`.
type MipsKey = (u8, u32);

fn mips_key(mips: Mips) -> MipsKey {
    match mips {
        Mips::Complete => (0, 0),
        Mips::AlphaTest(c) => (1, c.to_bits()),
        Mips::Source => (2, 0),
    }
}

/// A source texture (by content hash) prepared for particular mip levels.
type CacheKey = (u64, MipsKey);

fn cache_key(data: &[u8], mips: Mips) -> CacheKey {
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    (h.finish(), mips_key(mips))
}

enum Prepared {
    /// Not in the package yet.
    Ready(Vec<u8>),
    /// At this index of the package's textures.
    Added(u32),
    Failed,
}

/// Prepared textures (see `texture::prepare`), shared across the models of a track so
/// each is prepared and added once.
#[derive(Default)]
pub struct TextureCache(HashMap<CacheKey, Prepared>);

impl TextureCache {
    /// Package index of a prepared texture, moving it into the package on first use.
    fn index(&mut self, key: &CacheKey, visual: &mut VisualBuilder) -> Option<u32> {
        let entry = self.0.get_mut(key)?;
        if let Prepared::Ready(data) = entry {
            *entry = Prepared::Added(visual.add_texture(Texture { data: std::mem::take(data) }));
        }
        match entry {
            Prepared::Added(i) => Some(*i),
            _ => None,
        }
    }
}

/// Adds one model's materials, and their textures, to the package as meshes need them.
pub struct Materials<'a> {
    kn5: &'a kn5::Kn5,
    /// Cache entries by lower-case texture name and mip levels.
    textures: HashMap<(String, MipsKey), CacheKey>,
    added: Vec<Option<u32>>,
    fallback: Option<u32>,
}

impl<'a> Materials<'a> {
    /// Prepares, in parallel, the textures of the materials in `used` that `cache` lacks.
    pub fn new(kn5: &'a kn5::Kn5, used: impl IntoIterator<Item = usize>, cache: &mut TextureCache) -> Self {
        let mut textures = HashMap::new();
        let mut missing: HashMap<CacheKey, (&str, &[u8], Mips)> = HashMap::new();
        for m in used.into_iter().filter_map(|i| kn5.materials.get(i)) {
            for (name, mips) in texture_uses(m) {
                let Some(t) = kn5.textures.iter().find(|t| t.name.eq_ignore_ascii_case(name)) else { continue };
                let key = cache_key(&t.data, mips);
                textures.insert((name.to_ascii_lowercase(), mips_key(mips)), key);
                if !cache.0.contains_key(&key) {
                    missing.insert(key, (name, &t.data, mips));
                }
            }
        }
        let prepared: Vec<_> = missing
            .into_par_iter()
            .map(|(key, (name, data, mips))| match texture::prepare(data, mips) {
                Ok(out) => (key, Prepared::Ready(out)),
                Err(e) => {
                    eprintln!("warning: texture {name}: {e}");
                    (key, Prepared::Failed)
                }
            })
            .collect();
        cache.0.extend(prepared);
        Self { kn5, textures, added: vec![None; kn5.materials.len()], fallback: None }
    }

    pub fn get(&mut self, visual: &mut VisualBuilder, cache: &mut TextureCache, index: usize) -> u32 {
        let Some(m) = self.kn5.materials.get(index) else {
            return *self.fallback.get_or_insert_with(|| {
                visual.add_material(Material { base_color: [0.5, 0.5, 0.5, 1.0], base_color_texture: None, alpha_mode: AlphaMode::Opaque, double_sided: false, detail: None })
            });
        };
        if let Some(i) = self.added[index] {
            return i;
        }
        let mut texture = |sampler: &str, mips: Mips| {
            let key = self.textures.get(&(m.texture(sampler)?.to_ascii_lowercase(), mips_key(mips)))?;
            cache.index(key, visual)
        };
        let alpha_mode = alpha_mode(m);
        let base_color_texture = texture("txDiffuse", diffuse_mips(m));
        let detail = is_multilayer(m).then(|| detail(m, &mut texture)).flatten();
        let tint = if is_multilayer(m) && detail.is_none() { DETAIL_TINT } else { 1.0 };
        // The game culls back faces; surfaces seen from both sides have a triangle per side,
        // which would z-fight if both sides of each were drawn.
        let material = Material { base_color: [tint, tint, tint, 1.0], base_color_texture, alpha_mode, double_sided: false, detail };
        let i = visual.add_material(material);
        self.added[index] = Some(i);
        i
    }
}

/// The detail layers of a multi-layer material. Its shaders multiply the base texture by
/// detail textures weighed by the mask's channels, tiled `mult<channel>` times per metre
/// over the ground plane, and scale the result by `magicMult`.
fn detail(m: &kn5::Material, texture: &mut impl FnMut(&str, Mips) -> Option<u32>) -> Option<Detail> {
    let mask = texture("txMask", Mips::Complete)?;
    let layers = DETAIL_LAYERS.map(|(sampler, mult)| {
        let texture = texture(sampler, Mips::Complete)?;
        // A zero tiling would stretch one texel over the surface; tile once per metre instead.
        Some(DetailLayer { texture, scale: m.property(mult).filter(|&s| s > 0.0).unwrap_or(1.0) })
    });
    let multiplier = m.property("magicMult").unwrap_or(1.0).max(0.0).powf(GAMMA);
    Some(Detail { mask, layers, multiplier, world_uv: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multilayer() -> kn5::Material {
        kn5::Material {
            shader: "ksMultilayer_objsp".into(),
            properties: vec![("multR".into(), 0.2), ("multG".into(), 0.0), ("magicMult".into(), 2.0)],
            samplers: ["txDiffuse", "txMask", "txDetailR", "txDetailG"].iter().map(|s| (s.to_string(), format!("{s}.dds"))).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn multilayer_becomes_detail_layers() {
        let names: Vec<String> = multilayer().samplers.iter().map(|(_, t)| t.clone()).collect();
        let mut texture = |s: &str, _| names.iter().position(|n| n.starts_with(s)).map(|i| i as u32);
        let d = detail(&multilayer(), &mut texture).unwrap();
        assert_eq!(d.mask, 1);
        assert_eq!(d.layers[0], Some(DetailLayer { texture: 2, scale: 0.2 }));
        assert_eq!(d.layers[1], Some(DetailLayer { texture: 3, scale: 1.0 }));
        assert_eq!(d.layers[2], None);
        assert!((d.multiplier - 2.0f32.powf(2.2)).abs() < 1e-5);
        assert!(detail(&multilayer(), &mut |_: &str, _| None).is_none());
    }

    #[test]
    fn diffuse_mips_follow_the_alpha_use() {
        let m = kn5::Material { alpha_tested: true, samplers: vec![("txDiffuse".into(), "leaf.dds".into())], ..Default::default() };
        assert_eq!(alpha_mode(&m), AlphaMode::Mask(0.5));
        assert_eq!(texture_uses(&m), [("leaf.dds", Mips::AlphaTest(0.5))]);
        assert_eq!(texture_uses(&multilayer()).len(), 4);
        let glass = kn5::Material { blend_mode: 1, samplers: vec![("txDiffuse".into(), "glass.dds".into())], ..Default::default() };
        assert_eq!(texture_uses(&glass), [("glass.dds", Mips::Source)]);
    }
}
