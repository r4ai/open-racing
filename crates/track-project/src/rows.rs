//! Rows of a model along a road: trees, cones, marker boards, lamp posts, spectators'
//! stands, pit garages. Each copy stands at a distance from the road's edge on one side,
//! every so many metres along its stretches or at given places, turned to face the road
//! (the model's +X along the road, +Y towards it, as a wall's models), and may vary a
//! little in place, turn and size, the same way every build.

use glam::DVec3;

use crate::curve::Sampled;
use crate::project::{Prop, PropRow, Road, Side};

/// A number in [-1, 1] that is the same for the same row and copy.
fn wobble(name: &str, k: usize, what: u32) -> f64 {
    let mut h: u32 = 0x811c_9dc5 ^ what.wrapping_mul(0x9e37_79b9);
    for b in name.bytes().chain(k.to_le_bytes()) {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f64 / 32767.5 - 1.0
}

/// Distances along the road (m) of a row's copies.
fn places(road: &Road, smp: &Sampled, row: &PropRow) -> Vec<f64> {
    if !row.at.is_empty() {
        return row.at.iter().map(|&u| smp.s_at(u)).collect();
    }
    let spacing = row.spacing.max(0.1);
    let stretches: Vec<(f64, f64)> = if row.ranges.is_empty() {
        vec![(0.0, smp.length)]
    } else {
        row.ranges
            .iter()
            .map(|r| {
                let (a, mut b) = (smp.s_at(r.from), smp.s_at(r.to));
                if b < a && smp.closed {
                    b += smp.length;
                }
                (a, b)
            })
            .collect()
    };
    let mut out = Vec::new();
    for (a, b) in stretches {
        // A whole number of gaps, a copy at each end of an open stretch; round a loop
        // the last gap is the first.
        let whole = row.ranges.is_empty() && road.closed;
        let n = ((b - a) / spacing).round().max(1.0) as usize;
        let step = (b - a) / n as f64;
        let count = if whole { n } else { n + 1 };
        out.extend((0..count).map(|k| a + k as f64 * step));
    }
    out
}

/// Where each copy of `row` stands along `road`, as props: on the road's side at the
/// row's distance from its edge, at the road's height there (stood on the ground under
/// it when the row drapes).
pub fn copies(road: &Road, smp: &Sampled, row: &PropRow) -> Vec<Prop> {
    if smp.frames.is_empty() {
        return vec![];
    }
    places(road, smp, row)
        .into_iter()
        .enumerate()
        .map(|(k, s)| {
            let f = smp.frame_at(s);
            let edge = match row.side {
                Side::Left => f.width_left,
                Side::Right => f.width_right,
            };
            let j = &row.jitter;
            let out = edge + row.offset + j.offset * wobble(&row.name, k, 1);
            let left = DVec3::Z.cross(f.tangent).normalize_or(DVec3::Y);
            let heading = f.tangent.truncate().to_angle();
            // +Y towards the road: on the left side the road lies to the model's right.
            let facing = match row.side {
                Side::Left => heading + std::f64::consts::PI,
                Side::Right => heading,
            };
            let pos = f.pos + left * (row.side.sign() * out);
            Prop {
                name: format!("{} {}", row.name, k + 1),
                model: row.model.clone(),
                pos: pos.with_z(f.pos.z),
                yaw: facing + row.yaw + j.yaw * wobble(&row.name, k, 2),
                scale: (row.scale * (1.0 + j.scale * wobble(&row.name, k, 3))).max(1e-3),
                drape: row.drape,
                collide: row.collide,
            }
        })
        .collect()
}

/// Every row's copies on every road of a built scene, as props.
pub fn all(project: &crate::Project, sampled: &[&Sampled]) -> Vec<Prop> {
    project
        .roads
        .iter()
        .zip(sampled)
        .flat_map(|(r, smp)| r.rows.iter().flat_map(move |row| copies(r, smp, row)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Jitter, Range};
    use glam::DVec2;

    fn row() -> PropRow {
        PropRow {
            name: "trees".into(),
            model: "assets/models/tree.glb".into(),
            side: Side::Left,
            offset: 10.0,
            spacing: 20.0,
            ranges: vec![Range { from: 0.0, to: 1.0 }],
            at: vec![],
            yaw: 0.0,
            scale: 1.0,
            jitter: Jitter::default(),
            drape: true,
            collide: false,
        }
    }

    #[test]
    fn copies_stand_evenly_beside_the_road_facing_it() {
        let p = crate::Project::new("t");
        let road = &p.roads[0];
        let smp = Sampled::new(road, road.resolution);
        // The first straight runs east from (0, 0) to (250, 0), 6 m wide on the left.
        let c = copies(road, &smp, &row());
        let n = (smp.s_at(1.0) / 20.0).round() as usize + 1;
        assert_eq!(c.len(), n);
        // Halfway along it: 16 m north of the middle, facing south.
        let mid = copies(
            road,
            &smp,
            &PropRow {
                at: vec![0.5],
                ..row()
            },
        );
        let f = smp.frame_at(smp.s_at(0.5));
        assert!((mid[0].pos.truncate() - f.pos.truncate()).length() - 16.0 < 1e-6);
        assert!(mid[0].pos.y > f.pos.y + 15.9, "{:?}", mid[0].pos);
        // +Y towards the road: south, so +X points west.
        let x = DVec2::from_angle(mid[0].yaw);
        assert!((x - DVec2::new(-1.0, 0.0)).length() < 0.1, "{x}");
        // On the right the other way round.
        let right = copies(
            road,
            &smp,
            &PropRow {
                side: Side::Right,
                at: vec![0.5],
                ..row()
            },
        );
        assert!(right[0].pos.y < f.pos.y - 15.9);
        assert!((DVec2::from_angle(right[0].yaw) - DVec2::X).length() < 0.1);
        // Jitter varies them, the same each time.
        let j = PropRow {
            jitter: Jitter {
                offset: 2.0,
                yaw: 0.3,
                scale: 0.2,
            },
            ..row()
        };
        let (a, b) = (copies(road, &smp, &j), copies(road, &smp, &j));
        assert_eq!(a, b);
        let plain = copies(road, &smp, &row());
        let moved: Vec<f64> = a
            .iter()
            .zip(&plain)
            .map(|(p, q)| p.pos.distance(q.pos))
            .collect();
        assert!(moved.iter().any(|&d| d > 0.1));
        assert!(moved.iter().all(|&d| d <= 2.0 + 1e-9));
        // At given places instead.
        let at = copies(
            road,
            &smp,
            &PropRow {
                at: vec![0.5, 2.0],
                ..row()
            },
        );
        assert_eq!(at.len(), 2);
    }
}

/// The row that puts a garage (model `model`) behind each pit box, `offset` m beyond
/// the pit lane's edge on the boxes' side.
pub fn garages(
    project: &crate::Project,
    model: &std::path::Path,
    offset: f64,
) -> Result<crate::ops::Op, crate::Error> {
    let pit = project
        .markers
        .pit
        .as_ref()
        .ok_or_else(|| crate::Error::Invalid("there is no pit lane: lay one first".into()))?;
    if pit.boxes.is_empty() {
        return Err(crate::Error::Invalid("the pit lane has no boxes".into()));
    }
    Ok(crate::ops::Op::PutRow {
        road: pit.road.clone(),
        row: PropRow {
            name: "garages".into(),
            model: model.to_path_buf(),
            side: pit.box_side,
            offset,
            spacing: 10.0,
            ranges: vec![],
            at: pit.boxes.clone(),
            yaw: 0.0,
            scale: 1.0,
            jitter: crate::project::Jitter::default(),
            drape: true,
            collide: true,
        },
    })
}

/// Rows of distance boards before each corner of road `road` turning at least
/// `least` radians: one board `distances` metres (100, 200, 300…) before where the
/// corner starts, on its outside, `offset` m beyond the road's edge. Each corner's
/// boards are the row "T3 boards" (for corner 3), replacing those laid before.
pub fn boards(
    project: &crate::Project,
    road: &str,
    model: &std::path::Path,
    distances: &[f64],
    least: f64,
    offset: f64,
) -> Result<Vec<crate::ops::Op>, crate::Error> {
    let i = project
        .road_index(road)
        .ok_or_else(|| crate::Error::Invalid(format!("no road named \"{road}\"")))?;
    let (smp, corners) = crate::corners::of_road(project, i);
    let mut ops = Vec::new();
    for c in corners.iter().filter(|c| c.angle.abs() >= least) {
        let at: Vec<f64> = distances
            .iter()
            .map(|d| {
                let s = c.entry - d;
                if smp.closed {
                    smp.u_at(s.rem_euclid(smp.length))
                } else {
                    smp.u_at(s.max(0.0))
                }
            })
            .collect();
        ops.push(crate::ops::Op::PutRow {
            road: road.to_string(),
            row: PropRow {
                name: format!("T{} boards", c.number),
                model: model.to_path_buf(),
                side: c.outside(),
                offset,
                spacing: 100.0,
                ranges: vec![],
                at,
                yaw: 0.0,
                scale: 1.0,
                jitter: crate::project::Jitter::default(),
                drape: true,
                collide: false,
            },
        });
    }
    Ok(ops)
}
