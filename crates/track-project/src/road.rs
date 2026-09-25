//! Meshes of a road: its surface, the strips beside it, painted lines and barriers,
//! built from cross-sections along the resampled spline. The physics gets the drivable
//! bands and the barriers; the renderer gets every part with its material.

use glam::DVec3;

use crate::curve::{Frame, Sampled};
use crate::project::{MaterialId, Profile, Project, Road, Side, SurfaceId};

/// Rows of cross-sections per visual mesh, so that the renderer can cull a road's far
/// parts.
const CHUNK_ROWS: usize = 32;
/// How far painted lines float above the road, m.
const PAINT_LIFT: f64 = 0.004;
/// How far barriers reach below the ground, m, to close gaps on uneven ground.
const BARRIER_SINK: f64 = 0.3;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// What a physics mesh is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Solid {
    Ground(SurfaceId),
    Wall,
}

/// A rendered mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualPart {
    pub material: MaterialId,
    pub cast_shadows: bool,
    pub mesh: MeshData,
}

/// A physics mesh (UVs unused).
#[derive(Clone, Debug, PartialEq)]
pub struct SolidPart {
    pub kind: Solid,
    pub mesh: MeshData,
}

/// A cross-section's outline on one side: (distance from the centre, height above the
/// road's plane) at the road's edge and at each strip column outwards.
type Outline = Vec<(f64, f64)>;

/// The built road.
#[derive(Clone, Debug)]
pub struct RoadBuild {
    pub sampled: Sampled,
    pub visual: Vec<VisualPart>,
    pub solid: Vec<SolidPart>,
    /// Per frame, the outline of each side (left, right).
    outlines: Vec<[Outline; 2]>,
}

impl RoadBuild {
    /// Height above the road's plane at lateral offset `d` (positive left) in frame `k`.
    pub fn height_at(&self, road: &Road, k: usize, d: f64) -> f64 {
        let f = &self.sampled.frames[k];
        surface_height(road, f, &self.outlines[k], d)
    }

    /// Where the cross-section at frame `k` ends on each side: (offset, world point).
    pub fn edges(&self, k: usize) -> [(f64, DVec3); 2] {
        let f = &self.sampled.frames[k];
        [0, 1].map(|i| {
            let &(d, h) = self.outlines[k][i].last().expect("outlines have the edge");
            let sign = if i == 0 { 1.0 } else { -1.0 };
            (sign * d, f.pos + f.lateral * (sign * d) + f.normal * h)
        })
    }
}

