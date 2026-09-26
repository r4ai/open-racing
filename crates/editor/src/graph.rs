//! What the graphs below the 3D view share, the elevation and the width and bank curves,
//! as Blender's graph editor: axes that zoom about the pointer (the wheel along the road,
//! Ctrl + wheel up and down) and pan (middle drag), a grid, the road's nodes along the
//! bottom, and selecting as in the view: a click picks, Shift + click adds or takes out,
//! a drag over empty space draws a box (Shift adds, Ctrl takes out).

use bevy_egui::egui;

/// The ranges a graph shows: along the road (x) and the value (y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axes {
    pub x: (f64, f64),
    pub y: (f64, f64),
}

impl Default for Axes {
    fn default() -> Self {
        Self {
            x: (0.0, 1.0),
            y: (0.0, 1.0),
        }
    }
}

impl Axes {
    /// `x` whole, and the values `ys` with room round them: at least `min_span` tall.
    pub fn fit(x: (f64, f64), ys: impl Iterator<Item = f64>, pad: f64, min_span: f64) -> Self {
        let (lo, hi) = ys.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (0.0, 0.0) };
        let mid = 0.5 * (lo + hi);
        let half = (0.5 * (hi - lo) * (1.0 + 2.0 * pad)).max(0.5 * min_span);
        Self {
            x: (x.0, x.1.max(x.0 + 1e-6)),
            y: (mid - half, mid + half),
        }
    }

    /// Makes room for values that went out of sight (a point dragged past the top).
    pub fn include(&mut self, ys: impl Iterator<Item = f64>) {
        let (lo, hi) = ys.fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
        if lo < self.y.0 || hi > self.y.1 {
            let pad = 0.1 * (self.y.1 - self.y.0);
            self.y = (self.y.0.min(lo - pad), self.y.1.max(hi + pad));
        }
    }
}

/// Axes laid over a rectangle of the screen: from graph to screen and back.
#[derive(Clone, Copy, Debug)]
pub struct Plot {
    pub rect: egui::Rect,
    pub axes: Axes,
}

impl Plot {
    pub fn x(&self, x: f64) -> f32 {
        let (a, b) = self.axes.x;
        self.rect.left() + ((x - a) / (b - a)) as f32 * self.rect.width()
    }

    pub fn y(&self, y: f64) -> f32 {
        let (a, b) = self.axes.y;
        self.rect.bottom() - ((y - a) / (b - a)) as f32 * self.rect.height()
    }

    pub fn pos(&self, x: f64, y: f64) -> egui::Pos2 {
        egui::pos2(self.x(x), self.y(y))
    }

    pub fn x_at(&self, px: f32) -> f64 {
        let (a, b) = self.axes.x;
        a + ((px - self.rect.left()) / self.rect.width()) as f64 * (b - a)
    }

    pub fn y_at(&self, py: f32) -> f64 {
        let (a, b) = self.axes.y;
        a + ((self.rect.bottom() - py) / self.rect.height()) as f64 * (b - a)
    }

    /// How far a pointer motion goes in the graph, along and up.
    pub fn delta(&self, d: egui::Vec2) -> (f64, f64) {
        (
            d.x as f64 * (self.axes.x.1 - self.axes.x.0) / self.rect.width() as f64,
            -d.y as f64 * (self.axes.y.1 - self.axes.y.0) / self.rect.height() as f64,
        )
    }
}

