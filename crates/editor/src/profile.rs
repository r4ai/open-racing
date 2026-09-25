//! The selected road's elevation along its length. The view keeps its scale until asked
//! to fit, so a node dragged up or down follows the pointer instead of the axes moving
//! under it. Nodes move by how far the pointer moves, not by where it is.

use bevy_egui::egui;
use open_racing_track_project::curve::Sampled;
use open_racing_track_project::ops::Op;

use crate::state::{Editor, Item};

/// Smallest height span a fit shows, m.
const MIN_SPAN: f64 = 10.0;
/// Pointer distance within which a node is grabbed, px.
const GRAB: f32 = 12.0;

/// What part of the profile is shown, and the node being dragged.
#[derive(Default)]
pub struct ProfileView {
    /// Road the view was fitted to; another road fits again.
    road: Option<String>,
    /// Shown distance and height ranges, m.
    s: (f64, f64),
    z: (f64, f64),
    /// Keep one metre of height as long on screen as one metre of distance.
    true_scale: bool,
    drag: Option<NodeDrag>,
}

struct NodeDrag {
    /// The road and node dragged.
    road: String,
    node: usize,
    /// Started with G: follows the pointer without a button held.
    modal: bool,
    /// Height the drag has reached, before snapping.
    z: f64,
}

impl ProfileView {
    /// Puts back a node being dragged, when the view goes away or shows another road.
    pub fn stop(&mut self, editor: &mut Editor) {
        if self.drag.take().is_some() {
            editor.cancel_drag();
        }
    }

    fn fit(&mut self, smp: &Sampled, nodes: impl Iterator<Item = f64>) {
        let (lo, hi) = smp
            .frames
            .iter()
            .map(|f| f.pos.z)
            .chain(nodes)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), z| {
                (a.min(z), b.max(z))
            });
        let mid = 0.5 * (lo + hi);
        let half = (0.5 * (hi - lo) + 2.0).max(0.5 * MIN_SPAN);
        self.s = (0.0, smp.length.max(1.0));
        self.z = (mid - half, mid + half);
    }
}

