//! Edits of the selection that menus, shortcuts and the search all offer: selecting more
//! or less, handle types, subdividing, opening and closing lines, renaming, and a road's
//! width and bank at its nodes.

use glam::DVec3;
use open_racing_track_project::Project;
use open_racing_track_project::curve::{handles, segments};
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::{HandleMode, Key, Road, StationCurve};

use crate::state::{Editor, Item, item_line};

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
    let picked = editor.picked_nodes();
    let Some((name, nodes, closed)) = editor.line() else {
        return;
    };
    let ops: Vec<Op> = picked
        .into_iter()
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

/// Makes a node's handles automatic again (Alt + click on a handle).
pub fn auto_handles(editor: &mut Editor, item: Item, index: usize) {
    let Some((name, ..)) = item_line(&editor.project, item) else {
        return;
    };
    let line = name.to_string();
    editor.apply(
        vec![Op::SetNodeHandles {
            line,
            index,
            mode: HandleMode::Auto,
            incoming: DVec3::ZERO,
            outgoing: DVec3::ZERO,
        }],
        None,
    );
}

/// Adds a node halfway along each segment between selected nodes, or along every
/// segment with none selected, keeping the line's shape.
pub fn subdivide(editor: &mut Editor) {
    let (Some(item), Some((name, nodes, closed))) = (editor.selection.item, editor.line()) else {
        return;
    };
    let n = nodes.len();
    let sel = editor.selection.nodes.clone();
    let picked: Vec<usize> = (0..segments(n, closed))
        .filter(|&i| sel.is_empty() || (sel.contains(&i) && sel.contains(&((i + 1) % n))))
        .collect();
    if picked.is_empty() {
        editor.status = "select two neighbouring nodes to subdivide between".into();
        return;
    }
    let op = Op::Subdivide {
        line: name.to_string(),
        segments: picked.clone(),
    };
    // Where the old nodes and the new ones end up.
    let shift = |j: usize| j + picked.iter().filter(|&&i| i < j).count();
    let mut selected: Vec<usize> = sel.iter().map(|&j| shift(j)).collect();
    selected.extend(picked.iter().map(|&i| shift(i) + 1));
    if editor.apply(vec![op], None) {
        editor.selection.item = Some(item);
        editor.selection.nodes = if sel.is_empty() { vec![] } else { selected };
    }
}

/// What smoothing evens out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Smooth {
    /// The nodes' heights.
    Heights,
    /// Their places in plan.
    Shape,
}

/// Evens out the selected nodes (all with none selected) with their neighbours two
/// either side, as Blender's Smooth; an open line's ends stay. Again smooths more.
pub fn smooth(editor: &mut Editor, what: Smooth) {
    let picked = editor.picked_nodes();
    let Some((name, nodes, closed)) = editor.line() else {
        return;
    };
    let n = nodes.len();
    let at = |i: usize, d: isize| -> Option<usize> {
        let j = i as isize + d;
        if closed {
            Some(j.rem_euclid(n as isize) as usize)
        } else {
            (0..n as isize).contains(&j).then_some(j as usize)
        }
    };
    let ops: Vec<Op> = picked
        .into_iter()
        .filter(|&i| closed || (i > 0 && i + 1 < n))
        .map(|i| {
            let (mut sum, mut total) = (DVec3::ZERO, 0.0);
            for (d, w) in [(-2, 1.0), (-1, 2.0), (0, 3.0), (1, 2.0), (2, 1.0)] {
                if let Some(j) = at(i, d) {
                    sum += nodes[j].pos * w;
                    total += w;
                }
            }
            let mean = sum / total;
            let pos = nodes[i].pos;
            Op::MoveNode {
                line: name.to_string(),
                index: i,
                pos: match what {
                    Smooth::Heights => pos.with_z(mean.z),
                    Smooth::Shape => mean.with_z(pos.z),
                },
            }
        })
        .collect();
    if !ops.is_empty() {
        editor.apply(ops, None);
    }
}

/// Puts the selected nodes at their mean height: a level stretch.
pub fn flatten(editor: &mut Editor) {
    let picked = editor.picked_nodes();
    let Some((name, nodes, _)) = editor.line() else {
        return;
    };
    if picked.is_empty() {
        return;
    }
    let z = picked.iter().map(|&i| nodes[i].pos.z).sum::<f64>() / picked.len() as f64;
    let ops = picked
        .iter()
        .map(|&i| Op::MoveNode {
            line: name.to_string(),
            index: i,
            pos: nodes[i].pos.with_z(z),
        })
        .collect();
    editor.apply(ops, None);
}

