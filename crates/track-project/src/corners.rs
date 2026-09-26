//! A road's corners, found from its curvature, numbered from the start line as
//! circuits number their turns, and what is laid round each: kerbs on the outside where
//! cars turn in and run out and on the inside at the apex, gravel or run-off and a wall
//! on the outside. Each such strip or barrier keeps an anchor on its corner, so that it
//! stays with the corner however the corners are renumbered, and is refitted round it
//! whenever the road changes.

use crate::curve::Sampled;
use crate::ops::Op;
pub use crate::project::CornerPart;
use crate::project::{Anchor, Project, Range, Road, Side, Strip};

/// A corner of a road. Places are distances along the road, m, in order: on a loop a
/// corner across the start runs on past the loop's length.
#[derive(Clone, Debug, PartialEq)]
pub struct Corner {
    /// 1 for the first corner after the start line.
    pub number: usize,
    /// The way it turns: left is anticlockwise seen from above.
    pub dir: Side,
    pub entry: f64,
    /// Where it turns tightest.
    pub apex: f64,
    pub exit: f64,
    /// Tightest radius, m.
    pub radius: f64,
    /// How far it turns, radians.
    pub angle: f64,
}

impl Corner {
    /// The side on the inside of the turn.
    pub fn inside(&self) -> Side {
        self.dir
    }

