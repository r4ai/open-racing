//! Laying kerbs, run-off and walls along the stretch of road the selected nodes span, on
//! the inside or the outside of the curve they make there, or on either side: a whole
//! curve of several nodes kerbed in one go.

use bevy_egui::egui;
use open_racing_sim::Surface;
use open_racing_track_project::curve::Sampled;
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{Range, Road, Side};

use crate::commands::Ctx;
use crate::preview::Built;
use crate::state::Editor;

/// Which side of the road to lay on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaySide {
    /// The side the road turns towards over the stretch.
    #[default]
    Inside,
    Outside,
    Left,
    Right,
    Both,
}

impl LaySide {
    pub const ALL: [LaySide; 5] = [
        LaySide::Inside,
        LaySide::Outside,
        LaySide::Left,
        LaySide::Right,
        LaySide::Both,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LaySide::Inside => "Inside",
            LaySide::Outside => "Outside",
            LaySide::Left => "Left",
            LaySide::Right => "Right",
            LaySide::Both => "Both",
        }
    }

    /// The sides of the road it means over a stretch turning towards `turn`.
    fn sides(self, turn: Side) -> Vec<Side> {
        match self {
            LaySide::Inside => vec![turn],
            LaySide::Outside => vec![turn.other()],
            LaySide::Left => vec![Side::Left],
            LaySide::Right => vec![Side::Right],
            LaySide::Both => vec![Side::Left, Side::Right],
        }
    }
}

/// What to lay: a strip or a wall of the type called so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lay {
    Strip(String),
    Wall(String),
}

/// The stretch of a line of `count` nodes that `selected` span: from the first to the
/// last, on a loop the way round that leaves out the longest gap between them. One node
/// gives half a segment either side of it; all of a loop's, none (everywhere).
pub fn span(selected: &[usize], count: usize, closed: bool) -> Option<Vec<Range>> {
    let mut nodes: Vec<usize> = selected.iter().copied().filter(|&n| n < count).collect();
    nodes.sort_unstable();
    nodes.dedup();
    let (&first, &last) = (nodes.first()?, nodes.last()?);
    let period = if closed {
        count
    } else {
        count.saturating_sub(1)
    } as f64;
    if closed && nodes.len() == count {
        return Some(vec![]);
    }
    if nodes.len() == 1 {
        let u = first as f64;
        return Some(vec![if closed {
            Range {
                from: (u - 0.5).rem_euclid(period),
                to: (u + 0.5).rem_euclid(period),
            }
        } else {
            Range {
                from: (u - 0.5).max(0.0),
                to: (u + 0.5).min(period),
            }
        }]);
    }
    if !closed {
        return Some(vec![Range {
            from: first as f64,
            to: last as f64,
        }]);
    }
    // The longest gap, from one selected node round to the next, is left out.
    let gap = |i: usize| (nodes[(i + 1) % nodes.len()] + count - nodes[i]) % count;
    let widest = (0..nodes.len()).max_by_key(|&i| gap(i)).unwrap_or(0);
    Some(vec![Range {
        from: nodes[(widest + 1) % nodes.len()] as f64,
        to: nodes[widest] as f64,
    }])
}

