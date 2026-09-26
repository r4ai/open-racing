//! 3D models for props: glTF files (.glb, or .gltf with its buffers and images) read into
//! meshes and materials in the simulation's frame (Z up), and placed in the scene.

use std::collections::HashMap;
use std::path::Path;

use glam::{DMat3, DVec3, Mat3, Mat4, Vec3};
use open_racing_sim::GroundMesh;
use open_racing_track::texture::{self, Image};
use open_racing_track::{AlphaMode, Material, Mesh, Texture, Varies, Visual, VisualBuilder};

use crate::Error;
use crate::project::Prop;

/// A model as its file describes it, in its own frame.
#[derive(Debug)]
pub struct Model {
    /// Meshes with materials indexing `look.materials`.
    pub meshes: Vec<Mesh>,
    /// The model's materials and their textures (no meshes).
    pub look: Visual,
    pub triangles: usize,
    /// Corners of the box round it, m.
    pub bounds: [Vec3; 2],
}

/// glTF's Y-up frame to the simulation's Z-up one: a quarter turn about X.
fn z_up(v: Vec3) -> Vec3 {
    Vec3::new(v.x, -v.z, v.y)
}

fn rgba(data: &gltf::image::Data) -> Option<Image> {
    use gltf::image::Format as F;
    let px = &data.pixels;
    let pixels: Vec<u8> = match data.format {
        F::R8G8B8A8 => px.clone(),
        F::R8G8B8 => px
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        F::R8G8 => px
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        F::R8 => px.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        _ => return None,
    };
    Some(Image {
        width: data.width as usize,
        height: data.height as usize,
        pixels,
    })
}

/// Reads a glTF file.
pub fn load(path: &Path) -> Result<Model, Error> {
    let fail = |e: &dyn std::fmt::Display| Error::Invalid(format!("{}: {e}", path.display()));
    let (doc, buffers, images) = gltf::import(path).map_err(|e| fail(&e))?;

    let mut look = VisualBuilder::new();
    let mut textures: HashMap<usize, Option<u32>> = HashMap::new();
    let mut texture = |look: &mut VisualBuilder, t: gltf::Texture| {
        let i = t.source().index();
        *textures.entry(i).or_insert_with(|| {
            let image = rgba(images.get(i)?)?;
            Some(look.add_texture(Texture {
                data: texture::encode(image),
            }))
        })
    };
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        let base_color_texture = pbr
            .base_color_texture()
            .and_then(|t| texture(&mut look, t.texture()));
        let normal_texture = m
            .normal_texture()
            .and_then(|t| texture(&mut look, t.texture()));
        look.add_material(Material {
            base_color: pbr.base_color_factor(),
            base_color_texture,
            roughness: pbr.roughness_factor(),
            normal_texture,
            alpha_mode: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                gltf::material::AlphaMode::Mask => AlphaMode::Mask(m.alpha_cutoff().unwrap_or(0.5)),
                gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            },
            double_sided: m.double_sided(),
            varies: leaves(&m).then_some(Varies {
                tinted: true,
                ..Default::default()
            }),
            ..Default::default()
        });
    }
    // For primitives without a material.
    let plain = look.add_material(Material::default());

    let mut meshes = Vec::new();
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    let mut stack: Vec<(gltf::Node, Mat4)> = scene
        .iter()
        .flat_map(|s| s.nodes())
        .map(|n| (n, Mat4::IDENTITY))
        .collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        stack.extend(node.children().map(|c| (c, world)));
        let Some(mesh) = node.mesh() else { continue };
        let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
        let flip = world.determinant() < 0.0;
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let positions: Vec<Vec3> = positions
                .map(|p| z_up(world.transform_point3(Vec3::from(p))))
                .collect();
            let mut indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };
            if flip {
                for t in indices.as_chunks_mut::<3>().0 {
                    t.swap(1, 2);
                }
            }
            let normals: Vec<Vec3> = match reader.read_normals() {
                Some(n) => n
                    .map(|n| z_up(normal_matrix * Vec3::from(n)).normalize_or_zero())
                    .collect(),
                None => face_normals(&positions, &indices),
            };
            let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
                Some(uv) => uv.into_f32().collect(),
                None => vec![[0.0; 2]; positions.len()],
            };
            meshes.push(Mesh {
                material: prim.material().index().map_or(plain, |i| i as u32),
                cast_shadows: true,
                positions: positions.iter().map(|p| p.to_array()).collect(),
                normals: normals.iter().map(|n| n.to_array()).collect(),
                uvs,
                indices,
            });
        }
    }
    let triangles = meshes.iter().map(|m| m.indices.len() / 3).sum();
    let bounds = meshes
        .iter()
        .flat_map(|m| &m.positions)
        .fold([Vec3::MAX, Vec3::MIN], |[lo, hi], p| {
            [lo.min(Vec3::from(*p)), hi.max(Vec3::from(*p))]
        });
    if meshes.is_empty() {
        return Err(fail(&"no triangles"));
    }
    Ok(Model {
        meshes,
        look: look.build(),
        triangles,
        bounds,
    })
}

