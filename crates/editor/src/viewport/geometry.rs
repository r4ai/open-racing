//! Where the parts of a road limited to stretches lie in the view, and the handles on
//! them.

use super::*;

/// The parts of a road limited to stretches, with the stretches.
pub(super) fn parts(road: &Road) -> impl Iterator<Item = (Part, &[Range])> {
    let strips = [Side::Left, Side::Right].into_iter().flat_map(move |side| {
        road.strips(side)
            .iter()
            .enumerate()
            .map(move |(i, s)| (Part::Strip(side, i), s.ranges.as_slice()))
    });
    let barriers = road
        .barriers
        .iter()
        .enumerate()
        .map(|(i, b)| (Part::Barrier(i), b.ranges.as_slice()));
    let rows = road
        .rows
        .iter()
        .enumerate()
        .filter(|(_, w)| w.at.is_empty())
        .map(|(i, w)| (Part::Row(i), w.ranges.as_slice()));
    let lines = road
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| (Part::Line(i), l.ranges.as_slice()));
    strips
        .chain(barriers)
        .chain(rows)
        .chain(lines)
        .filter(|(_, r)| !r.is_empty())
}

/// A copy of one of a road's parts, to change and put back with one operation.
pub enum PartValue {
    Strip(Side, Strip),
    Barrier(Barrier),
    Row(PropRow),
    Line(PaintLine),
}

impl PartValue {
    /// The part, if the road still has it (a menu or a drag may outlive it).
    pub fn of(road: &Road, part: Part) -> Option<Self> {
        Some(match part {
            Part::Strip(side, i) => Self::Strip(side, road.strips(side).get(i)?.clone()),
            Part::Barrier(i) => Self::Barrier(road.barriers.get(i)?.clone()),
            Part::Row(i) => Self::Row(road.rows.get(i)?.clone()),
            Part::Line(i) => Self::Line(road.lines.get(i)?.clone()),
        })
    }

    pub fn ranges_mut(&mut self) -> &mut Vec<Range> {
        match self {
            Self::Strip(_, s) => &mut s.ranges,
            Self::Barrier(b) => &mut b.ranges,
            Self::Row(r) => &mut r.ranges,
            Self::Line(l) => &mut l.ranges,
        }
    }

    /// The corner it was laid round, for the parts that can be.
    pub fn corner_mut(&mut self) -> Option<&mut Option<Anchor>> {
        match self {
            Self::Strip(_, s) => Some(&mut s.corner),
            Self::Barrier(b) => Some(&mut b.corner),
            Self::Row(_) | Self::Line(_) => None,
        }
    }

    /// The operation that puts it on `road` as it now is.
    pub fn put(self, road: &str) -> Op {
        let road = road.to_string();
        match self {
            Self::Strip(side, strip) => Op::PutStrip {
                road,
                side,
                strip,
                at: None,
            },
            Self::Barrier(barrier) => Op::PutBarrier { road, barrier },
            Self::Row(row) => Op::PutRow { road, row },
            Self::Line(line) => Op::PutLine { road, line },
        }
    }

    /// The operation that takes it off `road`.
    pub fn remove(self, road: &str) -> Op {
        let road = road.to_string();
        match self {
            Self::Strip(side, s) => Op::RemoveStrip {
                road,
                side,
                name: s.name,
            },
            Self::Barrier(b) => Op::RemoveBarrier { road, name: b.name },
            Self::Row(r) => Op::RemoveRow { road, name: r.name },
            Self::Line(l) => Op::RemoveLine { road, name: l.name },
        }
    }
}

/// Width of a side's strips inside strip `i` at frame `f`: each as wide as it is there,
/// so none where it is limited to stretches elsewhere, as the road is built.
pub(super) fn inner_width(road: &Road, smp: &Sampled, side: Side, i: usize, f: &Frame) -> f64 {
    road.strips(side)[..i]
        .iter()
        .map(|s| smp.strip_shape(s, f.s).0 * smp.presence(&s.ranges, s.fade, f.s))
        .sum()
}

/// Width of strip `i` of a side at frame `f`, from its keys.
fn strip_width(road: &Road, smp: &Sampled, side: Side, i: usize, f: &Frame) -> f64 {
    smp.strip_shape(&road.strips(side)[i], f.s).0
}

