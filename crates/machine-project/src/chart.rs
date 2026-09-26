//! Pictures for people and agents without the editor: line charts (dyno curves, cycle
//! traces, order tracks) and side and top views of a machine's stand-in shapes, as PNG.

use glam::{DAffine3, DVec3};

use crate::assembly::Assembly;
use crate::library::Library;

/// An 8-bit RGB image with a depth buffer.
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    depth: Vec<f32>,
}

/// 3 × 5 glyphs, rows top to bottom, 3 bits each (MSB left).
fn glyph(c: char) -> [u8; 5] {
    match c.to_ascii_uppercase() {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 2, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        '.' => [0, 0, 0, 0, 2],
        ',' => [0, 0, 0, 2, 4],
        '-' => [0, 0, 7, 0, 0],
        '+' => [0, 2, 7, 2, 0],
        '/' => [1, 1, 2, 4, 4],
        ':' => [0, 2, 0, 2, 0],
        '%' => [5, 1, 2, 4, 5],
        '(' => [2, 4, 4, 4, 2],
        ')' => [2, 1, 1, 1, 2],
        '·' => [0, 0, 2, 0, 0],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [7, 4, 4, 4, 7],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [7, 4, 5, 5, 7],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 7],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [7, 5, 5, 5, 7],
        'P' => [7, 5, 7, 4, 4],
        'Q' => [7, 5, 5, 7, 1],
        'R' => [7, 5, 6, 5, 5],
        'S' => [7, 4, 7, 1, 7],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        _ => [0; 5],
    }
}

impl Canvas {
    pub fn new(width: usize, height: usize, bg: [u8; 3]) -> Self {
        Self {
            width,
            height,
            rgb: bg.repeat(width * height),
            depth: vec![f32::INFINITY; width * height],
        }
    }

    pub fn set(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let i = (y as usize * self.width + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    pub fn rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: [u8; 3]) {
        for y in y0.min(y1)..=y0.max(y1) {
            for x in x0.min(x1)..=x0.max(x1) {
                self.set(x, y, c);
            }
        }
    }

    /// A line `w` pixels wide; `dash` > 0 draws dashes of that length.
    pub fn line(&mut self, a: (f64, f64), b: (f64, f64), c: [u8; 3], w: i64, dash: f64) {
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let n = (len * 2.0).ceil().max(1.0) as usize;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            if dash > 0.0 && ((t * len / dash) as i64) % 2 == 1 {
                continue;
            }
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            for dy in 0..w {
                for dx in 0..w {
                    self.set(x as i64 + dx - w / 2, y as i64 + dy - w / 2, c);
                }
            }
        }
    }

    /// Text at a scale (pixels per glyph cell).
    pub fn text(&mut self, x: i64, y: i64, s: &str, c: [u8; 3], scale: i64) {
        for (k, ch) in s.chars().enumerate() {
            let g = glyph(ch);
            for (row, bits) in g.iter().enumerate() {
                for col in 0..3 {
                    if bits >> (2 - col) & 1 == 1 {
                        let (px, py) = (x + (k as i64 * 4 + col) * scale, y + row as i64 * scale);
                        self.rect(px, py, px + scale - 1, py + scale - 1, c);
                    }
                }
            }
        }
    }

    pub fn text_width(s: &str, scale: i64) -> i64 {
        s.chars().count() as i64 * 4 * scale
    }

    /// A triangle with a depth test (smaller is nearer).
    fn triangle(&mut self, p: [(f64, f64, f64); 3], c: [u8; 3]) {
        let (x0, x1) = (
            p.iter().map(|v| v.0).fold(f64::MAX, f64::min),
            p.iter().map(|v| v.0).fold(f64::MIN, f64::max),
        );
        let (y0, y1) = (
            p.iter().map(|v| v.1).fold(f64::MAX, f64::min),
            p.iter().map(|v| v.1).fold(f64::MIN, f64::max),
        );
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
        if area.abs() < 1e-9 {
            return;
        }
        for y in (y0.floor().max(0.0) as i64)..=(y1.ceil().min(self.height as f64 - 1.0) as i64) {
            for x in (x0.floor().max(0.0) as i64)..=(x1.ceil().min(self.width as f64 - 1.0) as i64)
            {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = (w0 * p[0].2 + w1 * p[1].2 + w2 * p[2].2) as f32;
                let i = y as usize * self.width + x as usize;
                if z < self.depth[i] {
                    self.depth[i] = z;
                    self.set(x, y, c);
                }
            }
        }
    }

    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("PNG header");
        w.write_image_data(&self.rgb).expect("PNG data");
        w.finish().expect("PNG end");
        out
    }
}

