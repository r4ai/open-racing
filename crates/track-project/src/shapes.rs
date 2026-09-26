//! Built-in models the tool makes itself, so that a project can have woods, bushes,
//! rocks and cones without a model file: low in triangles, for thousands of copies.
//! A project names them as `builtin:pine` wherever it takes a model's path.

use std::path::Path;

use glam::Vec3;
use open_racing_track::{Material, Mesh, Varies, VisualBuilder};

use crate::model::Model;
use crate::project::Kind;

/// What a path to a built-in model starts with.
pub const PREFIX: &str = "builtin:";

/// The built-in models, with what each is.
pub const BUILTIN: [(&str, &str); 8] = [
    ("pine", "a conifer, about 11 m tall"),
    ("tree", "a broadleaf tree, about 9 m tall"),
    ("poplar", "a tall, narrow tree, about 12 m"),
    ("bush", "a bush, about 1.4 m"),
    ("rock", "a boulder, about 1.2 m"),
    ("grass", "a tuft of long grass, about 0.6 m"),
    ("cone", "a traffic cone, 0.7 m"),
    ("spectator", "a spectator standing, facing +X, about 1.75 m"),
];

/// What kind of plant the built-in model `name` is.
pub fn kind(name: &str) -> Option<Kind> {
    Some(match name {
        "pine" => Kind::Evergreen,
        "tree" | "poplar" | "bush" => Kind::Deciduous,
        "grass" => Kind::Grass,
        "rock" | "cone" => Kind::Rigid,
        "spectator" => Kind::Crowd,
        _ => return None,
    })
}

/// The built-in model's name, if `path` names one.
pub fn name(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix(PREFIX)
}

/// The path that names a built-in model.
pub fn path(name: &str) -> std::path::PathBuf {
    format!("{PREFIX}{name}").into()
}

/// Colours of the parts, linear.
const BARK: [f32; 3] = [0.045, 0.026, 0.014];
const NEEDLES: [f32; 3] = [0.006, 0.026, 0.01];
const LEAVES: [f32; 3] = [0.022, 0.06, 0.007];
const POPLAR: [f32; 3] = [0.03, 0.075, 0.01];
const SHRUB: [f32; 3] = [0.025, 0.05, 0.012];
const STONE: [f32; 3] = [0.12, 0.11, 0.095];
const STRAW: [f32; 3] = [0.09, 0.1, 0.025];
const ORANGE: [f32; 3] = [0.9, 0.12, 0.01];
const WHITE: [f32; 3] = [0.8, 0.8, 0.8];
const SKIN: [f32; 3] = [0.45, 0.28, 0.2];
const TROUSERS: [f32; 3] = [0.03, 0.035, 0.05];
/// Clothes of a luminance of 0.2, which each copy's colour replaces.
const CLOTHES: [f32; 3] = [0.2, 0.2, 0.2];

/// Triangles of one material.
struct Part {
    colour: [f32; 3],
    roughness: f32,
    /// It takes each copy's colour: a plant's leaves, clothes.
    tinted: bool,
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
}

impl Part {
    fn new(colour: [f32; 3], roughness: f32) -> Self {
        Self {
            colour,
            roughness,
            tinted: false,
            positions: vec![],
            normals: vec![],
            indices: vec![],
        }
    }

    /// It takes each copy's colour: a plant's leaves, clothes.
    fn tinted(colour: [f32; 3], roughness: f32) -> Self {
        Self {
            tinted: true,
            ..Self::new(colour, roughness)
        }
    }

    fn vertex(&mut self, p: Vec3, n: Vec3) -> u32 {
        self.positions.push(p);
        self.normals.push(n.normalize_or(Vec3::Z));
        self.positions.len() as u32 - 1
    }