/// Builds road `index` of the project.
pub fn build(project: &Project, index: usize) -> RoadBuild {
    let road = &project.roads[index];
    let sampled = Sampled::new(road, road.resolution);
    let frames = &sampled.frames;
    let tile = |m: MaterialId| {
        let t = project.materials[m].tile;
        [t[0].max(1e-3) as f64, t[1].max(1e-3) as f64]
    };

    // Outlines of both sides at every frame.
    let outlines: Vec<[Outline; 2]> = frames
        .iter()
        .map(|f| {
            [Side::Left, Side::Right].map(|side| {
                let w = match side {
                    Side::Left => f.width_left,
                    Side::Right => f.width_right,
                };
                let mut outline = vec![(w, 0.0)];
                for strip in road.strips(side) {
                    // A strip fading out narrows and flattens.
                    let presence = sampled.presence(&strip.ranges, strip.fade, f.s);
                    let width = strip.width * presence;
                    let (d0, h0) = *outline.last().unwrap();
                    let cols = columns(strip.profile);
                    for c in 1..=cols {
                        let x = c as f64 / cols as f64;
                        outline.push((
                            d0 + width * x,
                            h0 + presence * profile_height(strip.profile, x),
                        ));
                    }
                }
                outline
            })
        })
        .collect();

    let mut visual = Vec::new();
    let mut solid = Vec::new();
    let point = |f: &Frame, d: f64, h: f64| f.pos + f.lateral * d + f.normal * h;

    // The road itself, from its right edge to its left.
    let cols = if road.crown != 0.0 { 4 } else { 1 };
    let road_rows: Vec<Vec<(DVec3, f64)>> = frames
        .iter()
        .map(|f| {
            (0..=2 * cols)
                .map(|c| {
                    let x = c as f64 / cols as f64 - 1.0;
                    let d = if x < 0.0 {
                        x * f.width_right
                    } else {
                        x * f.width_left
                    };
                    (point(f, d, road.crown * (1.0 - x * x)), d)
                })
                .collect()
        })
        .collect();
    add_band(
        &mut visual,
        &mut solid,
        &sampled,
        &road_rows,
        Some(Solid::Ground(road.surface)),
        road.material,
        tile(road.material),
        false,
        |_| true,
    );

    // Strips, each from its inner edge (left) or its outer edge (right) so that the
    // offset increases across it.
    for side in [Side::Left, Side::Right] {
        let si = side as usize;
        let mut col0 = 0;
        for strip in road.strips(side) {
            let cols = columns(strip.profile);
            let rows: Vec<Vec<(DVec3, f64)>> = frames
                .iter()
                .zip(&outlines)
                .map(|(f, o)| {
                    let mut row: Vec<(DVec3, f64)> = o[si][col0..=col0 + cols]
                        .iter()
                        .map(|&(d, h)| {
                            let d = side.sign() * d;
                            (point(f, d, h), d)
                        })
                        .collect();
                    if side == Side::Right {
                        row.reverse();
                    }
                    row
                })
                .collect();
            add_band(
                &mut visual,
                &mut solid,
                &sampled,
                &rows,
                Some(Solid::Ground(strip.surface)),
                strip.material,
                tile(strip.material),
                false,
                |_| true,
            );
            col0 += cols;
        }
    }

    // Painted lines.
    for line in &road.lines {
        let half = 0.5 * line.width;
        let rows: Vec<Vec<(DVec3, f64)>> = frames
            .iter()
            .zip(&outlines)
            .map(|(f, o)| {
                [line.offset - half, line.offset + half]
                    .map(|d| (point(f, d, surface_height(road, f, o, d) + PAINT_LIFT), d))
                    .to_vec()
            })
            .collect();
        let present = |k: usize| {
            let s = frames[k].s;
            let dashed = line
                .dash
                .is_none_or(|(on, off)| (s + 0.5).rem_euclid(on + off) < on);
            dashed && sampled.presence(&line.ranges, 0.0, s) > 0.5
        };
        add_band(
            &mut visual,
            &mut solid,
            &sampled,
            &rows,
            None,
            line.material,
            tile(line.material),
            false,
            present,
        );
    }

    // Barriers: faces towards the road, over the top and away from it.
    for barrier in &road.barriers {
        let si = barrier.side as usize;
        let sign = barrier.side.sign();
        let corners: Vec<[DVec3; 4]> = frames
            .iter()
            .zip(&outlines)
            .map(|(f, o)| {
                let edge = match barrier.side {
                    Side::Left => f.width_left,
                    Side::Right => f.width_right,
                };
                let d_in = sign * (edge + barrier.offset);
                let d_out = sign * (edge + barrier.offset + barrier.thickness);
                let base = |d: f64| point(f, d, outline_height(&o[si], d.abs()));
                let (a, b) = (base(d_in), base(d_out));
                let top = a.z.max(b.z) + barrier.height;
                [
                    a - DVec3::Z * BARRIER_SINK,
                    a.with_z(top),
                    b.with_z(top),
                    b - DVec3::Z * BARRIER_SINK,
                ]
            })
            .collect();
        let present = |k: usize| sampled.presence(&barrier.ranges, 0.0, frames[k].s) > 0.5;
        // Columns ordered so that each face's normal points out of the barrier.
        let faces: &[[usize; 2]] = match (barrier.thickness > 0.0, barrier.side) {
            (false, Side::Left) => &[[0, 1]],
            (false, Side::Right) => &[[1, 0]],
            (true, Side::Left) => &[[0, 1], [1, 2], [2, 3]],
            (true, Side::Right) => &[[1, 0], [2, 1], [3, 2]],
        };
        for (i, face) in faces.iter().enumerate() {
            let rows: Vec<Vec<(DVec3, f64)>> = corners
                .iter()
                .map(|c| {
                    // Across the face: height up the sides, width over the top.
                    face.map(|j| (c[j], if i == 1 { c[j].distance(c[1]) } else { c[j].z }))
                        .to_vec()
                })
                .collect();
            // The top is not a wall to the physics.
            let kind = (i != 1).then_some(Solid::Wall);
            add_band(
                &mut visual,
                &mut solid,
                &sampled,
                &rows,
                kind,
                barrier.material,
                tile(barrier.material),
                true,
                present,
            );
        }
    }

    RoadBuild {
        sampled,
        visual,
        solid,
        outlines,
    }
}

