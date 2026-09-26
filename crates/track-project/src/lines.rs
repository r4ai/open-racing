//! Cutting roads and splines in two, joining them end to end and turning them round,
//! keeping what lies along a road (its widths and banking, strips, lines, barriers,
//! marks and the race markers on it) where it was.

use glam::DVec3;

use crate::Error;
use crate::project::{Key, Node, Project, Range, Road, Side, StationCurve};

/// How close an end must be to the other line's start for the two to share a node, m.
const SHARED: f64 = 0.5;

fn invalid(msg: String) -> Error {
    Error::Invalid(msg)
}

/// A road's or spline's nodes and whether it is closed.
fn line(p: &Project, name: &str) -> Result<(Vec<Node>, bool), Error> {
    p.line(name)
        .map(|(n, c)| (n.to_vec(), c))
        .ok_or_else(|| invalid(format!("no road or spline named \"{name}\"")))
}

/// Nodes in the other order, each with its handles swapped round. A loop keeps its
/// first node.
fn reversed_nodes(nodes: &[Node], closed: bool) -> Vec<Node> {
    let mut out: Vec<Node> = if closed {
        std::iter::once(nodes[0])
            .chain(nodes[1..].iter().rev().copied())
            .collect()
    } else {
        nodes.iter().rev().copied().collect()
    };
    for n in &mut out {
        n.handles = n.handles.reversed();
    }
    out
}

/// A profile along a road driven the other way, its values through `value`.
fn reversed_curve(c: &StationCurve, f: &impl Fn(f64) -> f64, value: f64) -> StationCurve {
    let mut keys: Vec<Key> = c
        .keys
        .iter()
        .map(|k| Key {
            u: f(k.u),
            value: value * k.value,
            // d/du' = -d/du, and in and out swap.
            slope_in: -value * k.slope_out,
            slope_out: -value * k.slope_in,
        })
        .collect();
    keys.sort_by(|a, b| a.u.total_cmp(&b.u));
    StationCurve { keys }
}

fn reversed_ranges(ranges: &mut [Range], f: &impl Fn(f64) -> f64) {
    for r in ranges {
        (r.from, r.to) = (f(r.to), f(r.from));
    }
}

/// Turns road or spline `name` round, to be driven or drawn the other way. A road's left
/// and right swap, its banking changes sign, and the parts laid round corners swap
/// entry and exit.
pub fn reverse(p: &mut Project, name: &str) -> Result<(), Error> {
    let (nodes, closed) = line(p, name)?;
    let n = nodes.len();
    let period = crate::curve::segments(n, closed) as f64;
    let f = move |u: f64| {
        if closed {
            (period - u).rem_euclid(period.max(1.0))
        } else {
            period - u
        }
    };
    if let Some(sp) = p.splines.iter_mut().find(|s| s.name == name) {
        sp.nodes = reversed_nodes(&nodes, closed);
        if let crate::project::Shape::Band { align, .. } = &mut sp.shape {
            *align = match *align {
                crate::project::Align::Left => crate::project::Align::Right,
                crate::project::Align::Right => crate::project::Align::Left,
                crate::project::Align::Center => crate::project::Align::Center,
            };
        }
        return Ok(());
    }
    let road = p
        .roads
        .iter_mut()
        .find(|r| r.name == name)
        .expect("a road or a spline");
    road.nodes = reversed_nodes(&nodes, closed);
    let (left, right) = (road.width_left.clone(), road.width_right.clone());
    road.width_left = reversed_curve(&right, &f, 1.0);
    road.width_right = reversed_curve(&left, &f, 1.0);
    road.bank = reversed_curve(&road.bank, &f, -1.0);
    std::mem::swap(&mut road.left, &mut road.right);
    let turn = |a: &mut Option<crate::project::Anchor>| {
        if let Some(a) = a {
            a.apex = f(a.apex);
            a.shift = [-a.shift[1], -a.shift[0]];
            a.part = match a.part {
                crate::project::CornerPart::Entry => crate::project::CornerPart::Exit,
                crate::project::CornerPart::Exit => crate::project::CornerPart::Entry,
                other => other,
            };
        }
    };
    for s in road.left.iter_mut().chain(road.right.iter_mut()) {
        reversed_ranges(&mut s.ranges, &f);
        turn(&mut s.corner);
        s.keys.iter_mut().for_each(|k| k.u = f(k.u));
        s.keys.sort_by(|a, b| a.u.total_cmp(&b.u));
    }
    for b in &mut road.barriers {
        b.side = b.side.other();
        reversed_ranges(&mut b.ranges, &f);
        turn(&mut b.corner);
    }
    for l in &mut road.lines {
        l.offset = -l.offset;
        reversed_ranges(&mut l.ranges, &f);
    }
    for m in &mut road.marks {
        m.at = f(m.at);
        (m.from, m.to) = (-m.to, -m.from);
    }
    for w in &mut road.rows {
        w.side = w.side.other();
        reversed_ranges(&mut w.ranges, &f);
        w.at.iter_mut().for_each(|u| *u = f(*u));
    }
    let m = &mut p.markers;
    if p.main_road == name {
        m.start = f(m.start);
        m.sectors.iter_mut().for_each(|u| *u = f(*u));
        m.sectors.sort_by(f64::total_cmp);
    }
    if let Some(pit) = &mut m.pit
        && pit.road == name
    {
        pit.boxes.iter_mut().for_each(|u| *u = f(*u));
        pit.box_side = pit.box_side.other();
    }
    Ok(())
}

