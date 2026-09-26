//! Kerbs, walls and fences along their own splines, placed anywhere: draped over the
//! roads and the terrain, or at the heights of their nodes.

use glam::{DVec2, DVec3};
use open_racing_sim::GroundMesh;

use crate::curve::Sampled;
use crate::project::{Align, Node, Project, Shape};
use crate::road::{
    BARRIER_SINK, Layer, LinePoint, MeshData, ModelLine, Solid, SolidPart, VisualPart, add_band,
    add_wall, column_xs,
};

/// How far above a point draping looks for the ground first, m: enough for ground a
/// little above the node, while a spline under a bridge stays under it.
const DRAPE_REACH: f64 = 3.0;

/// A built spline.
#[derive(Clone, Debug)]
pub struct SplineBuild {
    pub sampled: Sampled,
    pub visual: Vec<VisualPart>,
    pub solid: Vec<SolidPart>,
    /// The wall, if a model shows it.
    pub models: Vec<ModelLine>,
}

/// Builds spline `index` of the project. Draped splines follow `ground`, the roads and
/// terrain built before them.
pub fn build(project: &Project, index: usize, ground: Option<&GroundMesh>) -> SplineBuild {
    let sp = &project.splines[index];
    let ground = ground.filter(|_| sp.drape);
    let drape = |p: DVec3| match ground {
        Some(g) => g
            .raycast_down(p, DRAPE_REACH)
            .or_else(|| g.raycast_down(p, 1e4))
            .map_or(p, |h| h.point),
        None => p,
    };
    let sampled = Sampled::line(&sp.nodes, sp.closed, sp.resolution, drape);
    let frames = &sampled.frames;
    // Each frame's size, times the spline's own, from its nodes' radii.
    let radius: Vec<f64> = frames
        .iter()
        .map(|f| radius_at(&sp.nodes, sp.closed, f.u))
        .collect();
    // Unknown names (in a project being edited) fall back to the first entry.
    let material = project.material_index(sp.material()).unwrap_or(0);
    let tile = project
        .materials
        .get(material)
        .map_or([1.0; 2], |m| m.tile.map(|t| t.max(1e-3) as f64));
    let (mut visual, mut solid, mut models) = (Vec::new(), Vec::new(), Vec::new());

    match &sp.shape {
        Shape::Band {
            width,
            align,
            profile,
            surface,
            lift,
            model,
            ..
        } => {
            let edges = |r: f64| {
                let w = width * r;
                match align {
                    Align::Center => (-0.5 * w, 0.5 * w),
                    Align::Left => (0.0, w),
                    Align::Right => (-w, 0.0),
                }
            };
            let widest = radius.iter().copied().fold(0.0, f64::max);
            // Its right edge on what is under it, then up any step its profile starts
            // with.
            let xs: Vec<f64> = std::iter::once(0.0)
                .chain(column_xs(profile, width * widest.max(1e-3)))
                .collect();
            let height = |x: f64| if x == 0.0 { 0.0 } else { profile.height(x) };
            // From the right edge to the left, so that the band faces up.
            let rows: Vec<Vec<(DVec3, f64)>> = frames
                .iter()
                .zip(&radius)
                .map(|(f, &r)| {
                    let (d0, d1) = edges(r);
                    xs.iter()
                        .map(|&x| {
                            let d = d0 + (d1 - d0) * x;
                            let h = lift + height(x);
                            (drape(f.pos + f.lateral * d) + DVec3::Z * h, d - d0)
                        })
                        .collect()
                })
                .collect();
            if let Some(run) = model {
                // At the foot of its right edge halfway across, facing the line's left.
                let points: Vec<LinePoint> = frames
                    .iter()
                    .zip(&radius)
                    .map(|(f, &r)| {
                        let (d0, d1) = edges(r);
                        LinePoint {
                            pos: drape(f.pos + f.lateral * (0.5 * (d0 + d1))) + DVec3::Z * *lift,
                            toward: f.lateral.with_z(0.0).normalize_or(DVec3::Y),
                        }
                    })
                    .collect();
                models.push(ModelLine::new(run, &points, sampled.closed, |_| true));
            }
            let mut hidden = Vec::new();
            add_band(
                if model.is_some() {
                    &mut hidden
                } else {
                    &mut visual
                },
                &mut solid,
                &sampled,
                &rows,
                Layer::Strip,
                Some(Solid::Ground(project.surface_index(surface).unwrap_or(0))),
                material,
                tile,
                false,
                |_| true,
            );
        }
        Shape::Wall {
            height,
            thickness,
            collide,
            model,
            ..
        } => {
            let corners: Vec<[DVec3; 4]> = frames
                .iter()
                .zip(&radius)
                .map(|(f, &r)| {
                    let a = drape(f.pos - f.lateral * (0.5 * thickness));
                    let b = drape(f.pos + f.lateral * (0.5 * thickness));
                    let top = a.z.max(b.z) + height * r;
                    [
                        a - DVec3::Z * BARRIER_SINK,
                        a.with_z(top),
                        b.with_z(top),
                        b - DVec3::Z * BARRIER_SINK,
                    ]
                })
                .collect();
            let mut hidden = Vec::new();
            if let Some(run) = model {
                // Standing on the ground, facing the line's left.
                let points: Vec<LinePoint> = corners
                    .iter()
                    .zip(frames)
                    .map(|(c, f)| LinePoint {
                        pos: (c[0] + c[3]) * 0.5 + DVec3::Z * BARRIER_SINK,
                        toward: f.lateral.with_z(0.0).normalize_or(DVec3::Y),
                    })
                    .collect();
                models.push(ModelLine::new(run, &points, sampled.closed, |_| true));
            }
            add_wall(
                if model.is_some() {
                    &mut hidden
                } else {
                    &mut visual
                },
                &mut solid,
                &sampled,
                &corners,
                *thickness > 0.0,
                *collide,
                material,
                tile,
                |_| true,
            );
        }
        Shape::Area { surface, lift, .. } => {
            let outline: Vec<DVec3> = frames.iter().map(|f| f.pos).collect();
            let mesh = area(&outline, sp.resolution, tile, |p| {
                drape(p) + DVec3::Z * *lift
            });
            if !mesh.is_empty() {
                solid.push(SolidPart {
                    layer: Layer::Strip,
                    kind: Solid::Ground(project.surface_index(surface).unwrap_or(0)),
                    mesh: MeshData {
                        uvs: vec![],
                        ..mesh.clone()
                    },
                });
                visual.push(VisualPart {
                    layer: Layer::Strip,
                    material,
                    cast_shadows: false,
                    mesh,
                });
            }
        }
    }
    SplineBuild {
        sampled,
        visual,
        solid,
        models,
    }
}

