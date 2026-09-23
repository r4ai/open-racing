//! KN5 materials and the textures they use, as package materials.
//!
//! The game shades with a Blinn-Phong highlight (`ksSpecular`, `ksSpecularEXP`) and a
//! Fresnel-weighted cube-map reflection (`fresnelC`, `fresnelEXP`, `fresnelMaxLevel`),
//! which `txMaps` scales per texel (R highlight, G exponent, B reflection). Packages
//! describe surfaces with a roughness, a specular reflectance and how much of the
//! surroundings they reflect instead; `Shading` converts one into the other.
//!
//! Normal maps carry over unchanged. The files store tangents that make normal × tangent
//! point along +V, down the image, and the game's normal maps are authored for that
//! bitangent (green down): the package's tangent frame, which follows from the UVs.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use open_racing_track::texture::{self, Image, Mips};
use open_racing_track::{AlphaMode, Detail, DetailLayer, Material, Texture, VisualBuilder};
use rayon::prelude::*;

use crate::kn5;

/// Stand-in for the detail layers of multi-layer materials that lack a mask.
const DETAIL_TINT: f32 = 0.25;
/// Detail samplers of the multi-layer shaders, with the property holding their tiling.
const DETAIL_LAYERS: [(&str, &str); 4] = [
    ("txDetailR", "multR"),
    ("txDetailG", "multG"),
    ("txDetailB", "multB"),
    ("txDetailA", "multA"),
];
/// The files' shading runs on gamma-encoded values; multipliers apply to linear ones.
const GAMMA: f32 = 2.2;
/// Floor for `ksDiffuse` when relating the highlight to the diffuse term.
const MIN_DIFFUSE: f32 = 0.1;
/// Highest F0 given to a surface. Common dielectrics stay below it (glass ≈ 0.04); the
/// game's highlights, which are not energy-normalised, would ask for more at low
/// exponents.
const MAX_F0: f32 = 0.08;
/// F0 of surfaces that reflect their surroundings, such as glass, unless `fresnelC`
/// asks for more.
const REFLECTIVE_F0: f32 = 0.04;
/// Roughness of the game's cube-map reflections, which are sharp whatever the highlight.
const MIRROR_ROUGHNESS: f32 = 0.1;

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

/// The tangent-space normal map. Object-space ones (`nmObjectSpace`) are not supported.
fn normal_map(m: &kn5::Material) -> Option<&str> {
    m.texture("txNormal")
        .filter(|_| m.property("nmObjectSpace").unwrap_or(0.0) == 0.0)
}

/// The game's highlight and reflection strengths, from which package surfaces follow.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shading {
    /// `ksSpecular` relative to `ksDiffuse`: the package's base colour is the diffuse
    /// texture alone, so the highlight keeps its ratio to the diffuse term.
    specular: f32,
    exponent: f32,
    /// `fresnelC`: reflection at normal incidence.
    fresnel: f32,
    /// `fresnelMaxLevel`: reflection at grazing angles, 0 without a reflection.
    max_level: f32,
}

/// A package surface (see `Material`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Surface {
    roughness: f32,
    reflectance: f32,
    reflection: f32,
}

impl Shading {
    fn of(m: &kn5::Material) -> Self {
        let p = |name: &str| m.property(name).unwrap_or(0.0).max(0.0);
        Self {
            specular: p("ksSpecular") / p("ksDiffuse").max(MIN_DIFFUSE),
            exponent: p("ksSpecularEXP"),
            fresnel: p("fresnelC"),
            max_level: p("fresnelMaxLevel"),
        }
    }