/// The side a road turns towards over a stretch: left when it turns anticlockwise
/// overall.
fn turning(smp: &Sampled, ranges: &[Range]) -> Side {
    let Some(rg) = ranges.first() else {
        return Side::Left;
    };
    let (a, mut b) = (smp.s_at(rg.from), smp.s_at(rg.to));
    if b < a && smp.closed {
        b += smp.length;
    }
    let steps = ((b - a) / 2.0).ceil().max(1.0) as usize;
    let heading = |s: f64| {
        let t = smp.frame_at(s.rem_euclid(smp.length.max(1e-9))).tangent;
        t.y.atan2(t.x)
    };
    let turn: f64 = (0..steps)
        .map(|k| {
            let (s0, s1) = (
                a + (b - a) * k as f64 / steps as f64,
                a + (b - a) * (k + 1) as f64 / steps as f64,
            );
            let d = heading(s1) - heading(s0);
            (d + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
        })
        .sum();
    if turn >= 0.0 { Side::Left } else { Side::Right }
}

/// How far out a side's strips reach in the middle of a stretch, m from the road's edge.
fn strips_reach(road: &Road, smp: &Sampled, side: Side, ranges: &[Range]) -> f64 {
    let s = ranges.first().map_or(0.0, |rg| {
        let (a, mut b) = (smp.s_at(rg.from), smp.s_at(rg.to));
        if b < a && smp.closed {
            b += smp.length;
        }
        (0.5 * (a + b)).rem_euclid(smp.length.max(1e-9))
    });
    road.strips(side)
        .iter()
        .map(|st| smp.strip_shape(st, s).0 * smp.presence(&st.ranges, st.fade, s))
        .sum()
}

/// Lays `what` along the stretch of the selected road its selected nodes span, on
/// `side`: a strip of a kerb type against the road, any other outermost, a wall just
/// beyond the strips there. Selects nothing new; the status says what was laid.
pub fn lay_along(editor: &mut Editor, built: &Built, what: &Lay, side: LaySide) -> bool {
    let Some(r) = editor.selection.road() else {
        return false;
    };
    let (Some(road), Some(smp)) = (editor.project.roads.get(r), built.roads.get(r)) else {
        return false;
    };
    let Some(ranges) = span(&editor.selection.nodes, road.nodes.len(), road.closed) else {
        editor.status = "select the nodes of the stretch to lay it along".into();
        return false;
    };
    let p = &editor.project;
    let sides = side.sides(turning(smp, &ranges));
    let mut ops = Vec::new();
    let mut names = Vec::new();
    for &s in &sides {
        match what {
            Lay::Strip(style) => {
                let Some(t) = p.strip_style(style) else {
                    return false;
                };
                let name = crate::presets::free_name(style, |n| {
                    road.strips(s).iter().any(|x| x.name == n)
                });
                let kerb = p
                    .surface_index(&t.surface)
                    .is_some_and(|k| p.surfaces[k].props.kind == Surface::Kerb);
                names.push(format!("{name} ({s:?})"));
                ops.push(Op::PutStrip {
                    road: road.name.clone(),
                    side: s,
                    strip: t.strip(&name, ranges.clone()),
                    at: kerb.then_some(0),
                });
            }
            Lay::Wall(style) => {
                let Some(t) = p.wall_style(style) else {
                    return false;
                };
                let name =
                    crate::presets::free_name(style, |n| road.barriers.iter().any(|x| x.name == n));
                let offset = (strips_reach(road, smp, s, &ranges) + 0.5).ceil();
                names.push(format!("{name} ({s:?})"));
                ops.push(Op::PutBarrier {
                    road: road.name.clone(),
                    barrier: t.barrier(&name, s, offset, ranges.clone()),
                });
            }
        }
    }
    let along = match ranges.first() {
        Some(rg) => format!("from u {:.1} to {:.1}", rg.from, rg.to),
        None => "all round".into(),
    };
    if editor.apply(ops, None) {
        editor.status = format!(
            "laid {} {along}: drag its ends, nodes and edge in the view",
            names.join(", ")
        );
        true
    } else {
        false
    }
}

/// Whether there is a stretch to lay along: a road's nodes selected in edit mode.
pub fn can_lay(c: &Ctx) -> bool {
    c.tool.edit && c.editor.selection.road().is_some() && !c.editor.selection.nodes.is_empty()
}

/// The entries that lay kerbs, strips and walls along the selected nodes, with the side
/// to lay them on; true once one is used.
pub fn menu(ui: &mut egui::Ui, c: &mut Ctx) -> bool {
    ui.set_min_width(190.0);
    if !can_lay(c) {
        ui.weak("Select a road's nodes (Tab) to lay along them");
        return false;
    }
    ui.horizontal(|ui| {
        for s in LaySide::ALL {
            ui.selectable_value(&mut c.shell.lay_side, s, s.label())
                .on_hover_text(match s {
                    LaySide::Inside => "The side the road turns towards between the nodes",
                    LaySide::Outside => "The side the road turns away from between the nodes",
                    _ => "",
                });
        }
    });
    ui.separator();
    let mut chosen = None;
    ui.weak("Kerbs & run-off");
    for s in &c.editor.project.strip_styles {
        if ui.button(&s.name).clicked() {
            chosen = Some(Lay::Strip(s.name.clone()));
        }
    }
    ui.weak("Walls & fences");
    for w in &c.editor.project.wall_styles {
        if ui.button(&w.name).clicked() {
            chosen = Some(Lay::Wall(w.name.clone()));
        }
    }
    match chosen {
        Some(what) => {
            lay_along(c.editor, c.built, &what, c.shell.lay_side);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_nodes_span_the_stretch_between_them() {
        let r = |from, to| Range { from, to };
        assert_eq!(span(&[4, 2, 3], 10, false), Some(vec![r(2.0, 4.0)]));
        // Round a loop, leaving out the longest gap: 8, 9, 0, 1.
        assert_eq!(span(&[0, 9, 1, 8], 10, true), Some(vec![r(8.0, 1.0)]));
        assert_eq!(span(&[3], 10, true), Some(vec![r(2.5, 3.5)]));
        assert_eq!(span(&[0], 10, true), Some(vec![r(9.5, 0.5)]));
        assert_eq!(span(&[0], 5, false), Some(vec![r(0.0, 0.5)]));
        assert_eq!(span(&(0..6).collect::<Vec<_>>(), 6, true), Some(vec![]));
        assert_eq!(span(&[], 6, true), None);
    }

    #[test]
    fn kerbs_go_inside_the_curve_and_walls_beyond_the_strips() {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-lay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        let scene = open_racing_track_project::bake::build(&editor.project);
        let built = Built {
            roads: scene.roads.into_iter().map(|b| b.sampled).collect(),
            ..Default::default()
        };
        // Nodes 2 to 4 of the oval, round its first bend, which turns left.
        editor.selection.item = Some(crate::state::Item::Road(0));
        editor.selection.nodes = vec![2, 3, 4];
        let before = editor.project.roads[0].left.len();
        assert!(lay_along(
            &mut editor,
            &built,
            &Lay::Strip("raised kerb".into()),
            LaySide::Inside
        ));
        let road = &editor.project.roads[0];
        assert_eq!(road.left.len(), before + 1);
        let kerb = &road.left[0];
        assert_eq!(kerb.name, "raised kerb");
        assert_eq!(kerb.ranges, vec![Range { from: 2.0, to: 4.0 }]);
        assert!(lay_along(
            &mut editor,
            &built,
            &Lay::Wall("tyre wall".into()),
            LaySide::Outside
        ));
        let wall = editor.project.roads[0].barriers.last().unwrap();
        assert_eq!(wall.side, Side::Right);
        // Beyond the kerb and the grass on that side.
        assert!(wall.offset > 1.0, "{}", wall.offset);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
