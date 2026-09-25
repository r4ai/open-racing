//! Direct editing of the selected road's width and bank profiles, as Blender's graph
//! editor: along the road left to right (its nodes marked), the value up and down.

use bevy_egui::egui;
use open_racing_track_project::Key;
use open_racing_track_project::ops::{Curve, Op};

use crate::profile::{ProfileView, profile};
use crate::state::Editor;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Shown {
    #[default]
    Elevation,
    Left,
    Right,
    Bank,
}

impl Shown {
    fn curve(self) -> Option<Curve> {
        match self {
            Self::Elevation => None,
            Self::Left => Some(Curve::WidthLeft),
            Self::Right => Some(Curve::WidthRight),
            Self::Bank => Some(Curve::Bank),
        }
    }

    fn scale(self) -> f64 {
        if self == Self::Bank {
            180.0 / std::f64::consts::PI
        } else {
            1.0
        }
    }
}

#[derive(Clone, Copy)]
enum Part {
    Key,
    Before,
    After,
}

#[derive(Clone, Copy)]
struct Drag {
    index: usize,
    part: Part,
    original: Key,
}

#[derive(Default)]
pub struct CurveGraph {
    shown: Shown,
    road: Option<String>,
    selected: Option<usize>,
    range: (f64, f64),
    drag: Option<Drag>,
}