/// One line of a chart.
pub struct Series {
    pub label: String,
    pub colour: [u8; 3],
    pub points: Vec<(f64, f64)>,
    /// Plotted against the right-hand axis.
    pub right: bool,
    pub dashed: bool,
}

fn nice_step(span: f64) -> f64 {
    let raw = span / 6.0;
    let mag = 10f64.powf(raw.max(1e-12).log10().floor());
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
    } else {
        format!("{v:.2}")
    }
}

/// A line chart with a left and optionally a right axis.
pub fn line_chart(
    title: &str,
    x_label: &str,
    left: &str,
    right: Option<&str>,
    series: &[Series],
) -> Vec<u8> {
    let (w, h) = (1000usize, 600usize);
    let mut c = Canvas::new(w, h, [250, 250, 250]);
    let (l, r, t, b) = (
        80.0,
        if right.is_some() { 920.0 } else { 960.0 },
        60.0,
        530.0,
    );
    let range = |right: bool| {
        let pts = series
            .iter()
            .filter(|s| s.right == right)
            .flat_map(|s| s.points.iter());
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for p in pts {
            lo = lo.min(p.1);
            hi = hi.max(p.1);
        }
        if lo > hi {
            (0.0, 1.0)
        } else {
            let lo = lo.min(0.0);
            let step = nice_step(hi - lo);
            ((lo / step).floor() * step, (hi / step).ceil() * step)
        }
    };
    let (mut x0, mut x1) = (f64::MAX, f64::MIN);
    for p in series.iter().flat_map(|s| s.points.iter()) {
        x0 = x0.min(p.0);
        x1 = x1.max(p.0);
    }
    if x0 >= x1 {
        x1 = x0 + 1.0;
    }
    let (yl, yr) = (range(false), range(true));
    let px = |x: f64| l + (x - x0) / (x1 - x0) * (r - l);
    let py = |y: f64, (lo, hi): (f64, f64)| b - (y - lo) / (hi - lo) * (b - t);
    // Grid and labels.
    let grey = [210, 210, 210];
    let dark = [60, 60, 60];
    let xs = nice_step(x1 - x0);
    let mut x = (x0 / xs).ceil() * xs;
    while x <= x1 + 1e-9 {
        c.line((px(x), t), (px(x), b), grey, 1, 0.0);
        let s = fmt(x, xs);
        c.text(
            px(x) as i64 - Canvas::text_width(&s, 2) / 2,
            b as i64 + 10,
            &s,
            dark,
            2,
        );
        x += xs;
    }
    let ys = nice_step(yl.1 - yl.0);
    let mut y = yl.0;
    while y <= yl.1 + 1e-9 {
        c.line((l, py(y, yl)), (r, py(y, yl)), grey, 1, 0.0);
        let s = fmt(y, ys);
        c.text(
            l as i64 - 8 - Canvas::text_width(&s, 2),
            py(y, yl) as i64 - 5,
            &s,
            dark,
            2,
        );
        y += ys;
    }
    if right.is_some() {
        let ys = nice_step(yr.1 - yr.0);
        let mut y = yr.0;
        while y <= yr.1 + 1e-9 {
            let s = fmt(y, ys);
            c.text(r as i64 + 8, py(y, yr) as i64 - 5, &s, dark, 2);
            y += ys;
        }
    }
    c.line((l, b), (r, b), dark, 2, 0.0);
    c.line((l, t), (l, b), dark, 2, 0.0);
    c.text(l as i64, 18, title, [20, 20, 20], 3);
    c.text(
        (l + r) as i64 / 2 - Canvas::text_width(x_label, 2) / 2,
        565,
        x_label,
        dark,
        2,
    );
    c.text(8, t as i64 - 28, left, dark, 2);
    if let Some(rl) = right {
        c.text(
            r as i64 - Canvas::text_width(rl, 2),
            t as i64 - 28,
            rl,
            dark,
            2,
        );
    }
    // Series and legend.
    let mut lx = l as i64 + 10;
    for s in series {
        let rng = if s.right { yr } else { yl };
        for p in s.points.windows(2) {
            c.line(
                (px(p[0].0), py(p[0].1, rng)),
                (px(p[1].0), py(p[1].1, rng)),
                s.colour,
                3,
                if s.dashed { 8.0 } else { 0.0 },
            );
        }
        c.rect(lx, t as i64 + 8, lx + 18, t as i64 + 12, s.colour);
        c.text(lx + 24, t as i64 + 5, &s.label, dark, 2);
        lx += 40 + Canvas::text_width(&s.label, 2);
    }
    c.to_png()
}

