//! `ground.bin`: the drivable surfaces and walls, grouped into one patch per surface.

use glam::DVec3;
use open_racing_sim::{GroundMesh, GroundMeshBuilder, SurfaceProps};

use crate::Error;
use crate::bin::{Reader, Writer};

const MAGIC: &[u8; 4] = b"ORGD";
const VERSION: u32 = 1;
/// Kind stored in the file for a wall patch; other values are surface ids.
const WALL: u32 = u32::MAX;

/// What a patch is to the physics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PatchKind {
    /// Drivable, with the surface at this index of `TrackPackage::surfaces`.
    Ground(u16),
    /// Solid: the car collides with it.
    Wall,
}

/// Triangles sharing one `PatchKind`, in world coordinates (m, Z up).
#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub kind: PatchKind,
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex normals for a smooth ride; empty to use face normals.
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ground {
    pub patches: Vec<Patch>,
}

impl Ground {
    /// Appends a mesh to the patch of its kind. Normals are dropped unless there is one
    /// per position.
    pub fn add(
        &mut self,
        kind: PatchKind,
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        indices: &[u32],
    ) {
        let i = match self.patches.iter().position(|p| p.kind == kind) {
            Some(i) => i,
            None => {
                self.patches.push(Patch {
                    kind,
                    positions: Vec::new(),
                    normals: Vec::new(),
                    indices: Vec::new(),
                });
                self.patches.len() - 1
            }
        };
        let p = &mut self.patches[i];
        // A patch keeps normals only while every mesh in it has them.
        let has_normals = p.normals.len() == p.positions.len();
        if kind != PatchKind::Wall && has_normals && normals.len() == positions.len() {
            p.normals.extend_from_slice(normals);
        } else {
            p.normals.clear();
        }
        let base = p.positions.len() as u32;
        p.positions.extend_from_slice(positions);
        p.indices.extend(indices.iter().map(|i| base + i));
    }

    pub fn is_drivable(&self) -> bool {
        self.patches
            .iter()
            .any(|p| matches!(p.kind, PatchKind::Ground(_)) && !p.indices.is_empty())
    }

    /// Builds the physics' lookup structure.
    pub fn build(&self, surfaces: &[SurfaceProps]) -> GroundMesh {
        let mut b = GroundMeshBuilder::new();
        for &s in surfaces {
            b.add_surface(s);
        }
        let d = |v: &[[f32; 3]]| {
            v.iter()
                .map(|p| DVec3::from(p.map(f64::from)))
                .collect::<Vec<_>>()
        };
        for p in &self.patches {
            match p.kind {
                PatchKind::Ground(s) => {
                    b.add_ground(&d(&p.positions), &d(&p.normals), &p.indices, s)
                }
                PatchKind::Wall => b.add_wall(&d(&p.positions), &p.indices),
            }
        }
        b.build()
    }

    /// Checks that indices and surface ids are in range.
    pub(crate) fn validate(&self, surfaces: usize) -> Result<(), Error> {
        for p in &self.patches {
            if let PatchKind::Ground(s) = p.kind
                && s as usize >= surfaces
            {
                return Err(Error::Format(format!("ground: surface {s} is not defined")));
            }
            if p.indices.len() % 3 != 0
                || p.indices.iter().any(|&i| i as usize >= p.positions.len())
            {
                return Err(Error::Format("ground: invalid triangle indices".into()));
            }
            if !p.normals.is_empty() && p.normals.len() != p.positions.len() {
                return Err(Error::Format(
                    "ground: normal count does not match the vertices".into(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new(MAGIC, VERSION);
        w.u32(self.patches.len() as u32);
        for p in &self.patches {
            w.u32(match p.kind {
                PatchKind::Ground(s) => s.into(),
                PatchKind::Wall => WALL,
            });
            w.vecs(&p.positions);
            w.vecs(&p.normals);
            w.u32s(&p.indices);
        }
        w.finish()
    }

    pub(crate) fn decode(buf: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(buf, MAGIC, VERSION..=VERSION, "ground.bin")?;
        let n = r.u32()?;
        let mut patches = Vec::new();
        for _ in 0..n {
            let kind = match r.u32()? {
                WALL => PatchKind::Wall,
                s => PatchKind::Ground(
                    u16::try_from(s)
                        .map_err(|_| Error::Format(format!("ground: surface id {s} too large")))?,
                ),
            };
            patches.push(Patch {
                kind,
                positions: r.vecs()?,
                normals: r.vecs()?,
                indices: r.u32s()?,
            });
        }
        r.finish()?;
        Ok(Self { patches })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meshes_merge_by_kind() {
        let mut g = Ground::default();
        let tri = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        g.add(
            PatchKind::Ground(0),
            &tri,
            &[[0.0, 0.0, 1.0]; 3],
            &[0, 1, 2],
        );
        g.add(PatchKind::Wall, &tri, &[], &[0, 1, 2]);
        g.add(
            PatchKind::Ground(0),
            &tri,
            &[[0.0, 0.0, 1.0]; 3],
            &[0, 2, 1],
        );
        assert_eq!(g.patches.len(), 2);
        assert_eq!(g.patches[0].indices, [0, 1, 2, 3, 5, 4]);
        assert_eq!(g.patches[0].normals.len(), 6);
        // A mesh without normals makes the whole patch fall back to face normals.
        g.add(PatchKind::Ground(0), &tri, &[], &[0, 1, 2]);
        assert!(g.patches[0].normals.is_empty());
        assert!(g.patches[1].normals.is_empty());
    }
}