/// Zooming about the pointer with the wheel (along the road) or Ctrl + wheel (up and
/// down, or both with `same_scale`), and panning with the middle button.
pub fn navigate(
    ui: &egui::Ui,
    resp: &egui::Response,
    plot: &Plot,
    axes: &mut Axes,
    same_scale: bool,
) {
    let rect = plot.rect;
    if let Some(hover) = resp.hover_pos() {
        // egui turns Ctrl + wheel into a zoom.
        let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let fx = ((hover.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
        let fy = ((rect.bottom() - hover.y) / rect.height()).clamp(0.0, 1.0) as f64;
        let scale = |(a, b): (f64, f64), f: f64, k: f64| {
            let at = a + (b - a) * f;
            (at - (at - a) * k, at + (b - at) * k)
        };
        if zoom != 1.0 && !same_scale {
            axes.y = scale(axes.y, fy, 1.0 / zoom as f64);
        } else if scroll != 0.0 || zoom != 1.0 {
            let k = if scroll != 0.0 {
                (-scroll as f64 * 0.002).exp()
            } else {
                1.0 / zoom as f64
            };
            axes.x = scale(axes.x, fx, k);
            if same_scale {
                axes.y = scale(axes.y, fy, k);
            }
        }
    }
    if resp.dragged_by(egui::PointerButton::Middle) {
        let (dx, dy) = plot.delta(resp.drag_delta());
        axes.x = (axes.x.0 - dx, axes.x.1 - dx);
        axes.y = (axes.y.0 - dy, axes.y.1 - dy);
    }
}

/// 1, 2 or 5 times a power of ten, at least `x`.
pub fn nice_step(x: f64) -> f64 {
    let p = 10f64.powf(x.max(1e-3).log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|k| k * p)
        .find(|&v| v >= x)
        .unwrap_or(10.0 * p)
}

/// Lines across at a round step of the value, labelled at the left.
pub fn grid(painter: &egui::Painter, plot: &Plot, unit: &str) {
    let (y0, y1) = plot.axes.y;
    let step = nice_step((y1 - y0) / 6.0);
    let decimals = if step < 1.0 { 1 } else { 0 };
    let mut y = (y0 / step).ceil() * step;
    while y < y1 {
        // Not "-0" for a line that is 0 but for rounding.
        let label = if y.abs() < 1e-6 * step { 0.0 } else { y };
        painter.hline(
            plot.rect.x_range(),
            plot.y(y),
            egui::Stroke::new(1.0, egui::Color32::from_gray(45)),
        );
        painter.text(
            egui::pos2(plot.rect.left() + 2.0, plot.y(y)),
            egui::Align2::LEFT_BOTTOM,
            format!("{label:.decimals$}{unit}"),
            egui::FontId::monospace(10.0),
            egui::Color32::from_gray(110),
        );
        y += step;
    }
}

/// How a node or key is drawn: the active one white, the other selected orange, as in
/// the view.
pub fn point_color(selected: bool, active: bool, normal: egui::Color32) -> egui::Color32 {
    if active {
        crate::theme::ACTIVE_NODE_UI
    } else if selected {
        crate::theme::SELECTED_NODE_UI
    } else {
        normal
    }
}

/// The road's nodes: a line up the graph at each, lit when selected, numbered in
/// `strip` where there is room (and always when selected).
pub fn node_ticks(
    painter: &egui::Painter,
    plot: &Plot,
    strip: egui::Rect,
    nodes: impl Iterator<Item = (usize, f64)>,
    selected: &[usize],
) {
    let active = selected.last().copied();
    let mut last_label = f32::NEG_INFINITY;
    for (n, x) in nodes {
        let px = plot.x(x);
        if px < plot.rect.left() - 1.0 || px > plot.rect.right() + 1.0 {
            continue;
        }
        let lit = selected.contains(&n);
        let color = point_color(lit, active == Some(n), egui::Color32::from_gray(60));
        painter.vline(
            px,
            plot.rect.y_range(),
            egui::Stroke::new(if lit { 1.5 } else { 1.0 }, color),
        );
        if !lit && px - last_label < 22.0 {
            continue;
        }
        last_label = px;
        painter.text(
            egui::pos2(px, strip.top() + 1.0),
            egui::Align2::CENTER_TOP,
            n.to_string(),
            egui::FontId::monospace(9.0),
            if lit { color } else { egui::Color32::GRAY },
        );
    }
}

/// How a click or a box changes a selection: Shift adds to it, Ctrl takes from it, else
/// it is replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Set,
    Add,
    Remove,
}

impl Pick {
    pub fn of(m: egui::Modifiers) -> Self {
        if m.command {
            Pick::Remove
        } else if m.shift {
            Pick::Add
        } else {
            Pick::Set
        }
    }
}

/// Changes `sel` by what a box found, the active one (last) staying so if it stays
/// selected.
pub fn pick<T: PartialEq + Copy>(sel: &mut Vec<T>, found: &[T], how: Pick) {
    match how {
        Pick::Set => {
            let active = sel.last().copied().filter(|a| found.contains(a));
            *sel = found.to_vec();
            if let Some(a) = active {
                sel.retain(|x| *x != a);
                sel.push(a);
            }
        }
        Pick::Add => {
            let active = sel.last().copied();
            for f in found {
                if !sel.contains(f) {
                    sel.push(*f);
                }
            }
            if let Some(a) = active {
                sel.retain(|x| *x != a);
                sel.push(a);
            }
        }
        Pick::Remove => sel.retain(|x| !found.contains(x)),
    }
}

/// Shift + click: adds `t` as the active one, or takes it out if it is the active one
/// already, as in the view.
pub fn toggle<T: PartialEq + Copy>(sel: &mut Vec<T>, t: T) {
    if sel.last() == Some(&t) {
        sel.pop();
    } else {
        sel.retain(|x| *x != t);
        sel.push(t);
    }
}

/// The box being dragged out to select, as in the view.
pub fn draw_box(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2) {
    let r = egui::Rect::from_two_pos(from, to);
    painter.rect_filled(r, 0.0, egui::Color32::from_white_alpha(12));
    painter.rect_stroke(
        r,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(200)),
        egui::StrokeKind::Inside,
    );
}

