//! Copying and pasting items (Ctrl C, Ctrl V) as text: the operations that make them,
//! with the surfaces, materials and strip and wall types they use, so that they paste
//! into another project too. Any list of operations pastes, as `trackctl apply` takes.

use std::collections::BTreeSet;

use open_racing_track_project::Project;
use open_racing_track_project::ops::{self, Op};
use open_racing_track_project::project::Shape;

use crate::state::{Editor, Item};

/// Names of the surfaces, materials, strip types and wall types something uses.
#[derive(Default)]
struct Uses {
    surfaces: BTreeSet<String>,
    materials: BTreeSet<String>,
    strips: BTreeSet<String>,
    walls: BTreeSet<String>,
}

impl Uses {
    fn of(&mut self, p: &Project, item: Item) {
        match item {
            Item::Road(r) => {
                let Some(r) = p.roads.get(r) else { return };
                self.surfaces.insert(r.surface.clone());
                self.materials.insert(r.material.clone());
                for s in r.left.iter().chain(&r.right) {
                    self.surfaces.insert(s.surface.clone());
                    self.materials.insert(s.material.clone());
                    self.strips.extend(s.style.clone());
                }
                for l in &r.lines {
                    self.materials.insert(l.material.clone());
                }
                for b in &r.barriers {
                    self.materials.insert(b.material.clone());
                    self.walls.extend(b.style.clone());
                }
                for m in &r.marks {
                    self.materials.insert(m.material.clone());
                }
            }
            Item::Spline(s) => {
                let Some(s) = p.splines.get(s) else { return };
                self.materials.insert(s.material().to_string());
                match &s.shape {
                    Shape::Band { surface, .. } => {
                        self.surfaces.insert(surface.clone());
                        self.strips.extend(s.style.clone());
                    }
                    Shape::Wall { .. } => {
                        self.walls.extend(s.style.clone());
                    }
                }
            }
            Item::Prop(_) => {}
        }
    }

    /// The types' own surfaces and materials, too.
    fn close(&mut self, p: &Project) {
        for s in &self.strips {
            if let Some(t) = p.strip_style(s) {
                self.surfaces.insert(t.surface.clone());
                self.materials.insert(t.material.clone());
            }
        }
        for w in &self.walls {
            if let Some(t) = p.wall_style(w) {
                self.materials.insert(t.material.clone());
            }
        }
    }
}

/// The selected items as operations that make them, and how many there are.
pub fn copy(editor: &Editor) -> Option<(String, usize)> {
    let p = &editor.project;
    let items = editor.selection.items();
    if items.is_empty() {
        return None;
    }
    let mut uses = Uses::default();
    for &i in &items {
        uses.of(p, i);
    }
    uses.close(p);
    let mut out: Vec<Op> = Vec::new();
    out.extend(
        p.surfaces
            .iter()
            .filter(|s| uses.surfaces.contains(&s.name))
            .map(|s| Op::PutSurface { surface: s.clone() }),
    );
    out.extend(
        p.materials
            .iter()
            .filter(|m| uses.materials.contains(&m.name))
            .map(|m| Op::PutMaterial {
                material: m.clone(),
            }),
    );
    out.extend(
        p.strip_styles
            .iter()
            .filter(|s| uses.strips.contains(&s.name))
            .map(|s| Op::PutStripStyle { style: s.clone() }),
    );
    out.extend(
        p.wall_styles
            .iter()
            .filter(|s| uses.walls.contains(&s.name))
            .map(|s| Op::PutWallStyle { style: s.clone() }),
    );
    for &i in &items {
        match i {
            Item::Road(r) => out.push(Op::PutRoad {
                road: p.roads.get(r)?.clone(),
            }),
            Item::Spline(s) => out.push(Op::PutSpline {
                spline: p.splines.get(s)?.clone(),
            }),
            Item::Prop(x) => out.push(Op::PutProp {
                prop: p.props.get(x)?.clone(),
            }),
        }
    }
    let text = ron::ser::to_string_pretty(&out, ron::ser::PrettyConfig::default()).ok()?;
    Some((text, items.len()))
}

/// Pastes operations: roads, splines and props come in as new items (renamed where
/// their names are taken) and are selected; surfaces, materials and types the project
/// has already keep its own. Gives how many items came in.
pub fn paste(editor: &mut Editor, text: &str) -> Result<usize, String> {
    let list = ops::parse(text).map_err(|e| e.to_string())?;
    // Names taken as each comes in, on a copy of the project.
    let mut p = editor.project.clone();
    let mut out = Vec::new();
    let mut items = Vec::new();
    for op in list {
        let op = match op {
            Op::PutSurface { ref surface } if p.surface_index(&surface.name).is_some() => continue,
            Op::PutMaterial { ref material } if p.material_index(&material.name).is_some() => {
                continue;
            }
            Op::PutStripStyle { ref style } if p.strip_style(&style.name).is_some() => continue,
            Op::PutWallStyle { ref style } if p.wall_style(&style.name).is_some() => continue,
            Op::PutRoad { mut road } => {
                road.name = crate::presets::unique_name(&p, &road.name);
                items.push(Item::Road(p.roads.len()));
                Op::PutRoad { road }
            }
            Op::PutSpline { mut spline } => {
                spline.name = crate::presets::unique_name(&p, &spline.name);
                items.push(Item::Spline(p.splines.len()));
                Op::PutSpline { spline }
            }
            Op::PutProp { mut prop } => {
                prop.name = crate::presets::unique_prop_name(&p, &prop.name);
                items.push(Item::Prop(p.props.len()));
                Op::PutProp { prop }
            }
            op => op,
        };
        op.apply(&mut p).map_err(|e| e.to_string())?;
        out.push(op);
    }
    if out.is_empty() {
        return Ok(0);
    }
    if !editor.apply(out, None) {
        return Err(editor.status.clone());
    }
    if !items.is_empty() {
        editor.selection = Default::default();
        for i in &items {
            if editor.selection.item.is_none() {
                editor.selection.select(*i);
            } else {
                editor.selection.others.push(*i);
            }
        }
    }
    Ok(items.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_items_paste_into_another_project_with_what_they_use() {
        let base = std::env::temp_dir().join(format!(
            "open-racing-editor-clipboard-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let mut from = Editor::open(base.join("from")).unwrap();
        // A material of its own on the circuit, which the other project lacks.
        let mut m = from.project.materials[0].clone();
        m.name = "red asphalt".into();
        let road = {
            let mut r = from.project.roads[0].clone();
            r.material = "red asphalt".into();
            r
        };
        assert!(from.apply(
            vec![Op::PutMaterial { material: m }, Op::PutRoad { road }],
            None
        ));
        from.selection.select(Item::Road(0));
        let (text, n) = copy(&from).unwrap();
        assert_eq!(n, 1);

        let mut to = Editor::open(base.join("to")).unwrap();
        assert_eq!(paste(&mut to, &text), Ok(1));
        assert_eq!(to.project.roads.len(), 2);
        let pasted = &to.project.roads[1];
        assert_ne!(pasted.name, "circuit", "renamed, the name was taken");
        assert_eq!(pasted.material, "red asphalt");
        assert!(to.project.material_index("red asphalt").is_some());
        assert_eq!(to.selection.item, Some(Item::Road(1)));
        // Pasting again brings another copy, and the material once.
        assert_eq!(paste(&mut to, &text), Ok(1));
        assert_eq!(to.project.roads.len(), 3);
        let materials = to
            .project
            .materials
            .iter()
            .filter(|m| m.name == "red asphalt");
        assert_eq!(materials.count(), 1);
        std::fs::remove_dir_all(base).unwrap();
    }
}