    /// The surface for a `txMaps` texel (highlight, exponent and reflection scales, 0–1).
    ///
    /// A Blinn-Phong lobe of exponent n peaks like a GGX lobe of α² = 2 / (n + 2), whose
    /// peak is F0 · (n + 2) / 8π. Matching the ratio of highlight to Lambertian diffuse
    /// gives F0 = 8 · specular / (n + 2). The surroundings are reflected up to
    /// `fresnelMaxLevel`, as in the game, so that surfaces without a reflection keep only
    /// their highlights.
    fn surface(&self, [highlight, gloss, mirror]: [f32; 3]) -> Surface {
        let n = (self.exponent * gloss).max(1.0);
        let roughness = (2.0 / (n + 2.0)).powf(0.25);
        let f0_highlight = 8.0 * self.specular * highlight / (n + 2.0);
        let reflection = (self.max_level * mirror).min(1.0);
        let f0_reflection = if self.max_level > 0.0 {
            self.fresnel.max(REFLECTIVE_F0) * mirror
        } else {
            0.0
        };
        let f0 = f0_highlight.max(f0_reflection).min(MAX_F0);
        // Where the reflection is present its sharpness wins over the highlight's spread.
        let sharp = if self.max_level > 0.0 { mirror } else { 0.0 };
        let roughness = roughness + (roughness.min(MIRROR_ROUGHNESS) - roughness) * sharp;
        Surface {
            roughness,
            reflectance: (f0 / 0.16).sqrt(),
            reflection,
        }
    }
}

/// Reflectance that a surface texture's A = 1 stands for.
fn max_reflectance() -> f32 {
    (MAX_F0 / 0.16).sqrt()
}

/// A surface texture baked from `txMaps`: R = reflection, G = roughness,
/// A = reflectance / `max_reflectance()`.
fn bake_surface(maps: &Image, shading: Shading) -> Image {
    let unit = |v: u8| f32::from(v) / 255.0;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let pixels = maps
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let s = shading.surface([unit(p[0]), unit(p[1]), unit(p[2])]);
            [
                byte(s.reflection),
                byte(s.roughness),
                0,
                byte(s.reflectance / max_reflectance()),
            ]
        })
        .collect();
    Image {
        width: maps.width,
        height: maps.height,
        pixels,
    }
}

/// How a source texture becomes a package texture.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Job {
    /// `texture::prepare` with these mip levels.
    Prepare(Mips),
    /// Baked from `txMaps` into a surface texture.
    Surface(Shading),
}

impl Job {
    fn run(self, data: &[u8]) -> Result<Vec<u8>, String> {
        match self {
            Self::Prepare(mips) => texture::prepare(data, mips),
            Self::Surface(shading) => Ok(texture::encode(bake_surface(
                &texture::decode(data)?,
                shading,
            ))),
        }
    }

    /// Hashable form.
    fn key(self) -> JobKey {
        match self {
            Self::Prepare(Mips::Complete) => (0, [0; 4]),
            Self::Prepare(Mips::AlphaTest(c)) => (1, [c.to_bits(), 0, 0, 0]),
            Self::Prepare(Mips::Source) => (2, [0; 4]),
            Self::Surface(s) => (
                3,
                [s.specular, s.exponent, s.fresnel, s.max_level].map(f32::to_bits),
            ),
        }
    }
}

type JobKey = (u8, [u32; 4]);

/// Textures a material samples, with the job preparing each.
fn texture_uses(m: &kn5::Material) -> Vec<(&str, Job)> {
    let mut uses: Vec<(&str, Job)> = m
        .texture("txDiffuse")
        .map(|t| (t, Job::Prepare(diffuse_mips(m))))
        .into_iter()
        .collect();
    uses.extend(normal_map(m).map(|t| (t, Job::Prepare(Mips::Complete))));
    uses.extend(
        m.texture("txMaps")
            .map(|t| (t, Job::Surface(Shading::of(m)))),
    );
    if is_multilayer(m) {
        let detail = std::iter::once("txMask").chain(DETAIL_LAYERS.iter().map(|(s, _)| *s));
        uses.extend(
            detail
                .filter_map(|s| m.texture(s))
                .map(|t| (t, Job::Prepare(Mips::Complete))),
        );
    }
    uses
}

/// A source texture (by content hash) prepared by a job.
type CacheKey = (u64, JobKey);

fn cache_key(data: &[u8], job: Job) -> CacheKey {
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    (h.finish(), job.key())
}