/// Whether a material of a plant's model is its leaves: cut out by its alpha, as leaf
/// cards are, or named as leaves.
fn leaves(m: &gltf::Material) -> bool {
    let name = m.name().unwrap_or_default().to_lowercase();
    m.alpha_mode() != gltf::material::AlphaMode::Opaque
        || [
            "leaf", "leav", "foliage", "needle", "grass", "canopy", "frond", "twig",
        ]
        .iter()
        .any(|w| name.contains(w))
}

/// Vertex normals averaged from the faces round each vertex.
fn face_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for t in indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        let n = (b - a).cross(c - a);
        for &i in t {
            normals[i as usize] += n;
        }
    }
    normals.iter().map(|n| n.normalize_or(Vec3::Z)).collect()
}

/// Where a prop stands: its position (on the ground if it drapes), its turn and size.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub pos: DVec3,
    pub yaw: f64,
    pub scale: f64,
}

impl Placement {
    pub fn of(prop: &Prop, ground: Option<&GroundMesh>) -> Self {
        let pos = match ground.filter(|_| prop.drape) {
            Some(g) => g
                .raycast_down(prop.pos, 3.0)
                .or_else(|| g.raycast_down(prop.pos, 1e4))
                .map_or(prop.pos, |h| h.point),
            None => prop.pos,
        };
        Self {
            pos,
            yaw: prop.yaw,
            scale: prop.scale,
        }
    }

    fn turn(&self) -> DMat3 {
        DMat3::from_rotation_z(self.yaw)
    }

    /// A point of the model in the world.
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let p = self.pos + self.turn() * (DVec3::from(p.map(f64::from)) * self.scale);
        p.as_vec3().to_array()
    }

    pub fn normal(&self, n: [f32; 3]) -> [f32; 3] {
        (self.turn() * DVec3::from(n.map(f64::from)))
            .as_vec3()
            .to_array()
    }
}

