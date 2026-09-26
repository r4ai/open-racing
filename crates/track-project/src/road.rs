//! Meshes of a road: its surface, the strips beside it, painted lines and barriers,
//! built from cross-sections along the resampled spline. The physics gets the drivable
//! bands and the barriers; the renderer gets every part with its material.

use glam::DVec3;

use crate::curve::{Frame, Sampled};
use crate::project::{ModelRun, Profile, Project, Road, Side};

/// Rows of cross-sections per visual mesh, so that the renderer can cull a road's far
/// parts.
const CHUNK_ROWS: usize = 32;
/// How far painted lines float above the road, m.
const PAINT_LIFT: f64 = 0.004;
/// How far barriers reach below the ground, m, to close gaps on uneven ground.
pub(crate) const BARRIER_SINK: f64 = 0.3;

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
    /// Drivable, with the surface at this index of `Project::surfaces`.
    Ground(usize),
    Wall,
}

/// Which part of a road a mesh is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Surface,
    Strip,
    Line,
    Barrier,
}

/// A rendered mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct VisualPart {
    pub layer: Layer,
    /// Index into `Project::materials`.
    pub material: usize,
    pub cast_shadows: bool,
    pub mesh: MeshData,
}

/// A physics mesh (UVs unused).
#[derive(Clone, Debug, PartialEq)]
pub struct SolidPart {
    pub layer: Layer,
    pub kind: Solid,
    pub mesh: MeshData,
}

/// A cross-section's outline on one side: (distance from the centre, height above the
/// road's plane) at the road's edge and at each strip column outwards.
type Outline = Vec<(f64, f64)>;

/// A point of a wall's line: where it stands, and which way (level) is the road's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinePoint {
    pub pos: DVec3,
    pub toward: DVec3,
}

/// A wall shown by a model repeated along its line: the model, and each stretch of the
/// line it stands along.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelLine {
    pub run: ModelRun,
    pub stretches: Vec<Vec<LinePoint>>,
}

impl ModelLine {
    /// The line through the rows (one per frame) where `present(k)` keeps the stretch
    /// from row `k` to `k + 1`.
    pub(crate) fn new(
        run: &ModelRun,
        points: &[LinePoint],
        closed: bool,
        present: impl Fn(usize) -> bool,
    ) -> Self {
        let n = points.len();
        let quads = if closed { n } else { n.saturating_sub(1) };
        let flip = if run.flip { -1.0 } else { 1.0 };
        let point = |k: usize| {
            let p = points[k % n];
            LinePoint {
                toward: p.toward * flip,
                ..p
            }
        };
        let mut stretches: Vec<Vec<LinePoint>> = Vec::new();
        let mut open = false;
        for k in 0..quads {
            if !present(k) {
                open = false;
                continue;
            }
            if !open {
                stretches.push(vec![point(k)]);
                open = true;
            }
            stretches
                .last_mut()
                .expect("just opened")
                .push(point(k + 1));
        }
        Self {
            run: run.clone(),
            stretches,
        }
    }
}

/// The built road.
#[derive(Clone, Debug)]
pub struct RoadBuild {
    pub sampled: Sampled,
    pub visual: Vec<VisualPart>,
    pub solid: Vec<SolidPart>,
    /// Barriers shown by models.
    pub models: Vec<ModelLine>,
    /// Per frame, the outline of each side (left, right).
    outlines: Vec<[Outline; 2]>,
    crown: f64,
}