    /// A cone (or, with a top radius, a tapered cylinder) standing on `base`, with a
    /// closed bottom.
    fn cone(&mut self, base: Vec3, r0: f32, r1: f32, height: f32, sides: usize) {
        let slope = (r0 - r1) / height;
        let ring = |a: f32| Vec3::new(a.cos(), a.sin(), 0.0);
        let first = self.positions.len() as u32;
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * std::f32::consts::TAU;
            let d = ring(a);
            let n = d + Vec3::Z * slope;
            self.vertex(base + d * r0, n);
            self.vertex(base + d * r1 + Vec3::Z * height, n);
        }
        for k in 0..sides as u32 {
            let (a, b, c, d) = (
                first + 2 * k,
                first + 2 * k + 2,
                first + 2 * k + 1,
                first + 2 * k + 3,
            );
            if r1 > 0.0 {
                self.indices.extend([a, b, d, a, d, c]);
            } else {
                self.indices.extend([a, b, c]);
            }
        }
        // The bottom, facing down.
        let middle = self.vertex(base, -Vec3::Z);
        let rim = self.positions.len() as u32;
        for k in 0..sides {
            let a = k as f32 / sides as f32 * std::f32::consts::TAU;
            self.vertex(base + ring(a) * r0, -Vec3::Z);
        }
        for k in 0..sides as u32 {
            self.indices
                .extend([middle, rim + (k + 1) % sides as u32, rim + k]);
        }
    }

    /// A lumpy ball round `centre` of radii `r`, its surface pushed in and out by up to
    /// `lumps` of itself, the same way for the same `seed`.
    fn blob(&mut self, centre: Vec3, r: Vec3, lumps: f32, seed: u32) {
        let (points, faces) = sphere();
        let first = self.positions.len() as u32;
        for (i, &d) in points.iter().enumerate() {
            let k = 1.0 + lumps * wobble(seed, i as u32);
            let p = centre + d * r * k;
            // Smooth shading of the lumpy ball: its own direction, a little flattened.
            self.vertex(p, d / r);
        }
        for [a, b, c] in faces {
            self.indices.extend([first + a, first + b, first + c]);
        }
    }

    /// A blade of grass: a thin triangle from `base`, leaning towards `lean`.
    fn blade(&mut self, base: Vec3, lean: Vec3, height: f32, width: f32) {
        let side = Vec3::Z.cross(lean).normalize_or(Vec3::X) * (0.5 * width);
        let top = base + lean * (0.4 * height) + Vec3::Z * height;
        let n = side.cross(top - base).normalize_or(Vec3::Z);
        let n = if n.z < 0.0 { -n } else { n };
        let (a, b, c) = (
            self.vertex(base - side, n),
            self.vertex(base + side, n),
            self.vertex(top, n),
        );
        // Both faces, as a blade is seen from either side.
        self.indices.extend([a, b, c, a, c, b]);
    }
}

/// A number in [-1, 1], the same for the same seed and index.
fn wobble(seed: u32, i: u32) -> f32 {
    let mut h = seed.wrapping_mul(0x9e37_79b9) ^ i.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 32767.5 - 1.0
}

/// A unit sphere: an icosahedron split once, its points pushed out onto the sphere.
fn sphere() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut points: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .map(|(x, y, z)| Vec3::new(x, y, z).normalize())
    .to_vec();
    let faces: [[u32; 3]; 20] = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    let mut middles = std::collections::HashMap::new();
    let mut middle = |a: u32, b: u32, points: &mut Vec<Vec3>| {
        *middles.entry((a.min(b), a.max(b))).or_insert_with(|| {
            points.push(((points[a as usize] + points[b as usize]) * 0.5).normalize());
            points.len() as u32 - 1
        })
    };
    let mut out = Vec::with_capacity(80);
    for [a, b, c] in faces {
        let ab = middle(a, b, &mut points);
        let bc = middle(b, c, &mut points);
        let ca = middle(c, a, &mut points);
        out.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
    }
    (points, out)
}