/// A spline's radius at parameter `u`, easing from node to node.
pub fn radius_at(nodes: &[Node], closed: bool, u: f64) -> f64 {
    let n = nodes.len();
    if n == 0 {
        return 1.0;
    }
    let segs = crate::curve::segments(n, closed);
    if segs == 0 {
        return nodes[0].radius;
    }
    let u = if closed {
        u.rem_euclid(segs as f64)
    } else {
        u.clamp(0.0, segs as f64)
    };
    let i = (u.floor() as usize).min(segs - 1);
    let t = u - i as f64;
    let t = t * t * (3.0 - 2.0 * t);
    let (a, b) = (nodes[i].radius, nodes[(i + 1) % n].radius);
    a + (b - a) * t
}

/// The inside of a closed outline, cut into cells about `resolution` × 4 m across so that
/// it follows what `place` puts its points on (the ground under them, or the outline's
/// heights), facing up, its texture laid by the world's x and y at `tile`.
pub fn area(
    outline: &[DVec3],
    resolution: f64,
    tile: [f64; 2],
    place: impl Fn(DVec3) -> DVec3,
) -> MeshData {
    let mut mesh = MeshData::default();
    let flat: Vec<DVec2> = outline.iter().map(|p| p.truncate()).collect();
    let triangles = triangulate(&flat);
    let cell = (resolution * 4.0).clamp(0.5, 8.0);
    // Points shared between triangles, by where they are, so that normals are smooth.
    let mut at: std::collections::HashMap<(i64, i64), u32> = Default::default();
    let mut normals: Vec<DVec3> = Vec::new();
    let mut points: Vec<DVec3> = Vec::new();
    for [a, b, c] in triangles {
        let corner = [outline[a], outline[b], outline[c]];
        let tri = [flat[a], flat[b], flat[c]];
        let lo = tri[0].min(tri[1]).min(tri[2]);
        let hi = tri[0].max(tri[1]).max(tri[2]);
        let (i0, j0) = ((lo.x / cell).floor() as i64, (lo.y / cell).floor() as i64);
        let (i1, j1) = ((hi.x / cell).floor() as i64, (hi.y / cell).floor() as i64);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let box_lo = DVec2::new(i as f64, j as f64) * cell;
                let piece = clip(&tri, box_lo, box_lo + DVec2::splat(cell));
                if piece.len() < 3 {
                    continue;
                }
                // Each point's height from the triangle's corners, then placed.
                let index: Vec<u32> = piece
                    .iter()
                    .map(|&p| {
                        let key = ((p.x * 1000.0).round() as i64, (p.y * 1000.0).round() as i64);
                        *at.entry(key).or_insert_with(|| {
                            let z = barycentric_z(&tri, &corner, p);
                            points.push(place(p.extend(z)));
                            normals.push(DVec3::ZERO);
                            points.len() as u32 - 1
                        })
                    })
                    .collect();
                for k in 1..index.len() - 1 {
                    let t = [index[0], index[k], index[k + 1]];
                    let q = t.map(|v| points[v as usize]);
                    let n = (q[1] - q[0]).cross(q[2] - q[0]);
                    if n.length_squared() < 1e-12 {
                        continue;
                    }
                    for v in t {
                        normals[v as usize] += n;
                    }
                    mesh.indices.extend(t);
                }
            }
        }
    }
    mesh.positions = points.iter().map(|p| p.as_vec3().to_array()).collect();
    mesh.normals = normals
        .iter()
        .map(|n| n.normalize_or(DVec3::Z).as_vec3().to_array())
        .collect();
    mesh.uvs = points
        .iter()
        .map(|p| [(p.x / tile[0]) as f32, (p.y / tile[1]) as f32])
        .collect();
    mesh
}

