//! Models from elsewhere made ready for the game once, before they are used: a file of
//! several variants (Poly Haven's hold three trees, or seventeen tufts of grass, side by
//! side) split into a model each standing where it was made, the alpha of leaves put
//! back where glTF lost it, and each reduced to a budget of triangles in levels of
//! detail. Photoscanned plants have millions of triangles; the game draws thousands of
//! copies of each.
//!
//! Each model is saved as a .glb of its own, its levels as the nodes `<name>_LOD0`,
//! `_LOD1`, … sharing its materials, which `model::load` reads as levels of detail.
//!
//! Solid parts are simplified (meshoptimizer). Leaves are cards cut out by their alpha,
//! each a separate island of triangles that simplifying cannot merge: each card is
//! simplified on its own, and beyond that levels keep only some of the cards, each made
//! larger so that the crown covers as much.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{Mat4, Vec2, Vec3};
use meshopt::SimplifyOptions;
use open_racing_track::texture::{self, Image};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::project::Kind;

/// Levels of detail each model gets, the most detailed first.
pub const LODS: usize = 3;
/// Each level has about this fraction of the triangles of the one before.
const STEP: f64 = 0.25;
/// Cards are made at most this much larger to cover for those left out.
const MAX_GROWTH: f32 = 4.0;

/// Triangles a model of a kind may have in its most detailed level.
pub fn budget(kind: Kind) -> usize {
    match kind {
        Kind::Evergreen | Kind::Deciduous => 30_000,
        Kind::Grass => 4_000,
        Kind::Rigid => 12_000,
        Kind::Crowd => 3_000,
    }
}

/// A model prepared from one of a file's variants.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prepared {
    pub name: String,
    pub path: PathBuf,
    /// Triangles in each level of detail.
    pub triangles: Vec<usize>,
    /// Triangles it had.
    pub source_triangles: usize,
    /// Size of the box round it, m (glTF's frame: Y up).
    pub size: [f32; 3],
}

/// A material and its textures, ready to write.
struct Look {
    name: String,
    color: [f32; 4],
    base: Option<Image>,
    normal: Option<Image>,
    roughness: f32,
    /// Alpha-tested at this cut-off, if at all.
    cutoff: Option<f32>,
    double_sided: bool,
    leaves: bool,
    /// KHR_texture_transform of its colour map, to bake into texture coordinates:
    /// offset, rotation, scale.
    uv: Option<([f32; 2], f32, [f32; 2])>,
}

/// One material's triangles of a model, in glTF's frame.
#[derive(Clone, Default)]
struct Part {
    material: usize,
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<Vec2>,
    indices: Vec<u32>,
}

impl Part {
    fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Only the vertices `indices` uses, in the order they are first used.
    fn compact(&self, indices: &[u32]) -> Part {
        let mut map: HashMap<u32, u32> = HashMap::new();
        let mut out = Part {
            material: self.material,
            ..Default::default()
        };
        for &i in indices {
            let j = *map.entry(i).or_insert_with(|| {
                let k = i as usize;
                out.positions.push(self.positions[k]);
                out.normals.push(self.normals[k]);
                out.uvs.push(self.uvs[k]);
                out.positions.len() as u32 - 1
            });
            out.indices.push(j);
        }
        out
    }
}

/// A variant of the file: the name of its node and its parts.
struct Variant {
    name: String,
    parts: Vec<Part>,
}

/// Prepares the models of a glTF file into `out`, as `<name>.glb` or, when the file
/// holds several, a file named after each (their nodes' names without `_LOD0`).
/// `kind` sets the budget of triangles; `copyright` is written into each file.
pub fn prepare(
    gltf: &Path,
    out: &Path,
    name: &str,
    kind: Kind,
    copyright: &str,
) -> Result<Vec<Prepared>, Error> {
    let fail = |e: &dyn std::fmt::Display| Error::Invalid(format!("{}: {e}", gltf.display()));
    let (doc, buffers, images) = gltf::import(gltf).map_err(|e| fail(&e))?;
    let looks = looks(&doc, &images, gltf.parent().unwrap_or(Path::new(".")))?;
    let variants = variants(&doc, &buffers, &looks, name);
    if variants.is_empty() {
        return Err(fail(&"no triangles"));
    }
    std::fs::create_dir_all(out).map_err(|e| Error::Io(out.to_path_buf(), e))?;
    let mut prepared = Vec::new();
    for v in variants {
        let levels = levels(&v.parts, &looks, kind);
        let (lo, hi) = v
            .parts
            .iter()
            .flat_map(|p| &p.positions)
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            });
        let path = out.join(format!("{}.glb", v.name));
        let bytes = write_glb(&v.name, &levels, &looks, copyright)?;
        std::fs::write(&path, bytes).map_err(|e| Error::Io(path.clone(), e))?;
        prepared.push(Prepared {
            name: v.name,
            path,
            triangles: levels
                .iter()
                .map(|l| l.iter().map(Part::triangles).sum())
                .collect(),
            source_triangles: v.parts.iter().map(Part::triangles).sum(),
            size: (hi - lo).to_array(),
        });
    }
    Ok(prepared)
}

