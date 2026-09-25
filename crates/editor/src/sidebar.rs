//! The 3D view's sidebar (N), as Blender's: the selection's transform (Item), the active
//! tool's settings (Tool) and the view and its overlays (View).

use bevy_egui::egui;
use glam::DVec3;
use open_racing_track_project::curve::handles;
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::HandleMode;
use open_racing_track_render::{from_bevy, to_bevy};

use crate::commands::{Ctx, handle_label};
use crate::edit;
use crate::properties::{number, prop_fields, row, section, vector};
use crate::state::Item;
use crate::viewport::ToolKind;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Item,
    Tool,
    View,
}

pub fn show(ui: &mut egui::Ui, c: &mut Ctx) {
    ui.horizontal(|ui| {
        for (tab, label) in [
            (Tab::Item, "Item"),
            (Tab::Tool, "Tool"),
            (Tab::View, "View"),
        ] {
            ui.selectable_value(&mut c.shell.sidebar_tab, tab, label);
        }
    });
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| match c.shell.sidebar_tab {
            Tab::Item => item_tab(ui, c),
            Tab::Tool => tool_tab(ui, c),
            Tab::View => view_tab(ui, c),
        });
}

fn item_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(item) = c.editor.selection.item else {
        ui.weak("Nothing selected. Click a road, a node, a kerb or a prop in the view.");
        return;
    };
    let name = edit::item_name(&c.editor.project, item)
        .unwrap_or_default()
        .to_string();
    ui.horizontal(|ui| {
        ui.weak(edit::item_kind(item));
        ui.strong(&name);
    });
    if let Item::Prop(i) = item {
        section(ui, "Transform", "sidebar prop", true, |ui| {
            prop_fields(ui, c.editor, i);
        });
        return;
    }
    let Some((_, nodes, closed)) = c.editor.line() else {
        return;
    };
    let nodes = nodes.to_vec();
    let selected: Vec<usize> = c
        .editor
        .selection
        .nodes
        .iter()
        .copied()
        .filter(|&n| n < nodes.len())
        .collect();

    // One node: its place. Several, or none (the whole line): their middle, which moves
    // them all together.
    let (title, picked) = match selected.len() {
        0 => (
            "Whole line (median)".to_string(),
            (0..nodes.len()).collect(),
        ),
        1 => (format!("Node {}", selected[0]), selected.clone()),
        n => (format!("{n} nodes (median)"), selected.clone()),
    };
    section(ui, "Transform", "sidebar transform", true, |ui| {
        ui.weak(&title);
        let median =
            picked.iter().map(|&n| nodes[n].pos).sum::<DVec3>() / picked.len().max(1) as f64;
        let mut at = median;
        if vector(ui, "Location", &mut at, 0.25) {
            let d = at - median;
            let ops = picked
                .iter()
                .map(|&index| Op::MoveNode {
                    line: name.clone(),
                    index,
                    pos: nodes[index].pos + d,
                })
                .collect();
            c.editor
                .apply(ops, Some(&format!("median {name} {picked:?}")));
        }
        if let [n] = selected[..] {
            row(ui, "Along (u)", |ui| ui.label(format!("{n}")));
        }
    });

    if let Item::Road(r) = item {
        section(ui, "Width & Bank", "sidebar shape", true, |ui| {
            shape_ui(ui, c, r, &title, selected.first().copied());
        });
    }
    if let Some(n) = c.editor.selection.node().filter(|&n| n < nodes.len()) {
        section(ui, "Handles", "sidebar handles", true, |ui| {
            handles_ui(ui, c, &name, &nodes, closed, n);
        });
    }
}