pub fn profile(ui: &mut egui::Ui, editor: &mut Editor, view: &mut ProfileView) {
    let Some(r) = editor
        .selection
        .road()
        .filter(|&r| r < editor.project.roads.len())
    else {
        view.stop(editor);
        ui.label("Select a road to see its elevation.");
        return;
    };
    let road = editor.project.roads[r].clone();
    if view
        .drag
        .as_ref()
        .is_some_and(|d| d.road != road.name || d.node >= road.nodes.len())
    {
        view.stop(editor);
    }
    if road.nodes.len() < 2 {
        return;
    }
    // Sampled from the project as it is now, so the curve and the nodes agree while
    // dragging.
    let smp = Sampled::new(&road, road.resolution.max(1.0));
    if smp.frames.is_empty() {
        return;
    }
    let node_s: Vec<f64> = (0..road.nodes.len()).map(|i| smp.s_at(i as f64)).collect();
    let mut fit = view.road.as_deref() != Some(road.name.as_str());

    ui.horizontal(|ui| {
        ui.strong(format!(
            "Elevation of \"{}\" ({:.0} m)",
            road.name, smp.length
        ));
        fit |= ui
            .button("Fit")
            .on_hover_text("Home, or double-click the background")
            .clicked();
        ui.checkbox(&mut view.true_scale, "1:1");
        ui.weak("controls").on_hover_text(
            "Drag a node, or G over the view, to move the selected one up or down\n\
             Shift: fine · Ctrl: snap to 0.1 m · click or Enter: done · right click or Esc: undo\n\
             Double-click the line: add a node there\n\
             Middle drag: pan · wheel: zoom along the road · Ctrl+wheel: zoom the height\n\
             Home or double-click the background: fit",
        );
    });
    if fit {
        view.road = Some(road.name.clone());
        view.fit(&smp, road.nodes.iter().map(|n| n.pos.z));
    }

    let size = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        egui::vec2(size.x, size.y.max(60.0)),
        egui::Sense::click_and_drag(),
    );
    let rect = resp.rect.shrink(8.0);
    painter.rect_filled(resp.rect, 4.0, egui::Color32::from_gray(24));
    if view.true_scale {
        let mid = 0.5 * (view.z.0 + view.z.1);
        let half = 0.5 * (view.s.1 - view.s.0) * (rect.height() / rect.width()) as f64;
        view.z = (mid - half, mid + half);
    }

    // Zooming about the pointer.
    if let Some(hover) = resp.hover_pos() {
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let fs = ((hover.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
        let fz = ((rect.bottom() - hover.y) / rect.height()).clamp(0.0, 1.0) as f64;
        let scale = |(a, b): (f64, f64), f: f64, k: f64| {
            let at = a + (b - a) * f;
            (at - (at - a) * k, at + (b - at) * k)
        };
        if zoom != 1.0 && !view.true_scale {
            view.z = scale(view.z, fz, 1.0 / zoom as f64);
        } else if scroll != 0.0 {
            let k = (-scroll as f64 * 0.002).exp();
            view.s = scale(view.s, fs, k);
            if view.true_scale {
                view.z = scale(view.z, fz, k);
            }
        }
    }

    let (s0, s1, z0, z1) = (view.s.0, view.s.1, view.z.0, view.z.1);
    let x = |s: f64| rect.left() + ((s - s0) / (s1 - s0)) as f32 * rect.width();
    let y = |z: f64| rect.bottom() - ((z - z0) / (z1 - z0)) as f32 * rect.height();
    let s_at = |px: f32| s0 + ((px - rect.left()) / rect.width()) as f64 * (s1 - s0);
    let z_at = |py: f32| z0 + ((rect.bottom() - py) / rect.height()) as f64 * (z1 - z0);
    let painter = painter.with_clip_rect(resp.rect);

    // Height lines at a round step.
    let step = nice_step((z1 - z0) / 6.0);
    let mut z = (z0 / step).ceil() * step;
    while z < z1 {
        painter.hline(
            rect.x_range(),
            y(z),
            egui::Stroke::new(1.0, egui::Color32::from_gray(45)),
        );
        painter.text(
            egui::pos2(rect.left(), y(z)),
            egui::Align2::LEFT_BOTTOM,
            format!("{z:.1} m"),
            egui::FontId::monospace(10.0),
            egui::Color32::from_gray(110),
        );
        z += step;
    }

    let line: Vec<egui::Pos2> = smp
        .frames
        .iter()
        .map(|f| egui::pos2(x(f.s), y(f.pos.z)))
        .chain(
            smp.closed
                .then(|| egui::pos2(x(smp.length), y(smp.frames[0].pos.z))),
        )
        .collect();
    painter.add(egui::Shape::line(
        line,
        egui::Stroke::new(2.0, egui::Color32::from_rgb(90, 200, 255)),
    ));

    let nodes: Vec<egui::Pos2> = road
        .nodes
        .iter()
        .zip(&node_s)
        .map(|(n, &s)| egui::pos2(x(s), y(n.pos.z)))
        .collect();
    // Grade between neighbouring nodes.
    let segs = road.segments();
    for i in 0..segs {
        let j = (i + 1) % road.nodes.len();
        let ds = if j == 0 {
            smp.length - node_s[i]
        } else {
            node_s[j] - node_s[i]
        };
        if ds <= 0.0 {
            continue;
        }
        let grade = 100.0 * (road.nodes[j].pos.z - road.nodes[i].pos.z) / ds;
        let end = if j == 0 {
            egui::pos2(x(smp.length), nodes[0].y)
        } else {
            nodes[j]
        };
        let mid = nodes[i].lerp(end, 0.5);
        painter.text(
            mid + egui::vec2(0.0, 4.0),
            egui::Align2::CENTER_TOP,
            format!("{grade:+.1}%"),
            egui::FontId::monospace(10.0),
            grade_color(grade),
        );
    }
    for (i, &p) in nodes.iter().enumerate() {
        let selected = editor.selection.nodes.contains(&i);
        let color = if selected {
            egui::Color32::from_rgb(255, 215, 30)
        } else {
            egui::Color32::from_rgb(90, 230, 255)
        };
        painter.circle_filled(p, if selected { 6.0 } else { 4.5 }, color);
        painter.text(
            p + egui::vec2(0.0, -9.0),
            egui::Align2::CENTER_BOTTOM,
            i.to_string(),
            egui::FontId::monospace(10.0),
            egui::Color32::WHITE,
        );
    }

    // Where the pointer is along the road.
    if let Some(hover) = resp.hover_pos() {
        let s = s_at(hover.x);
        if (0.0..=smp.length).contains(&s) {
            let f = smp.frame_at(s);
            let grade = 100.0 * f.tangent.z / f.tangent.truncate().length().max(1e-9);
            painter.vline(
                x(s),
                rect.y_range(),
                egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
            );
            painter.circle_filled(egui::pos2(x(s), y(f.pos.z)), 3.0, egui::Color32::WHITE);
            painter.text(
                rect.right_top(),
                egui::Align2::RIGHT_TOP,
                format!("s {s:.0} m  z {:.2} m  {grade:+.1}%", f.pos.z),
                egui::FontId::monospace(11.0),
                egui::Color32::from_gray(200),
            );
        }
    }

    let nearest = |pos: egui::Pos2| {
        nodes
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.distance(pos)))
            .filter(|&(_, d)| d < GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
    let key = |k| ui.input(|i| i.key_pressed(k));
    let per_px = (z1 - z0) / rect.height() as f64;
    let hovered = resp.hovered();

    // Starting a move: dragging a node, or G over the view with a node selected
    // (Blender's grab: follows the pointer until a click or Enter, Esc or a right
    // click puts it back).
    if view.drag.is_none() {
        let start = if resp.drag_started_by(egui::PointerButton::Primary) {
            resp.interact_pointer_pos()
                .and_then(nearest)
                .map(|i| (i, false))
        } else if hovered && !ui.ctx().egui_wants_keyboard_input() && key(egui::Key::G) {
            editor
                .selection
                .node()
                .filter(|&i| i < road.nodes.len())
                .map(|i| (i, true))
        } else {
            None
        };
        if let Some((node, modal)) = start {
            view.drag = Some(NodeDrag {
                road: road.name.clone(),
                node,
                z: road.nodes[node].pos.z,
                modal,
            });
            editor.selection.select_node(Item::Road(r), node);
            editor.begin_drag();
        }
    }
    if let Some(d) = &mut view.drag {
        let dy = if d.modal {
            ui.input(|i| i.pointer.delta().y)
        } else {
            resp.drag_delta().y
        };
        d.z -= dy as f64 * per_px * if shift { 0.1 } else { 1.0 };
        let z = if ctrl {
            (d.z * 10.0).round() / 10.0
        } else {
            d.z
        };
        let mut pos = road.nodes[d.node].pos;
        if editor.dragging && pos.z != z {
            pos.z = z;
            editor.apply(
                vec![Op::MoveNode {
                    line: road.name.clone(),
                    index: d.node,
                    pos,
                }],
                None,
            );
        }
        painter.text(
            rect.left_top(),
            egui::Align2::LEFT_TOP,
            format!("node {}  z {z:.2} m", d.node),
            egui::FontId::monospace(11.0),
            egui::Color32::from_rgb(255, 215, 30),
        );
        let (confirm, cancel) = if d.modal {
            ui.input(|i| {
                (
                    i.pointer.primary_pressed() || i.key_pressed(egui::Key::Enter),
                    i.pointer.secondary_pressed() || i.key_pressed(egui::Key::Escape),
                )
            })
        } else {
            (
                resp.drag_stopped(),
                ui.input(|i| i.key_pressed(egui::Key::Escape)),
            )
        };
        if cancel {
            view.drag = None;
            editor.cancel_drag();
        } else if confirm {
            view.drag = None;
            editor.end_drag();
            // Make room for a node dragged out of view.
            let z = editor.project.roads[r].nodes.iter().map(|n| n.pos.z);
            let (lo, hi) = z.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), z| {
                (a.min(z), b.max(z))
            });
            if lo < view.z.0 || hi > view.z.1 {
                let pad = 0.1 * (view.z.1 - view.z.0);
                view.z = (view.z.0.min(lo - pad), view.z.1.max(hi + pad));
            }
        }
        // The click that confirmed a grab selects nothing else.
        return;
    }

    // Panning with the middle button, fitting with Home.
    if resp.dragged_by(egui::PointerButton::Middle) {
        let delta = resp.drag_delta();
        let ds = -delta.x as f64 * (s1 - s0) / rect.width() as f64;
        let dz = delta.y as f64 * per_px;
        view.s = (s0 + ds, s1 + ds);
        view.z = (z0 + dz, z1 + dz);
    }
    if hovered && key(egui::Key::Home) {
        view.fit(&smp, road.nodes.iter().map(|n| n.pos.z));
    }
    if resp.clicked()
        && let Some(i) = resp.interact_pointer_pos().and_then(nearest)
    {
        editor.selection.select_node(Item::Road(r), i);
    }
    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && nearest(pos).is_none()
    {
        let s = s_at(pos.x);
        let near_curve =
            (0.0..=smp.length).contains(&s) && (y(smp.frame_at(s).pos.z) - pos.y).abs() < GRAB;
        if near_curve {
            // A node on the road at this distance, at the pointer's height.
            let u = smp.u_at(s);
            let mut p = smp.frame_at(s).pos;
            p.z = z_at(pos.y);
            let next = u.floor() as usize + 1;
            let before = (next < road.nodes.len()).then_some(next);
            if editor.apply(
                vec![Op::AddNode {
                    line: road.name.clone(),
                    pos: p,
                    before,
                }],
                None,
            ) {
                editor
                    .selection
                    .select_node(Item::Road(r), before.unwrap_or(road.nodes.len()));
            }
        } else {
            view.fit(&smp, road.nodes.iter().map(|n| n.pos.z));
        }
    }
}

/// 1, 2 or 5 times a power of ten, at least `x`.
fn nice_step(x: f64) -> f64 {
    let p = 10f64.powf(x.max(1e-3).log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|k| k * p)
        .find(|&v| v >= x)
        .unwrap_or(10.0 * p)
}

/// Green on the flat, through yellow to red on steep grades either way.
fn grade_color(grade: f64) -> egui::Color32 {
    let t = (grade.abs() / 12.0).clamp(0.0, 1.0) as f32;
    let g = egui::Color32::from_rgb(120, 220, 120);
    let r = egui::Color32::from_rgb(255, 110, 90);
    g.lerp_to_gamma(r, t)
}