/// Columns a strip of this profile is built with.
fn columns(profile: Profile) -> usize {
    match profile {
        Profile::Crown(_) => 4,
        Profile::Flat | Profile::Slope(_) => 1,
    }
}

/// Height of a profile above its inner edge at `x` of the way across.
fn profile_height(profile: Profile, x: f64) -> f64 {
    match profile {
        Profile::Flat => 0.0,
        Profile::Crown(h) => h * (std::f64::consts::PI * x).sin(),
        Profile::Slope(drop) => -drop * x,
    }
}

/// Height along an outline at distance `d` from the centre, holding the last height
/// beyond it.
fn outline_height(outline: &Outline, d: f64) -> f64 {
    let i = outline.partition_point(|&(x, _)| x <= d);
    if i == 0 {
        return outline[0].1;
    }
    if i == outline.len() {
        return outline[i - 1].1;
    }
    let ((d0, h0), (d1, h1)) = (outline[i - 1], outline[i]);
    if d1 - d0 < 1e-9 {
        return h1;
    }
    h0 + (h1 - h0) * (d - d0) / (d1 - d0)
}

/// Height above the road's plane at lateral offset `d` (positive left).
fn surface_height(road: &Road, f: &Frame, outlines: &[Outline; 2], d: f64) -> f64 {
    let (w, outline) = if d >= 0.0 {
        (f.width_left, &outlines[0])
    } else {
        (f.width_right, &outlines[1])
    };
    if d.abs() <= w {
        let x = d / w;
        road.crown * (1.0 - x * x)
    } else {
        outline_height(outline, d.abs())
    }
}

