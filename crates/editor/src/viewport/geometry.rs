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
    strips.chain(barriers).filter(|(_, r)| !r.is_empty())
}

/// Width of a side's strips inside strip `i` at frame `f`: each as wide as it is there,
/// so none where it is limited to stretches elsewhere, as the road is built.
pub(super) fn inner_width(road: &Road, smp: &Sampled, side: Side, i: usize, f: &Frame) -> f64 {
    road.strips(side)[..i]
        .iter()
        .map(|s| s.width * smp.presence(&s.ranges, s.fade, f.s))
        .sum()
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
            side.sign() * (edge_of(f, side) + inner + 0.5 * road.strips(side)[i].width)
        }
        Part::Barrier(i) => {
            let b = &road.barriers[i];
            b.side.sign() * (edge_of(f, b.side) + b.offset)
        }
    }
}

/// How far to the left of the road's centre a part's outer edge lies: a strip's far
/// side, a barrier's face.
pub(super) fn part_reach(road: &Road, smp: &Sampled, part: Part, f: &Frame) -> f64 {
    match part {
        Part::Strip(side, i) => {
            let inner = inner_width(road, smp, side, i, f);
            side.sign() * (edge_of(f, side) + inner + road.strips(side)[i].width)
        }
        Part::Barrier(_) => part_offset(road, smp, part, f),
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