/// Where the pointer went down for the drag now starting: egui reports a drag only once
/// the pointer has moved a little, and what it began on is where it went down.
pub fn press_origin(ui: &egui::Ui, resp: &egui::Response) -> Option<egui::Pos2> {
    ui.input(|i| i.pointer.press_origin())
        .or_else(|| resp.interact_pointer_pos())
}

/// The keys a graph reads while the pointer is over it and nothing is being typed.
pub fn key(ui: &egui::Ui, hovered: bool, key: egui::Key, mods: egui::Modifiers) -> bool {
    hovered
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(key) && i.modifiers.matches_exact(mods))
}

/// Pointer and keys played into a window-less egui, to test the graphs as they are used.
#[cfg(test)]
pub mod sim {
    use bevy_egui::egui;

    use crate::commands::Ctx;
    use crate::jobs::Jobs;
    use crate::preview::Built;
    use crate::state::Editor;
    use crate::ui::Shell;
    use crate::viewport::{Orbit, Tool};

    pub struct Sim {
        pub ctx: egui::Context,
        time: f64,
        pub modifiers: egui::Modifiers,
        pointer: egui::Pos2,
        tool: Tool,
        orbit: Orbit,
        jobs: Jobs,
        built: Built,
        shell: Shell,
    }

    impl Default for Sim {
        fn default() -> Self {
            Self {
                ctx: egui::Context::default(),
                time: 0.0,
                modifiers: egui::Modifiers::NONE,
                pointer: egui::Pos2::ZERO,
                tool: Tool::default(),
                orbit: Orbit::default(),
                jobs: Jobs::default(),
                built: Built::default(),
                shell: Shell::default(),
            }
        }
    }

    impl Sim {
        /// One frame with these events, drawing `f`.
        pub fn frame(
            &mut self,
            editor: &mut Editor,
            events: Vec<egui::Event>,
            mut f: impl FnMut(&mut egui::Ui, &mut Ctx),
        ) {
            self.time += 1.0 / 60.0;
            let mut events = events;
            events.insert(0, egui::Event::ModifiersChanged(self.modifiers));
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 500.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut c = Ctx {
                editor,
                tool: &mut self.tool,
                orbit: &mut self.orbit,
                jobs: &mut self.jobs,
                built: &self.built,
                shell: &mut self.shell,
                pointer: bevy::math::Vec2::ZERO,
            };
            let mut out = self.ctx.run_ui(input, |ui| f(ui, &mut c));
            out.textures_delta.clear();
        }

        fn button(&self, pressed: bool, button: egui::PointerButton) -> egui::Event {
            egui::Event::PointerButton {
                pos: self.pointer,
                button,
                pressed,
                modifiers: self.modifiers,
            }
        }

