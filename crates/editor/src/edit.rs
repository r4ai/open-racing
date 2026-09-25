//! Edits of the selection that menus, shortcuts and the search all offer: selecting more
//! or less, handle types, subdividing, opening and closing lines, renaming, and a road's
//! width and bank at its nodes.

use glam::DVec3;
use open_racing_track_project::Project;
use open_racing_track_project::curve::handles;
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::{HandleMode, Key, Road, StationCurve};

use crate::preview::Built;
use crate::state::{Editor, Item};

/// The name of a road, spline or prop.
pub fn item_name(project: &Project, item: Item) -> Option<&str> {
    match item {
        Item::Road(r) => project.roads.get(r).map(|r| r.name.as_str()),
        Item::Spline(s) => project.splines.get(s).map(|s| s.name.as_str()),
        Item::Prop(p) => project.props.get(p).map(|p| p.name.as_str()),
    }
}

/// What kind of thing an item is, for labels.
pub fn item_kind(item: Item) -> &'static str {
    match item {
        Item::Road(_) => "Road",
        Item::Spline(_) => "Spline",
        Item::Prop(_) => "Prop",
    }
}

/// The selected line's node count and whether it is closed.
fn line_shape(editor: &Editor) -> Option<(usize, bool)> {
    editor
        .line()
        .map(|(_, nodes, closed)| (nodes.len(), closed))
}

/// The nodes next to `i`.
fn neighbours(i: usize, n: usize, closed: bool) -> impl Iterator<Item = usize> {
    let prev = if i > 0 {
        Some(i - 1)
    } else {
        (closed && n > 1).then(|| n - 1)
    };
    let next = if i + 1 < n {
        Some(i + 1)
    } else {
        (closed && n > 1).then_some(0)
    };
    prev.into_iter().chain(next)
}

/// Selects the nodes not selected (Ctrl + I).
pub fn select_invert(editor: &mut Editor) {
    let Some((n, _)) = line_shape(editor) else {
        return;
    };
    let sel = &mut editor.selection;
    sel.nodes = (0..n).filter(|i| !sel.nodes.contains(i)).collect();
}

/// Adds the neighbours of the selected nodes (Ctrl + numpad +).
pub fn select_more(editor: &mut Editor) {
    let Some((n, closed)) = line_shape(editor) else {
        return;
    };
    let sel = &mut editor.selection;
    let grown: Vec<usize> = sel
        .nodes
        .iter()
        .flat_map(|&i| neighbours(i, n, closed))
        .collect();
    let active = sel.node();
    for i in grown {
        if !sel.nodes.contains(&i) {
            sel.nodes.insert(0, i);
        }
    }
    // The active node stays the active one.
    if let Some(a) = active {
        sel.nodes.retain(|&i| i != a);
        sel.nodes.push(a);
    }
}

/// Drops the selected nodes at the edges of the selection (Ctrl + numpad -).
pub fn select_less(editor: &mut Editor) {
    let Some((n, closed)) = line_shape(editor) else {
        return;
    };
    let sel = &mut editor.selection;
    let before = sel.nodes.clone();
    sel.nodes
        .retain(|&i| neighbours(i, n, closed).all(|j| before.contains(&j)));
}

/// Sets the handles of the selected nodes, keeping their present shape.
pub fn set_handles(editor: &mut Editor, mode: HandleMode) {
    let Some((name, nodes, closed)) = editor.line() else {
        return;
    };
    let picked: Vec<usize> = if editor.selection.nodes.is_empty() {
        (0..nodes.len()).collect()
    } else {
        editor.selection.nodes.clone()
    };
    let ops: Vec<Op> = picked
        .into_iter()
        .filter(|&i| i < nodes.len())
        .map(|index| {
            let (incoming, mut outgoing) = handles(nodes, closed, index);
            if !closed && index + 1 == nodes.len() && outgoing.length_squared() < 1e-12 {
                outgoing = -incoming;
            }
            let (incoming, outgoing) = match mode {
                HandleMode::Auto => (DVec3::ZERO, DVec3::ZERO),
                HandleMode::Aligned => {
                    (-outgoing.normalize_or_zero() * incoming.length(), outgoing)
                }
                HandleMode::Free => (incoming, outgoing),
            };
            Op::SetNodeHandles {
                line: name.to_string(),
                index,
                mode,
                incoming,
                outgoing,
            }
        })
        .collect();
    if !ops.is_empty() {
        editor.apply(ops, None);
    }
}