    pub fn outside(&self) -> Side {
        match self.dir {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }

    /// The corner's length along the road, m, on a road `length` long.
    pub fn length(&self, length: f64) -> f64 {
        (self.exit - self.entry).rem_euclid(length.max(1e-9))
    }

    /// Whether `s` lies within the corner, or `margin` metres either side.
    pub fn covers(&self, s: f64, margin: f64, length: f64, closed: bool) -> bool {
        let (a, b) = (self.entry - margin, self.exit + margin);
        if closed {
            (s - a).rem_euclid(length) <= (b - a).rem_euclid(length)
        } else {
            (a..=b).contains(&s)
        }
    }
}

/// Tighter than this radius, m, a road is turning.
const TURNING: f64 = 350.0;
/// Least turn that makes a corner.
const LEAST_ANGLE: f64 = 0.35;
/// Length curvature is averaged over, m, so that a wobble is not a corner.
const WINDOW: f64 = 20.0;

/// The corners of a sampled road, numbered from `start` (m along it).
pub fn find(smp: &Sampled, start: f64) -> Vec<Corner> {
    let f = &smp.frames;
    let n = f.len();
    if n < 5 {
        return vec![];
    }
    let idx = |k: isize| -> Option<usize> {
        if smp.closed {
            Some(k.rem_euclid(n as isize) as usize)
        } else {
            (0..n as isize).contains(&k).then_some(k as usize)
        }
    };
    let gap = |a: usize, b: usize| {
        let d = f[b].s - f[a].s;
        if smp.closed {
            d.rem_euclid(smp.length)
        } else {
            d
        }
    };
    // Signed curvature at each frame, positive to the left, then averaged.
    let raw: Vec<f64> = (0..n as isize)
        .map(|k| {
            let (Some(a), Some(b)) = (idx(k - 1).or(idx(k)), idx(k + 1).or(idx(k))) else {
                return 0.0;
            };
            let ds = gap(a, b);
            if ds < 1e-9 {
                return 0.0;
            }
            let (ta, tb) = (f[a].tangent.truncate(), f[b].tangent.truncate());
            ta.normalize_or_zero().angle_to(tb.normalize_or_zero()) / ds
        })
        .collect();
    let spacing = smp.length / n as f64;
    let reach = ((0.5 * WINDOW / spacing.max(1e-9)).round() as isize).max(1);
    let kappa: Vec<f64> = (0..n as isize)
        .map(|k| {
            let ks: Vec<usize> = (-reach..=reach).filter_map(|d| idx(k + d)).collect();
            ks.iter().map(|&j| raw[j]).sum::<f64>() / ks.len() as f64
        })
        .collect();
    let turning = |k: usize| kappa[k].abs() > 1.0 / TURNING;
    // Runs of frames turning the same way; on a loop, begin where it is not turning.
    let first = if smp.closed {
        match (0..n).find(|&k| !turning(k)) {
            Some(k) => k,
            None => return vec![],
        }
    } else {
        0
    };
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<usize> = None;
    for step in 0..=n {
        let k = (first + step) % n;
        let end = step == n || (!smp.closed && k < first);
        let same = open.is_some_and(|o| turning(k) && kappa[k].signum() == kappa[o].signum());
        if let Some(o) = open
            && (end || !same)
        {
            runs.push((o, (k + n - 1) % n));
            open = None;
        }
        if end {
            break;
        }
        if open.is_none() && turning(k) {
            open = Some(k);
        }
    }
    let mut corners: Vec<Corner> = runs
        .into_iter()
        .filter_map(|(a, b)| {
            let len = (b + n - a) % n + 1;
            let ks = (0..len).map(|i| (a + i) % n);
            let apex = ks
                .clone()
                .max_by(|&x, &y| kappa[x].abs().total_cmp(&kappa[y].abs()))?;
            let angle: f64 = ks.map(|k| raw[k] * spacing).sum::<f64>().abs();
            // Across the start of a loop, the later places run on past its length.
            let (entry, mut apex, mut exit) = (f[a].s, f[apex].s, f[b].s);
            if smp.closed {
                if apex < entry {
                    apex += smp.length;
                }
                if exit < apex {
                    exit += smp.length;
                }
            }
            let tightest = kappa[(0..len)
                .map(|i| (a + i) % n)
                .max_by(|&x, &y| kappa[x].abs().total_cmp(&kappa[y].abs()))?];
            (angle >= LEAST_ANGLE).then(|| Corner {
                number: 0,
                dir: if tightest > 0.0 {
                    Side::Left
                } else {
                    Side::Right
                },
                entry,
                apex,
                exit,
                radius: 1.0 / tightest.abs(),
                angle,
            })
        })
        .collect();
    let from_start = |c: &Corner| {
        if smp.closed {
            (c.entry - start).rem_euclid(smp.length)
        } else {
            c.entry
        }
    };
    corners.sort_by(|a, b| from_start(a).total_cmp(&from_start(b)));
    for (i, c) in corners.iter_mut().enumerate() {
        c.number = i + 1;
    }
    corners
}

/// Where a part lies round a corner: its side, and where it starts and ends, m along
/// the road.
pub fn place(part: CornerPart, c: &Corner) -> (Side, f64, f64) {
    let before = c.apex - c.entry;
    let after = c.exit - c.apex;
    match part {
        CornerPart::Entry => (c.outside(), c.entry - 25.0, c.entry + 0.4 * before),
        CornerPart::Apex => (
            c.inside(),
            c.apex - 0.5 * before - 5.0,
            c.apex + 0.5 * after + 5.0,
        ),
        CornerPart::Exit => (c.outside(), c.apex + 0.5 * after, c.exit + 30.0),
        CornerPart::Outside => (c.outside(), c.entry - 10.0, c.exit + 30.0),
    }
}

/// How far outside a corner's entry and exit, m, its parts still belong to it.
const REACH: f64 = 15.0;

/// The corner a part laid at `anchor` belongs to: the one round its apex, the nearest if
/// several are.
pub fn owner<'a>(smp: &Sampled, corners: &'a [Corner], anchor: &Anchor) -> Option<&'a Corner> {
    let s = smp.s_at(anchor.apex);
    let gap = |c: &Corner| {
        let d = (c.apex - s).abs();
        if smp.closed {
            let d = d.rem_euclid(smp.length);
            d.min(smp.length - d)
        } else {
            d
        }
    };
    corners
        .iter()
        .filter(|c| c.covers(s, REACH, smp.length, smp.closed))
        .min_by(|a, b| gap(a).total_cmp(&gap(b)))
}

/// A part laid round a corner: a strip on a side, or a barrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Laid {
    Strip(Side, usize),
    Barrier(usize),
}

/// The strips and barriers of `road` laid round corner `c`, with which part each is.
pub fn parts_of(
    road: &Road,
    smp: &Sampled,
    corners: &[Corner],
    c: &Corner,
) -> Vec<(CornerPart, Laid)> {
    let mine = |a: &Option<Anchor>| {
        a.as_ref()
            .filter(|a| owner(smp, corners, a).is_some_and(|o| o.number == c.number))
            .map(|a| a.part)
    };
    let mut out = Vec::new();
    for side in [Side::Left, Side::Right] {
        for (i, s) in road.strips(side).iter().enumerate() {
            if let Some(part) = mine(&s.corner) {
                out.push((part, Laid::Strip(side, i)));
            }
        }
    }
    for (i, b) in road.barriers.iter().enumerate() {
        if let Some(part) = mine(&b.corner) {
            out.push((part, Laid::Barrier(i)));
        }
    }
    out
}