/// The file's materials, with each leaf's alpha brought in from a map beside its
/// colour map (`x_diff_1k.jpg` → `x_alpha_1k.png`) where the file has none.
fn looks(
    doc: &gltf::Document,
    images: &[gltf::image::Data],
    dir: &Path,
) -> Result<Vec<Look>, Error> {
    let image = |t: gltf::Texture| images.get(t.source().index()).and_then(crate::model::rgba);
    let mut out = Vec::new();
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        let base_info = pbr.base_color_texture();
        let mut base = base_info.as_ref().and_then(|t| image(t.texture()));
        let alpha_map = base_info
            .as_ref()
            .and_then(|t| match t.texture().source().source() {
                gltf::image::Source::Uri { uri, .. } => alpha_beside(&dir.join(
                    urlencoding::decode(uri).map_or_else(|_| uri.into(), |u| u.into_owned()),
                )),
                gltf::image::Source::View { .. } => None,
            });
        let mut cutoff = match m.alpha_mode() {
            gltf::material::AlphaMode::Opaque => None,
            gltf::material::AlphaMode::Mask => Some(m.alpha_cutoff().unwrap_or(0.5)),
            // Blending needs sorting the game does not do for scattered copies.
            gltf::material::AlphaMode::Blend => Some(0.5),
        };
        if let (Some(b), Some(path)) = (&mut base, alpha_map) {
            let bytes = std::fs::read(&path).map_err(|e| Error::Io(path.clone(), e))?;
            let alpha = texture::decode(&bytes)
                .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
            put_alpha(b, &alpha);
            cutoff = cutoff.or(Some(0.5));
        }
        // The roughness map's average, as the game's materials take a single roughness.
        let roughness = pbr.roughness_factor()
            * pbr
                .metallic_roughness_texture()
                .and_then(|t| image(t.texture()))
                .map_or(1.0, |i| channel_mean(&i, 1));
        out.push(Look {
            name: m.name().unwrap_or("material").to_string(),
            color: pbr.base_color_factor(),
            base,
            normal: m.normal_texture().and_then(|t| image(t.texture())),
            roughness,
            cutoff,
            double_sided: m.double_sided(),
            leaves: crate::model::leaves(&m) || cutoff.is_some(),
            uv: base_info
                .and_then(|t| t.texture_transform())
                .map(|t| (t.offset(), t.rotation(), t.scale())),
        });
    }
    Ok(out)
}

/// The alpha map belonging to a colour map, if there is one beside it.
fn alpha_beside(colour: &Path) -> Option<PathBuf> {
    let name = colour.file_stem()?.to_str()?;
    let at = name.rfind("_diff")?;
    let alpha = format!("{}_alpha{}", &name[..at], &name[at + "_diff".len()..]);
    ["png", "jpg"]
        .iter()
        .map(|ext| colour.with_file_name(format!("{alpha}.{ext}")))
        .find(|p| p.is_file())
}

/// Sets `image`'s alpha to `alpha`'s red, sampled at the nearest texel.
pub(crate) fn put_alpha(image: &mut Image, alpha: &Image) {
    for y in 0..image.height {
        let ay = y * alpha.height / image.height;
        for x in 0..image.width {
            let ax = x * alpha.width / image.width;
            image.pixels[(y * image.width + x) * 4 + 3] = alpha.pixels[(ay * alpha.width + ax) * 4];
        }
    }
}

pub(crate) fn channel_mean(image: &Image, channel: usize) -> f32 {
    let n = (image.pixels.len() / 4).max(1);
    let sum: u64 = image
        .pixels
        .iter()
        .skip(channel)
        .step_by(4)
        .map(|&v| v as u64)
        .sum();
    sum as f32 / n as f32 / 255.0
}