/// The height at `p` of the plane through a triangle's corners.
fn barycentric_z(tri: &[DVec2; 3], corner: &[DVec3; 3], p: DVec2) -> f64 {
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let d = (b - a).perp_dot(c - a);
    if d.abs() < 1e-12 {
        return corner[0].z;
    }
    let wb = (p - a).perp_dot(c - a) / d;
    let wc = (b - a).perp_dot(p - a) / d;
    corner[0].z * (1.0 - wb - wc) + corner[1].z * wb + corner[2].z * wc
}

/// The part of a triangle (anticlockwise) inside a box: a convex polygon, anticlockwise.
fn clip(tri: &[DVec2; 3], lo: DVec2, hi: DVec2) -> Vec<DVec2> {
    let mut poly = tri.to_vec();
    // Each side of the box: a point is inside where `inside` is not negative.
    let sides: [(DVec2, f64); 4] = [
        (DVec2::X, -lo.x),
        (-DVec2::X, hi.x),
        (DVec2::Y, -lo.y),
        (-DVec2::Y, hi.y),
    ];
    for (n, c) in sides {
        let inside = |p: DVec2| p.dot(n) + c;
        let mut out = Vec::with_capacity(poly.len() + 2);
        for k in 0..poly.len() {
            let (p, q) = (poly[k], poly[(k + 1) % poly.len()]);
            let (dp, dq) = (inside(p), inside(q));
            if dp >= 0.0 {
                out.push(p);
            }
            if (dp >= 0.0) != (dq >= 0.0) {
                out.push(p + (q - p) * (dp / (dp - dq)));
            }
        }
        poly = out;
        if poly.len() < 3 {
            return poly;
        }
    }
    poly
}