pub fn panel(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    elevation: &mut ProfileView,
    state: &mut CurveGraph,
) {
    ui.horizontal(|ui| {
        for (shown, label) in [
            (Shown::Elevation, "Elevation"),
            (Shown::Left, "Left width"),
            (Shown::Right, "Right width"),
            (Shown::Bank, "Bank"),
        ] {
            if ui.selectable_label(state.shown == shown, label).clicked() {
                if state.drag.take().is_some() {
                    editor.cancel_drag();
                }
                state.shown = shown;
                state.road = None;
                state.selected = None;
            }
        }
    });
    let Some(curve) = state.shown.curve() else {
        profile(ui, editor, elevation);
        return;
    };
    let Some(index) = editor
        .selection
        .road()
        .filter(|&i| i < editor.project.roads.len())
    else {
        if state.drag.take().is_some() {
            editor.cancel_drag();
        }
        ui.label("Select a road to edit its width or bank.");
        return;
    };
    let road = editor.project.roads[index].clone();
    let c = match curve {
        Curve::WidthLeft => &road.width_left,
        Curve::WidthRight => &road.width_right,
        Curve::Bank => &road.bank,
        Curve::Width => unreachable!(),
    };
    let period = road.period();
    let scale = state.shown.scale();
    let fit_clicked = ui.button("Fit").clicked();
    let fit = state.road.as_deref() != Some(&road.name) || fit_clicked;
    if fit {
        if state.drag.take().is_some() {
            editor.cancel_drag();
        }
        state.road = Some(road.name.clone());
        state.selected = None;
        let values = (0..=256)
            .map(|i| c.eval(period * i as f64 / 256.0, period, road.closed) * scale)
            .chain(c.keys.iter().flat_map(|k| {
                let du = period.max(1.0) * 0.07;
                [
                    (k.value - du * k.slope_in) * scale,
                    (k.value + du * k.slope_out) * scale,
                ]
            }));
        let (lo, hi) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
        let pad = ((hi - lo) * 0.15).max(if curve == Curve::Bank { 1.0 } else { 0.5 });
        state.range = (lo - pad, hi + pad);
    }
    let unit = if curve == Curve::Bank { "°" } else { " m" };
    ui.horizontal_wrapped(|ui| {
        ui.weak(match curve {
            Curve::WidthLeft => "Road width left of the centre line",
            Curve::WidthRight => "Road width right of the centre line",
            _ => "Bank: positive raises the right edge",
        });
        ui.weak("· along the road →, nodes marked below");
    });
    let mut remove = None;
    ui.horizontal(|ui| match state.selected.filter(|&i| i < c.keys.len()) {
        Some(i) => {
            let mut k = c.keys[i];
            ui.label(format!("Key {i}"));
            ui.label("at node");
            let mut changed = ui
                .add(
                    egui::DragValue::new(&mut k.u)
                        .speed(0.01)
                        .range(0.0..=period),
                )
                .changed();
            let mut v = k.value * scale;
            if ui
                .add(egui::DragValue::new(&mut v).speed(0.05).suffix(unit))
                .changed()
            {
                k.value = v / scale;
                changed = true;
            }
            if changed {
                let mut keys = c.keys.clone();
                keys[i] = k;
                editor.apply(
                    vec![Op::SetProfile {
                        road: road.name.clone(),
                        curve,
                        keys,
                    }],
                    Some(&format!("graph {} {curve:?} {i}", road.name)),
                );
            }
            if c.keys.len() > 1 && ui.small_button("Remove").clicked() {
                remove = Some(i);
            }
        }
        None => {
            ui.weak("Click a key to edit it.");
        }
    });
    ui.small("Double-click: add a key · drag a key or its small handles · right click a key: remove · Alt S / Ctrl T in the view: width / bank at the selected nodes");
    let size = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        egui::vec2(size.x, size.y.max(80.0)),
        egui::Sense::click_and_drag(),
    );
    let rect = resp.rect.shrink2(egui::vec2(12.0, 10.0));
    let rect = egui::Rect::from_min_max(rect.min, rect.max - egui::vec2(0.0, 6.0));
    painter.rect_filled(resp.rect, 4.0, egui::Color32::from_gray(24));
    let pad_u = period.max(1.0) * 0.08;
    let x = |u: f64| rect.left() + ((u + pad_u) / (period + 2.0 * pad_u)) as f32 * rect.width();
    let y = |v: f64| {
        rect.bottom()
            - ((v - state.range.0) / (state.range.1 - state.range.0)) as f32 * rect.height()
    };
    let u_at =
        |px: f32| (px - rect.left()) as f64 / rect.width() as f64 * (period + 2.0 * pad_u) - pad_u;
    let painter = painter.with_clip_rect(resp.rect);
    painter.text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        format!("{:.1}", state.range.1),
        egui::FontId::monospace(10.0),
        egui::Color32::GRAY,
    );
    painter.text(
        rect.left_bottom(),
        egui::Align2::LEFT_BOTTOM,
        format!("{:.1}", state.range.0),
        egui::FontId::monospace(10.0),
        egui::Color32::GRAY,
    );
    painter.text(
        rect.right_top(),
        egui::Align2::RIGHT_TOP,
        format!("{} nodes", road.nodes.len()),
        egui::FontId::monospace(10.0),
        egui::Color32::GRAY,
    );
    // The nodes along the road, the selected ones lit.
    for n in 0..=period.round() as usize {
        let px = x(n as f64);
        let lit = editor.selection.nodes.contains(&n)
            || (road.closed && n as f64 >= period && editor.selection.nodes.contains(&0));
        let color = if lit {
            egui::Color32::from_rgb(255, 160, 40)
        } else {
            egui::Color32::from_gray(60)
        };
        painter.line_segment(
            [egui::pos2(px, rect.top()), egui::pos2(px, rect.bottom())],
            egui::Stroke::new(1.0, color),
        );
        let label = if road.closed && n as f64 >= period {
            0
        } else {
            n
        };
        painter.text(
            egui::pos2(px, rect.bottom() + 1.0),
            egui::Align2::CENTER_TOP,
            label.to_string(),
            egui::FontId::monospace(9.0),
            if lit { color } else { egui::Color32::GRAY },
        );
    }
    let line = (0..=256)
        .map(|i| {
            let u = period * i as f64 / 256.0;
            egui::pos2(x(u), y(c.eval(u, period, road.closed) * scale))
        })
        .collect();
    painter.add(egui::Shape::line(
        line,
        egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
    ));

    let handle_u = |i: usize, before: bool| -> f64 {
        let key = c.keys[i].u;
        let neighbor = if before {
            if i > 0 {
                c.keys[i - 1].u
            } else if road.closed {
                c.keys[c.keys.len() - 1].u - period
            } else {
                key - period
            }
        } else if i + 1 < c.keys.len() {
            c.keys[i + 1].u
        } else if road.closed {
            c.keys[0].u + period
        } else {
            key + period
        };
        let distance = (neighbor - key).abs();
        let du = (distance / 3.0).min(period.max(1.0) * 0.07).max(0.02);
        key + if before { -du } else { du }
    };
    let point = |i: usize, part: Part| -> egui::Pos2 {
        let k = c.keys[i];
        match part {
            Part::Key => egui::pos2(x(k.u), y(k.value * scale)),
            Part::Before => {
                let hu = handle_u(i, true);
                egui::pos2(x(hu), y((k.value + (hu - k.u) * k.slope_in) * scale))
            }
            Part::After => {
                let hu = handle_u(i, false);
                egui::pos2(x(hu), y((k.value + (hu - k.u) * k.slope_out) * scale))
            }
        }
    };
    for i in 0..c.keys.len() {
        let p = point(i, Part::Key);
        let color = if state.selected == Some(i) {
            egui::Color32::YELLOW
        } else {
            egui::Color32::LIGHT_BLUE
        };
        painter.circle_filled(p, 5.0, color);
        if c.keys.len() > 1 {
            for part in [Part::Before, Part::After] {
                if !road.closed
                    && (i == 0 && matches!(part, Part::Before)
                        || i + 1 == c.keys.len() && matches!(part, Part::After))
                {
                    continue;
                }
                let h = point(i, part);
                painter.line_segment([p, h], egui::Stroke::new(1.0, color));
                painter.circle_filled(h, 3.5, color);
            }
        }
    }

    let nearest = |p: egui::Pos2| -> Option<(usize, Part)> {
        (0..c.keys.len())
            .flat_map(|i| {
                [Part::Key, Part::Before, Part::After]
                    .into_iter()
                    .filter(move |part| match part {
                        Part::Key => true,
                        Part::Before => c.keys.len() > 1 && (road.closed || i > 0),
                        Part::After => c.keys.len() > 1 && (road.closed || i + 1 < c.keys.len()),
                    })
                    .map(move |part| (i, part))
            })
            .map(|(i, part)| (i, part, point(i, part).distance(p)))
            .filter(|(_, _, distance)| *distance < 10.0)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(i, part, _)| (i, part))
    };
    if resp.secondary_clicked()
        && c.keys.len() > 1
        && let Some((i, _)) = resp.interact_pointer_pos().and_then(nearest)
    {
        remove = Some(i);
    }
    if let Some(i) = remove {
        let mut keys = c.keys.clone();
        keys.remove(i);
        editor.apply(
            vec![Op::SetProfile {
                road: road.name.clone(),
                curve,
                keys,
            }],
            None,
        );
        state.selected = None;
        return;
    }
    if resp.clicked() {
        state.selected = resp
            .interact_pointer_pos()
            .and_then(nearest)
            .map(|(i, _)| i);
    }
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some((i, part)) = resp.interact_pointer_pos().and_then(nearest)
    {
        state.selected = Some(i);
        state.drag = Some(Drag {
            index: i,
            part,
            original: c.keys[i],
        });
        editor.begin_drag();
    }
    if let Some(drag) = state.drag {
        let mut keys = c.keys.clone();
        let delta = resp.drag_delta();
        let du = delta.x as f64 / rect.width() as f64 * (period + 2.0 * pad_u);
        let dv = -delta.y as f64 / rect.height() as f64 * (state.range.1 - state.range.0) / scale;
        let key = &mut keys[drag.index];
        match drag.part {
            Part::Key => {
                let low = if drag.index > 0 {
                    c.keys[drag.index - 1].u + 1e-4
                } else {
                    0.0
                };
                let high = if drag.index + 1 < c.keys.len() {
                    c.keys[drag.index + 1].u - 1e-4
                } else {
                    period
                };
                key.u = (drag.original.u + du).clamp(low, high.max(low));
                key.value = (drag.original.value + dv).max(if curve == Curve::Bank {
                    -f64::MAX
                } else {
                    0.1
                });
            }
            Part::Before => {
                let span = drag.original.u - handle_u(drag.index, true);
                key.slope_in = drag.original.slope_in - dv / span;
            }
            Part::After => {
                let span = handle_u(drag.index, false) - drag.original.u;
                key.slope_out = drag.original.slope_out + dv / span;
            }
        }
        if keys != c.keys {
            editor.apply(
                vec![Op::SetProfile {
                    road: road.name.clone(),
                    curve,
                    keys,
                }],
                None,
            );
        }
        if resp.drag_stopped() {
            state.drag = None;
            editor.end_drag();
        } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.drag = None;
            editor.cancel_drag();
        }
    }
    if resp.double_clicked()
        && state.drag.is_none()
        && let Some(p) = resp.interact_pointer_pos()
        && nearest(p).is_none()
    {
        let u = u_at(p.x).clamp(0.0, if road.closed { period - 1e-4 } else { period });
        // Near a node, on it.
        let u = if (u - u.round()).abs() * (rect.width() as f64 / period.max(1.0)) < 8.0 {
            u.round()
                .min(if road.closed { period - 1.0 } else { period })
        } else {
            u
        };
        if c.keys.iter().all(|k| (k.u - u).abs() > 1e-4) {
            // On the line: keep the value there; elsewhere, where it was clicked.
            let on_line = c.eval(u, period, road.closed);
            let value = if (y(on_line * scale) - p.y).abs() < 10.0 {
                on_line
            } else {
                let v = state.range.1
                    - (p.y - rect.top()) as f64 / rect.height() as f64
                        * (state.range.1 - state.range.0);
                let v = v / scale;
                if curve == Curve::Bank { v } else { v.max(0.1) }
            };
            let mut keys = c.keys.clone();
            keys.push(Key::new(u, value));
            editor.apply(
                vec![Op::SetProfile {
                    road: road.name.clone(),
                    curve,
                    keys,
                }],
                None,
            );
        }
    }
}