/// Whether a node is a less detailed level of another (`tree_LOD1`).
fn lesser_lod(name: &str) -> bool {
    crate::model::lod_of(name).is_some_and(|l| l > 0)
}

/// The file's variants: each node at the top of its scene that has triangles, moved
/// back to where it was made (they are set out side by side), or the whole scene as
/// one if it has a single one.
fn variants(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    looks: &[Look],
    name: &str,
) -> Vec<Variant> {
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    let tops: Vec<gltf::Node> = scene
        .iter()
        .flat_map(|s| s.nodes())
        .filter(|n| !n.name().is_some_and(lesser_lod))
        .collect();
    let single = tops.len() == 1;
    let mut out: Vec<Variant> = Vec::new();
    for (i, top) in tops.iter().enumerate() {
        let (_, rotation, scale) = top.transform().decomposed();
        let placed = Mat4::from_scale_rotation_translation(
            scale.into(),
            glam::Quat::from_array(rotation),
            Vec3::ZERO,
        );
        let mut parts: Vec<Part> = Vec::new();
        let mut stack = vec![(top.clone(), placed)];
        while let Some((node, world)) = stack.pop() {
            stack.extend(
                node.children()
                    .filter(|c| !c.name().is_some_and(lesser_lod))
                    .map(|c| {
                        let m = world * Mat4::from_cols_array_2d(&c.transform().matrix());
                        (c, m)
                    }),
            );
            if let Some(mesh) = node.mesh() {
                add_mesh(&mesh, world, buffers, looks, &mut parts);
            }
        }
        parts.retain(|p| !p.indices.is_empty());
        if parts.is_empty() {
            continue;
        }
        let own = top.name().map(|n| {
            let n = n.trim();
            match crate::model::lod_of(n) {
                Some(_) => n[..n.rfind('_').unwrap_or(n.len())].to_string(),
                None => n.to_string(),
            }
        });
        let base = match own {
            Some(n) if !single && !n.is_empty() => file_name(&n),
            _ if single => file_name(name),
            _ => format!("{}_{i}", file_name(name)),
        };
        let mut unique = base.clone();
        let mut k = 1;
        while out.iter().any(|v| v.name == unique) {
            unique = format!("{base}-{k}");
            k += 1;
        }
        out.push(Variant {
            name: unique,
            parts,
        });
    }
    out
}

/// A name safe for a file: letters, digits, `-` and `_`.
fn file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Adds a mesh's triangles, placed by `world`, to the parts of their materials.
fn add_mesh(
    mesh: &gltf::Mesh,
    world: Mat4,
    buffers: &[gltf::buffer::Data],
    looks: &[Look],
    parts: &mut Vec<Part>,
) {
    let normal_matrix = glam::Mat3::from_mat4(world).inverse().transpose();
    let flip = world.determinant() < 0.0;
    for prim in mesh.primitives() {
        if prim.mode() != gltf::mesh::Mode::Triangles {
            continue;
        }
        let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
        let Some(positions) = reader.read_positions() else {
            continue;
        };
        let material = prim.material().index().unwrap_or(looks.len());
        let at = match parts.iter().position(|p| p.material == material) {
            Some(i) => i,
            None => {
                parts.push(Part {
                    material,
                    ..Default::default()
                });
                parts.len() - 1
            }
        };
        let part = &mut parts[at];
        let first = part.positions.len() as u32;
        let positions: Vec<Vec3> = positions
            .map(|p| world.transform_point3(Vec3::from(p)))
            .collect();
        let count = positions.len();
        let mut indices: Vec<u32> = match reader.read_indices() {
            Some(i) => i.into_u32().collect(),
            None => (0..count as u32).collect(),
        };
        if flip {
            for t in indices.as_chunks_mut::<3>().0 {
                t.swap(1, 2);
            }
        }
        let normals: Vec<Vec3> = match reader.read_normals() {
            Some(n) => n
                .map(|n| (normal_matrix * Vec3::from(n)).normalize_or_zero())
                .collect(),
            None => vec![Vec3::Y; count],
        };
        let transform = looks.get(material).and_then(|l| l.uv);
        let uvs = reader
            .read_tex_coords(0)
            .map(|uv| {
                uv.into_f32()
                    .map(|uv| uv_transformed(Vec2::from(uv), transform))
                    .collect()
            })
            .unwrap_or_else(|| vec![Vec2::ZERO; count]);
        part.positions.extend(positions);
        part.normals.extend(normals);
        part.uvs.extend(uvs);
        part.indices.extend(indices.iter().map(|i| i + first));
    }
}