/// A profile's stretch from `from` to `to` (in `u`), as a profile starting at 0 there;
/// the values at its ends are kept with keys.
fn cut_curve(c: &StationCurve, period: f64, closed: bool, from: f64, to: f64) -> StationCurve {
    // The profile is cubic between keys: keys at the cut with its value and slope there
    // keep the stretch as it was.
    let at = |u: f64, place: f64| {
        let wrapped = if closed { u.rem_euclid(period) } else { u };
        if let Some(k) = c.keys.iter().find(|k| (k.u - wrapped).abs() < 1e-9) {
            return Key { u: place, ..*k };
        }
        let h = 1e-4;
        let slope = (c.eval(u + h, period, closed) - c.eval(u - h, period, closed)) / (2.0 * h);
        Key {
            u: place,
            value: c.eval(u, period, closed),
            slope_in: slope,
            slope_out: slope,
        }
    };
    if c.keys.len() == 1 {
        return c.clone();
    }
    let mut keys = vec![at(from, 0.0)];
    let inside = |u: f64| u > from + 1e-9 && u < to - 1e-9;
    // On a loop cut open, the keys past its first node come after the ones before it.
    let shifted = c.keys.iter().flat_map(|k| {
        [k.u, k.u + period]
            .into_iter()
            .filter(move |_| closed)
            .chain((!closed).then_some(k.u))
            .map(move |u| (u, *k))
    });
    for (u, k) in shifted {
        if inside(u) {
            keys.push(Key { u: u - from, ..k });
        }
    }
    keys.push(at(to, to - from));
    keys.sort_by(|a, b| a.u.total_cmp(&b.u));
    keys.dedup_by(|a, b| (a.u - b.u).abs() < 1e-9);
    StationCurve { keys }
}

/// Stretches (in `u`) within the stretch from `from` to `to`, measured from `from`.
/// `None` for everywhere stays everywhere; stretches left with nothing in it give an
/// empty list, and the part goes.
fn cut_ranges(
    ranges: &[Range],
    period: f64,
    closed: bool,
    from: f64,
    to: f64,
) -> Option<Vec<Range>> {
    if ranges.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for r in ranges {
        // Each stretch as plain intervals, a loop's over its start as two, and again a
        // lap on for a cut that runs past the loop's end.
        let pieces: Vec<(f64, f64)> = if closed && r.from > r.to {
            vec![(r.from, period + r.to), (r.from - period, r.to)]
        } else {
            vec![(r.from, r.to)]
        };
        let laps: &[f64] = if closed { &[0.0, 1.0] } else { &[0.0] };
        for (a, b) in pieces {
            for lap in laps {
                let (a, b) = (a + lap * period, b + lap * period);
                let (a, b) = (a.max(from), b.min(to));
                if b - a > 1e-6 {
                    out.push(Range {
                        from: a - from,
                        to: b - from,
                    });
                }
            }
        }
    }
    Some(out)
}