/// Adds a node halfway along each segment between selected nodes, or along every
/// segment with none selected, on the curve as last built.
pub fn subdivide(editor: &mut Editor, built: &Built) {
    let (Some(item), Some((name, nodes, closed))) = (editor.selection.item, editor.line()) else {
        return;
    };
    let n = nodes.len();
    let sel = editor.selection.nodes.clone();
    let segs = if closed { n } else { n.saturating_sub(1) };
    let picked: Vec<usize> = (0..segs)
        .filter(|&i| sel.is_empty() || (sel.contains(&i) && sel.contains(&((i + 1) % n))))
        .collect();
    if picked.is_empty() {
        editor.status = "select two neighbouring nodes to subdivide between".into();
        return;
    }
    let sampled = match item {
        Item::Road(r) => built.roads.get(r),
        Item::Spline(s) => built.splines.get(s),
        Item::Prop(_) => None,
    };
    let mid = |i: usize| {
        sampled
            .filter(|s| !s.frames.is_empty())
            .map(|s| s.frame_at(s.s_at(i as f64 + 0.5)).pos)
            .unwrap_or_else(|| 0.5 * (nodes[i].pos + nodes[(i + 1) % n].pos))
    };
    // From the last segment back, so the earlier indices hold.
    let ops: Vec<Op> = picked
        .iter()
        .rev()
        .map(|&i| Op::AddNode {
            line: name.to_string(),
            pos: mid(i),
            before: (i + 1 < n).then_some(i + 1),
        })
        .collect();
    // Where the old nodes and the new ones end up.
    let shift = |j: usize| j + picked.iter().filter(|&&i| i < j).count();
    let mut selected: Vec<usize> = sel.iter().map(|&j| shift(j)).collect();
    selected.extend(picked.iter().map(|&i| shift(i) + 1));
    if editor.apply(ops, None) {
        editor.selection.item = Some(item);
        editor.selection.nodes = if sel.is_empty() { vec![] } else { selected };
    }
}

/// Opens a closed road or spline, or closes an open one (Alt + C).
pub fn toggle_closed(editor: &mut Editor) {
    let op = match editor.selection.item {
        Some(Item::Road(r)) => {
            let Some(road) = editor.project.roads.get(r) else {
                return;
            };
            Op::SetRoad {
                road: road.name.clone(),
                closed: Some(!road.closed),
                crown: None,
                surface: None,
                material: None,
                resolution: None,
            }
        }
        Some(Item::Spline(s)) => {
            let Some(spline) = editor.project.splines.get(s) else {
                return;
            };
            let mut spline = spline.clone();
            spline.closed = !spline.closed;
            Op::PutSpline { spline }
        }
        _ => return,
    };
    editor.apply(vec![op], None);
}

/// Makes the selected road the circuit.
pub fn set_main(editor: &mut Editor) {
    if let Some(road) = editor.road_name() {
        editor.apply(vec![Op::SetMainRoad { road }], None);
    }
}

/// Renames a road, spline or prop, keeping it selected.
pub fn rename(editor: &mut Editor, item: Item, to: &str) -> bool {
    let to = to.trim();
    let Some(name) = item_name(&editor.project, item).map(str::to_string) else {
        return false;
    };
    if to.is_empty() || to == name {
        return false;
    }
    let p = &editor.project;
    let (ops, after) = match item {
        Item::Road(_) => (
            vec![Op::RenameRoad {
                road: name,
                to: to.to_string(),
            }],
            item,
        ),
        Item::Spline(s) => {
            let mut spline = p.splines[s].clone();
            spline.name = to.to_string();
            (
                vec![Op::RemoveSpline { name }, Op::PutSpline { spline }],
                Item::Spline(p.splines.len() - 1),
            )
        }
        Item::Prop(i) => {
            if p.props.iter().any(|x| x.name == to) {
                editor.status = format!("a prop is called \"{to}\" already");
                return false;
            }
            let mut prop = p.props[i].clone();
            prop.name = to.to_string();
            (
                vec![Op::RemoveProp { name }, Op::PutProp { prop }],
                Item::Prop(p.props.len() - 1),
            )
        }
    };
    let nodes = editor.selection.nodes.clone();
    let was = editor.selection.item;
    if editor.apply(ops, None) {
        if was == Some(item) {
            editor.selection.item = Some(after);
            editor.selection.nodes = nodes;
        }
        true
    } else {
        false
    }
}

/// A road's profile: its left or right width, or its bank.
pub fn profile(road: &Road, curve: Curve) -> &StationCurve {
    match curve {
        Curve::WidthLeft | Curve::Width => &road.width_left,
        Curve::WidthRight => &road.width_right,
        Curve::Bank => &road.bank,
    }
}

/// `curve`'s keys with the values `changes` sets at nodes (a node's `u` is its index).
/// Unchanged neighbours with no key between them and a changed node keep their value
/// with a key of their own, so an edit at a node shapes the road round it only, as
/// Blender's radius and tilt do at a curve's points.
pub fn node_keys(
    curve: &StationCurve,
    count: usize,
    closed: bool,
    changes: &[(usize, f64)],
) -> Vec<Key> {
    let period = if closed {
        count
    } else {
        count.saturating_sub(1)
    } as f64;
    let mut out = curve.clone();
    let changed = |j: usize| changes.iter().any(|&(i, _)| i == j);
    for &(i, _) in changes {
        // Each neighbour, and the stretch of `u` between it and the node.
        let prev = if i > 0 {
            Some((i - 1, (i - 1) as f64, i as f64))
        } else if closed && count > 1 {
            Some((count - 1, (count - 1) as f64, count as f64))
        } else {
            None
        };
        let next = if i + 1 < count {
            Some((i + 1, i as f64, (i + 1) as f64))
        } else if closed && count > 1 {
            Some((0, i as f64, (i + 1) as f64))
        } else {
            None
        };
        for (j, lo, hi) in prev.into_iter().chain(next) {
            if changed(j) || curve.keys.iter().any(|k| k.u > lo && k.u < hi) {
                continue;
            }
            out.set(j as f64, curve.eval(j as f64, period, closed));
        }
    }
    for &(i, v) in changes {
        out.set(i as f64, v);
    }
    out.keys
}