impl RoadBuild {
    /// Height of the road's own surface above its plane at lateral offset `d` within
    /// its edges, in frame `k`.
    pub fn surface_height(&self, k: usize, d: f64) -> f64 {
        let f = &self.sampled.frames[k];
        let w = if d >= 0.0 {
            f.width_left
        } else {
            f.width_right
        };
        let x = (d / w).clamp(-1.0, 1.0);
        self.crown * (1.0 - x * x)
    }

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
    // Unknown names (in a project being edited) fall back to the first entry.
    let surface = |name: &str| project.surface_index(name).unwrap_or(0);
    let material = |name: &str| project.material_index(name).unwrap_or(0);
    let tile = |m: usize| {
        let t = project.materials.get(m).map_or([1.0; 2], |m| m.tile);
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
                    let (width, height) = sampled.strip_shape(strip, f.s);
                    let (width, height) = (width * presence, height * presence);
                    let (d0, h0) = *outline.last().unwrap();
                    for x in column_xs(&strip.profile, strip.widest()) {
                        outline.push((d0 + width * x, h0 + height * strip.profile.height(x)));
                    }
                }
                outline
            })
        })
        .collect();

    let mut visual = Vec::new();
    let mut solid = Vec::new();
    let mut models = Vec::new();
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
        Layer::Surface,
        Some(Solid::Ground(surface(&road.surface))),
        material(&road.material),
        tile(material(&road.material)),
        false,
        |_| true,
    );

    // Strips, each from its inner edge (left) or its outer edge (right) so that the
    // offset increases across it.
    for side in [Side::Left, Side::Right] {
        let si = side as usize;
        let mut col0 = 0;
        for strip in road.strips(side) {
            let cols = column_xs(&strip.profile, strip.widest()).len();
            // Only where the strip is: nothing is built where it has narrowed to none,
            // and only the rows of chunks with something in them are worked out.
            let n = frames.len();
            let here: Vec<bool> = frames
                .iter()
                .map(|f| sampled.presence(&strip.ranges, strip.fade, f.s) > 0.0)
                .collect();
            let needed = rows_needed(n, sampled.closed, |k| here[k] || here[(k + 1) % n]);
            let rows: Vec<Vec<(DVec3, f64)>> = frames
                .iter()
                .zip(&outlines)
                .zip(&needed)
                .map(|((f, o), &needed)| {
                    if !needed {
                        return vec![];
                    }
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
            if let Some(run) = &strip.model {
                // At the foot of its inner edge halfway across, facing the road.
                let sign = side.sign();
                let points: Vec<LinePoint> = frames
                    .iter()
                    .zip(&outlines)
                    .map(|(f, o)| {
                        let ((d_in, h_in), (d_out, _)) = (o[si][col0], o[si][col0 + cols]);
                        LinePoint {
                            pos: point(f, sign * 0.5 * (d_in + d_out), h_in),
                            toward: -sign * f.lateral.with_z(0.0).normalize_or(DVec3::Y),
                        }
                    })
                    .collect();
                let present = |k: usize| here[k] && here[(k + 1) % n];
                models.push(ModelLine::new(run, &points, sampled.closed, present));
            }
            // A strip a model shows is only there for the cars to drive on.
            let mut hidden = Vec::new();
            add_band(
                if strip.model.is_some() {
                    &mut hidden
                } else {
                    &mut visual
                },
                &mut solid,
                &sampled,
                &rows,
                Layer::Strip,
                Some(Solid::Ground(surface(&strip.surface))),
                material(&strip.material),
                tile(material(&strip.material)),
                false,
                |k| here[k] || here[(k + 1) % n],
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
            Layer::Line,
            None,
            material(&line.material),
            tile(material(&line.material)),
            false,
            present,
        );
    }

    // Marks across the road, a little over the lines so that they cross them cleanly.
    let spacing = sampled.length / frames.len().max(1) as f64;
    for mark in &road.marks {
        let mid = sampled.s_at(mark.at);
        let across = ((mark.to - mark.from) / 0.5).ceil().max(1.0) as usize;
        let mut mesh = MeshData::default();
        for k in 0..=2 {
            let s = mid + mark.length * (k as f64 / 2.0 - 0.5);
            let f = sampled.frame_at(s);
            let near = if sampled.closed {
                ((s / spacing).round() as isize).rem_euclid(frames.len() as isize) as usize
            } else {
                ((s / spacing).round().max(0.0) as usize).min(frames.len() - 1)
            };
            for c in 0..=across {
                let d = mark.from + (mark.to - mark.from) * c as f64 / across as f64;
                let h = surface_height(road, &f, &outlines[near], d) + 2.0 * PAINT_LIFT;
                mesh.positions.push(point(&f, d, h).as_vec3().to_array());
                mesh.normals.push(f.normal.as_vec3().to_array());
                mesh.uvs.push([
                    ((d - mark.from) / mark.length.max(0.01)) as f32,
                    k as f32 * 0.5,
                ]);
            }
        }
        let w = (across + 1) as u32;
        for k in 0..2u32 {
            for c in 0..across as u32 {
                let (a, b) = (k * w + c, (k + 1) * w + c);
                // Facing up: along × across.
                mesh.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        visual.push(VisualPart {
            layer: Layer::Line,
            material: material(&mark.material),
            cast_shadows: false,
            mesh,
        });
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
        if let Some(run) = &barrier.model {
            // On the ground in the wall's middle, facing the road.
            let points: Vec<LinePoint> = corners
                .iter()
                .zip(frames)
                .map(|(c, f)| LinePoint {
                    pos: (c[0] + c[3]) * 0.5 + DVec3::Z * BARRIER_SINK,
                    toward: -sign * f.lateral.with_z(0.0).normalize_or(DVec3::Y),
                })
                .collect();
            models.push(ModelLine::new(run, &points, sampled.closed, present));
        }
        // A left barrier's corners run away from the road, a right one's towards it.
        let corners: Vec<[DVec3; 4]> = match barrier.side {
            Side::Left => corners,
            Side::Right => corners
                .into_iter()
                .map(|[a, b, c, d]| [d, c, b, a])
                .collect(),
        };
        // A wall shown by a model is only there for the cars to hit.
        let mut hidden = Vec::new();
        add_wall(
            if barrier.model.is_some() {
                &mut hidden
            } else {
                &mut visual
            },
            &mut solid,
            &sampled,
            &corners,
            barrier.thickness > 0.0,
            true,
            material(&barrier.material),
            tile(material(&barrier.material)),
            present,
        );
    }

    RoadBuild {
        sampled,
        visual,
        solid,
        models,
        outlines,
        crown: road.crown,
    }
}

/// Builds a wall from its corners at each row: the bottom and top of its right side
/// (looking along the rows), then the top and bottom of its left side. A thin wall
/// (`thick` false, both sides the same) is one face, for a double-sided material.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_wall(
    visual: &mut Vec<VisualPart>,
    solid: &mut Vec<SolidPart>,
    sampled: &Sampled,
    corners: &[[DVec3; 4]],
    thick: bool,
    collide: bool,
    material: usize,
    tile: [f64; 2],
    present: impl Fn(usize) -> bool,
) {
    // Columns ordered so that each face's normal points out of the wall.
    let faces: &[[usize; 2]] = if thick {
        &[[0, 1], [1, 2], [2, 3]]
    } else {
        &[[0, 1]]
    };
    for (i, face) in faces.iter().enumerate() {
        let top = i == 1;
        let rows: Vec<Vec<(DVec3, f64)>> = corners
            .iter()
            .map(|c| {
                // Across the face: height up the sides, width over the top.
                face.map(|j| (c[j], if top { c[j].distance(c[1]) } else { c[j].z }))
                    .to_vec()
            })
            .collect();
        // The top is not a wall to the physics.
        let kind = (collide && !top).then_some(Solid::Wall);
        add_band(
            visual,
            solid,
            sampled,
            &rows,
            Layer::Barrier,
            kind,
            material,
            tile,
            true,
            &present,
        );
    }
}

/// Width of a strip's columns at most, m: fine enough for other roads crossing it to
/// press it down under themselves.
const STRIP_COLUMN: f64 = 2.0;

/// How far across a stepped strip's step rises, m: steep, but not sheer.
const STEP_RUN: f64 = 0.02;

/// Where a strip's outline has points across it, as fractions of its width from its
/// inner edge (1 its outer edge): evenly, and just inside its inner edge too when its
/// profile starts with a step up (or down) from what is inside it.
pub(crate) fn column_xs(profile: &Profile, width: f64) -> Vec<f64> {
    let cols = columns(profile, width);
    let mut xs: Vec<f64> = (1..=cols).map(|c| c as f64 / cols as f64).collect();
    if profile.height(0.0).abs() > 1e-6 {
        xs.insert(0, (STEP_RUN / width.max(STEP_RUN)).min(0.25 / cols as f64));
    }
    xs
}

/// Columns a strip of this profile and full width is built with.
pub(crate) fn columns(profile: &Profile, width: f64) -> usize {
    let across = (width / STRIP_COLUMN).ceil().max(1.0) as usize;
    match profile {
        Profile::Crown(_) => across.max(4),
        // Fine enough for its steps.
        Profile::Shape(points) => across.max(4 * points.len()).min(64),
        Profile::Flat | Profile::Slope(_) => across,
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

/// Which rows `add_band` reads when `present(k)` keeps the quads it does: those of each
/// chunk with a quad kept, and one either side.
fn rows_needed(n: usize, closed: bool, present: impl Fn(usize) -> bool) -> Vec<bool> {
    let quads = if closed { n } else { n.saturating_sub(1) };
    let mut needed = vec![false; n];
    let mut start = 0;
    while start < quads {
        let end = (start + CHUNK_ROWS).min(quads);
        if (start..end).any(&present) {
            for k in start as isize - 1..=end as isize + 1 {
                if closed {
                    needed[k.rem_euclid(n as isize) as usize] = true;
                } else if (0..n as isize).contains(&k) {
                    needed[k as usize] = true;
                }
            }
        }
        start = end;
    }
    needed
}

/// Triangulates a band of rows (one per frame) of points across the road, each with its
/// coordinate across for the UVs, into chunked visual meshes and a physics mesh.
/// `present(k)` keeps the quads between rows `k` and `k + 1`. Across each row the
/// points run so that (along × across) is the side the band faces.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_band(
    visual: &mut Vec<VisualPart>,
    solid: &mut Vec<SolidPart>,
    sampled: &Sampled,
    rows: &[Vec<(DVec3, f64)>],
    layer: Layer,
    kind: Option<Solid>,
    material: usize,
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
        // Nothing to build here: no vertices either.
        if !(start..end).any(&present) {
            start = end;
            continue;
        }
        let mut mesh = MeshData::default();
        // Rows start..=end; the last may wrap to row 0.
        let row = |k: usize| k % n;
        let cols = rows[start % n].len();
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
                layer,
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
        solid.push(SolidPart {
            layer,
            kind,
            mesh: whole,
        });
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
                .map(|s| {
                    b.sampled.strip_shape(s, f.s).0 * b.sampled.presence(&s.ranges, s.fade, f.s)
                })
                .sum::<f64>()
        };
        assert!(
            (offsets[0] + reach(Side::Right)).abs() < 1e-2,
            "{offsets:?}"
        );
        assert!((offsets[offsets.len() - 1] - reach(Side::Left)).abs() < 1e-2);
    }

    /// How far out the left side's outline reaches at the frame nearest `u`.
    fn left_reach(b: &RoadBuild, u: f64) -> f64 {
        let k = b.sampled.nearest(b.sampled.frame_at(b.sampled.s_at(u)).pos);
        b.edges(k)[0].0
    }

    #[test]
    fn strip_keys_widen_and_raise_a_strip_where_they_are() {
        let mut project = Project::new("t");
        let road = &mut project.roads[0];
        // The kerb all round, 1.2 m wide but 3 m at node 3.
        road.left[0].ranges.clear();
        road.left[0].keys = vec![
            crate::project::StripKey {
                u: 1.0,
                width: 1.2,
                height: 1.0,
            },
            crate::project::StripKey {
                u: 3.0,
                width: 3.0,
                height: 2.0,
            },
            crate::project::StripKey {
                u: 5.0,
                width: 1.2,
                height: 1.0,
            },
        ];
        let mut plain = project.clone();
        plain.roads[0].left[0].keys.clear();
        let before = build(&plain, 0);
        let b = build(&project, 0);
        let wider = left_reach(&b, 3.0) - left_reach(&before, 3.0);
        assert!((wider - 1.8).abs() < 0.05, "{wider}");
        assert!((left_reach(&b, 1.0) - left_reach(&before, 1.0)).abs() < 0.05);
        // Its crown twice as high there.
        let road = &project.roads[0];
        let k = b
            .sampled
            .nearest(b.sampled.frame_at(b.sampled.s_at(3.0)).pos);
        let edge = b.sampled.frames[k].width_left;
        let top = b.height_at(road, k, edge + 1.5);
        assert!((top - 0.06).abs() < 0.005, "{top}");
    }

    #[test]
    fn a_profile_starting_high_steps_up_from_the_road() {
        let mut project = Project::new("t");
        let kerb = &mut project.roads[0].left[0];
        kerb.ranges.clear();
        kerb.profile = Profile::Shape(vec![[0.0, 0.05], [1.0, 0.05]]);
        let b = build(&project, 0);
        let road = &project.roads[0];
        let edge = b.sampled.frames[0].width_left;
        assert!(b.height_at(road, 0, edge - 0.01).abs() < 1e-3);
        assert!((b.height_at(road, 0, edge + 0.05) - 0.05).abs() < 1e-3);
    }

    #[test]
    fn a_strip_a_model_shows_is_driven_on_but_not_drawn() {
        let mut project = Project::new("t");
        let plain = build(&project, 0);
        project.roads[0].left[0].model = Some(ModelRun {
            model: "kerb.glb".into(),
            length: 2.0,
            bend: true,
            flip: false,
        });
        let b = build(&project, 0);
        assert_eq!(b.models.len(), plain.models.len() + 1);
        let strips = |b: &RoadBuild, solid: bool| {
            if solid {
                b.solid.iter().filter(|p| p.layer == Layer::Strip).count()
            } else {
                b.visual.iter().filter(|p| p.layer == Layer::Strip).count()
            }
        };
        assert_eq!(strips(&b, true), strips(&plain, true));
        assert!(strips(&b, false) < strips(&plain, false));
        // Along the kerb's two stretches, halfway across it and facing the road.
        let line = b.models.last().unwrap();
        assert_eq!(line.stretches.len(), 2);
        let run = &line.stretches[0];
        let p = run[run.len() / 2];
        let f = &b.sampled.frames[b.sampled.nearest(p.pos)];
        let d = (p.pos - f.pos).dot(f.lateral);
        assert!((d - (f.width_left + 0.6)).abs() < 0.1, "{d}");
        assert!(p.toward.dot(f.lateral) < -0.99);
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
