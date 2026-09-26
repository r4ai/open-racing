//! Several items edited at once, as Blender's Alt + edit: a change made to the active
//! item's properties is made to the other selected items of its kind too, field by
//! field, leaving what is each one's own (its name, nodes, place, and a road's keys and
//! stretches) alone.

use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{Prop, Road, Spline};
use serde_json::Value;

use crate::state::{Editor, Item};

/// Fields each item keeps as its own: they mean something only on that item.
fn own(item: Item) -> &'static [&'static str] {
    match item {
        Item::Road(_) => &[
            "name",
            "nodes",
            "width_left",
            "width_right",
            "bank",
            "left",
            "right",
            "lines",
            "barriers",
            "marks",
        ],
        Item::Spline(_) => &["name", "nodes"],
        Item::Prop(_) => &["name", "pos"],
    }
}

/// An item as data, to compare and patch.
pub fn snapshot(editor: &Editor, item: Item) -> Option<Value> {
    let p = &editor.project;
    match item {
        Item::Road(r) => serde_json::to_value(p.roads.get(r)?).ok(),
        Item::Spline(s) => serde_json::to_value(p.splines.get(s)?).ok(),
        Item::Prop(i) => serde_json::to_value(p.props.get(i)?).ok(),
    }
}

fn same_kind(a: Item, b: Item) -> bool {
    std::mem::discriminant(&a) == std::mem::discriminant(&b)
}

/// The variant of an enum as serde writes it: a record of one field named in capitals
/// (`{"Band": {...}}`).
fn variant(v: &Value) -> Option<&str> {
    match v {
        Value::Object(m) if m.len() == 1 => m
            .keys()
            .next()
            .filter(|k| k.starts_with(|c: char| c.is_ascii_uppercase()))
            .map(String::as_str),
        _ => None,
    }
}

/// Makes the change from `before` to `after` in `other`: records field by field (a
/// field added or taken away is added or taken away), anything else whole. A record of
/// another kind (a wall's shape beside a kerb's) is left alone.
fn patch(before: &Value, after: &Value, other: &mut Value, skip: &[&str]) -> bool {
    if before == after {
        return false;
    }
    match (variant(before), variant(after)) {
        (Some(b), Some(a)) if b == a => {
            if variant(other) != Some(b) {
                return false;
            }
        }
        (Some(_), Some(_)) => {
            *other = after.clone();
            return true;
        }
        _ => {}
    }
    match (before, after, &mut *other) {
        (Value::Object(b), Value::Object(a), Value::Object(o)) => {
            let mut changed = false;
            let keys: std::collections::BTreeSet<&String> = b.keys().chain(a.keys()).collect();
            for k in keys {
                if skip.contains(&k.as_str()) || b.get(k) == a.get(k) {
                    continue;
                }
                match (b.get(k), a.get(k)) {
                    (Some(bv), Some(av)) => match o.get_mut(k) {
                        Some(ov) => changed |= patch(bv, av, ov, &[]),
                        None => {
                            o.insert(k.clone(), av.clone());
                            changed = true;
                        }
                    },
                    (None, Some(av)) => {
                        changed |= o.get(k) != Some(av);
                        o.insert(k.clone(), av.clone());
                    }
                    (Some(_), None) => changed |= o.remove(k).is_some(),
                    (None, None) => {}
                }
            }
            changed
        }
        _ => {
            *other = after.clone();
            true
        }
    }
}

/// The operations that make the change from `before` to the active item as it is now in
/// the other selected items of its kind.
pub fn spread_ops(editor: &Editor, item: Item, before: &Value) -> Vec<Op> {
    let Some(after) = snapshot(editor, item) else {
        return vec![];
    };
    if &after == before {
        return vec![];
    }
    let skip = own(item);
    let mut ops = Vec::new();
    for other in editor.selection.others.iter().copied() {
        if !same_kind(item, other) {
            continue;
        }
        let Some(mut v) = snapshot(editor, other) else {
            continue;
        };
        if !patch(before, &after, &mut v, skip) {
            continue;
        }
        let op = match other {
            Item::Road(_) => serde_json::from_value::<Road>(v).map(|road| Op::PutRoad { road }),
            Item::Spline(_) => {
                serde_json::from_value::<Spline>(v).map(|spline| Op::PutSpline { spline })
            }
            Item::Prop(_) => serde_json::from_value::<Prop>(v).map(|prop| Op::PutProp { prop }),
        };
        if let Ok(op) = op {
            ops.push(op);
        }
    }
    ops
}

/// How many other selected items take the active one's changes.
pub fn followers(editor: &Editor) -> usize {
    let Some(item) = editor.selection.item else {
        return 0;
    };
    editor
        .selection
        .others
        .iter()
        .filter(|&&o| same_kind(item, o))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn a_change_to_the_active_item_reaches_the_others_field_by_field() {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Editor::open(dir.clone()).unwrap();
        for (name, y, style) in [
            ("a", 0.0, "kerb"),
            ("b", 10.0, "kerb"),
            ("w", 20.0, "concrete wall"),
        ] {
            let spline = crate::presets::named(&e.project, style)
                .unwrap()
                .spline(
                    &e.project,
                    vec![DVec3::new(0.0, y, 0.0), DVec3::new(9.0, y, 0.0)],
                )
                .unwrap();
            let spline = Spline {
                name: name.into(),
                ..spline
            };
            assert!(e.apply(vec![Op::PutSpline { spline }], None));
        }
        e.selection.select(Item::Spline(0));
        e.selection.others = vec![Item::Spline(1), Item::Spline(2)];
        let before = snapshot(&e, Item::Spline(0)).unwrap();
        // The active kerb made 2.5 m wide and not draped.
        let mut sp = e.project.splines[0].clone();
        sp.drape = false;
        if let open_racing_track_project::project::Shape::Band { width, .. } = &mut sp.shape {
            *width = 2.5;
        }
        assert!(e.apply(vec![Op::PutSpline { spline: sp }], None));
        let ops = spread_ops(&e, Item::Spline(0), &before);
        assert!(e.apply(ops, None));
        let b = &e.project.splines[1];
        assert!(!b.drape);
        assert!(
            matches!(b.shape, open_racing_track_project::project::Shape::Band { width, .. } if width == 2.5)
        );
        assert_eq!(b.nodes[0].pos.y, 10.0, "its own nodes");
        assert_eq!(b.name, "b");
        // The wall is not a band: it only takes what it shares.
        let w = &e.project.splines[2];
        assert!(!w.drape);
        assert!(matches!(
            w.shape,
            open_racing_track_project::project::Shape::Wall { .. }
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