/// Where a part's stretch runs round corner `c`, with its shift, as a range of the road.
fn range(smp: &Sampled, c: &Corner, anchor: &Anchor) -> (Side, Range) {
    let (side, from, to) = place(anchor.part, c);
    let u = |s: f64| {
        if smp.closed {
            smp.u_at(s.rem_euclid(smp.length))
        } else {
            smp.u_at(s.clamp(0.0, smp.length))
        }
    };
    (
        side,
        Range {
            from: u(from + anchor.shift[0]),
            to: u(to + anchor.shift[1]),
        },
    )
}

/// The anchor of a part laid round corner `c` now.
pub fn anchor(smp: &Sampled, c: &Corner, part: CornerPart) -> Anchor {
    Anchor {
        part,
        apex: apex_u(smp, c),
        shift: [0.0; 2],
    }
}

fn apex_u(smp: &Sampled, c: &Corner) -> f64 {
    if smp.closed {
        smp.u_at(c.apex.rem_euclid(smp.length))
    } else {
        smp.u_at(c.apex)
    }
}

/// The shift that puts one end (`to`, else the start) of a part's stretch round corner
/// `c` at `s` along the road.
pub fn shift_to(smp: &Sampled, c: &Corner, anchor: &Anchor, to: bool, s: f64) -> f64 {
    let (_, from, until) = place(anchor.part, c);
    let usual = if to { until } else { from };
    if !smp.closed {
        return s - usual;
    }
    // The nearest way round.
    let d = (s - usual).rem_euclid(smp.length);
    if d > 0.5 * smp.length {
        d - smp.length
    } else {
        d
    }
}

/// The rest of a name given by corner number ("T3 entry" gives "entry"), which follows
/// the number.
fn numbered(name: &str) -> Option<&str> {
    let rest = name.strip_prefix('T')?;
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &rest[digits..];
    if rest.is_empty() {
        Some(rest)
    } else {
        rest.strip_prefix(' ')
    }
}

/// Names made unique: a repeated one gets " (2)", " (3)" and so on.
fn unique(names: &mut [String]) {
    for i in 0..names.len() {
        let base = names[i].clone();
        let mut k = 2;
        while names[..i].contains(&names[i]) {
            names[i] = format!("{base} ({k})");
            k += 1;
        }
    }
}

/// Road `index`'s corners as it now runs, numbered from the start line on the main
/// road, with the road sampled.
pub fn of_road(project: &Project, index: usize) -> (Sampled, Vec<Corner>) {
    let road = &project.roads[index];
    let smp = Sampled::new(road, road.resolution);
    let start = if road.name == project.main_road {
        smp.s_at(project.markers.start)
    } else {
        0.0
    };
    let corners = find(&smp, start);
    (smp, corners)
}

/// Puts each strip and barrier of road `index` laid round a corner back round it as the
/// road now runs, on the corner's inside or outside, and renumbers the names given by
/// corner number. Those whose corner is gone stay as they are.
pub fn fit(project: &mut Project, index: usize) {
    if !project.roads[index].has_corner_parts() {
        return;
    }
    let (smp, corners) = of_road(project, index);
    let road = &mut project.roads[index];
    settle_merged(road, &smp, &corners);
    let renamed = |name: &str, c: &Corner| match numbered(name) {
        Some("") => format!("T{}", c.number),
        Some(rest) => format!("T{} {rest}", c.number),
        None => name.to_string(),
    };
    // Strips may change sides with the corner's direction.
    let mut sides = [
        std::mem::take(&mut road.left),
        std::mem::take(&mut road.right),
    ];
    let mut moved: [Vec<Strip>; 2] = [vec![], vec![]];
    for (si, side) in [Side::Left, Side::Right].into_iter().enumerate() {
        let mut keep = Vec::new();
        for mut s in std::mem::take(&mut sides[si]) {
            if let Some(a) = s.corner
                && let Some(c) = owner(&smp, &corners, &a)
            {
                let (want, rg) = range(&smp, c, &a);
                s.ranges = vec![rg];
                s.corner = Some(Anchor {
                    apex: apex_u(&smp, c),
                    ..a
                });
                s.name = renamed(&s.name, c);
                if want != side {
                    moved[want as usize].push(s);
                    continue;
                }
            }
            keep.push(s);
        }
        sides[si] = keep;
    }
    for si in 0..2 {
        // Kerbs moving across go against the road, the rest outermost.
        for s in std::mem::take(&mut moved[si]) {
            if matches!(s.corner.map(|a| a.part), Some(CornerPart::Outside)) {
                sides[si].push(s);
            } else {
                sides[si].insert(0, s);
            }
        }
        let mut names: Vec<String> = sides[si].iter().map(|s| s.name.clone()).collect();
        unique(&mut names);
        for (s, n) in sides[si].iter_mut().zip(names) {
            s.name = n;
        }
    }
    let [left, right] = sides;
    road.left = left;
    road.right = right;
    for b in &mut road.barriers {
        if let Some(a) = b.corner
            && let Some(c) = owner(&smp, &corners, &a)
        {
            let (side, rg) = range(&smp, c, &a);
            b.side = side;
            b.ranges = vec![rg];
            b.corner = Some(Anchor {
                apex: apex_u(&smp, c),
                ..a
            });
            b.name = renamed(&b.name, c);
        }
    }
    let mut names: Vec<String> = road.barriers.iter().map(|b| b.name.clone()).collect();
    unique(&mut names);
    for (b, n) in road.barriers.iter_mut().zip(names) {
        b.name = n;
    }
}

