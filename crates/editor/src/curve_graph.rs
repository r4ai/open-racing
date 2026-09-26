//! The Curves area below the 3D view: the selected road's elevation, and its width either
//! side and its bank edited as Blender's graph editor, along the road left to right (its
//! nodes marked along the bottom), the value up and down.
//!
//! Keys are picked as nodes are in the view: a click selects one, Shift + click adds or
//! takes it out, a drag over empty space draws a box (Shift adds, Ctrl takes out), A
//! selects all and Alt A none. Dragging a key, or G, moves the selected ones (X along the
//! road only, Y the value only, Shift fine, Ctrl snapping to nodes and round values); a
//! selected key's handles set how the curve leaves it. Double-click or Ctrl + click adds a
//! key, X deletes the selected ones, and the right click opens a menu for the keys and
//! the road's nodes: the strip of node numbers along the bottom selects nodes the same
//! way, and nodes are added or deleted from here too.

use bevy_egui::egui;
use open_racing_track_project::Key;
use open_racing_track_project::ops::{Curve, Op};
use open_racing_track_project::project::Road;

use crate::commands::{Cmd, Ctx, entry};
use crate::edit;
use crate::graph::{self, Axes, Pick, Plot};
use crate::profile::{ProfileView, profile};
use crate::state::{Editor, Item};

/// Pointer distance within which a key or handle is grabbed, px.
const GRAB: f32 = 10.0;
/// Height of the strip of node numbers along the bottom, px.
const STRIP: f32 = 16.0;
/// Keys are the same key when this close along the road.
const SAME_U: f64 = 1e-6;
/// The least gap kept between keys, in `u`.
const GAP: f64 = 1e-3;

/// What the Curves area shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shown {
    #[default]
    Elevation,
    Left,
    Right,
    Bank,
}

impl Shown {
    /// The graph called `name`, as the command line names it: elevation, left, right
    /// or bank.
    pub fn named(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "elevation" => Some(Self::Elevation),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "bank" => Some(Self::Bank),
            _ => None,
        }
    }

    const ALL: [(Shown, &'static str); 4] = [
        (Shown::Elevation, "Elevation"),
        (Shown::Left, "Left width"),
        (Shown::Right, "Right width"),
        (Shown::Bank, "Bank"),
    ];

    fn curve(self) -> Option<Curve> {
        match self {
            Self::Elevation => None,
            Self::Left => Some(Curve::WidthLeft),
            Self::Right => Some(Curve::WidthRight),
            Self::Bank => Some(Curve::Bank),
        }
    }

    /// Shown value per stored value: degrees for the bank.
    fn scale(self) -> f64 {
        if self == Self::Bank {
            180.0 / std::f64::consts::PI
        } else {
            1.0
        }
    }

    fn unit(self) -> &'static str {
        if self == Self::Bank { "°" } else { " m" }
    }

    /// The step Ctrl snaps shown values to.
    fn step(self) -> f64 {
        if self == Self::Bank { 0.5 } else { 0.1 }
    }

    /// The least a stored value may be: a road is never narrower than 10 cm a side.
    fn least(self) -> f64 {
        if self == Self::Bank { f64::MIN } else { 0.1 }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Left => "Road width left of the centre line",
            Self::Right => "Road width right of the centre line",
            _ => "Bank: positive raises the right edge",
        }
    }
}

/// What of a key is grabbed: the key, or a handle on either side of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Key,
    Before,
    After,
}

/// Which way a move of keys is held (X, Y).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Lock {
    #[default]
    Free,
    /// Along the road only.
    Along,
    /// The value only.
    Value,
}

/// Keys being moved, or a key's handle.
struct Drag {
    /// The keys as they were when it began.
    keys: Vec<Key>,
    /// Those it moves, the grabbed one last.
    moved: Vec<usize>,
    part: Part,
    /// Pointer motion since it began, px (slowed while Shift is held).
    travel: egui::Vec2,
    /// Started with G: follows the pointer until a click.
    modal: bool,
    lock: Lock,
}

#[derive(Default)]
pub struct CurveGraph {
    shown: Shown,
    /// The road and curve the view was fitted to; another fits again.
    fitted: Option<(String, Shown)>,
    axes: Axes,
    /// Selected keys by where they are along the road (`u`), the active one last: a key's
    /// number changes as keys come and go, its place does not.
    selected: Vec<f64>,
    drag: Option<Drag>,
    /// A box being dragged out: where it began, and whether over the node numbers.
    boxing: Option<(egui::Pos2, bool)>,
    /// Where the menu was opened: along the road (`u`) and the value there.
    menu_at: Option<(f64, f64)>,
    /// The press that ended a grab (or cancelled it) clicks nothing when let go.
    swallow: bool,
    /// Where the graph was drawn last.
    plot: Option<Plot>,
}

impl CurveGraph {
    /// Shows another graph.
    pub fn show(&mut self, editor: &mut Editor, shown: Shown) {
        if shown != self.shown {
            self.stop(editor);
            self.shown = shown;
            self.fitted = None;
            self.selected.clear();
        }
    }

    /// Puts back keys being dragged, when the graph goes away or shows something else.
    pub fn stop(&mut self, editor: &mut Editor) {
        self.boxing = None;
        if self.drag.take().is_some() {
            editor.cancel_drag();
        }
    }
}