/// The dyno sheet: torque and power against speed, and the motoring drag.
pub fn dyno_png(title: &str, run: &open_racing_engine_sim::dyno::DynoRun) -> Vec<u8> {
    let s = vec![
        Series {
            label: "TORQUE N·M".into(),
            colour: [200, 60, 40],
            points: run.points.iter().map(|p| (p.rpm, p.brake_torque)).collect(),
            right: false,
            dashed: false,
        },
        Series {
            label: "DRAG N·M".into(),
            colour: [200, 60, 40],
            points: run.drag.clone(),
            right: false,
            dashed: true,
        },
        Series {
            label: "POWER KW".into(),
            colour: [40, 90, 200],
            points: run.points.iter().map(|p| (p.rpm, p.power / 1e3)).collect(),
            right: true,
            dashed: false,
        },
    ];
    line_chart(title, "RPM", "N·M", Some("KW"), &s)
}

/// Side (x–z) and top (x–y) views of a machine's shapes, flat-shaded.
pub fn preview_png(lib: &Library, asm: &Assembly) -> Vec<u8> {
    let mut tris = crate::mesh::Tris::default();
    let mut colours: Vec<[u8; 3]> = Vec::new();
    for inst in &asm.instances {
        let p = &lib.parts[&inst.part];
        for s in &p.physical.shapes {
            let t = crate::mesh::shape(s);
            let n = t.indices.len() / 3;
            tris.append(&t, &inst.transform);
            colours.extend(std::iter::repeat_n(
                s.colour.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8),
                n,
            ));
        }
    }
    let (w, h) = (1200usize, 900usize);
    let mut c = Canvas::new(w, h, [245, 245, 245]);
    let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    for p in &tris.positions {
        let v = DVec3::from(p.map(f64::from));
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if lo.x > hi.x {
        return c.to_png();
    }
    let scale =
        ((w as f64 - 80.0) / (hi.x - lo.x)).min(380.0 / (hi.z - lo.z).max(hi.y - lo.y).max(0.1));
    // Side view looks from the left (−y... the car's left side, +y towards the viewer);
    // top view from above.
    let views: [(DAffine3, f64); 2] = [
        (DAffine3::IDENTITY, 40.0 + 380.0),
        (DAffine3::IDENTITY, 470.0 + 380.0),
    ];
    let light = DVec3::new(-0.3, 0.5, 0.8).normalize();
    for (k, &(_, base)) in views.iter().enumerate() {
        for (ti, tri) in tris.indices.chunks(3).enumerate() {
            let v = [0, 1, 2].map(|j| DVec3::from(tris.positions[tri[j] as usize].map(f64::from)));
            let n = (v[1] - v[0]).cross(v[2] - v[0]).normalize_or_zero();
            let facing = if k == 0 { n.y } else { n.z };
            if facing <= 0.0 {
                continue;
            }
            let shade = 0.55 + 0.45 * n.dot(light).max(0.0);
            let col = colours[ti].map(|x| (x as f64 * shade).min(255.0) as u8);
            let p = v.map(|q| {
                let sx = 40.0 + (q.x - lo.x) * scale;
                if k == 0 {
                    (w as f64 - sx, base - (q.z - lo.z) * scale, -q.y)
                } else {
                    (w as f64 - sx, base - (hi.y - q.y) * scale, -q.z)
                }
            });
            c.triangle(p, col);
        }
    }
    c.text(20, 12, "SIDE", [60, 60, 60], 3);
    c.text(20, 442, "TOP", [60, 60, 60], 3);
    // Ground line and axle marks in the side view.
    let gy = 420.0 - (0.0 - lo.z) * scale;
    c.line((0.0, gy), (w as f64, gy), [120, 120, 120], 1, 6.0);
    c.to_png()
}