/// The stretch of `road` from `from` to `to` (in `u`, whole nodes), as a road of its own
/// named `name`: its nodes, and what lies on that stretch. `from` may run past a loop's
/// end.
fn cut(road: &Road, from: usize, to: usize, name: &str) -> Road {
    let n = road.nodes.len();
    let period = road.period();
    let closed = road.closed;
    let nodes: Vec<Node> = (from..=to).map(|i| road.nodes[i % n]).collect();
    let (a, b) = (from as f64, to as f64);
    let curve = |c: &StationCurve| cut_curve(c, period, closed, a, b);
    let mut out = Road {
        name: name.to_string(),
        closed: false,
        nodes,
        width_left: curve(&road.width_left),
        width_right: curve(&road.width_right),
        bank: curve(&road.bank),
        left: vec![],
        right: vec![],
        lines: vec![],
        barriers: vec![],
        marks: vec![],
        rows: vec![],
        ..road.clone()
    };
    let ranges = |r: &[Range]| cut_ranges(r, period, closed, a, b);
    // Corner anchors follow the cut too; parts whose corner is gone stay where they are.
    let along = |u: f64| {
        let u = if closed && u < a { u + period } else { u };
        u - a
    };
    for side in [Side::Left, Side::Right] {
        for s in road.strips(side) {
            let mut s = s.clone();
            match ranges(&s.ranges) {
                Some(r) if r.is_empty() => continue,
                Some(r) => s.ranges = r,
                None => {}
            }
            if let Some(c) = &mut s.corner {
                c.apex = along(c.apex);
            }
            s.keys = s
                .keys
                .iter()
                .map(|k| crate::project::StripKey {
                    u: along(k.u),
                    ..*k
                })
                .filter(|k| (0.0..=b - a).contains(&k.u))
                .collect();
            out.strips_mut(side).push(s);
        }
    }
    for l in &road.lines {
        let mut l = l.clone();
        match ranges(&l.ranges) {
            Some(r) if r.is_empty() => continue,
            Some(r) => l.ranges = r,
            None => {}
        }
        out.lines.push(l);
    }
    for bar in &road.barriers {
        let mut bar = bar.clone();
        match ranges(&bar.ranges) {
            Some(r) if r.is_empty() => continue,
            Some(r) => bar.ranges = r,
            None => {}
        }
        if let Some(c) = &mut bar.corner {
            c.apex = along(c.apex);
        }
        out.barriers.push(bar);
    }
    for m in &road.marks {
        let at = along(m.at);
        if (0.0..=b - a).contains(&at) {
            out.marks.push(crate::project::Mark { at, ..m.clone() });
        }
    }
    for w in &road.rows {
        let mut w = w.clone();
        if !w.at.is_empty() {
            w.at =
                w.at.iter()
                    .map(|&u| along(u))
                    .filter(|u| (0.0..=b - a).contains(u))
                    .collect();
            if w.at.is_empty() {
                continue;
            }
        } else {
            match ranges(&w.ranges) {
                Some(r) if r.is_empty() => continue,
                Some(r) => w.ranges = r,
                None => {}
            }
        }
        out.rows.push(w);
    }
    out
}

/// Cuts road or spline `name` at node `at`. An open line becomes two: itself up to the
/// node, and `to` from it on, both having the node. A loop opens there instead, starting
/// and ending at the node (`to` is not used).
pub fn split(p: &mut Project, name: &str, at: usize, to: &str) -> Result<(), Error> {
    let (nodes, closed) = line(p, name)?;
    let n = nodes.len();
    if at >= n {
        return Err(invalid(format!("\"{name}\" has no node {at} (it has {n})")));
    }
    if !closed && (at == 0 || at + 1 == n) {
        return Err(invalid(format!(
            "node {at} is an end of \"{name}\": cut at a node between its ends"
        )));
    }
    if closed && p.main_road == name {
        return Err(invalid(
            "the main road must stay a loop: make another road the main road first".into(),
        ));
    }
    if !closed && p.line(to).is_some() {
        return Err(invalid(format!("a road or spline named \"{to}\" exists")));
    }
    if let Some(sp) = p.splines.iter_mut().find(|s| s.name == name) {
        if closed {
            sp.nodes = (at..=at + n).map(|i| nodes[i % n]).collect();
            sp.closed = false;
        } else {
            let mut second = sp.clone();
            second.name = to.to_string();
            second.nodes = nodes[at..].to_vec();
            sp.nodes = nodes[..=at].to_vec();
            p.splines.push(second);
        }
        return Ok(());
    }
    let i = p.road_index(name).expect("a road or a spline");
    let road = p.roads[i].clone();
    let pit = p.markers.pit.as_ref().is_some_and(|pit| pit.road == name);
    if closed {
        p.roads[i] = cut(&road, at, at + n, name);
        if pit && let Some(pit) = &mut p.markers.pit {
            let period = n as f64;
            pit.boxes
                .iter_mut()
                .for_each(|u| *u = (*u - at as f64).rem_euclid(period));
        }
        return Ok(());
    }
    p.roads[i] = cut(&road, 0, at, name);
    p.roads.insert(i + 1, cut(&road, at, n - 1, to));
    if pit && let Some(pit) = &mut p.markers.pit {
        // The boxes stay on the first part; those beyond the cut go.
        pit.boxes.retain(|&u| u <= at as f64);
    }
    Ok(())
}