/// A steady slope from the first selected node to the last: the nodes between them
/// take heights in proportion to the distance along the line.
pub fn even_grade(editor: &mut Editor) {
    let mut sel = editor.selection.nodes.clone();
    sel.sort_unstable();
    let Some((name, nodes, _)) = editor.line() else {
        return;
    };
    let (Some(&a), Some(&b)) = (sel.first(), sel.last()) else {
        return;
    };
    if b >= nodes.len() || b < a + 2 {
        editor.status = "select the nodes at both ends of the slope, with nodes between".into();
        return;
    }
    let run = |i: usize| {
        nodes[i]
            .pos
            .truncate()
            .distance(nodes[i + 1].pos.truncate())
    };
    let total: f64 = (a..b).map(run).sum();
    if total < 1e-9 {
        return;
    }
    let (za, zb) = (nodes[a].pos.z, nodes[b].pos.z);
    let mut along = 0.0;
    let mut ops = Vec::new();
    for (i, node) in nodes.iter().enumerate().take(b).skip(a + 1) {
        along += run(i - 1);
        ops.push(Op::MoveNode {
            line: name.to_string(),
            index: i,
            pos: node.pos.with_z(za + (zb - za) * along / total),
        });
    }
    let grade = 100.0 * (zb - za) / total;
    if editor.apply(ops, None) {
        editor.status = format!("a steady {grade:+.1} % from node {a} to node {b}");
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

/// Y: cuts the selected road or spline at its active node. An open line becomes two,
/// the part from the node on a new line; a loop opens there.
pub fn split(editor: &mut Editor) {
    let (Some(item), Some(at)) = (editor.selection.item, editor.selection.node()) else {
        return;
    };
    let Some((name, _, closed)) = editor.line() else {
        return;
    };
    let name = name.to_string();
    let to = crate::presets::unique_name(&editor.project, &format!("{name} 2"));
    let op = Op::SplitLine {
        line: name.clone(),
        at,
        to: to.clone(),
    };
    if editor.apply(vec![op], None) {
        editor.selection.select(item);
        editor.status = if closed {
            format!("\"{name}\" opened at node {at}")
        } else {
            format!("\"{name}\" cut at node {at}; the rest is \"{to}\"")
        };
    }
}

/// Ctrl J: joins the other selected roads (or splines) onto the active one, end to end.
pub fn join(editor: &mut Editor) {
    let Some(item) = editor.selection.item else {
        return;
    };
    let Some(name) = item_name(&editor.project, item).map(str::to_string) else {
        return;
    };
    let ops: Vec<Op> = editor
        .selection
        .others
        .iter()
        .filter_map(|&o| item_name(&editor.project, o))
        .map(|with| Op::JoinLines {
            line: name.clone(),
            with: with.to_string(),
        })
        .collect();
    let n = ops.len();
    if editor.apply(ops, None) {
        // The joined lines are gone: find the active one again by its name.
        let p = &editor.project;
        let item = match item {
            Item::Road(_) => p.road_index(&name).map(Item::Road),
            Item::Spline(_) => p
                .splines
                .iter()
                .position(|s| s.name == name)
                .map(Item::Spline),
            Item::Prop(_) => None,
        };
        editor.selection = Default::default();
        if let Some(item) = item {
            editor.selection.select(item);
        }
        editor.status = format!("joined {n} onto \"{name}\"");
    }
}

/// Turns the selected road or spline round, to run the other way.
pub fn reverse(editor: &mut Editor) {
    let Some((name, nodes, closed)) = editor.line() else {
        return;
    };
    let (line, n) = (name.to_string(), nodes.len());
    if editor.apply(vec![Op::ReverseLine { line: line.clone() }], None) {
        // The same nodes stay selected, under their new numbers.
        let flip = |i: usize| if closed { (n - i) % n } else { n - 1 - i };
        let sel = &mut editor.selection;
        sel.nodes = sel.nodes.iter().map(|&i| flip(i)).collect();
        editor.status = format!("\"{line}\" runs the other way now");
    }
}

/// Selects every item like the active one: of its kind, its strip or wall type (or
/// model), or its material. Hidden and locked items are left out.
pub fn select_similar(editor: &mut Editor, by: crate::commands::Similar) {
    use crate::commands::Similar;
    use open_racing_track_project::project::Shape;
    let Some(active) = editor.selection.item else {
        return;
    };
    let p = &editor.project;
    // What each item is, compared by `by`.
    let key = |item: Item| -> Option<String> {
        match (by, item) {
            (Similar::Kind, i) => Some(item_kind(i).to_string()),
            (Similar::Type, Item::Road(r)) => p.roads.get(r).map(|r| r.surface.clone()),
            (Similar::Type, Item::Spline(s)) => p.splines.get(s).map(|s| {
                s.style.clone().unwrap_or_else(|| match &s.shape {
                    Shape::Band { .. } => "band".into(),
                    Shape::Wall { .. } => "wall".into(),
                })
            }),
            (Similar::Type | Similar::Material, Item::Prop(i)) => p
                .props
                .get(i)
                .map(|x| x.model.to_string_lossy().into_owned()),
            (Similar::Material, Item::Road(r)) => p.roads.get(r).map(|r| r.material.clone()),
            (Similar::Material, Item::Spline(s)) => {
                p.splines.get(s).map(|s| s.material().to_string())
            }
        }
        .map(|k| format!("{}:{k}", item_kind(item)))
    };
    let want = key(active);
    let found: Vec<Item> = editor
        .all_items()
        .into_iter()
        .filter(|&i| i != active && editor.pickable(i) && key(i) == want)
        .collect();
    let sel = &mut editor.selection;
    sel.nodes.clear();
    for i in &found {
        if !sel.has(*i) {
            sel.others.push(*i);
        }
    }
    editor.status = format!("{} more selected", found.len());
}

/// The new names batch renaming gives `items`: `find` replaced by `replace` in each,
/// or with nothing to find, `replace` numbered ("tree 1", "tree 2"…). Unchanged names
/// are left out.
pub fn batch_names(
    project: &Project,
    items: &[Item],
    find: &str,
    replace: &str,
) -> Vec<(Item, String)> {
    items
        .iter()
        .enumerate()
        .filter_map(|(k, &item)| {
            let name = item_name(project, item)?;
            let to = if find.is_empty() {
                if replace.trim().is_empty() {
                    return None;
                }
                format!("{} {}", replace.trim(), k + 1)
            } else {
                name.replace(find, replace)
            };
            (to != name && !to.trim().is_empty()).then(|| (item, to.trim().to_string()))
        })
        .collect()
}

/// Renames several items at once, as one step.
pub fn batch_rename(editor: &mut Editor, items: &[Item], find: &str, replace: &str) {
    let p = &editor.project;
    let ops: Vec<Op> = batch_names(p, items, find, replace)
        .into_iter()
        .filter_map(|(item, to)| {
            let name = item_name(p, item)?.to_string();
            Some(match item {
                Item::Road(_) => Op::RenameRoad { road: name, to },
                Item::Spline(_) => Op::RenameSpline { name, to },
                Item::Prop(_) => Op::RenameProp { name, to },
            })
        })
        .collect();
    let n = ops.len();
    if n > 0 && editor.apply(ops, None) {
        editor.status = format!("{n} renamed");
    }
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
    let to = to.to_string();
    let op = match item {
        Item::Road(_) => Op::RenameRoad { road: name, to },
        Item::Spline(_) => Op::RenameSpline { name, to },
        Item::Prop(_) => Op::RenameProp { name, to },
    };
    // The renamed item keeps its place in the list, and so the selection.
    editor.apply(vec![op], None)
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
    let period = segments(count, closed) as f64;
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
    let picked = editor.picked_nodes();
    let road = &editor.project.roads[r];
    let count = road.nodes.len();
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
            others: vec![],
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
        let road = &e.project.roads[0];
        let mid = open_racing_track_project::curve::point(road, 2.5);
        e.selection = Selection {
            item: Some(Item::Road(0)),
            nodes: vec![2, 3],
            others: vec![],
        };
        subdivide(&mut e);
        let nodes = &e.project.roads[0].nodes;
        assert_eq!(nodes.len(), n + 1);
        assert!((nodes[3].pos - mid).length() < 1e-9, "on the curve");
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

        // Splines keep their place, and a taken name leaves everything as it was.
        for (name, y) in [("a", 0.0), ("b", 5.0)] {
            let spline = crate::presets::named(&e.project, "kerb")
                .unwrap()
                .spline(
                    &e.project,
                    vec![DVec3::new(0.0, y, 0.0), DVec3::new(10.0, y, 0.0)],
                )
                .unwrap();
            let spline = open_racing_track_project::project::Spline {
                name: name.into(),
                ..spline
            };
            assert!(e.apply(vec![Op::PutSpline { spline }], None));
        }
        e.selection.select(Item::Spline(0));
        assert!(rename(&mut e, Item::Spline(0), "first"));
        assert_eq!(e.project.splines[0].name, "first");
        assert_eq!(e.selection.item, Some(Item::Spline(0)));
        assert!(!rename(&mut e, Item::Spline(0), "b"));
        assert_eq!(e.project.splines.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_a_whole_item_selects_nothing() {
        let (mut e, dir) = editor("delete");
        for y in [0.0, 5.0] {
            let spline = crate::presets::named(&e.project, "kerb")
                .unwrap()
                .spline(
                    &e.project,
                    vec![DVec3::new(0.0, y, 0.0), DVec3::new(10.0, y, 0.0)],
                )
                .unwrap();
            assert!(e.apply(vec![Op::PutSpline { spline }], None));
        }
        e.selection.select(Item::Spline(0));
        crate::viewport::delete(&mut e);
        assert_eq!(e.project.splines.len(), 1);
        assert_eq!(e.selection.item, None, "not the spline that moved up");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_join_and_reverse_keep_the_road_selected() {
        let (mut e, dir) = editor("split");
        assert!(e.apply(
            vec![Op::AddRoad {
                name: "pit".into(),
                closed: false,
                nodes: (0..4).map(|i| DVec3::new(i as f64 * 40.0, -50.0, 0.0)).collect(),
                like: None,
            }],
            None
        ));
        e.selection.select_node(Item::Road(1), 2);
        split(&mut e);
        assert_eq!(e.project.roads.len(), 3);
        assert_eq!(e.project.roads[2].name, "pit 2");
        assert_eq!(e.selection.item, Some(Item::Road(1)));
        // Join the rest back, then turn it round keeping node 0 selected at its end.
        e.selection.others = vec![Item::Road(2)];
        join(&mut e);
        assert_eq!(e.project.roads.len(), 2);
        assert_eq!(e.project.roads[1].nodes.len(), 4);
        assert_eq!(e.selection.item, Some(Item::Road(1)));
        e.selection.nodes = vec![0];
        reverse(&mut e);
        assert_eq!(e.selection.nodes, vec![3]);
        assert_eq!(e.project.roads[1].nodes[3].pos, DVec3::new(0.0, -50.0, 0.0));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn select_similar_finds_the_same_type() {
        let (mut e, dir) = editor("similar");
        for (name, y, style) in [
            ("k1", 0.0, "kerb"),
            ("k2", 10.0, "kerb"),
            ("w", 20.0, "concrete wall"),
        ] {
            let spline = crate::presets::named(&e.project, style)
                .unwrap()
                .spline(
                    &e.project,
                    vec![DVec3::new(0.0, y, 0.0), DVec3::new(9.0, y, 0.0)],
                )
                .unwrap();
            let spline = open_racing_track_project::project::Spline {
                name: name.into(),
                ..spline
            };
            assert!(e.apply(vec![Op::PutSpline { spline }], None));
        }
        e.selection.select(Item::Spline(0));
        select_similar(&mut e, crate::commands::Similar::Type);
        assert_eq!(e.selection.others, vec![Item::Spline(1)]);
        select_similar(&mut e, crate::commands::Similar::Kind);
        assert_eq!(e.selection.items().len(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn batch_rename_replaces_or_numbers() {
        let (mut e, dir) = editor("batch-rename");
        for y in [0.0, 10.0] {
            let spline = crate::presets::named(&e.project, "kerb")
                .unwrap()
                .spline(
                    &e.project,
                    vec![DVec3::new(0.0, y, 0.0), DVec3::new(9.0, y, 0.0)],
                )
                .unwrap();
            assert!(e.apply(vec![Op::PutSpline { spline }], None));
        }
        let items = [Item::Spline(0), Item::Spline(1)];
        batch_rename(&mut e, &items, "", "T4 sausage");
        let names: Vec<&str> = e.project.splines.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["T4 sausage 1", "T4 sausage 2"]);
        batch_rename(&mut e, &items, "T4", "T5");
        assert_eq!(e.project.splines[1].name, "T5 sausage 2");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn height_tools_level_slope_and_smooth() {
        let (mut e, dir) = editor("heights");
        let z =
            |e: &Editor| -> Vec<f64> { e.project.roads[0].nodes.iter().map(|n| n.pos.z).collect() };
        e.selection = Selection {
            item: Some(Item::Road(0)),
            nodes: vec![0],
            others: vec![],
        };
        let mut p = e.project.roads[0].nodes[4].pos;
        p.z = 12.0;
        assert!(e.apply(
            vec![Op::MoveNode {
                line: "circuit".into(),
                index: 4,
                pos: p,
            }],
            None
        ));
        // A slope from node 2 (0 m) to node 4 (12 m): node 3 in proportion.
        e.selection.nodes = vec![4, 2];
        even_grade(&mut e);
        let nodes = &e.project.roads[0].nodes;
        let (d23, d34) = (
            nodes[2].pos.truncate().distance(nodes[3].pos.truncate()),
            nodes[3].pos.truncate().distance(nodes[4].pos.truncate()),
        );
        assert!((z(&e)[3] - 12.0 * d23 / (d23 + d34)).abs() < 1e-9);
        // Smoothing spreads the bump to the neighbours and lowers its top.
        e.selection.nodes.clear();
        smooth(&mut e, Smooth::Heights);
        let after = z(&e);
        assert!(after[4] < 12.0 && after[6] > 0.0, "{after:?}");
        // Flattening levels the selected ones at their mean.
        e.selection.nodes = vec![3, 4, 5];
        flatten(&mut e);
        let after = z(&e);
        assert!((after[3] - after[5]).abs() < 1e-9 && (after[4] - after[3]).abs() < 1e-9);
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