/// Texture coordinates with KHR_texture_transform's offset, rotation and scale applied.
fn uv_transformed(uv: Vec2, transform: Option<([f32; 2], f32, [f32; 2])>) -> Vec2 {
    let Some((offset, rotation, scale)) = transform else {
        return uv;
    };
    let (s, c) = rotation.sin_cos();
    let v = uv * Vec2::from(scale);
    Vec2::new(c * v.x + s * v.y, -s * v.x + c * v.y) + Vec2::from(offset)
}

/// The model's levels of detail: its parts at each.
fn levels(parts: &[Part], looks: &[Look], kind: Kind) -> Vec<Vec<Part>> {
    let total: usize = parts.iter().map(Part::triangles).sum();
    let first = (budget(kind) as f64 / total.max(1) as f64).min(1.0);
    let mut out: Vec<Vec<Part>> = Vec::new();
    for level in 0..LODS {
        let fraction = first * STEP.powi(level as i32);
        let reduced: Vec<Part> = parts
            .iter()
            .map(|p| {
                let target = ((p.triangles() as f64 * fraction).ceil() as usize).max(2);
                let leaves = looks.get(p.material).is_some_and(|l| l.leaves);
                if leaves {
                    thin_cards(p, target, kind == Kind::Grass)
                } else {
                    simplified(p, target, level)
                }
            })
            .filter(|p| !p.indices.is_empty())
            .collect();
        let triangles: usize = reduced.iter().map(Part::triangles).sum();
        // A level hardly lighter than the one before is not worth its memory, and one
        // with nothing left shows nothing.
        let lighter = out.last().is_none_or(|before| {
            (triangles as f64) < 0.8 * before.iter().map(Part::triangles).sum::<usize>() as f64
        });
        if triangles == 0 || !lighter {
            break;
        }
        out.push(reduced);
    }
    out
}

/// Indices of `part` simplified towards `target` indices within `error` (relative to
/// the part's size), keeping its texture coordinates where they were: a card keeps the
/// outline of what its texture shows, however thin it is.
fn simplify(part: &Part, target: usize, error: f32) -> Vec<u32> {
    let positions: Vec<[f32; 3]> = part.positions.iter().map(|p| p.to_array()).collect();
    let uvs: Vec<f32> = part.uvs.iter().flat_map(|uv| uv.to_array()).collect();
    let locks = vec![false; positions.len()];
    meshopt::simplify_with_attributes_and_locks_decoder(
        &part.indices,
        &positions,
        &uvs,
        &[1.0, 1.0],
        std::mem::size_of::<[f32; 2]>(),
        &locks,
        target,
        error,
        SimplifyOptions::None,
        None,
    )
}

/// A solid part simplified to about `target` triangles, within an error that grows
/// with the level.
fn simplified(part: &Part, target: usize, level: usize) -> Part {
    if part.triangles() <= target {
        return part.clone();
    }
    let error = [0.01, 0.04, 0.12][level.min(2)];
    part.compact(&simplify(part, target * 3, error))
}

/// Leaves brought to about `target` triangles: only some of their cards (or blades,
/// or sprigs of needles: the islands of triangles) kept, each grown about its foot
/// (grass, by its lowest point) or its middle to cover for the rest. Simplifying would
/// take them for slivers not worth keeping, and leave the crown bare.
fn thin_cards(part: &Part, target: usize, grass: bool) -> Part {
    let triangles = part.triangles();
    if triangles <= target {
        return part.clone();
    }
    let mut part = part.clone();
    let keep = target as f32 / triangles as f32;
    let grow = (1.0 / keep.sqrt()).min(MAX_GROWTH);
    let islands = islands(&part);
    let count = islands.iter().copied().max().map_or(0, |m| m as usize + 1);
    // Each island's pivot: its lowest point, or its middle.
    let mut pivot = vec![(Vec3::ZERO, 0u32); count];
    for (v, &i) in islands.iter().enumerate() {
        let p = part.positions[v];
        let (at, n) = &mut pivot[i as usize];
        if grass {
            if *n == 0 || p.y < at.y {
                *at = p;
            }
            *n = 1;
        } else {
            *at += p;
            *n += 1;
        }
    }
    let pivot: Vec<Vec3> = pivot
        .iter()
        .map(|&(p, n)| if grass { p } else { p / n.max(1) as f32 })
        .collect();
    let kept = |i: u32| hash01(i) < keep;
    for (v, &i) in islands.iter().enumerate() {
        if kept(i) {
            let c = pivot[i as usize];
            part.positions[v] = c + (part.positions[v] - c) * grow;
        }
    }
    let indices: Vec<u32> = part
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| kept(islands[t[0] as usize]))
        .flatten()
        .copied()
        .collect();
    part.compact(&indices)
}