/// Where corners ran together (the road between them was straightened), the parts each
/// had now fall to one corner twice over. The one laid nearest its apex stays; another
/// just like it (the same type and width) goes, and one made different is let go of the
/// corner, staying where it is as a part of its own.
fn settle_merged(road: &mut Road, smp: &Sampled, corners: &[Corner]) {
    // (corner, part, barrier or not) → the nearest claim's distance.
    let claim = |a: &Anchor| {
        let c = owner(smp, corners, a)?;
        let d = (smp.s_at(a.apex) - c.apex).rem_euclid(smp.length.max(1e-9));
        Some(((c.number, a.part), d.min(smp.length - d)))
    };
    let mut nearest: std::collections::HashMap<(usize, CornerPart, bool), (f64, String, f64)> =
        Default::default();
    let strips = road.left.iter().chain(&road.right);
    for s in strips {
        if let Some((key, d)) = s.corner.as_ref().and_then(claim) {
            let e =
                nearest
                    .entry((key.0, key.1, false))
                    .or_insert((f64::INFINITY, String::new(), 0.0));
            if d < e.0 {
                *e = (d, s.style.clone().unwrap_or_default(), s.width);
            }
        }
    }
    for b in &road.barriers {
        if let Some((key, d)) = b.corner.as_ref().and_then(claim) {
            let e =
                nearest
                    .entry((key.0, key.1, true))
                    .or_insert((f64::INFINITY, String::new(), 0.0));
            if d < e.0 {
                *e = (d, b.style.clone().unwrap_or_default(), b.offset);
            }
        }
    }
    // What becomes of a part: kept, gone, or let go of its corner.
    let fate = |a: &Option<Anchor>, barrier: bool, style: &Option<String>, size: f64| {
        let Some((key, d)) = a.as_ref().and_then(claim) else {
            return Some(false);
        };
        let (best, st, sz) = &nearest[&(key.0, key.1, barrier)];
        if d <= *best {
            Some(false)
        } else if style.as_deref().unwrap_or_default() == st && (size - sz).abs() < 1e-6 {
            None
        } else {
            Some(true)
        }
    };
    for side in [Side::Left, Side::Right] {
        let list = std::mem::take(road.strips_mut(side));
        *road.strips_mut(side) = list
            .into_iter()
            .filter_map(|mut s| {
                let release = fate(&s.corner, false, &s.style, s.width)?;
                if release {
                    s.corner = None;
                }
                Some(s)
            })
            .collect();
    }
    let list = std::mem::take(&mut road.barriers);
    road.barriers = list
        .into_iter()
        .filter_map(|mut b| {
            let release = fate(&b.corner, true, &b.style, b.offset)?;
            if release {
                b.corner = None;
            }
            Some(b)
        })
        .collect();
}

/// What to lay round a corner: a strip type and width for each kerb and for what is
/// outside them, and a wall type and its distance from the road's edge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kit {
    pub entry: Option<(String, f64)>,
    pub apex: Option<(String, f64)>,
    pub exit: Option<(String, f64)>,
    /// Gravel or run-off on the outside, beyond the kerbs.
    pub outside: Option<(String, f64)>,
    pub wall: Option<(String, f64)>,
}