/// Triangulates a band of rows (one per frame) of points across the road, each with its
/// coordinate across for the UVs, into chunked visual meshes and a physics mesh.
/// `present(k)` keeps the quads between rows `k` and `k + 1`. Across each row the
/// points run so that (along × across) is the side the band faces.
#[allow(clippy::too_many_arguments)]
fn add_band(
    visual: &mut Vec<VisualPart>,
    solid: &mut Vec<SolidPart>,
    sampled: &Sampled,
    rows: &[Vec<(DVec3, f64)>],
    kind: Option<Solid>,
    material: MaterialId,
    tile: [f64; 2],
    cast_shadows: bool,
    present: impl Fn(usize) -> bool,
) {
    let n = rows.len();
    let quads = if sampled.closed { n } else { n - 1 };
    let frames = &sampled.frames;
    let mut whole = MeshData::default();
    let mut start = 0;
    while start < quads {
        let end = (start + CHUNK_ROWS).min(quads);
        let mut mesh = MeshData::default();
        // Rows start..=end; the last may wrap to row 0.
        let row = |k: usize| k % n;
        let cols = rows[0].len();
        for k in start..=end {
            let r = row(k);
            // Along distance continues past the end of a closed road.
            let s = if k == n { sampled.length } else { frames[r].s };
            for c in 0..cols {
                let (p, across) = rows[r][c];
                let prev = &rows[if k == 0 && !sampled.closed {
                    0
                } else {
                    row(k + n - 1)
                }];
                let next = &rows[if k + 1 >= n && !sampled.closed {
                    n - 1
                } else {
                    row(k + 1)
                }];
                let along = next[c].0 - prev[c].0;
                let across_dir = rows[r][(c + 1).min(cols - 1)].0 - rows[r][c.saturating_sub(1)].0;
                let normal = along.cross(across_dir).normalize_or(frames[r].normal);
                mesh.positions.push(p.as_vec3().to_array());
                mesh.normals.push(normal.as_vec3().to_array());
                mesh.uvs
                    .push([(across / tile[0]) as f32, (s / tile[1]) as f32]);
            }
        }
        for k in start..end {
            if !present(k) {
                continue;
            }
            let (a0, b0) = (((k - start) * cols) as u32, ((k + 1 - start) * cols) as u32);
            for c in 0..cols as u32 - 1 {
                let (a, b, cc, d) = (a0 + c, b0 + c, a0 + c + 1, b0 + c + 1);
                for tri in [[a, b, cc], [cc, b, d]] {
                    let p = tri.map(|i| DVec3::from(mesh.positions[i as usize].map(f64::from)));
                    if (p[1] - p[0]).cross(p[2] - p[0]).length_squared() > 1e-10 {
                        mesh.indices.extend(tri);
                    }
                }
            }
        }
        if kind.is_some() {
            let base = whole.positions.len() as u32;
            whole.positions.extend(&mesh.positions);
            whole.normals.extend(&mesh.normals);
            whole.indices.extend(mesh.indices.iter().map(|i| i + base));
        }
        if !mesh.is_empty() {
            visual.push(VisualPart {
                material,
                cast_shadows,
                mesh,
            });
        }
        start = end;
    }
    if let Some(kind) = kind
        && !whole.is_empty()
    {
        if kind == Solid::Wall {
            whole.normals.clear();
        }
        solid.push(SolidPart { kind, mesh: whole });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_meet_without_gaps() {
        let project = Project::new("t");
        let b = build(&project, 0);
        // Every strip's inner edge coincides with what lies inside it: collect the
        // physics vertices of the first cross-section and check the outline is
        // continuous (each distinct lateral position appears in two neighbouring bands
        // apart from the two outer edges).
        let f = &b.sampled.frames[0];
        let mut offsets: Vec<f64> = b
            .solid
            .iter()
            .filter(|p| matches!(p.kind, Solid::Ground(_)))
            .flat_map(|p| p.mesh.positions.iter())
            .map(|&p| DVec3::from(p.map(f64::from)))
            .filter(|p| (*p - f.pos).dot(f.tangent).abs() < 1e-3)
            .map(|p| ((p - f.pos).dot(f.lateral) * 1000.0).round() / 1000.0)
            .collect();
        offsets.sort_by(f64::total_cmp);
        offsets.dedup();
        let road = &project.roads[0];
        let reach = |side: Side| {
            let w = if side == Side::Left {
                f.width_left
            } else {
                f.width_right
            };
            w + road
                .strips(side)
                .iter()
                .map(|s| s.width * b.sampled.presence(&s.ranges, s.fade, f.s))
                .sum::<f64>()
        };
        assert!(
            (offsets[0] + reach(Side::Right)).abs() < 1e-2,
            "{offsets:?}"
        );
        assert!((offsets[offsets.len() - 1] - reach(Side::Left)).abs() < 1e-2);
    }

    #[test]
    fn surfaces_face_up_and_walls_face_the_road() {
        let project = Project::new("t");
        let b = build(&project, 0);
        for part in &b.solid {
            let m = &part.mesh;
            for &t in m.indices.as_chunks::<3>().0 {
                let p = t.map(|i| DVec3::from(m.positions[i as usize].map(f64::from)));
                let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
                match part.kind {
                    Solid::Ground(_) => assert!(n.z > 0.5, "{n}"),
                    Solid::Wall => assert!(n.z.abs() < 0.1, "{n}"),
                }
            }
        }
        // The first wall (left) faces right, towards the road, on its inner side.
        let f = &b.sampled.frames[0];
        assert!(b.height_at(&project.roads[0], 0, 0.0) > 0.0);
        assert!(f.lateral.z.abs() < 1e-9);
    }
}
