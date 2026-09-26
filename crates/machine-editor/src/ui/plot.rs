//! Line plots drawn with egui's painter: curves against an x axis, with a left and an
//! optional right y axis, a grid, a legend, and a readout under the pointer.

use bevy_egui::egui;

pub struct Line {
    pub label: String,
    pub colour: egui::Color32,
    pub points: Vec<[f64; 2]>,
    pub right: bool,
    pub dashed: bool,
}

impl Line {
    pub fn new(label: &str, colour: egui::Color32, points: Vec<[f64; 2]>) -> Self {
        Self {
            label: label.into(),
            colour,
            points,
            right: false,
            dashed: false,
        }
    }

    pub fn right(mut self) -> Self {
        self.right = true;
        self
    }

    pub fn dashed(mut self) -> Self {
        self.dashed = true;
        self
    }
}

pub const RED: egui::Color32 = egui::Color32::from_rgb(235, 90, 70);
pub const BLUE: egui::Color32 = egui::Color32::from_rgb(90, 150, 240);
pub const GREEN: egui::Color32 = egui::Color32::from_rgb(110, 200, 110);
pub const YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 200, 80);
pub const PURPLE: egui::Color32 = egui::Color32::from_rgb(190, 120, 230);
pub const GREY: egui::Color32 = egui::Color32::from_gray(150);

fn nice_step(span: f64) -> f64 {
    let raw = (span / 6.0).max(1e-12);
    let mag = 10f64.powf(raw.log10().floor());
    let f = raw / mag;
    mag * if f < 1.5 {
        1.0
    } else if f < 3.5 {
        2.0
    } else if f < 7.5 {
        5.0
    } else {
        10.0
    }
}