        /// Moves the pointer to `to` in `steps` frames.
        pub fn move_to(
            &mut self,
            editor: &mut Editor,
            to: egui::Pos2,
            steps: usize,
            f: &mut impl FnMut(&mut egui::Ui, &mut Ctx),
        ) {
            let from = self.pointer;
            for i in 1..=steps.max(1) {
                self.pointer = from.lerp(to, i as f32 / steps.max(1) as f32);
                let e = egui::Event::PointerMoved(self.pointer);
                self.frame(editor, vec![e], &mut *f);
            }
        }

        /// A click of `button` at `at`.
        pub fn click(
            &mut self,
            editor: &mut Editor,
            at: egui::Pos2,
            button: egui::PointerButton,
            f: &mut impl FnMut(&mut egui::Ui, &mut Ctx),
        ) {
            self.time += 1.0;
            self.move_to(editor, at, 1, f);
            let down = self.button(true, button);
            self.frame(editor, vec![down], &mut *f);
            let up = self.button(false, button);
            self.frame(editor, vec![up], &mut *f);
            self.frame(editor, vec![], &mut *f);
        }

        /// Two clicks in quick succession.
        pub fn double_click(
            &mut self,
            editor: &mut Editor,
            at: egui::Pos2,
            f: &mut impl FnMut(&mut egui::Ui, &mut Ctx),
        ) {
            self.time += 1.0;
            self.move_to(editor, at, 1, f);
            for _ in 0..2 {
                let down = self.button(true, egui::PointerButton::Primary);
                self.frame(editor, vec![down], &mut *f);
                let up = self.button(false, egui::PointerButton::Primary);
                self.frame(editor, vec![up], &mut *f);
            }
            self.frame(editor, vec![], &mut *f);
        }

        /// A drag with the left button from `from` to `to`.
        pub fn drag(
            &mut self,
            editor: &mut Editor,
            from: egui::Pos2,
            to: egui::Pos2,
            f: &mut impl FnMut(&mut egui::Ui, &mut Ctx),
        ) {
            self.time += 1.0;
            self.move_to(editor, from, 1, f);
            let down = self.button(true, egui::PointerButton::Primary);
            self.frame(editor, vec![down], &mut *f);
            self.move_to(editor, to, 8, f);
            let up = self.button(false, egui::PointerButton::Primary);
            self.frame(editor, vec![up], &mut *f);
            self.frame(editor, vec![], &mut *f);
        }

        /// A key pressed and let go with the pointer where it is.
        pub fn key(
            &mut self,
            editor: &mut Editor,
            key: egui::Key,
            f: &mut impl FnMut(&mut egui::Ui, &mut Ctx),
        ) {
            let e = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: self.modifiers,
            };
            let (down, up) = (e(true), e(false));
            self.frame(editor, vec![down], &mut *f);
            self.frame(editor, vec![up], &mut *f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picking_keeps_the_active_one_last() {
        let mut sel = vec![1, 2, 3];
        pick(&mut sel, &[4, 3], Pick::Set);
        assert_eq!(sel, vec![4, 3]);
        pick(&mut sel, &[5, 4], Pick::Add);
        assert_eq!(sel, vec![4, 5, 3]);
        pick(&mut sel, &[3], Pick::Remove);
        assert_eq!(sel, vec![4, 5]);
        toggle(&mut sel, 4);
        assert_eq!(sel, vec![5, 4]);
        toggle(&mut sel, 4);
        assert_eq!(sel, vec![5]);
    }

    #[test]
    fn a_plot_maps_both_ways_and_fits_with_room() {
        let axes = Axes::fit((0.0, 10.0), [2.0, 4.0].into_iter(), 0.25, 1.0);
        assert_eq!(axes.y, (1.5, 4.5));
        let plot = Plot {
            rect: egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 50.0)),
            axes,
        };
        let p = plot.pos(5.0, 3.0);
        assert!((plot.x_at(p.x) - 5.0).abs() < 1e-5 && (plot.y_at(p.y) - 3.0).abs() < 1e-5);
        let (dx, dy) = plot.delta(egui::vec2(10.0, -10.0));
        assert!((dx - 1.0).abs() < 1e-9 && (dy - 0.6).abs() < 1e-9);
        let flat = Axes::fit((0.0, 1.0), [6.0].into_iter(), 0.25, 1.0);
        assert_eq!(flat.y, (5.5, 6.5), "at least the smallest span");
    }
}
