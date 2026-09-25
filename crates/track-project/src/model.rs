//! 3D models for props: glTF files (.glb, or .gltf with its buffers and images) read into
//! meshes and materials in the simulation's frame (Z up), and placed in the scene.

use std::collections::HashMap;
use std::path::Path;

use glam::{DMat3, DVec3, Mat3, Mat4, Vec3};
use open_racing_sim::GroundMesh;
use open_racing_track::texture::{self, Image};
use open_racing_track::{AlphaMode, Material, Mesh, Texture, Visual, VisualBuilder};

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
