//! Direct editing of the selected road's width and bank profiles.

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
    if let Some(i) = state.selected.filter(|&i| i < c.keys.len()) {
        ui.horizontal(|ui| {
            let k = c.keys[i];
            ui.label(format!(
                "key {i}: u {:.2}, value {:.2}, before {:.2}/u, after {:.2}/u",
                k.u,
                k.value * scale,
                k.slope_in * scale,
                k.slope_out * scale
            ));
            if c.keys.len() > 1 && ui.small_button("Remove").clicked() {
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
            }
        });
    }
    ui.small("Drag a key to move it; drag either small handle to change its slope. Double-click the line to add a key.");
    let size = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        egui::vec2(size.x, size.y.max(80.0)),
        egui::Sense::click_and_drag(),
    );
    let rect = resp.rect.shrink2(egui::vec2(12.0, 10.0));
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
        rect.right_bottom(),
        egui::Align2::RIGHT_BOTTOM,
        format!("u {period:.1}"),
        egui::FontId::monospace(10.0),
        egui::Color32::GRAY,
    );
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
        if (y(c.eval(u, period, road.closed) * scale) - p.y).abs() < 10.0
            && c.keys.iter().all(|k| (k.u - u).abs() > 1e-4)
        {
            let mut keys = c.keys.clone();
            keys.push(Key::new(u, c.eval(u, period, road.closed)));
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