/// The road's width either side and its bank at the selected nodes, as Blender's radius
/// and tilt of a curve's points. Editing sets them all; the road between eases to its
/// neighbours.
fn shape_ui(ui: &mut egui::Ui, c: &mut Ctx, r: usize, title: &str, first: Option<usize>) {
    let road = &c.editor.project.roads[r];
    let name = road.name.clone();
    let (period, closed) = (road.period(), road.closed);
    // The first selected node's values; with none selected, the start's.
    let u = c
        .editor
        .selection
        .node()
        .or(first)
        .map_or(0.0, |n| n as f64);
    let at = |curve: Curve| edit::profile(road, curve).eval(u, period, closed);
    let (mut left, mut right, mut bank) = (
        at(Curve::WidthLeft),
        at(Curve::WidthRight),
        at(Curve::Bank).to_degrees(),
    );
    ui.weak(title);
    let mut both = left + right;
    let fields: [(&str, &mut f64, &str, Curve); 3] = [
        ("Width left", &mut left, " m", Curve::WidthLeft),
        ("Width right", &mut right, " m", Curve::WidthRight),
        ("Bank", &mut bank, "°", Curve::Bank),
    ];
    for (label, v, suffix, curve) in fields {
        if row(ui, label, |ui| number(ui, v, 0.05, suffix)) {
            let value = match curve {
                Curve::Bank => v.to_radians(),
                _ => v.max(0.1),
            };
            edit::set_at_nodes(c.editor, curve, value, &format!("shape {name} {label}"));
        }
    }
    if row(ui, "Total width", |ui| number(ui, &mut both, 0.1, " m")) {
        edit::set_at_nodes(
            c.editor,
            Curve::Width,
            (both / 2.0).max(0.1),
            &format!("shape {name} both"),
        );
    }
    ui.weak("Alt S: width · Ctrl T: bank, with the mouse in the view. Positive bank raises the right edge.");
}

/// The active node's handles: their kind and offsets.
fn handles_ui(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    name: &str,
    nodes: &[open_racing_track_project::Node],
    closed: bool,
    n: usize,
) {
    let mut mode = nodes[n].handles.mode();
    let (mut incoming, mut outgoing) = handles(nodes, closed, n);
    if mode == HandleMode::Auto && !closed && n + 1 == nodes.len() {
        outgoing = -incoming;
    }
    let previous = mode;
    row(ui, "Type", |ui| {
        egui::ComboBox::from_id_salt("handle type")
            .selected_text(handle_label(mode))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for m in [HandleMode::Auto, HandleMode::Aligned, HandleMode::Free] {
                    ui.selectable_value(&mut mode, m, handle_label(m));
                }
            })
    })
    .response
    .on_hover_text("V in the view sets the selected nodes' handles");
    let mut changed = mode != previous;
    if mode == HandleMode::Auto {
        ui.weak("Automatic: the curve runs smoothly through the nodes. Drag a handle in the view to shape it.");
    } else {
        changed |= vector(ui, "Out", &mut outgoing, 0.25);
        if mode == HandleMode::Aligned {
            let mut length = incoming.length();
            if row(ui, "In length", |ui| number(ui, &mut length, 0.25, " m")) {
                changed = true;
            }
            incoming = -outgoing.normalize_or_zero() * length.max(0.0);
        } else {
            changed |= vector(ui, "In", &mut incoming, 0.25);
        }
        ui.weak("Alt+click a handle in the view: automatic");
    }
    if changed {
        c.editor.apply(
            vec![Op::SetNodeHandles {
                line: name.to_string(),
                index: n,
                mode,
                incoming,
                outgoing,
            }],
            Some(&format!("handle {name} {n}")),
        );
    }
}

fn tool_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    section(ui, "Active Tool", "sidebar tool", true, |ui| {
        for t in ToolKind::ALL {
            let text = match t {
                ToolKind::Select => "Select Box: click to pick, drag for a box",
                ToolKind::Move => "Move: drag the gizmo's arrows",
                ToolKind::Rotate => "Rotate: drag the gizmo's ring",
                ToolKind::Scale => "Scale: drag the gizmo's handles",
                ToolKind::AddNode => "Add Node: click to add to the selected line",
                ToolKind::Measure => "Measure: click two points",
            };
            ui.radio_value(&mut c.tool.active, t, text);
        }
    });
    if c.tool.active == ToolKind::Measure {
        section(ui, "Measure", "sidebar measure", true, |ui| {
            measure_ui(ui, c);
        });
    }
    section(ui, "Snapping", "sidebar snap", true, |ui| {
        ui.checkbox(&mut c.tool.snap, "Snap while transforming");
        ui.weak("Whole metres, 5° and tenths. Holding Ctrl while moving does the opposite.");
    });
    section(ui, "Draw", "sidebar draw", true, |ui| {
        ui.weak("Shift A in the view, or the toolbar, draws a road, kerb, wall or fence: click points, Enter finishes. Kerbs snap to road edges (Ctrl: free).");
    });
}