/// A road's stretches as they will be on a longer road they start `offset` along, a
/// road `length` long (in `u`): everywhere becomes their own stretch.
fn moved_ranges(ranges: &[Range], offset: f64, length: f64) -> Vec<Range> {
    if ranges.is_empty() {
        return vec![Range {
            from: offset,
            to: offset + length,
        }];
    }
    ranges
        .iter()
        .map(|r| Range {
            from: r.from + offset,
            to: r.to + offset,
        })
        .collect()
}

/// Stretches that meet end to start run on as one.
fn merge_ranges(mut ranges: Vec<Range>) -> Vec<Range> {
    ranges.sort_by(|a, b| a.from.total_cmp(&b.from));
    let mut out: Vec<Range> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.from <= last.to + 1e-6 => last.to = last.to.max(r.to),
            _ => out.push(r),
        }
    }
    out
}

/// Joins open line `with` onto the end of open line `name`, turning `with` round first
/// if its end is nearer; where the ends meet they share a node, else a segment joins
/// them. A road's strips, lines and barriers of the same name run on across the join;
/// those only one of them had stay on its stretch.
pub fn join(p: &mut Project, name: &str, with: &str) -> Result<(), Error> {
    if name == with {
        return Err(invalid(
            "a line joins another; to make a loop of one, close it".into(),
        ));
    }
    let (a, a_closed) = line(p, name)?;
    let (b, b_closed) = line(p, with)?;
    if a_closed || b_closed {
        return Err(invalid("only open lines join".into()));
    }
    let road_a = p.road(name).is_some();
    if road_a != p.road(with).is_some() {
        return Err(invalid("a road joins a road, and a spline a spline".into()));
    }
    if p.main_road == with {
        return Err(invalid(
            "the main road cannot be joined onto another".into(),
        ));
    }
    let end = a[a.len() - 1].pos;
    let near = |q: DVec3| q.distance(end);
    // Nearest ends meet: turn either round as needed.
    let (b_first, b_last) = (b[0].pos, b[b.len() - 1].pos);
    let a_first = a[0].pos;
    let options = [
        (near(b_first), false, false),
        (near(b_last), false, true),
        (a_first.distance(b_first), true, false),
        (a_first.distance(b_last), true, true),
    ];
    let (_, turn_a, turn_b) = options
        .into_iter()
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .expect("four ways");
    if turn_a {
        reverse(p, name)?;
    }
    if turn_b {
        reverse(p, with)?;
    }
    let (a, _) = line(p, name)?;
    let (b, _) = line(p, with)?;
    let shared = a[a.len() - 1].pos.distance(b[0].pos) < SHARED;
    // Node 0 of `with` becomes node `offset` of the joined line.
    let offset = if shared { a.len() - 1 } else { a.len() };
    let mut nodes = a.clone();
    if shared {
        // The shared node takes the handles leading in from `name` and out along
        // `with`.
        let (incoming, _) = crate::curve::handles(&a, false, a.len() - 1);
        let (_, outgoing) = crate::curve::handles(&b, false, 0);
        let last = nodes.last_mut().expect("nodes");
        if !(a[a.len() - 1].handles.is_auto() && b[0].handles.is_auto()) {
            last.handles = crate::project::NodeHandles::Free { incoming, outgoing };
        }
        nodes.extend_from_slice(&b[1..]);
    } else {
        nodes.extend_from_slice(&b);
    }
    if !road_a {
        let sp = p
            .splines
            .iter_mut()
            .find(|s| s.name == name)
            .expect("a spline");
        sp.nodes = nodes;
        p.splines.retain(|s| s.name != with);
        return Ok(());
    }
    let ra = p.road(name).expect("a road").clone();
    let rb = p.road(with).expect("a road").clone();
    let (la, lb) = (ra.period(), rb.period());
    let off = offset as f64;
    let mut out = Road {
        nodes,
        ..ra.clone()
    };
    // Profiles: `name`'s, then `with`'s moved on.
    let join_curve = |x: &StationCurve, y: &StationCurve| {
        let mut keys = x.keys.clone();
        keys.extend(y.keys.iter().map(|k| Key { u: k.u + off, ..*k }));
        keys.sort_by(|a, b| a.u.total_cmp(&b.u));
        keys.dedup_by(|a, b| (a.u - b.u).abs() < 1e-9);
        StationCurve { keys }
    };
    // A profile with one key is the same all along: give it its stretch's ends.
    let spread = |c: &StationCurve, length: f64| {
        if c.keys.len() == 1 {
            let v = c.keys[0].value;
            StationCurve {
                keys: vec![Key::new(0.0, v), Key::new(length, v)],
            }
        } else {
            c.clone()
        }
    };
    out.width_left = join_curve(&spread(&ra.width_left, la), &spread(&rb.width_left, lb));
    out.width_right = join_curve(&spread(&ra.width_right, la), &spread(&rb.width_right, lb));
    out.bank = join_curve(&spread(&ra.bank, la), &spread(&rb.bank, lb));
    // Parts by name: run on across the join where both have them.
    let total = off + lb;
    let everywhere = |r: &[Range], length| {
        r.is_empty()
            || r == [Range {
                from: 0.0,
                to: length,
            }]
    };
    let joined = |ra: &[Range], rb: &[Range]| {
        if everywhere(ra, la) && everywhere(rb, lb) {
            return vec![];
        }
        let mut all = moved_ranges(ra, 0.0, la);
        all.extend(moved_ranges(rb, off, lb));
        if !shared {
            // The joining segment carries what runs on across it.
            let reach_end =
                |r: &[Range], l: f64| r.is_empty() || r.iter().any(|g| g.to >= l - 1e-6);
            let reach_start = |r: &[Range]| r.is_empty() || r.iter().any(|g| g.from <= 1e-6);
            if reach_end(ra, la) && reach_start(rb) {
                all.push(Range { from: la, to: off });
            }
        }
        let m = merge_ranges(all);
        if m == [Range {
            from: 0.0,
            to: total,
        }] {
            vec![]
        } else {
            m
        }
    };
    let only_b = |r: &[Range]| moved_ranges(r, off, lb);
    for side in [Side::Left, Side::Right] {
        for s in rb.strips(side) {
            let mut s = s.clone();
            if let Some(c) = &mut s.corner {
                c.apex += off;
            }
            s.keys.iter_mut().for_each(|k| k.u += off);
            match out.strips(side).iter().position(|t| t.name == s.name) {
                Some(i) => {
                    let t = &mut out.strips_mut(side)[i];
                    t.ranges = joined(&t.ranges, &s.ranges);
                    t.keys.extend(s.keys);
                }
                None => {
                    s.ranges = only_b(&s.ranges);
                    out.strips_mut(side).push(s);
                }
            }
        }
        // Those only `name` had keep to its stretch.
        for t in out.strips_mut(side) {
            if !rb.strips(side).iter().any(|s| s.name == t.name) && t.ranges.is_empty() {
                t.ranges = vec![Range { from: 0.0, to: la }];
            }
        }
    }
    for l in &rb.lines {
        let mut l = l.clone();
        match out.lines.iter().position(|t| t.name == l.name) {
            Some(i) => out.lines[i].ranges = joined(&out.lines[i].ranges, &l.ranges),
            None => {
                l.ranges = only_b(&l.ranges);
                out.lines.push(l);
            }
        }
    }
    for t in &mut out.lines {
        if !rb.lines.iter().any(|l| l.name == t.name) && t.ranges.is_empty() {
            t.ranges = vec![Range { from: 0.0, to: la }];
        }
    }
    for bar in &rb.barriers {
        let mut bar = bar.clone();
        if let Some(c) = &mut bar.corner {
            c.apex += off;
        }
        match out
            .barriers
            .iter()
            .position(|t| t.name == bar.name && t.side == bar.side)
        {
            Some(i) => out.barriers[i].ranges = joined(&out.barriers[i].ranges, &bar.ranges),
            None => {
                bar.ranges = only_b(&bar.ranges);
                if out.barriers.iter().any(|t| t.name == bar.name) {
                    bar.name = format!("{} ({with})", bar.name);
                }
                out.barriers.push(bar);
            }
        }
    }
    for t in &mut out.barriers {
        if !rb.barriers.iter().any(|b| b.name == t.name) && t.ranges.is_empty() {
            t.ranges = vec![Range { from: 0.0, to: la }];
        }
    }
    for w in &rb.rows {
        let mut w = w.clone();
        w.at.iter_mut().for_each(|u| *u += off);
        match out.rows.iter().position(|t| t.name == w.name) {
            Some(i) if w.at.is_empty() && out.rows[i].at.is_empty() => {
                out.rows[i].ranges = joined(&out.rows[i].ranges, &w.ranges);
            }
            Some(i) if !w.at.is_empty() && !out.rows[i].at.is_empty() => {
                out.rows[i].at.extend(w.at);
            }
            found => {
                if w.at.is_empty() {
                    w.ranges = only_b(&w.ranges);
                }
                if found.is_some() {
                    w.name = format!("{} ({with})", w.name);
                }
                out.rows.push(w);
            }
        }
    }
    for t in &mut out.rows {
        if !rb.rows.iter().any(|w| w.name == t.name) && t.ranges.is_empty() && t.at.is_empty() {
            t.ranges = vec![Range { from: 0.0, to: la }];
        }
    }
    for m in &rb.marks {
        let mut m = m.clone();
        m.at += off;
        if out.marks.iter().any(|t| t.name == m.name) {
            m.name = format!("{} ({with})", m.name);
        }
        out.marks.push(m);
    }
    let i = p.road_index(name).expect("a road");
    p.roads[i] = out;
    p.roads.retain(|r| r.name != with);
    if let Some(pit) = &mut p.markers.pit
        && pit.road == with
    {
        pit.road = name.to_string();
        pit.boxes.iter_mut().for_each(|u| *u += off);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{Op, apply_all};

    fn project() -> Project {
        let mut p = Project::new("t");
        apply_all(
            &mut p,
            &[Op::AddRoad {
                name: "a".into(),
                closed: false,
                nodes: (0..5)
                    .map(|i| DVec3::new(i as f64 * 50.0, -100.0, 0.0))
                    .collect(),
                like: Some("circuit".into()),
            }],
        )
        .unwrap();
        p
    }

    #[test]
    fn turning_a_road_round_swaps_its_sides_and_keeps_it_in_place() {
        let mut p = project();
        apply_all(
            &mut p,
            &[
                Op::SetKey {
                    road: "circuit".into(),
                    curve: crate::ops::Curve::Bank,
                    u: 2.5,
                    value: 0.1,
                },
                Op::SetKey {
                    road: "circuit".into(),
                    curve: crate::ops::Curve::WidthLeft,
                    u: 3.0,
                    value: 8.0,
                },
            ],
        )
        .unwrap();
        let before = p.roads[0].clone();
        let point = |r: &Road, u: f64| crate::curve::point(r, u);
        apply_all(
            &mut p,
            &[Op::ReverseLine {
                line: "circuit".into(),
            }],
        )
        .unwrap();
        let r = &p.roads[0];
        let n = r.period();
        for u in [0.0, 1.3, 2.5, 7.9] {
            let v = (n - u).rem_euclid(n);
            assert!(point(&before, u).distance(point(r, v)) < 1e-9, "u {u}");
        }
        let at = |c: &StationCurve, u: f64| c.eval(u, n, true);
        assert!(
            (at(&r.bank, n - 2.5) + 0.1).abs() < 1e-9,
            "banking changes sign"
        );
        assert!(
            (at(&r.width_right, n - 3.0) - 8.0).abs() < 1e-9,
            "left is right"
        );
        assert_eq!(r.left[0].name, before.right[0].name);
        let (k0, k1) = (before.left[0].ranges[0], r.right[0].ranges[0]);
        assert!((k1.from - (n - k0.to)).abs() < 1e-9 && (k1.to - (n - k0.from)).abs() < 1e-9);
        // Twice round is as it was.
        apply_all(
            &mut p,
            &[Op::ReverseLine {
                line: "circuit".into(),
            }],
        )
        .unwrap();
        assert_eq!(p.roads[0].nodes, before.nodes);
    }

    #[test]
    fn a_road_cut_in_two_joins_back_as_it_was() {
        let mut p = project();
        apply_all(
            &mut p,
            &[Op::SetKey {
                road: "a".into(),
                curve: crate::ops::Curve::Width,
                u: 3.0,
                value: 9.0,
            }],
        )
        .unwrap();
        let before = p.road("a").unwrap().clone();
        apply_all(
            &mut p,
            &[Op::SplitLine {
                line: "a".into(),
                at: 2,
                to: "b".into(),
            }],
        )
        .unwrap();
        let (a, b) = (p.road("a").unwrap(), p.road("b").unwrap());
        assert_eq!((a.nodes.len(), b.nodes.len()), (3, 3));
        assert!((b.width_left.eval(1.0, 2.0, false) - 9.0).abs() < 1e-9);
        apply_all(
            &mut p,
            &[Op::JoinLines {
                line: "a".into(),
                with: "b".into(),
            }],
        )
        .unwrap();
        let a = p.road("a").unwrap();
        assert!(p.road("b").is_none());
        assert_eq!(a.nodes, before.nodes);
        for u in [0.0, 1.5, 3.0, 4.0] {
            let w = |r: &Road| r.width_left.eval(u, 4.0, false);
            assert!((w(a) - w(&before)).abs() < 1e-9, "u {u}");
        }
        // The grass ran along both halves, and runs along the whole again.
        assert!(
            a.left
                .iter()
                .any(|s| s.name == "grass" && s.ranges.is_empty())
        );
    }

    #[test]
    fn a_loop_cut_opens_at_its_node_and_ends_meeting_join() {
        let mut p = project();
        apply_all(
            &mut p,
            &[Op::AddRoad {
                name: "ring".into(),
                closed: true,
                nodes: vec![
                    DVec3::new(0.0, 400.0, 0.0),
                    DVec3::new(100.0, 400.0, 0.0),
                    DVec3::new(100.0, 500.0, 0.0),
                    DVec3::new(0.0, 500.0, 0.0),
                ],
                like: None,
            }],
        )
        .unwrap();
        apply_all(
            &mut p,
            &[Op::SplitLine {
                line: "ring".into(),
                at: 2,
                to: String::new(),
            }],
        )
        .unwrap();
        let r = p.road("ring").unwrap();
        assert!(!r.closed);
        assert_eq!(r.nodes.len(), 5);
        assert_eq!(r.nodes[0].pos, r.nodes[4].pos);
        assert_eq!(r.nodes[0].pos, DVec3::new(100.0, 500.0, 0.0));
        // The main road stays a loop.
        let e = apply_all(
            &mut p,
            &[Op::SplitLine {
                line: "circuit".into(),
                at: 2,
                to: "x".into(),
            }],
        );
        assert!(e.is_err());
        // Joining "a" onto a line whose far end meets it turns that one round.
        apply_all(
            &mut p,
            &[Op::AddRoad {
                name: "c".into(),
                closed: false,
                nodes: vec![
                    DVec3::new(300.0, -100.0, 0.0),
                    DVec3::new(200.0, -100.0, 0.0),
                ],
                like: None,
            }],
        )
        .unwrap();
        apply_all(
            &mut p,
            &[Op::JoinLines {
                line: "a".into(),
                with: "c".into(),
            }],
        )
        .unwrap();
        let a = p.road("a").unwrap();
        assert_eq!(a.nodes.len(), 6, "shares the node where they meet");
        assert_eq!(a.nodes[5].pos, DVec3::new(300.0, -100.0, 0.0));
    }
}