pub fn panel(ui: &mut egui::Ui, c: &mut Ctx, elevation: &mut ProfileView, state: &mut CurveGraph) {
    ui.horizontal(|ui| {
        for (shown, label) in Shown::ALL {
            if ui.selectable_label(state.shown == shown, label).clicked() {
                elevation.stop(c.editor);
                state.show(c.editor, shown);
            }
        }
    });
    let Some(curve) = state.shown.curve() else {
        state.stop(c.editor);
        profile(ui, c, elevation);
        return;
    };
    elevation.stop(c.editor);
    let Some((r, road)) = c.editor.road() else {
        state.stop(c.editor);
        ui.label("Select a road to edit its width or bank.");
        return;
    };
    let road = road.clone();
    keys_graph(ui, c, state, r, &road, curve);
}

/// Where a key's handle on one side is drawn along the road: a third of the way to the
/// next key, not too far.
fn handle_u(keys: &[Key], i: usize, before: bool, period: f64, closed: bool) -> f64 {
    let key = keys[i].u;
    let n = keys.len();
    let neighbour = match (before, i) {
        (true, 0) if closed => keys[n - 1].u - period,
        (true, 0) => key - period,
        (true, _) => keys[i - 1].u,
        (false, _) if i + 1 < n => keys[i + 1].u,
        (false, _) if closed => keys[0].u + period,
        (false, _) => key + period,
    };
    let du = ((neighbour - key).abs() / 3.0)
        .min(period.max(1.0) * 0.07)
        .max(0.02);
    key + if before { -du } else { du }
}

/// Whether a key has a handle on that side: an open road's end keys have none outwards.
fn has_handle(keys: &[Key], i: usize, part: Part, closed: bool) -> bool {
    keys.len() > 1
        && match part {
            Part::Key => true,
            Part::Before => closed || i > 0,
            Part::After => closed || i + 1 < keys.len(),
        }
}

/// The keys with those in `moved` shifted by `du` along the road and `dv` up (not below
/// `least`), kept in order (none passes a key not moved) and on the road.
fn shift_keys(
    keys: &[Key],
    moved: &[usize],
    du: f64,
    dv: f64,
    period: f64,
    closed: bool,
    least: f64,
) -> Vec<Key> {
    let end = if closed { period - GAP } else { period };
    let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
    for &i in moved {
        let below = (0..i)
            .rev()
            .find(|j| !moved.contains(j))
            .map_or(0.0, |j| keys[j].u + GAP);
        let above = (i + 1..keys.len())
            .find(|j| !moved.contains(j))
            .map_or(end, |j| keys[j].u - GAP);
        lo = lo.max(below - keys[i].u);
        hi = hi.min(above - keys[i].u);
    }
    let du = if lo <= hi { du.clamp(lo, hi) } else { 0.0 };
    keys.iter()
        .enumerate()
        .map(|(i, k)| {
            if moved.contains(&i) {
                Key {
                    u: k.u + du,
                    value: (k.value + dv).max(least),
                    ..*k
                }
            } else {
                *k
            }
        })
        .collect()
}

/// Slopes that carry the curve smoothly through each of `picked`, from its neighbours
/// either side (Catmull and Rom's); an open road's end keys level out.
fn smooth_slopes(keys: &[Key], picked: &[usize], period: f64, closed: bool) -> Vec<Key> {
    let n = keys.len();
    let mut out = keys.to_vec();
    for &i in picked {
        let slope = if n < 2 || (!closed && (i == 0 || i + 1 == n)) {
            0.0
        } else {
            let (p, pu) = if i == 0 {
                (keys[n - 1], keys[n - 1].u - period)
            } else {
                (keys[i - 1], keys[i - 1].u)
            };
            let (q, qu) = if i + 1 == n {
                (keys[0], keys[0].u + period)
            } else {
                (keys[i + 1], keys[i + 1].u)
            };
            (q.value - p.value) / (qu - pu).max(1e-9)
        };
        out[i].slope_in = slope;
        out[i].slope_out = slope;
    }
    out
}

/// The keys as a drag puts them: the moved ones by how far the pointer went (with Ctrl
/// the grabbed one onto a node and a round value), or the grabbed key's slope on the
/// handle's side (both sides while they are aligned, unless Alt breaks them).
fn dragged(
    d: &Drag,
    plot: &Plot,
    shown: Shown,
    period: f64,
    closed: bool,
    modifiers: egui::Modifiers,
) -> Vec<Key> {
    let scale = shown.scale();
    let (mut du, mut dv) = plot.delta(d.travel);
    match d.lock {
        Lock::Along => dv = 0.0,
        Lock::Value => du = 0.0,
        Lock::Free => {}
    }
    let grabbed = *d.moved.last().expect("a key is grabbed");
    let k0 = d.keys[grabbed];
    if d.part == Part::Key {
        if modifiers.command {
            if d.lock != Lock::Value {
                du = (k0.u + du).round() - k0.u;
            }
            if d.lock != Lock::Along {
                let step = shown.step();
                let v = k0.value * scale + dv;
                dv = (v / step).round() * step - k0.value * scale;
            }
        }
        return shift_keys(
            &d.keys,
            &d.moved,
            du,
            dv / scale,
            period,
            closed,
            shown.least(),
        );
    }
    let before = d.part == Part::Before;
    let span = (handle_u(&d.keys, grabbed, before, period, closed) - k0.u).abs();
    let slope = if before {
        k0.slope_in - dv / scale / span
    } else {
        k0.slope_out + dv / scale / span
    };
    let mut out = d.keys.clone();
    let k = &mut out[grabbed];
    let aligned = k0.slope_in == k0.slope_out && !modifiers.alt;
    if before || aligned {
        k.slope_in = slope;
    }
    if !before || aligned {
        k.slope_out = slope;
    }
    out
}