/// Each vertex's island: the triangles joined to it through shared vertices.
fn islands(part: &Part) -> Vec<u32> {
    let mut parent: Vec<u32> = (0..part.positions.len() as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    for t in part.indices.as_chunks::<3>().0 {
        let a = root(&mut parent, t[0]);
        for &v in &t[1..] {
            let b = root(&mut parent, v);
            parent[b as usize] = a;
        }
    }
    let mut ids: HashMap<u32, u32> = HashMap::new();
    (0..parent.len() as u32)
        .map(|v| {
            let r = root(&mut parent, v);
            let next = ids.len() as u32;
            *ids.entry(r).or_insert(next)
        })
        .collect()
}

/// A number in [0, 1) that `i` always gives.
fn hash01(i: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

pub(crate) fn png(image: &Image, alpha: bool) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut enc = ::png::Encoder::new(&mut out, image.width as u32, image.height as u32);
    enc.set_color(if alpha {
        ::png::ColorType::Rgba
    } else {
        ::png::ColorType::Rgb
    });
    enc.set_depth(::png::BitDepth::Eight);
    enc.set_compression(::png::Compression::Fast);
    let data: Vec<u8> = if alpha {
        image.pixels.clone()
    } else {
        image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect()
    };
    let fail = |e: &dyn std::fmt::Display| Error::Invalid(format!("PNG: {e}"));
    let mut w = enc.write_header().map_err(|e| fail(&e))?;
    w.write_image_data(&data).map_err(|e| fail(&e))?;
    w.finish().map_err(|e| fail(&e))?;
    Ok(out)
}

/// A binary glTF being written: its JSON's lists and its buffer.
#[derive(Default)]
struct Glb {
    views: Vec<serde_json::Value>,
    accessors: Vec<serde_json::Value>,
    bin: Vec<u8>,
}

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;

impl Glb {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = serde_json::json!({
            "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()
        });
        if let Some(t) = target {
            v["target"] = t.into();
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn floats(&mut self, values: &[f32], width: usize, bounds: bool) -> usize {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(ARRAY_BUFFER));
        let kind = ["", "SCALAR", "VEC2", "VEC3", "VEC4"][width];
        let mut a = serde_json::json!({
            "bufferView": view, "componentType": FLOAT, "count": values.len() / width,
            "type": kind,
        });
        if bounds {
            let (mut lo, mut hi) = (vec![f32::MAX; width], vec![f32::MIN; width]);
            for c in values.chunks(width) {
                for k in 0..width {
                    lo[k] = lo[k].min(c[k]);
                    hi[k] = hi[k].max(c[k]);
                }
            }
            a["min"] = lo.into();
            a["max"] = hi.into();
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn indices(&mut self, indices: &[u32]) -> usize {
        let bytes: Vec<u8> = indices.iter().flat_map(|v| v.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(ELEMENT_ARRAY_BUFFER));
        self.accessors.push(serde_json::json!({
            "bufferView": view, "componentType": UNSIGNED_INT, "count": indices.len(), "type": "SCALAR"
        }));
        self.accessors.len() - 1
    }
}

/// A model's levels of detail as a binary glTF: a node `<name>_LOD<n>` for each.
fn write_glb(
    name: &str,
    levels: &[Vec<Part>],
    looks: &[Look],
    copyright: &str,
) -> Result<Vec<u8>, Error> {
    let mut glb = Glb::default();
    // Only the materials the model uses, in their order in the file.
    let mut used: Vec<usize> = levels.iter().flatten().map(|p| p.material).collect();
    used.sort_unstable();
    used.dedup();
    let mut images = Vec::new();
    let mut materials = Vec::new();
    for &m in &used {
        let Some(look) = looks.get(m) else {
            materials.push(serde_json::json!({ "name": "material" }));
            continue;
        };
        let mut image = |i: &Image, alpha: bool| -> Result<usize, Error> {
            let view = glb.view(&png(i, alpha)?, None);
            images.push(serde_json::json!({ "bufferView": view, "mimeType": "image/png" }));
            Ok(images.len() - 1)
        };
        let mut pbr = serde_json::json!({
            "baseColorFactor": look.color, "metallicFactor": 0.0, "roughnessFactor": look.roughness,
        });
        if let Some(b) = &look.base {
            pbr["baseColorTexture"] =
                serde_json::json!({ "index": image(b, look.cutoff.is_some())? });
        }
        let mut mat = serde_json::json!({
            "name": look.name, "pbrMetallicRoughness": pbr, "doubleSided": look.double_sided,
        });
        if let Some(n) = &look.normal {
            mat["normalTexture"] = serde_json::json!({ "index": image(n, false)? });
        }
        if let Some(c) = look.cutoff {
            mat["alphaMode"] = "MASK".into();
            mat["alphaCutoff"] = c.into();
        }
        materials.push(mat);
    }
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    for (l, parts) in levels.iter().enumerate() {
        let mut primitives = Vec::new();
        for p in parts {
            let pos: Vec<f32> = p.positions.iter().flat_map(|v| v.to_array()).collect();
            let nor: Vec<f32> = p.normals.iter().flat_map(|v| v.to_array()).collect();
            let uv: Vec<f32> = p.uvs.iter().flat_map(|v| v.to_array()).collect();
            let (a, b, c) = (
                glb.floats(&pos, 3, true),
                glb.floats(&nor, 3, false),
                glb.floats(&uv, 2, false),
            );
            let i = glb.indices(&p.indices);
            let mut prim = serde_json::json!({
                "attributes": { "POSITION": a, "NORMAL": b, "TEXCOORD_0": c }, "indices": i,
            });
            if let Some(m) = used.iter().position(|&u| u == p.material) {
                prim["material"] = m.into();
            }
            primitives.push(prim);
        }
        meshes.push(
            serde_json::json!({ "name": format!("{name}_LOD{l}"), "primitives": primitives }),
        );
        nodes.push(serde_json::json!({ "name": format!("{name}_LOD{l}"), "mesh": l }));
    }
    let textures: Vec<serde_json::Value> = (0..images.len())
        .map(|i| serde_json::json!({ "source": i }))
        .collect();
    let mut json = serde_json::json!({
        "asset": { "version": "2.0", "generator": "open-racing prepare", "copyright": copyright },
        "scene": 0,
        "scenes": [{ "nodes": (0..nodes.len()).collect::<Vec<_>>() }],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": glb.accessors,
        "bufferViews": glb.views,
        "buffers": [{ "byteLength": glb.bin.len() }],
    });
    if !images.is_empty() {
        json["images"] = images.into();
        json["textures"] = textures.into();
    }
    let mut text = serde_json::to_vec(&json).map_err(|e| Error::Invalid(e.to_string()))?;
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let mut bin = glb.bin;
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut out = Vec::with_capacity(28 + text.len() + bin.len());
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((28 + text.len() + bin.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A glTF of two plants side by side, each of `cards` square leaf cards blended by
    /// a colour map whose alpha is in a map of its own beside it, as Poly Haven's are.
    fn plants(dir: &Path, cards: usize) -> PathBuf {
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        let image = |pixels: Vec<u8>| Image {
            width: 4,
            height: 4,
            pixels,
        };
        let colour = image([40, 160, 40, 255].repeat(16));
        let alpha: Vec<u8> = (0..16)
            .flat_map(|i| {
                let a = if i % 2 == 0 { 255 } else { 0 };
                [a, a, a, 255]
            })
            .collect();
        std::fs::write(
            dir.join("textures/plant_diff_1k.png"),
            png(&colour, false).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("textures/plant_alpha_1k.png"),
            png(&image(alpha), false).unwrap(),
        )
        .unwrap();
        // Cards in a column, each its own island.
        let (mut pos, mut uv, mut idx) = (Vec::<f32>::new(), Vec::<f32>::new(), Vec::<u32>::new());
        for c in 0..cards {
            let y = c as f32 * 0.1;
            let base = (pos.len() / 3) as u32;
            for (x, z, u, v) in [
                (0.0, 0.0, 0.0, 0.0),
                (0.2, 0.0, 1.0, 0.0),
                (0.2, 0.2, 1.0, 1.0),
                (0.0, 0.2, 0.0, 1.0),
            ] {
                pos.extend([x - 0.1, y + z, 0.0]);
                uv.extend([u, v]);
            }
            idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let mut bin: Vec<u8> = pos
            .iter()
            .chain(&uv)
            .flat_map(|f| f.to_le_bytes())
            .collect();
        bin.extend(idx.iter().flat_map(|i| i.to_le_bytes()));
        std::fs::write(dir.join("plant.bin"), &bin).unwrap();
        let n = pos.len() / 3;
        let (lo, hi) = pos
            .chunks(3)
            .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
                (
                    std::array::from_fn(|k| lo[k].min(p[k])),
                    std::array::from_fn(|k| hi[k].max(p[k])),
                )
            });
        let json = serde_json::json!({
            "asset": { "version": "2.0" },
            "scene": 0,
            "scenes": [{ "nodes": [0, 1] }],
            "nodes": [
                { "name": "plant_a_LOD0", "mesh": 0 },
                { "name": "plant_b_LOD0", "mesh": 0, "translation": [5.0, 0.0, 0.0] },
            ],
            "meshes": [{ "primitives": [{
                "attributes": { "POSITION": 0, "TEXCOORD_0": 1 }, "indices": 2, "material": 0,
            }] }],
            "materials": [{
                "name": "plant_leaves", "alphaMode": "BLEND", "doubleSided": true,
                "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } },
            }],
            "textures": [{ "source": 0 }],
            "images": [{ "uri": "textures/plant_diff_1k.png" }],
            "accessors": [
                { "bufferView": 0, "componentType": FLOAT, "count": n, "type": "VEC3", "min": lo, "max": hi },
                { "bufferView": 1, "componentType": FLOAT, "count": n, "type": "VEC2" },
                { "bufferView": 2, "componentType": UNSIGNED_INT, "count": idx.len(), "type": "SCALAR" },
            ],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": pos.len() * 4 },
                { "buffer": 0, "byteOffset": pos.len() * 4, "byteLength": uv.len() * 4 },
                { "buffer": 0, "byteOffset": (pos.len() + uv.len()) * 4, "byteLength": idx.len() * 4 },
            ],
            "buffers": [{ "uri": "plant.bin", "byteLength": bin.len() }],
        });
        let path = dir.join("plant_1k.gltf");
        std::fs::write(&path, serde_json::to_string(&json).unwrap()).unwrap();
        path
    }

    #[test]
    fn splits_variants_restores_alpha_and_makes_levels() {
        let dir = std::env::temp_dir().join(format!("open-racing-prepare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gltf = plants(&dir.join("raw"), 20_000);
        let out = dir.join("out");
        let made = prepare(&gltf, &out, "plant", Kind::Evergreen, "test (CC0)").unwrap();
        let names: Vec<&str> = made.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["plant_a", "plant_b"]);
        for m in &made {
            assert_eq!(m.source_triangles, 40_000);
            // To the budget, then each level a quarter of the one before.
            let t = &m.triangles;
            assert!(
                t[0] <= budget(Kind::Evergreen) + 1000 && t[0] > budget(Kind::Evergreen) / 2,
                "{t:?}"
            );
            assert!(t.windows(2).all(|w| w[1] < w[0] / 2), "{t:?}");
            let model = crate::model::load(&m.path).unwrap();
            assert_eq!(model.lods.len(), t.len() - 1);
            assert_eq!(model.triangles, t[0]);
            // Where it was made, not where the file set it out.
            let [lo, hi] = model.bounds;
            assert!(lo.x > -1.0 && hi.x < 1.0, "{lo} {hi}");
            let leaves = &model.look.materials[0];
            assert_eq!(leaves.alpha_mode, open_racing_track::AlphaMode::Mask(0.5));
            assert!(leaves.varies.is_some_and(|v| v.tinted));
            let t = leaves.base_color_texture.unwrap() as usize;
            let image = texture::decode(&model.look.textures[t].data).unwrap();
            assert!(image.pixels.chunks(4).any(|p| p[3] < 128), "alpha restored");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn names_levels_of_detail() {
        assert_eq!(crate::model::lod_of("fir_tree_01_a_LOD0"), Some(0));
        assert_eq!(crate::model::lod_of("rock_lod2"), Some(2));
        assert_eq!(crate::model::lod_of("fir_tree_01_a"), None);
        assert_eq!(crate::model::lod_of("LOD1"), None);
    }
}