enum Prepared {
    /// Not in the package yet.
    Ready(Vec<u8>),
    /// At this index of the package's textures.
    Added(u32),
    Failed,
}

/// Prepared textures, shared across the models of a track so each is prepared and
/// added once.
#[derive(Default)]
pub struct TextureCache(HashMap<CacheKey, Prepared>);

impl TextureCache {
    /// Package index of a prepared texture, moving it into the package on first use.
    fn index(&mut self, key: &CacheKey, visual: &mut VisualBuilder) -> Option<u32> {
        let entry = self.0.get_mut(key)?;
        if let Prepared::Ready(data) = entry {
            *entry = Prepared::Added(visual.add_texture(Texture {
                data: std::mem::take(data),
            }));
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
    /// Cache entries by lower-case texture name and job.
    textures: HashMap<(String, JobKey), CacheKey>,
    added: Vec<Option<u32>>,
    fallback: Option<u32>,
}

impl<'a> Materials<'a> {
    /// Prepares, in parallel, the textures of the materials in `used` that `cache` lacks.
    pub fn new(
        kn5: &'a kn5::Kn5,
        used: impl IntoIterator<Item = usize>,
        cache: &mut TextureCache,
    ) -> Self {
        let mut textures = HashMap::new();
        let mut missing: HashMap<CacheKey, (&str, &[u8], Job)> = HashMap::new();
        for m in used.into_iter().filter_map(|i| kn5.materials.get(i)) {
            for (name, job) in texture_uses(m) {
                let Some(t) = kn5
                    .textures
                    .iter()
                    .find(|t| t.name.eq_ignore_ascii_case(name))
                else {
                    continue;
                };
                let key = cache_key(&t.data, job);
                textures.insert((name.to_ascii_lowercase(), job.key()), key);
                if !cache.0.contains_key(&key) {
                    missing.insert(key, (name, &t.data, job));
                }
            }
        }
        let prepared: Vec<_> = missing
            .into_par_iter()
            .map(|(key, (name, data, job))| match job.run(data) {
                Ok(out) => (key, Prepared::Ready(out)),
                Err(e) => {
                    eprintln!("warning: texture {name}: {e}");
                    (key, Prepared::Failed)
                }
            })
            .collect();
        cache.0.extend(prepared);
        Self {
            kn5,
            textures,
            added: vec![None; kn5.materials.len()],
            fallback: None,
        }
    }

    pub fn get(
        &mut self,
        visual: &mut VisualBuilder,
        cache: &mut TextureCache,
        index: usize,
    ) -> u32 {
        let Some(m) = self.kn5.materials.get(index) else {
            return *self
                .fallback
                .get_or_insert_with(|| visual.add_material(Material::default()));
        };
        if let Some(i) = self.added[index] {
            return i;
        }
        let mut texture = |name: Option<&str>, job: Job| {
            let key = self
                .textures
                .get(&(name?.to_ascii_lowercase(), job.key()))?;
            cache.index(key, visual)
        };
        let shading = Shading::of(m);
        let base_color_texture = texture(m.texture("txDiffuse"), Job::Prepare(diffuse_mips(m)));
        let normal_texture = texture(normal_map(m), Job::Prepare(Mips::Complete));
        let surface_texture = texture(m.texture("txMaps"), Job::Surface(shading));
        let surface = match surface_texture {
            Some(_) => Surface {
                roughness: 1.0,
                reflectance: max_reflectance(),
                reflection: 1.0,
            },
            None => shading.surface([1.0; 3]),
        };
        let detail = is_multilayer(m)
            .then(|| detail(m, &mut |s, mips| texture(m.texture(s), Job::Prepare(mips))))
            .flatten();
        let tint = if is_multilayer(m) && detail.is_none() {
            DETAIL_TINT
        } else {
            1.0
        };
        let material = Material {
            base_color: [tint, tint, tint, 1.0],
            base_color_texture,
            roughness: surface.roughness,
            reflectance: surface.reflectance,
            reflection: surface.reflection,
            surface_texture,
            normal_texture,
            alpha_mode: alpha_mode(m),
            // The game culls back faces; surfaces seen from both sides have a triangle per
            // side, which would z-fight if both sides of each were drawn.
            double_sided: false,
            detail,
        };
        let i = visual.add_material(material);
        self.added[index] = Some(i);
        i
    }
}

/// The detail layers of a multi-layer material. Its shaders multiply the base texture by
/// detail textures weighed by the mask's channels, tiled `mult<channel>` times per metre
/// over the ground plane, and scale the result by `magicMult`.
fn detail(
    m: &kn5::Material,
    texture: &mut impl FnMut(&str, Mips) -> Option<u32>,
) -> Option<Detail> {
    let mask = texture("txMask", Mips::Complete)?;
    let layers = DETAIL_LAYERS.map(|(sampler, mult)| {
        let texture = texture(sampler, Mips::Complete)?;
        // A zero tiling would stretch one texel over the surface; tile once per metre instead.
        Some(DetailLayer {
            texture,
            scale: m.property(mult).filter(|&s| s > 0.0).unwrap_or(1.0),
        })
    });
    let multiplier = m.property("magicMult").unwrap_or(1.0).max(0.0).powf(GAMMA);
    Some(Detail {
        mask,
        layers,
        multiplier,
        world_uv: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multilayer() -> kn5::Material {
        kn5::Material {
            shader: "ksMultilayer_objsp".into(),
            properties: vec![
                ("multR".into(), 0.2),
                ("multG".into(), 0.0),
                ("magicMult".into(), 2.0),
            ],
            samplers: ["txDiffuse", "txMask", "txDetailR", "txDetailG"]
                .iter()
                .map(|s| (s.to_string(), format!("{s}.dds")))
                .collect(),
            ..Default::default()
        }
    }

    fn shaded(properties: &[(&str, f32)]) -> kn5::Material {
        kn5::Material {
            properties: properties.iter().map(|&(n, v)| (n.into(), v)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn multilayer_becomes_detail_layers() {
        let names: Vec<String> = multilayer()
            .samplers
            .iter()
            .map(|(_, t)| t.clone())
            .collect();
        let mut texture = |s: &str, _| {
            names
                .iter()
                .position(|n| n.starts_with(s))
                .map(|i| i as u32)
        };
        let d = detail(&multilayer(), &mut texture).unwrap();
        assert_eq!(d.mask, 1);
        assert_eq!(
            d.layers[0],
            Some(DetailLayer {
                texture: 2,
                scale: 0.2
            })
        );
        assert_eq!(
            d.layers[1],
            Some(DetailLayer {
                texture: 3,
                scale: 1.0
            })
        );
        assert_eq!(d.layers[2], None);
        assert!((d.multiplier - 2.0f32.powf(2.2)).abs() < 1e-5);
        assert!(detail(&multilayer(), &mut |_: &str, _| None).is_none());
    }

    #[test]
    fn textures_follow_the_material() {
        let m = kn5::Material {
            alpha_tested: true,
            samplers: vec![("txDiffuse".into(), "leaf.dds".into())],
            ..Default::default()
        };
        assert_eq!(alpha_mode(&m), AlphaMode::Mask(0.5));
        assert_eq!(
            texture_uses(&m),
            [("leaf.dds", Job::Prepare(Mips::AlphaTest(0.5)))]
        );
        assert_eq!(texture_uses(&multilayer()).len(), 4);
        let glass = kn5::Material {
            blend_mode: 1,
            samplers: vec![("txDiffuse".into(), "glass.dds".into())],
            ..Default::default()
        };
        assert_eq!(
            texture_uses(&glass),
            [("glass.dds", Job::Prepare(Mips::Source))]
        );

        let samplers = ["txDiffuse", "txNormal", "txMaps", "txDetail"]
            .iter()
            .map(|s| (s.to_string(), format!("{s}.dds")))
            .collect();
        let mut multimap = kn5::Material {
            shader: "ksPerPixelMultiMap".into(),
            samplers,
            ..shaded(&[("ksSpecular", 0.1)])
        };
        let jobs: Vec<Job> = texture_uses(&multimap)
            .into_iter()
            .map(|(_, j)| j)
            .collect();
        assert_eq!(
            jobs,
            [
                Job::Prepare(Mips::Complete),
                Job::Prepare(Mips::Complete),
                Job::Surface(Shading::of(&multimap))
            ]
        );
        multimap.properties.push(("nmObjectSpace".into(), 1.0));
        assert!(normal_map(&multimap).is_none());
    }

    #[test]
    fn highlights_become_roughness_and_reflectance() {
        let surface = |props: &[(&str, f32)]| Shading::of(&shaded(props)).surface([1.0; 3]);
        // No highlight, no reflection: no specular at all.
        assert_eq!(
            surface(&[("ksDiffuse", 0.3), ("ksSpecularEXP", 30.0)]).reflectance,
            0.0
        );
        // A tight highlight is smooth; a broad one rough.
        let tight = surface(&[("ksSpecular", 0.1), ("ksSpecularEXP", 200.0)]).roughness;
        let broad = surface(&[("ksSpecular", 0.1), ("ksSpecularEXP", 10.0)]).roughness;
        assert!(tight < 0.4 && broad > 0.6, "{tight} {broad}");
        // A typical glossy highlight lands near common dielectrics (F0 ≈ 0.04), and without
        // a reflection in the game the surroundings are not reflected.
        let s = surface(&[
            ("ksSpecular", 0.1),
            ("ksDiffuse", 0.3),
            ("ksSpecularEXP", 60.0),
        ]);
        assert!(
            (0.16 * s.reflectance.powi(2) - 0.043).abs() < 0.002,
            "{s:?}"
        );
        assert_eq!(s.reflection, 0.0);
        // Strong highlights are capped.
        assert_eq!(
            surface(&[
                ("ksSpecular", 1.0),
                ("ksDiffuse", 0.1),
                ("ksSpecularEXP", 5.0)
            ])
            .reflectance,
            max_reflectance()
        );
    }

    #[test]
    fn reflections_are_sharp_and_limited_to_the_games_level() {
        let glass = Shading::of(&shaded(&[
            ("ksDiffuse", 0.3),
            ("ksSpecularEXP", 10.0),
            ("fresnelMaxLevel", 0.2),
        ]));
        let s = glass.surface([1.0; 3]);
        assert!((s.roughness - MIRROR_ROUGHNESS).abs() < 1e-6);
        assert!((0.16 * s.reflectance.powi(2) - REFLECTIVE_F0).abs() < 1e-6);
        assert_eq!(s.reflection, 0.2);
        // txMaps B masks the reflection, and its sharpness, per texel.
        let s = glass.surface([1.0, 1.0, 0.0]);
        assert!(
            s.roughness > 0.6 && s.reflectance == 0.0 && s.reflection == 0.0,
            "{s:?}"
        );
    }

    #[test]
    fn maps_bake_into_a_surface_texture() {
        let shading = Shading::of(&shaded(&[
            ("ksSpecular", 0.1),
            ("ksDiffuse", 0.3),
            ("ksSpecularEXP", 60.0),
            ("fresnelMaxLevel", 0.5),
        ]));
        // Texel 0: full highlight and exponent, no reflection; texel 1: no highlight, a fifth
        // of the exponent, full reflection.
        let maps = Image {
            width: 2,
            height: 1,
            pixels: vec![255, 255, 0, 255, 0, 51, 255, 255],
        };
        let out = bake_surface(&maps, shading);
        let s = shading.surface([1.0, 1.0, 0.0]);
        let byte = |v: f32| (v * 255.0).round() as u8;
        assert_eq!(
            out.pixels[..4],
            [
                0,
                byte(s.roughness),
                0,
                byte(s.reflectance / max_reflectance())
            ]
        );
        assert_eq!(out.pixels[4], byte(0.5));
        assert_eq!(out.pixels[5], byte(MIRROR_ROUGHNESS));
    }
}