/// What the buttons, keys and menu ask of the keys and nodes.
enum Action {
    Keys(Vec<Key>, &'static str),
    Delete,
    AddKey(f64, f64),
    AddNode(f64),
    DeleteNodes,
    KeysAtNodes,
    Fit,
}

fn keys_graph(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    state: &mut CurveGraph,
    r: usize,
    road: &Road,
    curve: Curve,
) {
    let shown = state.shown;
    let scale = shown.scale();
    let unit = shown.unit();
    let (period, closed) = (road.period(), road.closed);
    let profile = edit::profile(road, curve);
    let keys = profile.keys.clone();
    let at = |u: f64| profile.eval(u, period, closed);
    // Another road or curve: what was being done to the last one stops, and its keys
    // are not this one's.
    if state.fitted.as_ref() != Some(&(road.name.clone(), shown)) {
        state.stop(c.editor);
        state.selected.clear();
    }
    let find = |u: f64| keys.iter().position(|k| (k.u - u).abs() < SAME_U);
    // Keys that went (undone, changed on disk) drop out of the selection.
    if state.drag.is_none() {
        state.selected.retain(|&u| find(u).is_some());
    }
    let picked: Vec<usize> = state.selected.iter().filter_map(|&u| find(u)).collect();
    let active = picked.last().copied();
    let nodes_selected: Vec<usize> = if c.editor.selection.item == Some(Item::Road(r)) {
        c.editor.selection.nodes.clone()
    } else {
        vec![]
    };
    let mut action = None;

    // The tool row: the selected keys' fields and what can be done to them.
    let mut fit = state.fitted.as_ref() != Some(&(road.name.clone(), shown));
    ui.horizontal(|ui| {
        if ui.button("Fit").on_hover_text("Home over the graph").clicked() {
            action = Some(Action::Fit);
        }
        ui.separator();
        match (picked.as_slice(), active) {
            ([i], _) => {
                let mut k = keys[*i];
                ui.label(format!("Key {i} at node"));
                let lo = if *i > 0 { keys[i - 1].u + GAP } else { 0.0 };
                let hi = match keys.get(i + 1) {
                    Some(next) => next.u - GAP,
                    None if closed => period - GAP,
                    None => period,
                };
                let mut changed = ui
                    .add(
                        egui::DragValue::new(&mut k.u)
                            .speed(0.01)
                            .range(lo..=hi.max(lo)),
                    )
                    .changed();
                let mut v = k.value * scale;
                if ui
                    .add(egui::DragValue::new(&mut v).speed(0.05).suffix(unit))
                    .changed()
                {
                    k.value = (v / scale).max(shown.least());
                    changed = true;
                }
                if changed {
                    let mut out = keys.clone();
                    out[*i] = k;
                    state.selected = vec![k.u];
                    action = Some(Action::Keys(out, "key"));
                }
            }
            (many, Some(a)) => {
                ui.label(format!("{} keys", many.len()));
                let mut v = keys[a].value * scale;
                if ui
                    .add(egui::DragValue::new(&mut v).speed(0.05).suffix(unit))
                    .on_hover_text("The value of every selected key")
                    .changed()
                {
                    let mut out = keys.clone();
                    for &i in many {
                        out[i].value = (v / scale).max(shown.least());
                    }
                    action = Some(Action::Keys(out, "keys"));
                }
            }
            _ => {
                ui.weak("Click a key to edit it, double-click to add one");
            }
        }
        if !picked.is_empty() {
            if ui
                .small_button("Delete")
                .on_hover_text("Delete the selected keys (X)")
                .clicked()
            {
                action = Some(Action::Delete);
            }
            if ui
                .small_button("Smooth")
                .on_hover_text("Handles that carry the curve smoothly through the selected keys")
                .clicked()
            {
                action = Some(Action::Keys(
                    smooth_slopes(&keys, &picked, period, closed),
                    "smooth",
                ));
            }
            if ui
                .small_button("Flat")
                .on_hover_text("Level handles: the curve eases in and out of the selected keys")
                .clicked()
            {
                let mut out = keys.clone();
                for &i in &picked {
                    out[i].slope_in = 0.0;
                    out[i].slope_out = 0.0;
                }
                action = Some(Action::Keys(out, "flat"));
            }
        }
        ui.separator();
        if ui
            .add_enabled(!nodes_selected.is_empty(), egui::Button::new("Keys at nodes").small())
            .on_hover_text("A key at each selected node, holding the value there")
            .on_disabled_hover_text("Select nodes (click their numbers along the bottom, or in the view)")
            .clicked()
        {
            action = Some(Action::KeysAtNodes);
        }
        ui.weak("controls").on_hover_text(format!(
            "{}, along the road left to right with its nodes numbered below\n\
             Click: select a key · Shift: add or take out · drag over empty space: box (Shift adds, Ctrl takes out)\n\
             Drag a key or G: move the selected (X along only, Y value only, Shift fine, Ctrl snap) · a selected key's handles: its slope (Alt: one side)\n\
             Double-click or Ctrl + click: add a key · X or Delete: delete keys · A: all · Alt A: none\n\
             The numbers along the bottom select the road's nodes; double-click there adds one\n\
             Right click: keys and nodes menu · wheel: zoom along · Ctrl + wheel: zoom up and down · middle drag: pan · Home: fit",
            shown.describe()
        ));
    });

    // The graph.
    let size = ui.available_size();
    let (resp, painter) = ui.allocate_painter(
        egui::vec2(size.x, size.y.max(80.0)),
        egui::Sense::click_and_drag(),
    );
    let inner = resp.rect.shrink2(egui::vec2(8.0, 6.0));
    let strip =
        egui::Rect::from_min_max(egui::pos2(inner.left(), inner.bottom() - STRIP), inner.max);
    let rect = egui::Rect::from_min_max(inner.min, egui::pos2(inner.right(), strip.top()));
    let hovered = resp.hovered() || state.drag.is_some();
    let key = |k, m| graph::key(ui, hovered, k, m);
    if matches!(action, Some(Action::Fit)) || key(egui::Key::Home, egui::Modifiers::NONE) {
        fit = true;
        action = None;
    }
    if fit {
        state.fitted = Some((road.name.clone(), shown));
        let pad = period.max(1.0) * 0.03;
        let values = (0..=256)
            .map(|i| at(period * i as f64 / 256.0) * scale)
            .chain(keys.iter().map(|k| k.value * scale));
        state.axes = Axes::fit(
            (-pad, period + pad),
            values,
            0.2,
            if shown == Shown::Bank { 2.0 } else { 1.0 },
        );
    }
    let mut plot = Plot {
        rect,
        axes: state.axes,
    };
    if state.drag.is_none() {
        graph::navigate(ui, &resp, &plot, &mut state.axes, false);
        plot.axes = state.axes;
    }
    state.plot = Some(plot);

    // Drawing: the road's stretch lighter, the grid, the nodes, the curve and its keys.
    let painter = painter.with_clip_rect(resp.rect);
    painter.rect_filled(resp.rect, 4.0, egui::Color32::from_gray(20));
    let road_rect = egui::Rect::from_x_y_ranges(
        plot.x(0.0).max(rect.left())..=plot.x(period).min(rect.right()),
        rect.y_range(),
    );
    painter.rect_filled(road_rect, 0.0, egui::Color32::from_gray(27));
    graph::grid(&painter, &plot, unit);
    let ticks = (0..road.nodes.len())
        .map(|n| (n, n as f64))
        .chain(closed.then_some((0, period)));
    graph::node_ticks(&painter, &plot, strip, ticks, &nodes_selected);
    let (x0, x1) = (plot.axes.x.0.max(0.0), plot.axes.x.1.min(period));
    if x1 > x0 {
        let line = (0..=400)
            .map(|i| {
                let u = x0 + (x1 - x0) * i as f64 / 400.0;
                plot.pos(u, at(u) * scale)
            })
            .collect();
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
        ));
    }
    let point = |keys: &[Key], i: usize, part: Part| -> egui::Pos2 {
        let k = keys[i];
        let hu = match part {
            Part::Key => return plot.pos(k.u, k.value * scale),
            Part::Before => handle_u(keys, i, true, period, closed),
            Part::After => handle_u(keys, i, false, period, closed),
        };
        let slope = if part == Part::Before {
            k.slope_in
        } else {
            k.slope_out
        };
        plot.pos(hu, (k.value + (hu - k.u) * slope) * scale)
    };
    for (i, k) in keys.iter().enumerate() {
        let selected = picked.contains(&i);
        let color = graph::point_color(selected, active == Some(i), egui::Color32::LIGHT_BLUE);
        let p = plot.pos(k.u, k.value * scale);
        if selected {
            for part in [Part::Before, Part::After] {
                if has_handle(&keys, i, part, closed) {
                    let h = point(&keys, i, part);
                    painter.line_segment([p, h], egui::Stroke::new(1.0, color));
                    painter.circle_filled(h, 3.5, color);
                }
            }
        }
        painter.circle_filled(p, if selected { 5.5 } else { 4.5 }, color);
        painter.circle_stroke(p, 5.5, egui::Stroke::new(1.0, egui::Color32::from_gray(20)));
    }
    // A key under the pointer, or a selected key's handle; a key wins a near tie.
    let hit = |p: egui::Pos2| -> Option<(usize, Part)> {
        let mut best: Option<(usize, Part, f32)> = None;
        for i in 0..keys.len() {
            for part in [Part::Key, Part::Before, Part::After] {
                if (part != Part::Key && !picked.contains(&i))
                    || !has_handle(&keys, i, part, closed)
                {
                    continue;
                }
                let d =
                    point(&keys, i, part).distance(p) + if part == Part::Key { 0.0 } else { 2.0 };
                if d < GRAB && best.is_none_or(|b| d < b.2) {
                    best = Some((i, part, d));
                }
            }
        }
        best.map(|(i, part, _)| (i, part))
    };
    // The node whose number or line is under the pointer, in the strip.
    let node_at = |p: egui::Pos2| -> Option<usize> {
        (0..road.nodes.len())
            .map(|n| (n, (plot.x(n as f64) - p.x).abs()))
            .chain(closed.then(|| (0, (plot.x(period) - p.x).abs())))
            .filter(|&(_, d)| d < 8.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(n, _)| n)
    };
    // Where the pointer is, along the road and up.
    let readout = |text: String| {
        painter.text(
            rect.right_top() + egui::vec2(-2.0, 2.0),
            egui::Align2::RIGHT_TOP,
            text,
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(200),
        );
    };
    let modifiers = ui.input(|i| i.modifiers);

    // Keys being moved, or a handle.
    if let Some(mut d) = state.drag.take() {
        let motion = if d.modal {
            ui.input(|i| i.pointer.delta())
        } else {
            resp.drag_delta()
        };
        d.travel += motion * if modifiers.shift { 0.1 } else { 1.0 };
        for (k, lock) in [(egui::Key::X, Lock::Along), (egui::Key::Y, Lock::Value)] {
            if ui.input(|i| i.key_pressed(k)) {
                d.lock = if d.lock == lock { Lock::Free } else { lock };
            }
        }
        let out = dragged(&d, &plot, shown, period, closed, modifiers);
        let grabbed = *d.moved.last().expect("a key is grabbed");
        let (k0, k) = (d.keys[grabbed], out[grabbed]);
        readout(match d.part {
            Part::Key => format!(
                "{}  {:+.2} nodes  {:+.2}{unit}{}",
                if d.moved.len() > 1 {
                    format!("{} keys", d.moved.len())
                } else {
                    format!("node {:.2}", k.u)
                },
                k.u - k0.u,
                (k.value - k0.value) * scale,
                match d.lock {
                    Lock::Along => "  (along only)",
                    Lock::Value => "  (value only)",
                    Lock::Free => "",
                }
            ),
            Part::Before => format!("slope in {:+.3}{unit} per node", k.slope_in * scale),
            Part::After => format!("slope out {:+.3}{unit} per node", k.slope_out * scale),
        });
        if out != keys {
            c.editor.apply(
                vec![Op::SetProfile {
                    road: road.name.clone(),
                    curve,
                    keys: out.clone(),
                }],
                None,
            );
        }
        // The moved keys stay selected where they went.
        state.selected = d.moved.iter().map(|&i| out[i].u).collect();
        let (confirm, cancel) = if d.modal {
            ui.input(|i| {
                (
                    i.pointer.primary_pressed() || i.key_pressed(egui::Key::Enter),
                    i.pointer.secondary_pressed() || i.key_pressed(egui::Key::Escape),
                )
            })
        } else {
            (
                resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()),
                ui.input(|i| i.key_pressed(egui::Key::Escape)),
            )
        };
        if cancel {
            state.selected = d.moved.iter().map(|&i| d.keys[i].u).collect();
            state.swallow = true;
            c.editor.cancel_drag();
        } else if confirm {
            state.swallow = d.modal;
            c.editor.end_drag();
            // Room for keys dragged out of sight.
            state.axes.include(out.iter().map(|k| k.value * scale));
        } else {
            state.drag = Some(d);
        }
        return;
    }

    if let Some(p) = resp.hover_pos()
        && rect.contains(p)
    {
        let u = plot.x_at(p.x);
        if (0.0..=period).contains(&u) {
            readout(format!("node {u:.2}  {:.2}{unit}", at(u) * scale));
        }
    }

    // A press that ended a grab clicks nothing.
    if state.swallow {
        if !ui.input(|i| i.pointer.any_down()) && !resp.clicked() && !resp.secondary_clicked() {
            state.swallow = false;
        }
        if resp.clicked() || resp.secondary_clicked() {
            state.swallow = false;
            return;
        }
    }

    // The right click's menu: for the keys, and the road's nodes.
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let u = plot.x_at(p.x).clamp(0.0, period);
        state.menu_at = Some((u, plot.y_at(p.y) / scale));
        if let Some((i, _)) = hit(p)
            && !picked.contains(&i)
        {
            state.selected = vec![keys[i].u];
        }
    }
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        let (u, v) = state.menu_at.unwrap_or((0.0, 0.0));
        ui.weak(format!("Keys: {}", shown.describe()));
        if ui.button("Add Key Here").clicked() {
            action = Some(Action::AddKey(u, v));
            ui.close();
        }
        if ui
            .add_enabled(
                !picked.is_empty(),
                egui::Button::new("Delete Keys").shortcut_text("X"),
            )
            .clicked()
        {
            action = Some(Action::Delete);
            ui.close();
        }
        if ui
            .add_enabled(!picked.is_empty(), egui::Button::new("Smooth Handles"))
            .clicked()
        {
            action = Some(Action::Keys(
                smooth_slopes(&keys, &picked, period, closed),
                "smooth",
            ));
            ui.close();
        }
        if ui
            .add_enabled(!picked.is_empty(), egui::Button::new("Flat Handles"))
            .clicked()
        {
            let mut out = keys.clone();
            for &i in &picked {
                out[i].slope_in = 0.0;
                out[i].slope_out = 0.0;
            }
            action = Some(Action::Keys(out, "flat"));
            ui.close();
        }
        if ui
            .add_enabled(
                !nodes_selected.is_empty(),
                egui::Button::new("Keys at Selected Nodes"),
            )
            .clicked()
        {
            action = Some(Action::KeysAtNodes);
            ui.close();
        }
        if ui
            .add(egui::Button::new("Select All Keys").shortcut_text("A"))
            .clicked()
        {
            state.selected = keys.iter().map(|k| k.u).collect();
            ui.close();
        }
        if ui
            .add(egui::Button::new("Select No Keys").shortcut_text("Alt A"))
            .clicked()
        {
            state.selected.clear();
            ui.close();
        }
        ui.separator();
        ui.weak("Road nodes");
        if ui.button("Add Node Here").clicked() {
            action = Some(Action::AddNode(u));
            ui.close();
        }
        if ui
            .add_enabled(
                !nodes_selected.is_empty(),
                egui::Button::new(format!("Delete {} Selected Nodes", nodes_selected.len())),
            )
            .clicked()
        {
            action = Some(Action::DeleteNodes);
            ui.close();
        }
        entry(ui, c, Cmd::Subdivide);
        entry(ui, c, Cmd::SelectMore);
        entry(ui, c, Cmd::SelectLess);
        ui.separator();
        if ui
            .add(egui::Button::new("Fit").shortcut_text("Home"))
            .clicked()
        {
            action = Some(Action::Fit);
            ui.close();
        }
    });

    // The left button: a drag moves what it began on or draws a box, a click selects.
    let how = Pick::of(modifiers);
    // Not while the view is moving something.
    let busy = c.editor.dragging();
    if resp.drag_started_by(egui::PointerButton::Primary)
        && !busy
        && let Some(origin) = graph::press_origin(ui, &resp)
    {
        match hit(origin).filter(|_| !strip.contains(origin)) {
            Some((i, part)) => {
                // Grabbing a key not selected selects it alone (Shift: with the rest).
                if !picked.contains(&i) {
                    if modifiers.shift {
                        state.selected.push(keys[i].u);
                    } else {
                        state.selected = vec![keys[i].u];
                    }
                } else if part == Part::Key {
                    // The grabbed key becomes the active one.
                    graph::pick(&mut state.selected, &[keys[i].u], Pick::Add);
                }
                let mut moved: Vec<usize> =
                    state.selected.iter().filter_map(|&u| find(u)).collect();
                if part != Part::Key {
                    moved = vec![i];
                }
                moved.retain(|&m| m != i);
                moved.push(i);
                // The pointer has moved a little from where it went down.
                let travel = ui
                    .input(|i| i.pointer.latest_pos())
                    .map_or(egui::Vec2::ZERO, |p| p - origin);
                state.drag = Some(Drag {
                    keys: keys.clone(),
                    moved,
                    part,
                    travel,
                    modal: false,
                    lock: Lock::Free,
                });
                c.editor.begin_drag();
            }
            None => state.boxing = Some((origin, strip.contains(origin))),
        }
    }
    if let Some((from, nodes)) = state.boxing {
        let to = ui.input(|i| i.pointer.latest_pos()).unwrap_or(from);
        graph::draw_box(&painter, from, to);
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            state.boxing = None;
            let b = egui::Rect::from_two_pos(from, to);
            if nodes {
                // The nodes whose numbers the box spans.
                let found: Vec<usize> = (0..road.nodes.len())
                    .filter(|&n| {
                        b.x_range().contains(plot.x(n as f64))
                            || (closed && n == 0 && b.x_range().contains(plot.x(period)))
                    })
                    .collect();
                let sel = &mut c.editor.selection;
                if sel.item != Some(Item::Road(r)) {
                    sel.select(Item::Road(r));
                }
                graph::pick(&mut sel.nodes, &found, how);
            } else {
                let found: Vec<f64> = keys
                    .iter()
                    .filter(|k| b.contains(plot.pos(k.u, k.value * scale)))
                    .map(|k| k.u)
                    .collect();
                graph::pick(&mut state.selected, &found, how);
            }
        }
    }
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if strip.contains(p) {
            action = Some(Action::AddNode(plot.x_at(p.x)));
        } else if hit(p).is_none() {
            action = Some(Action::AddKey(plot.x_at(p.x), plot.y_at(p.y) / scale));
        }
    } else if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if strip.contains(p) {
            let sel = &mut c.editor.selection;
            match node_at(p) {
                Some(n) if modifiers.shift => sel.toggle_node(Item::Road(r), n),
                Some(n) => sel.select_node(Item::Road(r), n),
                None if !modifiers.shift => sel.nodes.clear(),
                None => {}
            }
        } else {
            match hit(p) {
                Some((i, _)) if modifiers.shift => graph::toggle(&mut state.selected, keys[i].u),
                Some((i, _)) => state.selected = vec![keys[i].u],
                None if modifiers.command => {
                    action = Some(Action::AddKey(plot.x_at(p.x), plot.y_at(p.y) / scale));
                }
                None if !modifiers.shift => state.selected.clear(),
                None => {}
            }
        }
    }

    // Keys over the graph.
    let none = egui::Modifiers::NONE;
    if key(egui::Key::A, none) {
        state.selected = keys.iter().map(|k| k.u).collect();
    } else if key(egui::Key::A, egui::Modifiers::ALT) {
        state.selected.clear();
    } else if key(egui::Key::X, none) || key(egui::Key::Delete, none) {
        action = Some(Action::Delete);
    } else if key(egui::Key::G, none) && !picked.is_empty() && !busy {
        let mut moved = picked.clone();
        if let Some(a) = active {
            moved.retain(|&m| m != a);
            moved.push(a);
        }
        state.drag = Some(Drag {
            keys: keys.clone(),
            moved,
            part: Part::Key,
            travel: egui::Vec2::ZERO,
            modal: true,
            lock: Lock::Free,
        });
        c.editor.begin_drag();
    }

    let set = |editor: &mut Editor, out: Vec<Key>, what: Option<&str>| {
        editor.apply(
            vec![Op::SetProfile {
                road: road.name.clone(),
                curve,
                keys: out,
            }],
            what.map(|w| format!("graph {} {curve:?} {w}", road.name))
                .as_deref(),
        )
    };
    match action {
        Some(Action::Keys(out, what)) => {
            if out != keys {
                set(c.editor, out, Some(what));
            }
        }
        Some(Action::Delete) if picked.is_empty() => {
            c.editor.status = "select keys to delete (the right click deletes nodes)".into();
        }
        Some(Action::Delete) if picked.len() == keys.len() => {
            c.editor.status = "a curve keeps one key at least: select fewer".into();
        }
        Some(Action::Delete) => {
            let out: Vec<Key> = (0..keys.len())
                .filter(|i| !picked.contains(i))
                .map(|i| keys[i])
                .collect();
            if set(c.editor, out, None) {
                c.editor.status = format!("{} keys deleted", picked.len());
                state.selected.clear();
            }
        }
        Some(Action::AddKey(u, v)) => {
            let u = u.clamp(0.0, if closed { period - GAP } else { period });
            // Onto a node near the pointer.
            let u = if (plot.x(u) - plot.x(u.round())).abs() < 8.0 {
                u.round().min(if closed { period - 1.0 } else { period })
            } else {
                u
            };
            match keys.iter().find(|k| (k.u - u).abs() < GAP) {
                Some(k) => state.selected = vec![k.u],
                None => {
                    // Near the curve, on it; elsewhere, where it was asked for.
                    let on = at(u);
                    let near = (plot.y(on * scale) - plot.y(v * scale)).abs() < GRAB;
                    let value = if near { on } else { v.max(shown.least()) };
                    let mut out = keys.clone();
                    out.push(Key::new(u, value));
                    if set(c.editor, out, None) {
                        state.selected = vec![u];
                    }
                }
            }
        }
        Some(Action::KeysAtNodes) => {
            let mut out = profile.clone();
            for &n in &nodes_selected {
                let u = n as f64;
                if !out.keys.iter().any(|k| (k.u - u).abs() < GAP) {
                    out.set(u, at(u));
                }
            }
            if set(c.editor, out.keys, None) {
                state.selected = nodes_selected.iter().map(|&n| n as f64).collect();
            }
        }
        Some(Action::AddNode(u)) => {
            // The keys after it move along with the node numbers.
            state.selected.clear();
            edit::insert_road_node(c.editor, r, u, None);
        }
        Some(Action::DeleteNodes) => {
            state.selected.clear();
            edit::delete_nodes(c.editor);
        }
        Some(Action::Fit) => state.fitted = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::sim::Sim;
    use std::cell::RefCell;

    fn keys(us: &[(f64, f64)]) -> Vec<Key> {
        us.iter().map(|&(u, v)| Key::new(u, v)).collect()
    }

    #[test]
    fn moved_keys_keep_their_order_and_stay_on_the_road() {
        let k = keys(&[(0.0, 6.0), (2.0, 6.0), (3.0, 7.0), (5.0, 6.0)]);
        // Keys 1 and 2 together: stopped short of key 3.
        let out = shift_keys(&k, &[1, 2], 5.0, 1.0, 8.0, true, 0.1);
        assert!((out[2].u - (5.0 - GAP)).abs() < 1e-9, "{out:?}");
        assert!((out[1].u - (4.0 - GAP)).abs() < 1e-9);
        assert_eq!((out[1].value, out[2].value), (7.0, 8.0));
        assert_eq!(out[3], k[3], "not moved");
        // The last key of a closed road stops before its end, of an open one at it.
        let out = shift_keys(&k, &[3], 9.0, 0.0, 8.0, true, 0.1);
        assert!((out[3].u - (8.0 - GAP)).abs() < 1e-9);
        let out = shift_keys(&k, &[3], 9.0, 0.0, 8.0, false, 0.1);
        assert_eq!(out[3].u, 8.0);
        // Nor below the start, nor narrower than the least.
        let out = shift_keys(&k, &[0], -3.0, -10.0, 8.0, false, 0.1);
        assert_eq!((out[0].u, out[0].value), (0.0, 0.1));
    }

    #[test]
    fn smooth_handles_follow_the_neighbours() {
        let k = keys(&[(0.0, 6.0), (2.0, 8.0), (4.0, 10.0), (6.0, 6.0)]);
        let out = smooth_slopes(&k, &[1, 3], 8.0, true);
        assert_eq!((out[1].slope_in, out[1].slope_out), (1.0, 1.0));
        // Round the start of the loop: from key 2 to key 0 two nodes past the end.
        assert_eq!(out[3].slope_out, (6.0 - 10.0) / 4.0);
        let out = smooth_slopes(&k, &[0], 8.0, false);
        assert_eq!(out[0].slope_out, 0.0, "an open road's end levels out");
    }

    #[test]
    fn handles_sit_a_third_of_the_way_to_the_next_key() {
        let k = keys(&[(0.0, 6.0), (3.0, 6.0)]);
        assert!(
            (handle_u(&k, 0, false, 10.0, false) - 0.7).abs() < 1e-9,
            "capped"
        );
        let k = keys(&[(0.0, 6.0), (0.3, 6.0)]);
        assert!((handle_u(&k, 1, true, 10.0, false) - 0.2).abs() < 1e-9);
        assert!(!has_handle(&k, 0, Part::Before, false));
        assert!(has_handle(&k, 0, Part::Before, true));
    }

    #[test]
    fn keys_are_picked_moved_boxed_added_and_deleted_with_the_pointer() {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-curves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Editor::open(dir.clone()).unwrap();
        e.selection.select(Item::Road(0));
        assert!(e.apply(
            vec![Op::SetProfile {
                road: "circuit".into(),
                curve: Curve::WidthLeft,
                keys: keys(&[(1.0, 6.0), (3.0, 6.0), (5.0, 6.0)]),
            }],
            None
        ));
        let views = RefCell::new((
            ProfileView::default(),
            CurveGraph {
                shown: Shown::Left,
                ..Default::default()
            },
        ));
        let mut draw = |ui: &mut egui::Ui, c: &mut Ctx| {
            let (p, g) = &mut *views.borrow_mut();
            panel(ui, c, p, g);
        };
        let plot = || views.borrow().1.plot.expect("drawn");
        let selected = || {
            let mut s = views.borrow().1.selected.clone();
            s.sort_by(f64::total_cmp);
            s
        };
        let left = |e: &Editor| e.project.roads[0].width_left.keys.clone();
        let mut sim = Sim::default();
        sim.frame(&mut e, vec![], &mut draw);
        let steps = e.history().0.len();

        // A key dragged 40 px up follows the pointer all the way, as one step.
        let p = plot();
        let at = p.pos(1.0, 6.0);
        sim.drag(&mut e, at, at - egui::vec2(0.0, 40.0), &mut draw);
        let (_, up) = p.delta(egui::vec2(0.0, -40.0));
        let k = left(&e)[0];
        assert!((k.value - (6.0 + up)).abs() < 0.02 * up, "{k:?}, {up}");
        assert!((k.u - 1.0).abs() < 1e-6);
        assert_eq!(e.history().0.len(), steps + 1);
        assert_eq!(selected(), vec![1.0]);

        // A box over the other two selects them; a drag moves both.
        let p = plot();
        let (y0, y1) = p.axes.y;
        sim.drag(
            &mut e,
            p.pos(2.5, y1 - 0.02 * (y1 - y0)),
            p.pos(5.5, y0 + 0.02 * (y1 - y0)),
            &mut draw,
        );
        assert_eq!(selected(), vec![3.0, 5.0]);
        let p = plot();
        let at = p.pos(3.0, 6.0);
        sim.drag(&mut e, at, at - egui::vec2(0.0, 30.0), &mut draw);
        let (_, up) = p.delta(egui::vec2(0.0, -30.0));
        let ks = left(&e);
        assert!(
            (ks[1].value - (6.0 + up)).abs() < 0.02 * up && ks[1].value == ks[2].value,
            "{ks:?}"
        );

        // X over the graph deletes them.
        sim.key(&mut e, egui::Key::X, &mut draw);
        assert_eq!(left(&e).len(), 1);
        assert!(selected().is_empty());

        // A double-click adds a key, onto the node near it.
        let p = plot();
        let (y0, y1) = p.axes.y;
        let at = egui::pos2(p.x(4.02), p.y(y0 + 0.8 * (y1 - y0)));
        sim.double_click(&mut e, at, &mut draw);
        let ks = left(&e);
        assert_eq!(ks.len(), 2, "{ks:?}");
        assert_eq!(ks[1].u, 4.0);
        assert_eq!(selected(), vec![4.0]);

        // The numbers along the bottom pick the road's nodes, and add them.
        let p = plot();
        sim.click(
            &mut e,
            egui::pos2(p.x(2.0), p.rect.bottom() + 0.5 * STRIP),
            egui::PointerButton::Primary,
            &mut draw,
        );
        assert_eq!(e.selection.nodes, vec![2]);
        let n = e.project.roads[0].nodes.len();
        sim.double_click(
            &mut e,
            egui::pos2(p.x(2.5), p.rect.bottom() + 0.5 * STRIP),
            &mut draw,
        );
        assert_eq!(e.project.roads[0].nodes.len(), n + 1);
        assert_eq!(e.selection.nodes, vec![3]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