/// Triangles filling a simple polygon, as corners' indices, anticlockwise: ear clipping.
/// A polygon crossing itself is filled as a fan from its first point.
pub fn triangulate(points: &[DVec2]) -> Vec<[usize; 3]> {
    let n = points.len();
    if n < 3 {
        return vec![];
    }
    let area: f64 = (0..n)
        .map(|i| points[i].perp_dot(points[(i + 1) % n]))
        .sum();
    let mut left: Vec<usize> = if area >= 0.0 {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    let mut out = Vec::with_capacity(n);
    let cross = |a: DVec2, b: DVec2, c: DVec2| (b - a).perp_dot(c - a);
    let mut stuck = 0;
    let mut k = 0;
    while left.len() > 3 {
        let m = left.len();
        let (ia, ib, ic) = (left[(k + m - 1) % m], left[k % m], left[(k + 1) % m]);
        let (a, b, c) = (points[ia], points[ib], points[ic]);
        let convex = cross(a, b, c) > 1e-12;
        let empty = convex
            && left.iter().all(|&j| {
                if j == ia || j == ib || j == ic {
                    return true;
                }
                let p = points[j];
                !(cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0)
            });
        if empty {
            out.push([ia, ib, ic]);
            left.remove(k % m);
            stuck = 0;
        } else {
            k += 1;
            stuck += 1;
            if stuck > m {
                // No ear: the outline crosses itself.
                break;
            }
        }
    }
    if left.len() == 3 {
        out.push([left[0], left[1], left[2]]);
    } else if left.len() > 3 {
        for i in 1..left.len() - 1 {
            out.push([left[0], left[i], left[i + 1]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_l_shaped_outline_is_filled_facing_up_and_nowhere_else() {
        // An L: 20 × 20 m with a 10 × 10 m bite out of it.
        let l = [
            (0.0, 0.0),
            (20.0, 0.0),
            (20.0, 10.0),
            (10.0, 10.0),
            (10.0, 20.0),
            (0.0, 20.0),
        ]
        .map(|(x, y)| DVec3::new(x, y, 1.0));
        for outline in [l.to_vec(), l.iter().rev().copied().collect()] {
            let m = area(&outline, 0.5, [4.0, 4.0], |p| p);
            let mut total = 0.0;
            for t in m.indices.as_chunks::<3>().0 {
                let p = t.map(|i| DVec3::from(m.positions[i as usize].map(f64::from)));
                let n = (p[1] - p[0]).cross(p[2] - p[0]);
                assert!(n.z > 0.0);
                total += 0.5 * n.z;
                // None in the bite.
                let mid = (p[0] + p[1] + p[2]) / 3.0;
                assert!(!(mid.x > 10.0 && mid.y > 10.0), "{mid}");
            }
            assert!((total - 300.0).abs() < 1e-6, "{total}");
            assert!(m.positions.iter().all(|p| p[2] == 1.0));
        }
    }

    #[test]
    fn nodes_radii_ease_between_them() {
        let mut nodes = vec![Node::at(0.0, 0.0, 0.0), Node::at(10.0, 0.0, 0.0)];
        nodes[1].radius = 3.0;
        assert_eq!(radius_at(&nodes, false, 0.0), 1.0);
        assert_eq!(radius_at(&nodes, false, 0.5), 2.0);
        assert_eq!(radius_at(&nodes, false, 1.0), 3.0);
    }
}