impl Kit {
    /// Entry, apex and exit kerbs of the kerb type `style` (else the built-in "kerb",
    /// else the project's first strip type), `width` wide.
    pub fn kerbs(project: &Project, style: Option<&str>, width: f64) -> Self {
        let style = style
            .and_then(|s| project.strip_style(s))
            .or_else(|| project.strip_style("kerb"))
            .or_else(|| project.strip_styles.first())
            .map_or("kerb".to_string(), |s| s.name.clone());
        let k = Some((style, width));
        Kit {
            entry: k.clone(),
            apex: k.clone(),
            exit: k,
            ..Default::default()
        }
    }

    /// What corner `c` of `road` has.
    pub fn of(road: &Road, smp: &Sampled, corners: &[Corner], c: &Corner) -> Self {
        let mut kit = Kit::default();
        for (part, laid) in parts_of(road, smp, corners, c) {
            match laid {
                Laid::Strip(side, i) => {
                    let s = &road.strips(side)[i];
                    *kit.strip_mut(part) = Some((s.style.clone().unwrap_or_default(), s.width));
                }
                Laid::Barrier(i) => {
                    let b = &road.barriers[i];
                    kit.wall = Some((b.style.clone().unwrap_or_default(), b.offset));
                }
            }
        }
        kit
    }

    pub fn strip(&self, part: CornerPart) -> &Option<(String, f64)> {
        match part {
            CornerPart::Entry => &self.entry,
            CornerPart::Apex => &self.apex,
            CornerPart::Exit => &self.exit,
            CornerPart::Outside => &self.outside,
        }
    }