/// Sets `curve` of the selected road to `value` at the selected nodes (all of them when
/// none are), from the sidebar's fields.
pub fn set_at_nodes(editor: &mut Editor, curve: Curve, value: f64, key: &str) {
    let Some(r) = editor
        .selection
        .road()
        .filter(|&r| r < editor.project.roads.len())
    else {
        return;
    };
    let road = &editor.project.roads[r];
    let count = road.nodes.len();
    let picked: Vec<usize> = if editor.selection.nodes.is_empty() {
        (0..count).collect()
    } else {
        editor.selection.nodes.clone()
    };
    let changes: Vec<(usize, f64)> = picked.into_iter().map(|n| (n, value)).collect();
    let curves: &[Curve] = match curve {
        Curve::Width => &[Curve::WidthLeft, Curve::WidthRight],
        _ => std::slice::from_ref(&curve),
    };
    let ops = curves
        .iter()
        .map(|&c| Op::SetProfile {
            road: road.name.clone(),
            curve: c,
            keys: node_keys(profile(road, c), count, road.closed, &changes),
        })
        .collect();
    editor.apply(ops, Some(key));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Selection;

    fn editor(name: &str) -> (Editor, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "open-racing-editor-edit-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (Editor::open(dir.clone()).unwrap(), dir)
    }

    #[test]
    fn select_more_less_and_invert_walk_a_closed_line() {
        let (mut e, dir) = editor("select");
        let n = e.project.roads[0].nodes.len();
        e.selection = Selection {
            item: Some(Item::Road(0)),
            nodes: vec![0],
        };
        select_more(&mut e);
        let mut got = e.selection.nodes.clone();
        got.sort();
        assert_eq!(got, vec![0, 1, n - 1]);
        assert_eq!(e.selection.node(), Some(0));
        select_less(&mut e);
        assert_eq!(e.selection.nodes, vec![0]);
        select_invert(&mut e);
        assert_eq!(e.selection.nodes, (1..n).collect::<Vec<_>>());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn subdivide_adds_a_node_between_selected_neighbours() {
        let (mut e, dir) = editor("subdivide");
        let n = e.project.roads[0].nodes.len();
        let (a, b) = (
            e.project.roads[0].nodes[2].pos,
            e.project.roads[0].nodes[3].pos,
        );
        e.selection = Selection {
            item: Some(Item::Road(0)),
            nodes: vec![2, 3],
        };
        subdivide(&mut e, &Built::default());
        let nodes = &e.project.roads[0].nodes;
        assert_eq!(nodes.len(), n + 1);
        assert!((nodes[3].pos - 0.5 * (a + b)).length() < 1e-9);
        assert_eq!(e.selection.nodes, vec![2, 4, 3]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rename_keeps_a_prop_or_road_selected() {
        let (mut e, dir) = editor("rename");
        e.selection.select(Item::Road(0));
        assert!(rename(&mut e, Item::Road(0), "ring"));
        assert_eq!(e.project.roads[0].name, "ring");
        assert_eq!(e.selection.item, Some(Item::Road(0)));
        assert!(!rename(&mut e, Item::Road(0), "  "));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_node_edit_keeps_the_neighbours_and_stays_local() {
        let flat = StationCurve::constant(6.0);
        let keys = node_keys(&flat, 10, true, &[(3, 8.0)]);
        let us: Vec<f64> = keys.iter().map(|k| k.u).collect();
        assert_eq!(us, vec![0.0, 2.0, 3.0, 4.0]);
        let c = StationCurve { keys };
        assert_eq!(c.eval(3.0, 10.0, true), 8.0);
        assert_eq!(c.eval(6.0, 10.0, true), 6.0);
        assert_eq!(c.eval(1.0, 10.0, true), 6.0);
        // Across the start of a closed road, and a neighbour already changed.
        let keys = node_keys(&flat, 10, true, &[(0, 7.0), (9, 7.0)]);
        let us: Vec<f64> = keys.iter().map(|k| k.u).collect();
        assert_eq!(us, vec![0.0, 1.0, 8.0, 9.0]);
        // An open road's end has one neighbour.
        let keys = node_keys(&flat, 5, false, &[(4, 3.0)]);
        let us: Vec<f64> = keys.iter().map(|k| k.u).collect();
        assert_eq!(us, vec![0.0, 3.0, 4.0]);
    }
}
