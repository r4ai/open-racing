//! The gas network as a diagram: the intake to the left of the engine, the exhaust to its
//! right, volumes as boxes, pipes as lines labelled with their length and bore, the
//! terminals where parts join, and the mouths where the sound leaves. The layout follows
//! the network (by distance from the engine's ports), so nothing about it is stored.

use std::collections::{BTreeMap, HashMap, VecDeque};

use bevy_egui::egui;
use open_racing_engine_sim::spec::{End, Network};

/// A network drawn on one side of the engine.
pub struct Side<'a> {
    /// The part it belongs to (`intake/…`).
    pub part: String,
    pub network: &'a Network,
    /// Drawn leftwards of the engine.
    pub left: bool,
}

fn node_of(pipe: &str, end: &End, which: char) -> String {
    match end {
        End::Closed => format!("closed:{pipe}:{which}"),
        End::Volume { name, .. } => format!("volume:{name}"),
        End::Ambient { .. } => format!("air:{pipe}:{which}"),
        End::Terminal(t) => format!("terminal:{t}"),
        End::Join(j) => format!("join:{j}"),
    }
}

/// Draws the networks around an engine of `cylinders`; returns a clicked element
/// (part, pipe or volume name).
pub fn show(
    ui: &mut egui::Ui,
    sides: &[Side],
    cylinders: usize,
    selected: Option<&(String, String)>,
) -> Option<(String, String)> {
    let size = egui::vec2(
        ui.available_width().max(400.0),
        ui.available_height().max(300.0),
    );
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, egui::Color32::from_gray(28));
    let font = egui::FontId::proportional(11.0);
    let text = egui::Color32::from_gray(190);
    let centre_x = rect.center().x;
    let engine = egui::Rect::from_center_size(
        egui::pos2(centre_x, rect.center().y),
        egui::vec2(
            70.0,
            (cylinders as f32 * 22.0).clamp(80.0, rect.height() - 40.0),
        ),
    );
    painter.rect_filled(engine, 4.0, egui::Color32::from_gray(70));
    painter.text(
        engine.center_top() + egui::vec2(0.0, -4.0),
        egui::Align2::CENTER_BOTTOM,
        "engine",
        font.clone(),
        text,
    );
    let port_y =
        |n: usize| engine.top() + (n as f32 - 0.5) / cylinders.max(1) as f32 * engine.height();
    let mut clicked = None;
    let mut best = f32::MAX;
    let pointer = resp.interact_pointer_pos();
    for c in 1..=cylinders {
        painter.text(
            egui::pos2(engine.center().x, port_y(c)),
            egui::Align2::CENTER_CENTER,
            format!("{c}"),
            font.clone(),
            text,
        );
    }
    for side in sides {
        let net = side.network;
        // Graph: node → (pipe, other node).
        let mut adj: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
        for (i, p) in net.pipes.iter().enumerate() {
            let (a, b) = (node_of(&p.name, &p.a, 'a'), node_of(&p.name, &p.b, 'b'));
            adj.entry(a.clone()).or_default().push((i, b.clone()));
            adj.entry(b).or_default().push((i, a));
        }
        // Layers from the terminals.
        let mut layer: HashMap<String, usize> = HashMap::new();
        let mut q = VecDeque::new();
        for n in adj.keys().filter(|n| n.starts_with("terminal:")) {
            layer.insert(n.clone(), 0);
            q.push_back(n.clone());
        }
        if q.is_empty()
            && let Some(n) = adj.keys().next()
        {
            layer.insert(n.clone(), 0);
            q.push_back(n.clone());
        }
        while let Some(n) = q.pop_front() {
            let l = layer[&n];
            for (_, m) in &adj[&n] {
                if !layer.contains_key(m) {
                    layer.insert(m.clone(), l + 1);
                    q.push_back(m.clone());
                }
            }
        }
        for n in adj.keys() {
            layer.entry(n.clone()).or_insert(0);
        }
        let depth = layer.values().copied().max().unwrap_or(0).max(1);
        let span = (rect.width() * 0.5 - engine.width() * 0.5 - 40.0).max(80.0);
        let dx = span / depth as f32;
        let mut by_layer: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (n, l) in &layer {
            by_layer.entry(*l).or_default().push(n.clone());
        }
        let mut pos: HashMap<String, egui::Pos2> = HashMap::new();
        for (l, nodes) in &mut by_layer {
            // Terminals at their port's height; the rest spread evenly, in name order.
            nodes.sort();
            let x = if side.left {
                engine.left() - 20.0 - *l as f32 * dx
            } else {
                engine.right() + 20.0 + *l as f32 * dx
            };
            let n = nodes.len();
            for (k, name) in nodes.iter().enumerate() {
                let y = match name
                    .strip_prefix("terminal:")
                    .and_then(|t| t.rsplit('.').next())
                    .and_then(|c| c.parse::<usize>().ok())
                {
                    Some(c) if *l == 0 => port_y(c),
                    _ => rect.top() + 30.0 + (k as f32 + 0.5) / n as f32 * (rect.height() - 60.0),
                };
                pos.insert(name.clone(), egui::pos2(x, y));
            }
        }
        // Pipes.
        for (i, p) in net.pipes.iter().enumerate() {
            let (a, b) = (node_of(&p.name, &p.a, 'a'), node_of(&p.name, &p.b, 'b'));
            let (pa, pb) = (pos[&a], pos[&b]);
            let d = p.diameter.iter().map(|d| d.1).sum::<f64>() / p.diameter.len().max(1) as f64;
            let sel = selected.is_some_and(|(part, n)| *part == side.part && *n == p.name);
            let colour = if sel {
                egui::Color32::from_rgb(255, 160, 40)
            } else if p.friction > 1.5 {
                egui::Color32::from_rgb(150, 120, 90)
            } else {
                egui::Color32::from_rgb(120, 160, 200)
            };
            painter.line_segment(
                [pa, pb],
                egui::Stroke::new((d * 120.0).clamp(1.5, 9.0) as f32, colour),
            );
            // Labelled when selected or pointed at, or when the side has few pipes.
            let hovered = resp
                .hover_pos()
                .is_some_and(|h| segment_distance(h, pa, pb) < 7.0);
            if sel || hovered || net.pipes.len() <= 8 {
                let mid = pa + (pb - pa) * 0.5;
                painter.text(
                    mid + egui::vec2(0.0, -6.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!(
                        "{} {:.0} mm, {:.0} mm bore",
                        p.name,
                        p.length * 1e3,
                        d * 1e3
                    ),
                    egui::FontId::proportional(10.0),
                    if sel || hovered {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_gray(150)
                    },
                );
            }
            if let Some(pp) = pointer {
                let dist = segment_distance(pp, pa, pb);
                if dist < 7.0 && dist < best {
                    best = dist;
                    clicked = Some((side.part.clone(), p.name.clone()));
                }
            }
            let _ = i;
        }
        // Nodes.
        for (n, p) in &pos {
            if let Some(v) = n.strip_prefix("volume:") {
                let litres = net
                    .volumes
                    .iter()
                    .find(|x| x.name == v)
                    .map_or(0.0, |x| x.volume * 1e3);
                let r = egui::Rect::from_center_size(
                    *p,
                    egui::vec2(
                        16.0 + (litres as f32).sqrt() * 8.0,
                        16.0 + (litres as f32).sqrt() * 5.0,
                    ),
                );
                let sel = selected.is_some_and(|(part, s)| *part == side.part && s == v);
                painter.rect_filled(
                    r,
                    3.0,
                    if sel {
                        egui::Color32::from_rgb(200, 120, 30)
                    } else {
                        egui::Color32::from_gray(95)
                    },
                );
                painter.text(
                    r.center_bottom() + egui::vec2(0.0, 2.0),
                    egui::Align2::CENTER_TOP,
                    format!("{v} {litres:.2} l"),
                    font.clone(),
                    text,
                );
                if pointer.is_some_and(|pp| r.expand(3.0).contains(pp)) {
                    best = 0.0;
                    clicked = Some((side.part.clone(), v.to_string()));
                }
            } else if n.starts_with("air:") {
                let s = 7.0;
                let dir = if side.left { -1.0 } else { 1.0 };
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        *p + egui::vec2(dir * s, 0.0),
                        *p + egui::vec2(-dir * s, -s),
                        *p + egui::vec2(-dir * s, s),
                    ],
                    egui::Color32::from_rgb(120, 200, 120),
                    egui::Stroke::NONE,
                ));
            } else if let Some(t) = n.strip_prefix("terminal:") {
                painter.circle_filled(*p, 3.5, egui::Color32::from_gray(200));
                let _ = t;
            } else if n.starts_with("join:") {
                painter.circle_stroke(
                    *p,
                    3.0,
                    egui::Stroke::new(1.0, egui::Color32::from_gray(200)),
                );
            } else {
                painter.line_segment(
                    [*p + egui::vec2(0.0, -6.0), *p + egui::vec2(0.0, 6.0)],
                    egui::Stroke::new(2.0, egui::Color32::RED),
                );
            }
        }
    }
    if resp.clicked() { clicked } else { None }
}

fn segment_distance(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}