/// Where the handle of a strip's key is drawn: on its outer edge.
pub(super) fn key_pos(road: &Road, smp: &Sampled, key: KeyRef) -> Option<DVec3> {
    let k = road.strips(key.side).get(key.strip)?.keys.get(key.key)?;
    let f = smp.frame_at(smp.s_at(k.u));
    Some(f.pos + flat_left(&f) * part_reach(road, smp, Part::Strip(key.side, key.strip), &f))
}

/// The keys of a road's strips.
pub(super) fn strip_keys(road: &Road, r: usize) -> impl Iterator<Item = KeyRef> + '_ {
    [Side::Left, Side::Right].into_iter().flat_map(move |side| {
        road.strips(side)
            .iter()
            .enumerate()
            .flat_map(move |(strip, s)| {
                (0..s.keys.len()).map(move |key| KeyRef {
                    road: r,
                    side,
                    strip,
                    key,
                })
            })
    })
}

/// The strip `d` m left of the road's centre at frame `f`, where it is.
pub(super) fn strip_at(road: &Road, smp: &Sampled, f: &Frame, d: f64) -> Option<(Side, usize)> {
    let side = if d >= 0.0 { Side::Left } else { Side::Right };
    let mut edge = edge_of(f, side);
    let out = d.abs();
    for (i, s) in road.strips(side).iter().enumerate() {
        let w = smp.strip_shape(s, f.s).0 * smp.presence(&s.ranges, s.fade, f.s);
        if w > 0.0 && (edge..=edge + w).contains(&out) {
            return Some((side, i));
        }
        edge += w;
    }
    None
}

pub(super) fn edge_of(f: &Frame, side: Side) -> f64 {
    match side {
        Side::Left => f.width_left,
        Side::Right => f.width_right,
    }
}

/// How far to the left of the road's centre a part lies in frame `f`: a strip's middle,
/// a barrier's face.
pub(super) fn part_offset(road: &Road, smp: &Sampled, part: Part, f: &Frame) -> f64 {
    match part {
        Part::Strip(side, i) => {
            let inner = inner_width(road, smp, side, i, f);
            side.sign() * (edge_of(f, side) + inner + 0.5 * strip_width(road, smp, side, i, f))
        }
        Part::Barrier(i) => {
            let b = &road.barriers[i];
            b.side.sign() * (edge_of(f, b.side) + b.offset)
        }
        Part::Row(i) => {
            let w = &road.rows[i];
            w.side.sign() * (edge_of(f, w.side) + w.offset)
        }
        Part::Line(i) => road.lines[i].offset,
    }
}

/// How far to the left of the road's centre a part's outer edge lies: a strip's far
/// side, a barrier's face.
pub(super) fn part_reach(road: &Road, smp: &Sampled, part: Part, f: &Frame) -> f64 {
    match part {
        Part::Strip(side, i) => {
            let inner = inner_width(road, smp, side, i, f);
            side.sign() * (edge_of(f, side) + inner + strip_width(road, smp, side, i, f))
        }
        Part::Barrier(_) | Part::Row(_) | Part::Line(_) => part_offset(road, smp, part, f),
    }
}

/// Distance along the road to a stretch's middle.
pub(super) fn range_middle(smp: &Sampled, rg: &Range) -> f64 {
    let (a, mut b) = (smp.s_at(rg.from), smp.s_at(rg.to));
    if b < a && smp.closed {
        b += smp.length;
    }
    0.5 * (a + b)
}

/// Where the handle on a stretch's outer edge is drawn.
pub(super) fn reach_pos(road: &Road, smp: &Sampled, part: Part, rg: &Range) -> DVec3 {
    let f = smp.frame_at(range_middle(smp, rg));
    f.pos + flat_left(&f) * part_reach(road, smp, part, &f)
}

/// A frame's lateral, level.
pub(super) fn flat_left(f: &Frame) -> DVec3 {
    f.lateral.truncate().normalize_or(DVec2::Y).extend(0.0)
}

/// Where the handle on a road's edge at node `n` is drawn.
pub(super) fn edge_pos(smp: &Sampled, n: usize, side: Side) -> DVec3 {
    let f = smp.frame_at(smp.s_at(n as f64));
    let w = match side {
        Side::Left => f.width_left,
        Side::Right => f.width_right,
    };
    f.pos + flat_left(&f) * (side.sign() * w)
}

/// Where a stretch's end is drawn.
pub(super) fn range_end_pos(road: &Road, smp: &Sampled, part: Part, u: f64) -> DVec3 {
    let f = smp.frame_at(smp.s_at(u));
    f.pos + f.lateral * part_offset(road, smp, part, &f)
}