/// The built-in model called `name`.
pub fn model(name: &str) -> Option<Model> {
    let parts = match name {
        "pine" => {
            let mut trunk = Part::new(BARK, 0.9);
            trunk.cone(Vec3::ZERO, 0.28, 0.12, 3.0, 7);
            let mut needles = Part::tinted(NEEDLES, 0.95);
            for (z, r, h) in [(1.6, 2.9, 4.6), (4.0, 2.2, 4.2), (6.4, 1.5, 4.4)] {
                needles.cone(Vec3::Z * z, r, 0.0, h, 9);
            }
            vec![trunk, needles]
        }
        "tree" => {
            let mut trunk = Part::new(BARK, 0.9);
            trunk.cone(Vec3::ZERO, 0.32, 0.18, 4.2, 7);
            let mut leaves = Part::tinted(LEAVES, 0.9);
            leaves.blob(Vec3::new(0.0, 0.0, 5.9), Vec3::new(3.1, 3.1, 2.7), 0.14, 1);
            leaves.blob(Vec3::new(0.9, 0.6, 7.3), Vec3::new(2.1, 2.1, 1.8), 0.14, 2);
            leaves.blob(
                Vec3::new(-1.1, -0.4, 6.8),
                Vec3::new(1.9, 1.9, 1.7),
                0.14,
                3,
            );
            vec![trunk, leaves]
        }
        "poplar" => {
            let mut trunk = Part::new(BARK, 0.9);
            trunk.cone(Vec3::ZERO, 0.25, 0.12, 3.0, 7);
            let mut leaves = Part::tinted(POPLAR, 0.9);
            leaves.blob(Vec3::new(0.0, 0.0, 7.2), Vec3::new(1.5, 1.5, 5.0), 0.1, 4);
            vec![trunk, leaves]
        }
        "bush" => {
            let mut leaves = Part::tinted(SHRUB, 0.95);
            leaves.blob(Vec3::new(0.0, 0.0, 0.55), Vec3::new(1.3, 1.1, 0.85), 0.2, 5);
            leaves.blob(Vec3::new(0.6, 0.3, 0.8), Vec3::new(0.7, 0.7, 0.55), 0.2, 6);
            vec![leaves]
        }
        "rock" => {
            let mut stone = Part::new(STONE, 0.85);
            stone.blob(
                Vec3::new(0.0, 0.0, 0.25),
                Vec3::new(1.1, 0.85, 0.7),
                0.25,
                7,
            );
            vec![stone]
        }
        "grass" => {
            let mut blades = Part::tinted(STRAW, 0.95);
            for k in 0..9u32 {
                let a = k as f32 * 2.4;
                let at = Vec3::new(a.cos(), a.sin(), 0.0) * (0.06 + 0.02 * k as f32);
                let lean = Vec3::new((a + 0.6).cos(), (a + 0.6).sin(), 0.0);
                blades.blade(at, lean, 0.45 + 0.03 * (k % 4) as f32, 0.07);
            }
            vec![blades]
        }
        "cone" => {
            let mut orange = Part::new(ORANGE, 0.6);
            orange.cone(Vec3::Z * 0.03, 0.17, 0.11, 0.25, 12);
            orange.cone(Vec3::Z * 0.45, 0.07, 0.025, 0.25, 12);
            orange.cone(Vec3::ZERO, 0.24, 0.24, 0.03, 4);
            let mut white = Part::new(WHITE, 0.5);
            white.cone(Vec3::Z * 0.28, 0.11, 0.07, 0.17, 12);
            vec![orange, white]
        }
        "spectator" => {
            let mut legs = Part::new(TROUSERS, 0.8);
            for y in [-0.1, 0.1] {
                legs.cone(Vec3::new(0.0, y, 0.0), 0.085, 0.075, 0.86, 6);
            }
            let mut shirt = Part::tinted(CLOTHES, 0.85);
            shirt.cone(Vec3::Z * 0.84, 0.19, 0.22, 0.62, 8);
            for y in [-0.27, 0.27] {
                shirt.cone(Vec3::new(0.02, y, 0.82), 0.05, 0.065, 0.62, 5);
            }
            let mut skin = Part::new(SKIN, 0.6);
            skin.blob(Vec3::new(0.0, 0.0, 1.6), Vec3::new(0.1, 0.09, 0.12), 0.05, 9);
            vec![legs, shirt, skin]
        }
        _ => return None,
    };
    Some(assemble(parts))
}

/// A model of the parts, one material each.
fn assemble(parts: Vec<Part>) -> Model {
    let mut look = VisualBuilder::new();
    let mut meshes = Vec::new();
    for (i, part) in parts.into_iter().enumerate() {
        let [r, g, b] = part.colour;
        look.add_material(Material {
            base_color: [r, g, b, 1.0],
            roughness: part.roughness,
            reflectance: 0.3,
            varies: part.tinted.then_some(Varies {
                tinted: true,
                ..Default::default()
            }),
            ..Default::default()
        });
        let n = part.positions.len();
        meshes.push(Mesh {
            material: i as u32,
            cast_shadows: true,
            positions: part.positions.iter().map(|p| p.to_array()).collect(),
            normals: part.normals.iter().map(|n| n.to_array()).collect(),
            uvs: vec![[0.0; 2]; n],
            indices: part.indices,
        });
    }
    let triangles = meshes.iter().map(|m| m.indices.len() / 3).sum();
    let bounds = meshes
        .iter()
        .flat_map(|m| &m.positions)
        .fold([Vec3::MAX, Vec3::MIN], |[lo, hi], p| {
            [lo.min(Vec3::from(*p)), hi.max(Vec3::from(*p))]
        });
    Model {
        meshes,
        look: look.build(),
        triangles,
        bounds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_model_is_made_facing_out_and_standing_on_its_origin() {
        for (name, _) in BUILTIN {
            let m = model(name).unwrap_or_else(|| panic!("{name}"));
            assert!(
                m.triangles > 0 && m.triangles < 400,
                "{name}: {}",
                m.triangles
            );
            // It stands on the ground at its origin, bushes and rocks sunk into it.
            assert!(
                m.bounds[0].z > -0.7 && m.bounds[0].z < 0.3 && m.bounds[1].z > 0.3,
                "{name}"
            );
            if name == "grass" {
                continue;
            }
            // Its faces point away from its middle, as its normals do.
            for mesh in &m.meshes {
                let mut out = 0;
                let tris = mesh.indices.as_chunks::<3>().0;
                for t in tris {
                    let p = t.map(|i| Vec3::from(mesh.positions[i as usize]));
                    let face = (p[1] - p[0]).cross(p[2] - p[0]);
                    let n = Vec3::from(mesh.normals[t[0] as usize]);
                    if face.dot(n) > 0.0 {
                        out += 1;
                    }
                }
                assert!(
                    out * 10 >= tris.len() * 9,
                    "{name}: {out} of {}",
                    tris.len()
                );
            }
        }
        assert_eq!(name(Path::new("builtin:pine")), Some("pine"));
        assert_eq!(name(Path::new("assets/models/pine.glb")), None);
    }
}