    pub fn strip_mut(&mut self, part: CornerPart) -> &mut Option<(String, f64)> {
        match part {
            CornerPart::Entry => &mut self.entry,
            CornerPart::Apex => &mut self.apex,
            CornerPart::Exit => &mut self.exit,
            CornerPart::Outside => &mut self.outside,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Kit::default()
    }
}

/// The operations that give corner `c` of road `road` what `kit` asks for: the parts it
/// has already keep their names and stretches and take the types and widths asked for,
/// missing ones are laid, and those not asked for are taken away.
pub fn kit_ops(
    project: &Project,
    road: &str,
    smp: &Sampled,
    corners: &[Corner],
    c: &Corner,
    kit: &Kit,
) -> Vec<Op> {
    let Some(r) = project.road(road) else {
        return vec![];
    };
    let has = parts_of(r, smp, corners, c);
    let mut ops = Vec::new();
    for part in CornerPart::ALL {
        let laid = has.iter().find_map(|&(p, l)| match l {
            Laid::Strip(side, i) if p == part => Some((side, i)),
            _ => None,
        });
        let want = kit
            .strip(part)
            .as_ref()
            .and_then(|(style, width)| project.strip_style(style).map(|s| (s, *width)));
        match (laid, want) {
            (Some((side, i)), Some((style, width))) => {
                let mut s = r.strips(side)[i].clone();
                style.restyle(&mut s);
                s.width = width;
                ops.push(Op::PutStrip {
                    road: road.to_string(),
                    side,
                    strip: s,
                    at: None,
                });
            }
            (None, Some((style, width))) => {
                let a = anchor(smp, c, part);
                let (side, rg) = range(smp, c, &a);
                let strips = r.strips(side);
                let name = free_name(
                    strips.iter().map(|t| t.name.as_str()),
                    &format!("T{} {}", c.number, part.label()),
                );
                let mut s = style.strip(&name, vec![rg]);
                s.width = width;
                s.corner = Some(a);
                // Kerbs against the road; what is outside them next, before verges.
                let at = match part {
                    CornerPart::Outside => {
                        strips.iter().take_while(|t| is_kerb(project, t)).count()
                    }
                    _ => 0,
                };
                ops.push(Op::PutStrip {
                    road: road.to_string(),
                    side,
                    strip: s,
                    at: Some(at),
                });
            }
            (Some((side, i)), None) => ops.push(Op::RemoveStrip {
                road: road.to_string(),
                side,
                name: r.strips(side)[i].name.clone(),
            }),
            (None, None) => {}
        }
    }
    let wall = has.iter().find_map(|&(_, l)| match l {
        Laid::Barrier(i) => Some(i),
        _ => None,
    });
    let want = kit
        .wall
        .as_ref()
        .and_then(|(style, offset)| project.wall_style(style).map(|s| (s, *offset)));
    match (wall, want) {
        (Some(i), Some((style, offset))) => {
            let mut b = r.barriers[i].clone();
            style.restyle(&mut b);
            b.offset = offset;
            ops.push(Op::PutBarrier {
                road: road.to_string(),
                barrier: b,
            });
        }
        (None, Some((style, offset))) => {
            let a = anchor(smp, c, CornerPart::Outside);
            let (side, rg) = range(smp, c, &a);
            let name = free_name(
                r.barriers.iter().map(|b| b.name.as_str()),
                &format!("T{} wall", c.number),
            );
            let mut b = style.barrier(&name, side, offset, vec![rg]);
            b.corner = Some(a);
            ops.push(Op::PutBarrier {
                road: road.to_string(),
                barrier: b,
            });
        }
        (Some(i), None) => ops.push(Op::RemoveBarrier {
            road: road.to_string(),
            name: r.barriers[i].name.clone(),
        }),
        (None, None) => {}
    }
    ops
}

/// Whether a strip is a kerb, by its surface.
fn is_kerb(project: &Project, s: &Strip) -> bool {
    project
        .surface_index(&s.surface)
        .is_some_and(|i| project.surfaces[i].props.kind == open_racing_sim::Surface::Kerb)
}

/// `name`, or with a number after it, not among `taken`.
fn free_name<'a>(taken: impl Iterator<Item = &'a str> + Clone, name: &str) -> String {
    (1..)
        .map(|k| {
            if k == 1 {
                name.to_string()
            } else {
                format!("{name} ({k})")
            }
        })
        .find(|n| !taken.clone().any(|t| t == n))
        .expect("some name is free")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::apply_all;

    fn kerbed() -> (Project, Sampled, Vec<Corner>) {
        let p = Project::new("t");
        let (smp, corners) = of_road(&p, 0);
        (p, smp, corners)
    }

    #[test]
    fn finds_and_numbers_the_oval_s_corners_and_kerbs_them() {
        let (mut p, smp, corners) = kerbed();
        let start = smp.s_at(p.markers.start);
        // A rounded rectangle driven anticlockwise: two ends, turning left.
        assert_eq!(corners.len(), 2, "{corners:?}");
        assert!(corners.iter().all(|c| c.dir == Side::Left));
        assert_eq!(corners[0].number, 1);
        assert!(corners[0].entry > start, "numbered from the start line");
        for c in &corners {
            assert!((c.angle - std::f64::consts::PI).abs() < 0.5, "{c:?}");
            assert!(c.radius > 30.0 && c.radius < 200.0, "{c:?}");
        }
        let kit = Kit::kerbs(&p, None, 1.5);
        let ops = kit_ops(&p, "circuit", &smp, &corners, &corners[0], &kit);
        apply_all(&mut p, &ops).unwrap();
        let r = &p.roads[0];
        assert!(r.left.iter().any(|s| s.name == "T1 apex"), "inside is left");
        assert!(r.right.iter().any(|s| s.name == "T1 entry"));
        assert_eq!(Kit::of(r, &smp, &corners, &corners[0]), kit);
        assert!(Kit::of(r, &smp, &corners, &corners[1]).is_empty());
        // A corner across the start: its places still run in order.
        let mut q = Project::new("t");
        q.roads[0].nodes.rotate_left(3);
        let smp = Sampled::new(&q.roads[0], 2.0);
        let across = find(&smp, 0.0);
        assert_eq!(across.len(), 2, "{across:?}");
        for c in &across {
            assert!(c.entry <= c.apex && c.apex <= c.exit, "{c:?}");
            assert!(c.length(smp.length) < 0.5 * smp.length, "{c:?}");
        }
        // Taking one away removes its strip; gravel and a wall go outside.
        let (smp, corners) = of_road(&p, 0);
        let fewer = Kit {
            entry: None,
            outside: Some(("gravel".into(), 12.0)),
            wall: Some(("tyre wall".into(), 20.0)),
            ..kit
        };
        let ops = kit_ops(&p, "circuit", &smp, &corners, &corners[0], &fewer);
        apply_all(&mut p, &ops).unwrap();
        let r = &p.roads[0];
        assert!(!r.right.iter().any(|s| s.name == "T1 entry"));
        let exit = r.right.iter().position(|s| s.name == "T1 exit").unwrap();
        let gravel = r.right.iter().position(|s| s.name == "T1 outside").unwrap();
        assert!(exit < gravel, "gravel beyond the kerb");
        assert!(
            r.barriers
                .iter()
                .any(|b| b.name == "T1 wall" && b.side == Side::Right)
        );
        assert_eq!(Kit::of(r, &smp, &corners, &corners[0]), fewer);
    }

    #[test]
    fn corners_run_together_keep_one_of_each_part() {
        let (mut p, smp, corners) = kerbed();
        let kit = Kit::kerbs(&p, None, 1.5);
        let ops = kit_ops(&p, "circuit", &smp, &corners, &corners[1], &kit);
        apply_all(&mut p, &ops).unwrap();
        // As if a second corner's kerbs had fallen to this one: a copy of the entry
        // kerb just like it, and an apex kerb made wider, both laid a little off.
        let road = &mut p.roads[0];
        for side in [Side::Left, Side::Right] {
            let extra: Vec<Strip> = road
                .strips(side)
                .iter()
                .filter(|s| s.corner.is_some_and(|a| a.part != CornerPart::Exit))
                .map(|s| {
                    let mut t = s.clone();
                    t.name = format!("{} again", s.name);
                    let a = t.corner.as_mut().unwrap();
                    a.apex += 0.05;
                    if a.part == CornerPart::Apex {
                        t.width = 3.0;
                    }
                    t
                })
                .collect();
            road.strips_mut(side).extend(extra);
        }
        apply_all(
            &mut p,
            &[Op::FitCorners {
                road: "circuit".into(),
            }],
        )
        .unwrap();
        let road = &p.roads[0];
        let all: Vec<&Strip> = road.left.iter().chain(&road.right).collect();
        let held = |part| {
            all.iter()
                .filter(|s| s.corner.is_some_and(|a| a.part == part))
                .count()
        };
        assert_eq!(
            held(CornerPart::Entry),
            1,
            "the same entry kerb twice: one goes"
        );
        assert_eq!(held(CornerPart::Apex), 1);
        let wide = all
            .iter()
            .find(|s| s.width == 3.0)
            .expect("the wider one stays");
        assert!(wide.corner.is_none(), "let go of the corner");
    }

    #[test]
    fn corner_parts_follow_their_corner_and_its_number() {
        let (mut p, smp, corners) = kerbed();
        let kit = Kit::kerbs(&p, None, 1.5);
        let ops = kit_ops(&p, "circuit", &smp, &corners, &corners[1], &kit);
        apply_all(&mut p, &ops).unwrap();
        let apex = |p: &Project| {
            let s = p.roads[0]
                .left
                .iter()
                .find(|s| s.name.ends_with("apex"))
                .unwrap();
            (s.name.clone(), s.ranges[0])
        };
        let (name, before) = apex(&p);
        assert_eq!(name, "T2 apex");
        // Moving the start line past corner 1 makes this corner the first: the kerb is
        // renumbered, and stays where it was.
        let u = corners[0].exit;
        let u = smp.u_at(u + 20.0);
        apply_all(
            &mut p,
            &[Op::SetMarkers {
                start: Some(u),
                sectors: None,
                grid: None,
            }],
        )
        .unwrap();
        let (name, after) = apex(&p);
        assert_eq!(name, "T1 apex");
        assert!(
            (after.from - before.from).abs() < 1e-6,
            "{before:?} {after:?}"
        );
        // Pushing the corner's end nodes out moves its apex kerb with it.
        let ops: Vec<Op> = (6..=9)
            .map(|i| Op::MoveNode {
                line: "circuit".into(),
                index: i,
                pos: p.roads[0].nodes[i].pos + glam::DVec3::new(-60.0, 0.0, 0.0),
            })
            .collect();
        apply_all(&mut p, &ops).unwrap();
        let (_, moved) = apex(&p);
        let (smp, corners) = of_road(&p, 0);
        let c = corners.iter().find(|c| c.number == 1).unwrap();
        let fresh = range(&smp, c, &anchor(&smp, c, CornerPart::Apex)).1;
        assert!((moved.from - fresh.from).abs() < 1e-9 && (moved.to - fresh.to).abs() < 1e-9);
        let (a, b) = (smp.s_at(moved.from), smp.s_at(moved.to));
        assert!(
            a < c.apex && c.apex < b,
            "{a}..{b} round the apex at {}",
            c.apex
        );
        assert_ne!(moved, after);
    }
}