/// The distance measured, and scaling the reference image so that it comes out as a
/// distance known on the real circuit.
fn measure_ui(ui: &mut egui::Ui, c: &mut Ctx) {
    let [a, b] = match c.tool.measure[..] {
        [a, b] => [a, b],
        _ => {
            ui.weak("Click two points in the view.");
            return;
        }
    };
    let d = b - a;
    row(ui, "Distance", |ui| {
        ui.label(format!("{:.2} m", d.truncate().length()))
    });
    row(ui, "Height", |ui| ui.label(format!("{:+.2} m", d.z)));
    if d.truncate().length() > 1e-6 {
        row(ui, "Grade", |ui| {
            ui.label(format!("{:+.1} %", 100.0 * d.z / d.truncate().length()))
        });
    }
    if ui.button("Clear").clicked() {
        c.tool.measure.clear();
    }
    let Some(r) = c.editor.project.reference.clone() else {
        return;
    };
    ui.separator();
    ui.weak("Measured on the reference image, a distance you know on the real circuit (a straight, a pit building) scales the image to size.");
    let id = ui.make_persistent_id("known distance");
    let mut known: f64 = ui
        .data_mut(|m| m.get_temp(id))
        .unwrap_or_else(|| d.truncate().length().round());
    row(ui, "Real length", |ui| number(ui, &mut known, 0.5, " m"));
    ui.data_mut(|m| m.insert_temp(id, known));
    if ui.button("Scale the reference image").clicked() {
        match crate::reference::calibrated(&r, a, b, known) {
            Some(r) => {
                crate::reference::set(c.editor, Some(r), None);
                // The same points on the image are now `known` apart.
                c.tool.measure[1] = a + (b - a) * (known / d.truncate().length());
                c.editor.status = format!("reference image scaled: that is {known} m now");
            }
            None => c.editor.status = "measure a distance first".into(),
        }
    }
}

fn view_tab(ui: &mut egui::Ui, c: &mut Ctx) {
    section(ui, "View", "sidebar view", true, |ui| {
        let mut focus = from_bevy(c.orbit.focus);
        if vector(ui, "Focus", &mut focus, 1.0) {
            c.orbit.focus = to_bevy(focus);
        }
        let mut distance = c.orbit.distance as f64;
        if row(ui, "Distance", |ui| number(ui, &mut distance, 1.0, " m")) {
            c.orbit.distance = distance.clamp(2.0, 15_000.0) as f32;
        }
        let (mut yaw, mut pitch) = (
            (c.orbit.yaw as f64).to_degrees(),
            (c.orbit.pitch as f64).to_degrees(),
        );
        if row(ui, "Turn", |ui| number(ui, &mut yaw, 1.0, "°")) {
            c.orbit.yaw = (yaw.to_radians()) as f32;
        }
        if row(ui, "Tilt", |ui| number(ui, &mut pitch, 1.0, "°")) {
            c.orbit.pitch = (pitch.to_radians() as f32).clamp(-1.5695, 1.5695);
        }
        let mut ortho = c.orbit.ortho;
        if row(ui, "", |ui| {
            ui.checkbox(&mut ortho, "Orthographic").changed()
        }) {
            c.orbit.ortho = ortho;
            c.orbit.auto_ortho = false;
        }
    });
    section(ui, "Overlays", "sidebar overlays", true, |ui| {
        overlay_checks(ui, c);
    });
}

/// The overlay switches, shared with the header's popover.
pub fn overlay_checks(ui: &mut egui::Ui, c: &mut Ctx) {
    let o = &mut c.tool.overlays;
    ui.checkbox(&mut o.lines, "Lines and nodes of unselected items");
    ui.checkbox(&mut o.names, "Names");
    ui.checkbox(&mut o.indices, "Node numbers");
    ui.checkbox(&mut o.markers, "Race markers and grid");
    ui.checkbox(&mut o.stretches, "Stretches of strips and barriers");
    ui.checkbox(&mut o.props, "Props");
}