/// Copies of a model repeated along a wall's line, as meshes in the world with the
/// model's own materials. Each stretch holds a whole number of copies, each stretched a
/// little along the line to fit; a bent copy follows the line, a straight one stands on
/// the chord between its ends. Copies are merged into meshes of a few dozen each, so
/// that far ones can be culled.
pub fn along(model: &Model, line: &crate::road::ModelLine) -> Vec<Mesh> {
    const COPIES_PER_MESH: usize = 32;
    let [lo, hi] = model.bounds;
    let own = (hi.x - lo.x).max(0.05) as f64;
    let piece = if line.run.length > 0.0 {
        line.run.length
    } else {
        own
    };
    let mut out = Vec::new();
    for stretch in line.stretches.iter().filter(|s| s.len() > 1) {
        // Distance along the stretch at each point.
        let mut at = vec![0.0];
        for w in stretch.windows(2) {
            at.push(at.last().unwrap() + w[0].pos.distance(w[1].pos));
        }
        let length = *at.last().unwrap();
        if length < 0.1 {
            continue;
        }
        let point = |s: f64| {
            let i = at.partition_point(|&a| a <= s).clamp(1, at.len() - 1);
            let (a, b) = (stretch[i - 1], stretch[i]);
            let t = ((s - at[i - 1]) / (at[i] - at[i - 1]).max(1e-9)).clamp(0.0, 1.0);
            let along = (b.pos - a.pos).normalize_or(DVec3::X);
            let toward = a.toward.lerp(b.toward, t).normalize_or(DVec3::Y);
            (a.pos.lerp(b.pos, t), along, toward)
        };
        let copies = (length / piece).round().max(1.0) as usize;
        let each = length / copies as f64;
        let stretch_x = each / own;
        for first in (0..copies).step_by(COPIES_PER_MESH) {
            let last = (first + COPIES_PER_MESH).min(copies);
            for m in &model.meshes {
                let mut mesh = Mesh {
                    material: m.material,
                    cast_shadows: m.cast_shadows,
                    positions: vec![],
                    normals: vec![],
                    uvs: vec![],
                    indices: vec![],
                };
                for k in first..last {
                    let base = mesh.positions.len() as u32;
                    let s0 = k as f64 * each;
                    let (start, _, _) = point(s0);
                    let (end, _, _) = point(s0 + each);
                    let (_, _, facing) = point(s0 + 0.5 * each);
                    let chord = (end - start).normalize_or(DVec3::X);
                    for (p, n) in m.positions.iter().zip(&m.normals) {
                        let x = (p[0] - lo.x) as f64 * stretch_x;
                        let (origin, along, toward) = if line.run.bend {
                            let (o, a, t) = point(s0 + x);
                            (o - a * x, a, t)
                        } else {
                            (start, chord, facing)
                        };
                        // The model's +Y faces the road, level; +Z stays up.
                        let side = (toward - along * toward.dot(along)).normalize_or(toward);
                        let q = origin + along * x + side * p[1] as f64 + DVec3::Z * p[2] as f64;
                        let n = along * n[0] as f64 + side * n[1] as f64 + DVec3::Z * n[2] as f64;
                        mesh.positions.push(q.as_vec3().to_array());
                        mesh.normals
                            .push(n.normalize_or(DVec3::Z).as_vec3().to_array());
                    }
                    mesh.uvs.extend(&m.uvs);
                    // Facing the road on a line's right mirrors the model: its
                    // triangles then wind the other way round to keep facing out.
                    let mirrored = chord.cross(facing).z < 0.0;
                    for t in m.indices.as_chunks::<3>().0 {
                        let [a, b, c] = t.map(|i| i + base);
                        mesh.indices
                            .extend(if mirrored { [a, c, b] } else { [a, b, c] });
                    }
                }
                if !mesh.indices.is_empty() {
                    out.push(mesh);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::ModelRun;
    use crate::road::{LinePoint, ModelLine};

    /// A unit box, a metre along +X, faces winding outwards.
    fn cube() -> Model {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let n =
                    Vec3::from_array(std::array::from_fn(|i| if i == axis { sign } else { 0.0 }));
                let (u, v) = (
                    n.any_orthonormal_vector(),
                    n.cross(n.any_orthonormal_vector()),
                );
                let base = positions.len() as u32;
                for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let p = (n + u * a + v * b) * 0.5 + Vec3::new(0.5, 0.0, 0.5);
                    positions.push(p.to_array());
                    normals.push(n.to_array());
                }
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        let n = positions.len();
        Model {
            meshes: vec![Mesh {
                material: 0,
                cast_shadows: true,
                positions,
                normals,
                uvs: vec![[0.0; 2]; n],
                indices,
            }],
            look: VisualBuilder::new().build(),
            triangles: 12,
            bounds: [Vec3::new(0.0, -0.5, 0.0), Vec3::new(1.0, 0.5, 1.0)],
        }
    }

    #[test]
    fn copies_face_outwards_whichever_side_of_the_line_they_face() {
        let model = cube();
        for toward in [DVec3::Y, -DVec3::Y] {
            let line = ModelLine {
                run: ModelRun {
                    model: "cube.glb".into(),
                    length: 0.0,
                    bend: true,
                    flip: false,
                },
                stretches: vec![
                    (0..=4)
                        .map(|k| LinePoint {
                            pos: DVec3::new(k as f64, 0.0, 0.0),
                            toward,
                        })
                        .collect(),
                ],
            };
            for m in along(&model, &line) {
                for t in m.indices.as_chunks::<3>().0 {
                    let p = t.map(|i| Vec3::from(m.positions[i as usize]));
                    let face = (p[1] - p[0]).cross(p[2] - p[0]);
                    let n = Vec3::from(m.normals[t[0] as usize]);
                    assert!(face.dot(n) > 0.0, "toward {toward}: {face} vs {n}");
                }
            }
        }
    }
}