fn fmt(v: f64, step: f64) -> String {
    if step >= 1.0 {
        format!("{v:.0}")
    } else if step >= 0.1 {
        format!("{v:.1}")
    } else if step >= 0.01 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

fn range(lines: &[Line], right: bool, zero: bool) -> Option<(f64, f64)> {
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for p in lines
        .iter()
        .filter(|l| l.right == right)
        .flat_map(|l| l.points.iter())
    {
        if p[1].is_finite() {
            lo = lo.min(p[1]);
            hi = hi.max(p[1]);
        }
    }
    if lo > hi {
        return None;
    }
    if zero {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    if hi - lo < 1e-9 {
        hi = lo + 1.0;
    }
    let s = nice_step(hi - lo);
    Some(((lo / s).floor() * s, (hi / s).ceil() * s))
}

/// Draws a plot filling `height` of the available width.
pub fn plot(
    ui: &mut egui::Ui,
    x_label: &str,
    y_label: &str,
    y_right: Option<&str>,
    lines: &[Line],
    height: f32,
    zero: bool,
) {
    let width = ui.available_width().max(200.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(30));
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(52.0, 22.0),
        rect.max - egui::vec2(if y_right.is_some() { 52.0 } else { 12.0 }, 24.0),
    );
    let (mut x0, mut x1) = (f64::MAX, f64::MIN);
    for p in lines.iter().flat_map(|l| l.points.iter()) {
        x0 = x0.min(p[0]);
        x1 = x1.max(p[0]);
    }
    let text = egui::Color32::from_gray(170);
    let font = egui::FontId::proportional(11.0);
    if x0 >= x1 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no data yet",
            font,
            text,
        );
        return;
    }
    let yl = range(lines, false, zero).unwrap_or((0.0, 1.0));
    let yr = range(lines, true, zero).unwrap_or((0.0, 1.0));
    let px = |x: f64| plot.left() + ((x - x0) / (x1 - x0)) as f32 * plot.width();
    let py = |y: f64, (lo, hi): (f64, f64)| {
        plot.bottom() - ((y - lo) / (hi - lo)) as f32 * plot.height()
    };
    let grid = egui::Stroke::new(1.0, egui::Color32::from_gray(50));
    let xs = nice_step(x1 - x0);
    let mut x = (x0 / xs).ceil() * xs;
    while x <= x1 + 1e-9 {
        painter.line_segment(
            [
                egui::pos2(px(x), plot.top()),
                egui::pos2(px(x), plot.bottom()),
            ],
            grid,
        );
        painter.text(
            egui::pos2(px(x), plot.bottom() + 3.0),
            egui::Align2::CENTER_TOP,
            fmt(x, xs),
            font.clone(),
            text,
        );
        x += xs;
    }
    let ys = nice_step(yl.1 - yl.0);
    let mut y = yl.0;
    while y <= yl.1 + 1e-9 {
        painter.line_segment(
            [
                egui::pos2(plot.left(), py(y, yl)),
                egui::pos2(plot.right(), py(y, yl)),
            ],
            grid,
        );
        painter.text(
            egui::pos2(plot.left() - 4.0, py(y, yl)),
            egui::Align2::RIGHT_CENTER,
            fmt(y, ys),
            font.clone(),
            text,
        );
        y += ys;
    }
    if y_right.is_some() {
        let ys = nice_step(yr.1 - yr.0);
        let mut y = yr.0;
        while y <= yr.1 + 1e-9 {
            painter.text(
                egui::pos2(plot.right() + 4.0, py(y, yr)),
                egui::Align2::LEFT_CENTER,
                fmt(y, ys),
                font.clone(),
                text,
            );
            y += ys;
        }
        painter.text(
            egui::pos2(plot.right() + 4.0, rect.top() + 4.0),
            egui::Align2::LEFT_TOP,
            y_right.unwrap_or(""),
            font.clone(),
            text,
        );
    }
    painter.text(
        egui::pos2(rect.left() + 4.0, rect.top() + 4.0),
        egui::Align2::LEFT_TOP,
        y_label,
        font.clone(),
        text,
    );
    painter.text(
        egui::pos2(plot.right(), rect.bottom() - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        x_label,
        font.clone(),
        text,
    );
    // Legend.
    let mut lx = plot.left() + 6.0;
    for l in lines {
        painter.line_segment(
            [
                egui::pos2(lx, rect.top() + 10.0),
                egui::pos2(lx + 14.0, rect.top() + 10.0),
            ],
            egui::Stroke::new(2.0, l.colour),
        );
        let g = painter.layout_no_wrap(l.label.clone(), font.clone(), text);
        let w = g.size().x;
        painter.galley(egui::pos2(lx + 18.0, rect.top() + 4.0), g, text);
        lx += w + 32.0;
    }
    for l in lines {
        let rng = if l.right { yr } else { yl };
        let pts: Vec<egui::Pos2> = l
            .points
            .iter()
            .filter(|p| p[1].is_finite())
            .map(|p| egui::pos2(px(p[0]), py(p[1], rng)))
            .collect();
        let stroke = egui::Stroke::new(1.8, l.colour);
        if l.dashed {
            for (k, w) in pts.windows(2).enumerate() {
                if k % 2 == 0 {
                    painter.line_segment([w[0], w[1]], stroke);
                }
            }
        } else {
            painter.add(egui::Shape::line(pts, stroke));
        }
    }
    // Readout.
    if let Some(p) = resp.hover_pos().filter(|p| plot.contains(*p)) {
        let xv = x0 + ((p.x - plot.left()) / plot.width()) as f64 * (x1 - x0);
        painter.line_segment(
            [egui::pos2(p.x, plot.top()), egui::pos2(p.x, plot.bottom())],
            egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
        );
        let mut s = fmt(xv, xs / 10.0).to_string();
        for l in lines {
            if let Some(v) = sample(&l.points, xv) {
                s.push_str(&format!("   {} {:.4}", l.label, v));
            }
        }
        painter.text(
            egui::pos2(plot.left() + 4.0, plot.bottom() - 4.0),
            egui::Align2::LEFT_BOTTOM,
            s,
            font,
            egui::Color32::WHITE,
        );
    }
}

/// Linear interpolation in points of ascending x.
fn sample(points: &[[f64; 2]], x: f64) -> Option<f64> {
    let w = points.windows(2).find(|w| w[0][0] <= x && x <= w[1][0])?;
    let f = if w[1][0] > w[0][0] {
        (x - w[0][0]) / (w[1][0] - w[0][0])
    } else {
        0.0
    };
    Some(w[0][1] + f * (w[1][1] - w[0][1]))
}
