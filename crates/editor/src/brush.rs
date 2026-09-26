//! Brushes in the 3D view, as Blender's sculpting and texture painting: Sculpt Terrain
//! shapes the ground, Paint Ground paints its layers (dirt, gravel, sand), and Scatter
//! paints woods, bushes and rocks over it. A drag is one stroke, saved as an operation
//! when the button is let go; while it is dragged the view shows what it does.
//!
//! Ctrl turns a brush round (lowers, paints the ground's own material back, wipes
//! models out), Shift smooths; F sets the radius and Shift F the strength by moving the
//! mouse, [ and ] the radius in steps.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy_egui::egui;
use glam::{DVec2, DVec3};
use open_racing_track_project::ops::{Op, StrokeTarget};
use open_racing_track_project::project::{
    Brush, GroundLayer, LayerStroke, MAX_LAYERS, Scatter, ScatterModel, Stroke,
};
use open_racing_track_project::terrain::{Grid, PaintMask, TerrainBuild};
use open_racing_track_render::to_bevy;

use crate::commands::Ctx;
use crate::preview::{Built, GroundPaint, TerrainChunk};
use crate::properties::{row, section};
use crate::state::Editor;
use crate::viewport::{Tool, ToolKind};

/// What the Sculpt tool does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sculpt {
    #[default]
    Raise,
    Lower,
    Smooth,
    /// Levels the ground at the height where the stroke began.
    Flatten,
    Noise,
}

impl Sculpt {
    pub const ALL: [Sculpt; 5] = [
        Sculpt::Raise,
        Sculpt::Lower,
        Sculpt::Smooth,
        Sculpt::Flatten,
        Sculpt::Noise,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sculpt::Raise => "Raise",
            Sculpt::Lower => "Lower",
            Sculpt::Smooth => "Smooth",
            Sculpt::Flatten => "Flatten",
            Sculpt::Noise => "Roughen",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Sculpt::Raise => "Lifts the ground by the height under the brush (Ctrl: lowers)",
            Sculpt::Lower => "Digs the ground by the height under the brush (Ctrl: raises)",
            Sculpt::Smooth => "Evens bumps out (Shift with any brush)",
            Sculpt::Flatten => "Levels the ground at the height where the stroke began",
            Sculpt::Noise => "Roughens the ground by up to the height, the same way every build",
        }
    }

    /// Whether its strength is a height, m, rather than a share.
    fn by_height(self) -> bool {
        matches!(self, Sculpt::Raise | Sculpt::Lower | Sculpt::Noise)
    }
}

/// A brush's size and strength, for one tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    /// How far a stroke reaches from its path, m.
    pub radius: f64,
    /// 0 to 1: how far a stroke goes towards its target.
    pub strength: f64,
}

/// The brushes' settings, and the stroke being painted.
pub struct Brushes {
    pub sculpt: Sculpt,
    /// Sizes of Sculpt, Paint Ground and Scatter.
    pub sizes: [Size; 3],
    /// How far Raise, Lower and Roughen move the ground in a stroke, m.
    pub height: f64,
    /// The ground layer Paint Ground paints; with none, the ground's own material.
    pub layer: Option<String>,
    /// Paint Ground paints the ground's own material, by choice: else the first layer
    /// is painted when none is chosen.
    pub own: bool,
    /// The scatter Scatter paints.
    pub scatter: Option<String>,
    /// Scatter wipes models out rather than adding them (as Ctrl does).
    pub erase: bool,
    /// Lasso fill: a stroke's path is an outline, acting fully inside it (L).
    pub fill: bool,
    pub stroke: Option<Live>,
    /// F or Shift F: the radius or strength being set with the mouse.
    pub adjust: Option<Adjust>,
}

impl Default for Brushes {
    fn default() -> Self {
        Self {
            sculpt: Sculpt::default(),
            sizes: [
                Size {
                    radius: 25.0,
                    strength: 0.5,
                },
                Size {
                    radius: 6.0,
                    strength: 1.0,
                },
                Size {
                    radius: 25.0,
                    strength: 0.7,
                },
            ],
            height: 2.0,
            layer: None,
            own: false,
            scatter: None,
            erase: false,
            fill: false,
            stroke: None,
            adjust: None,
        }
    }
}

impl Brushes {
    /// The settings alone, for another project.
    pub fn settings(&self) -> Self {
        Self {
            sculpt: self.sculpt,
            sizes: self.sizes,
            height: self.height,
            erase: self.erase,
            fill: self.fill,
            ..Default::default()
        }
    }

    fn slot(tool: ToolKind) -> usize {
        match tool {
            ToolKind::Paint => 1,
            ToolKind::Scatter => 2,
            _ => 0,
        }
    }

    pub fn size(&self, tool: ToolKind) -> Size {
        self.sizes[Self::slot(tool)]
    }

    pub fn size_mut(&mut self, tool: ToolKind) -> &mut Size {
        &mut self.sizes[Self::slot(tool)]
    }
}

/// What F and Shift F set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Radius,
    Strength,
}

/// Setting the radius or strength with the mouse: its value when it began, and where
/// the pointer was.
pub struct Adjust {
    pub what: Setting,
    start: f64,
    from: Vec2,
}

/// A stroke being painted, and what the view shows of it so far.
pub struct Live {
    pub to: StrokeTarget,
    pub stroke: Stroke,
    /// The terrain as the stroke began.
    base: Option<Arc<TerrainBuild>>,
    /// The terrain's heights or its mask with the stroke so far.
    grid: Option<Grid>,
    mask: Option<PaintMask>,
    /// Points shown so far, and the build they were shown on.
    shown: usize,
    built: u64,
    /// When the mask was last given to the renderer.
    sent: Option<Instant>,
    /// Scatter: where its models will stand, near the stroke.
    pub dots: Vec<DVec3>,
}

/// How far apart a stroke's points are, m.
fn spacing(radius: f64, fill: bool) -> f64 {
    let s = (0.15 * radius).max(0.25);
    // An outline wants its shape more than its reach.
    if fill { s.min(2.0) } else { s }
}

/// The scatter and ground layer the brushes paint, made valid: a layer or scatter
/// gone (an undo, a rename) is dropped for the first there is.
pub fn settle(editor: &Editor, b: &mut Brushes) {
    let p = &editor.project;
    if b.layer
        .as_ref()
        .is_none_or(|l| !p.terrain.layers.iter().any(|x| &x.name == l))
    {
        b.layer = p
            .terrain
            .layers
            .first()
            .filter(|_| !b.own)
            .map(|l| l.name.clone());
    }
    if b.scatter
        .as_ref()
        .is_none_or(|s| !p.scatter.iter().any(|x| &x.name == s))
    {
        b.scatter = p.scatter.first().map(|s| s.name.clone());
    }
}

/// Starts a stroke at `at` on the ground.
fn start(
    editor: &Editor,
    tool: &Tool,
    built: &Built,
    at: DVec3,
    ctrl: bool,
    shift: bool,
) -> Result<Live, String> {
    let b = &tool.brush;
    let size = b.size(tool.active);
    let p = at.truncate();
    let mut live = Live {
        to: StrokeTarget::Sculpt,
        stroke: Stroke {
            brush: Brush::Paint,
            radius: size.radius,
            strength: size.strength,
            points: vec![p],
            fill: b.fill,
        },
        base: built.terrain.clone(),
        grid: None,
        mask: None,
        shown: 0,
        built: built.count,
        sent: None,
        dots: vec![],
    };
    match tool.active {
        ToolKind::Sculpt => {
            let t = built
                .terrain
                .as_ref()
                .ok_or("the terrain is off: switch it on in the Terrain tab to sculpt it")?;
            let mode = match b.sculpt {
                _ if shift => Sculpt::Smooth,
                Sculpt::Raise if ctrl => Sculpt::Lower,
                Sculpt::Lower if ctrl => Sculpt::Raise,
                m => m,
            };
            let level = t.grid.height_at(p).unwrap_or(at.z);
            (live.stroke.brush, live.stroke.strength) = match mode {
                Sculpt::Raise => (Brush::Raise, b.height),
                Sculpt::Lower => (Brush::Raise, -b.height),
                Sculpt::Noise => (Brush::Noise, b.height),
                Sculpt::Smooth => (Brush::Smooth, size.strength),
                Sculpt::Flatten => (Brush::Flatten(round(level)), size.strength),
            };
            live.grid = Some((*t.grid).clone());
        }
        ToolKind::Paint => {
            let t = built
                .terrain
                .as_ref()
                .ok_or("the terrain is off: switch it on in the Terrain tab to paint it")?;
            let mask = t.mask.as_ref().ok_or(
                "add a ground layer to paint first (the brush's settings, or the Terrain tab)",
            )?;
            let layer = if ctrl { None } else { b.layer.clone() };
            live.to = StrokeTarget::Paint(layer);
            live.mask = Some((**mask).clone());
        }
        ToolKind::Scatter => {
            let name = b
                .scatter
                .clone()
                .ok_or("add a scatter to paint first (the brush's settings, or the Scatter tab)")?;
            if !editor.project.scatter.iter().any(|s| s.name == name) {
                return Err(format!("there is no scatter \"{name}\" any more"));
            }
            live.to = StrokeTarget::Scatter(name);
            live.stroke.brush = if ctrl || b.erase {
                Brush::Erase
            } else {
                Brush::Paint
            };
        }
        _ => return Err("not a brush".into()),
    }
    Ok(live)
}

/// A value kept to the centimetre, for the file.
fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// The brush under the pointer: strokes, F and Shift F, [ and ]. Called by the view's
/// input while a brush tool is active; true while it has the pointer.
#[allow(clippy::too_many_arguments)]
pub fn input(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    over: Option<Vec2>,
    anywhere: Option<Vec2>,
    keys_free: bool,
) {
    settle(editor, &mut tool.brush);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let kind = tool.active;
    // Setting the radius or strength with the mouse.
    if let Some(a) = &tool.brush.adjust {
        let dx = anywhere.map_or(0.0, |p| (p.x - a.from.x) as f64);
        let (what, start) = (a.what, a.start);
        let size = tool.brush.size_mut(kind);
        match what {
            Setting::Radius => size.radius = (start * 2f64.powf(dx / 150.0)).clamp(0.5, 500.0),
            Setting::Strength => size.strength = (start + dx / 300.0).clamp(0.01, 1.0),
        }
        let size = *size;
        tool.hint = match what {
            Setting::Radius => format!("Radius {:.1} m", size.radius),
            Setting::Strength => format!("Strength {:.0} %", size.strength * 100.0),
        } + " · move the mouse · click or Enter sets it · right click or Esc goes back";
        if buttons.just_pressed(MouseButton::Left)
            || keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter])
        {
            tool.brush.adjust = None;
        } else if buttons.just_pressed(MouseButton::Right) || keys.just_pressed(KeyCode::Escape) {
            let size = tool.brush.size_mut(kind);
            match what {
                Setting::Radius => size.radius = start,
                Setting::Strength => size.strength = start,
            }
            tool.brush.adjust = None;
        }
        return;
    }

    // A stroke: pressed on the ground, points added as the pointer moves, saved when
    // the button is let go.
    if let Some(live) = &mut tool.brush.stroke {
        if let Some(at) = tool.pointer {
            let p = at.truncate();
            let last = *live.stroke.points.last().expect("a stroke has a point");
            if p.distance(last) >= spacing(live.stroke.radius, live.stroke.fill) {
                live.stroke.points.push(p);
            }
        }
        if !buttons.pressed(MouseButton::Left) {
            let live = tool.brush.stroke.take().expect("a stroke");
            commit(editor, live);
        } else if keys.just_pressed(KeyCode::Escape) {
            // Dropped: the next build puts the view back.
            tool.brush.stroke = None;
            editor.revision += 1;
            editor.status = "stroke dropped".into();
        }
        tool.hint = "Painting a stroke · let go to finish · Esc drops it".into();
        return;
    }
    if over.is_some()
        && buttons.just_pressed(MouseButton::Left)
        && let Some(at) = tool.pointer
    {
        match start(editor, tool, built, at, ctrl, shift) {
            Ok(live) => tool.brush.stroke = Some(live),
            Err(e) => editor.status = e,
        }
    }
    tool.hint = hint(tool, ctrl, shift);
    if !keys_free || over.is_none() {
        return;
    }
    if keys.just_pressed(KeyCode::KeyL) {
        tool.brush.fill = !tool.brush.fill;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        let what = if shift {
            Setting::Strength
        } else {
            Setting::Radius
        };
        let size = tool.brush.size(kind);
        tool.brush.adjust = Some(Adjust {
            what,
            start: match what {
                Setting::Radius => size.radius,
                Setting::Strength => size.strength,
            },
            from: over.unwrap_or_default(),
        });
    }
    let step = keys.just_pressed(KeyCode::BracketRight) as i32
        - keys.just_pressed(KeyCode::BracketLeft) as i32;
    if step != 0 {
        let size = tool.brush.size_mut(kind);
        size.radius = (size.radius * 1.25f64.powi(step)).clamp(0.5, 500.0);
    }
}

/// What the mouse does with the active brush, for the header and status bar.
fn hint(tool: &Tool, ctrl: bool, shift: bool) -> String {
    let b = &tool.brush;
    let size = b.size(tool.active);
    let what = match tool.active {
        ToolKind::Sculpt if shift => "smooth".to_string(),
        ToolKind::Sculpt => {
            let m = match b.sculpt {
                Sculpt::Raise if ctrl => Sculpt::Lower,
                Sculpt::Lower if ctrl => Sculpt::Raise,
                m => m,
            };
            m.label().to_lowercase()
        }
        ToolKind::Paint if ctrl => "paint the ground's own material".into(),
        ToolKind::Paint => format!(
            "paint {}",
            b.layer.as_deref().unwrap_or("the ground's own material")
        ),
        ToolKind::Scatter if ctrl || b.erase => {
            format!(
                "wipe out {}",
                b.scatter.as_deref().unwrap_or("(no scatter)")
            )
        }
        ToolKind::Scatter => format!("scatter {}", b.scatter.as_deref().unwrap_or("(no scatter)")),
        _ => String::new(),
    };
    let turn = match tool.active {
        ToolKind::Sculpt => "Ctrl: the other way · Shift: smooth",
        ToolKind::Paint => "Ctrl: the ground's own",
        _ => "Ctrl: wipe out",
    };
    let (drag, lasso) = if b.fill {
        ("Drag round an area to", "L: stroke")
    } else {
        ("Drag to", "L: lasso fill")
    };
    format!(
        "{drag} {what} · radius {:.1} m (F, [ ]) · strength {:.0} % (Shift F) · {turn} · {lasso}",
        size.radius,
        size.strength * 100.0
    )
}

/// Saves a stroke as an operation: the points its path needs to keep its shape, to
/// the centimetre.
fn commit(editor: &mut Editor, live: Live) {
    let mut stroke = live.stroke;
    if stroke.fill && stroke.points.len() < 3 {
        editor.status = "lasso fill: drag round the area to fill (L switches it off)".into();
        return;
    }
    let path: Vec<DVec3> = stroke.points.iter().map(|p| p.extend(0.0)).collect();
    let tolerance = (0.03 * stroke.radius).clamp(0.05, 0.5);
    stroke.points = crate::viewport::simplify(&path, tolerance)
        .iter()
        .map(|p| DVec2::new(round(p.x), round(p.y)))
        .collect();
    stroke.radius = round(stroke.radius);
    stroke.strength = (stroke.strength * 1000.0).round() / 1000.0;
    let n = stroke.points.len();
    if editor.apply(
        vec![Op::AddStroke {
            to: live.to.clone(),
            stroke,
        }],
        None,
    ) {
        editor.status = match live.to {
            StrokeTarget::Sculpt => format!("sculpted: a stroke of {n} points"),
            StrokeTarget::Paint(Some(l)) => format!("painted {l}"),
            StrokeTarget::Paint(None) => "painted the ground's own material back".into(),
            StrokeTarget::Scatter(s) => format!("scattered {s}"),
        };
    }
}

/// Shows the stroke being painted: the terrain's chunks it changes, sculpted again;
/// the mask, painted again; or where a scatter's models will stand.
#[allow(clippy::too_many_arguments)]
pub fn live(
    mut tool: ResMut<Tool>,
    built: Res<Built>,
    editor: Res<Editor>,
    paint: Res<GroundPaint>,
    chunks: Query<(&TerrainChunk, &Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(live) = &mut tool.brush.stroke else {
        return;
    };
    // A build finished since: its chunks are new and need the stroke too.
    let rebuilt = live.built != built.count;
    if live.shown == live.stroke.points.len() && !rebuilt {
        return;
    }
    live.shown = live.stroke.points.len();
    live.built = built.count;
    let (lo, hi) = live.stroke.bounds();
    let project = &editor.project;
    match &live.to {
        StrokeTarget::Sculpt => {
            let (Some(base), Some(grid)) = (&live.base, &mut live.grid) else {
                return;
            };
            let Some(span) = grid.span(lo, hi) else {
                return;
            };
            // The stroke so far on the ground as it was.
            let [i0, j0, i1, j1] = span;
            for j in j0..=j1 {
                let row = j * grid.nx;
                grid.z[row + i0..=row + i1].copy_from_slice(&base.grid.z[row + i0..=row + i1]);
            }
            grid.sculpt(&live.stroke);
            // The chunks it reaches, and one round them for their normals.
            let tile = project
                .material_index(&project.terrain.material)
                .map_or([1.0; 2], |m| project.materials[m].tile);
            let mask = base.mask.clone();
            let uv = |p: DVec2| match &mask {
                Some(m) => m.uv(p),
                None => [p.x as f32 / tile[0], p.y as f32 / tile[1]],
            };
            let spans = grid.chunks();
            for (chunk, mesh) in &chunks {
                let Some(&[a0, b0, a1, b1]) = spans.get(chunk.0) else {
                    continue;
                };
                if a1 + 1 < i0 || a0 > i1 + 1 || b1 + 1 < j0 || b0 > j1 + 1 {
                    continue;
                }
                if let Some(mut m) = meshes.get_mut(&mesh.0) {
                    *m = crate::preview::to_mesh(grid.mesh([a0, b0, a1, b1], Some(&uv)));
                }
            }
        }
        StrokeTarget::Paint(layer) => {
            let (Some(base), Some(mask)) = (&live.base, &mut live.mask) else {
                return;
            };
            let Some(base_mask) = &base.mask else { return };
            let Some([x0, y0, x1, y1]) = mask.span(lo, hi) else {
                return;
            };
            for y in y0..=y1 {
                let row = (y * mask.width + x0) * 4..(y * mask.width + x1 + 1) * 4;
                mask.rgba[row.clone()].copy_from_slice(&base_mask.rgba[row]);
            }
            mask.paint(
                &project.terrain.layers,
                &LayerStroke {
                    layer: layer.clone(),
                    stroke: live.stroke.clone(),
                },
            );
            // Big masks go to the renderer a few times a second.
            let every = Duration::from_millis(if mask.rgba.len() > 16 << 20 { 120 } else { 40 });
            if live.sent.is_none_or(|t| t.elapsed() > every) || rebuilt {
                live.sent = Some(Instant::now());
                if let Some(h) = &paint.mask
                    && let Some(mut image) = images.get_mut(h)
                    && image
                        .data
                        .as_ref()
                        .is_some_and(|d| d.len() == mask.rgba.len())
                {
                    image.data = Some(mask.rgba.clone());
                }
            }
        }
        StrokeTarget::Scatter(name) => {
            let Some(s) = project.scatter.iter().find(|s| &s.name == name) else {
                return;
            };
            let mut s = s.clone();
            s.strokes.push(live.stroke.clone());
            let ground = built.ground.as_deref();
            live.dots = open_racing_track_project::scatter::planned(&s)
                .into_iter()
                .filter(|(p, ..)| p.cmpge(lo).all() && p.cmple(hi).all())
                .take(4000)
                .map(|(p, ..)| {
                    let top = p.extend(1e4);
                    ground
                        .and_then(|g| g.raycast_down(top, 2e4))
                        .map_or(p.extend(0.0), |h| h.point)
                })
                .collect();
        }
    }
}

/// The brush on the ground under the pointer: its reach, and the half it acts fully
/// in; and the stroke's scatter.
pub fn draw(tool: &Tool, built: &Built, gizmos: &mut Gizmos) {
    if !tool.active.is_brush() || tool.modal.is_some() {
        return;
    }
    let ground = built.ground.as_deref();
    let on_ground = |p: DVec2, z: f64| {
        ground
            .and_then(|g| {
                g.raycast_down(p.extend(z + 50.0), 500.0)
                    .or_else(|| g.raycast_down(p.extend(1e4), 2e4))
            })
            .map_or(p.extend(z), |h| h.point)
            + DVec3::Z * 0.3
    };
    let b = &tool.brush;
    let colour = match (&b.stroke, tool.active) {
        (Some(l), _) if l.stroke.brush == Brush::Erase => Color::srgb(1.0, 0.35, 0.3),
        (_, ToolKind::Sculpt) => Color::srgb(0.95, 0.95, 1.0),
        (_, ToolKind::Paint) => Color::srgb(1.0, 0.8, 0.35),
        _ if b.erase => Color::srgb(1.0, 0.35, 0.3),
        _ => Color::srgb(0.45, 1.0, 0.45),
    };
    if let Some(live) = &b.stroke {
        // A lasso's outline so far, closed.
        if live.stroke.fill {
            let s = &live.stroke;
            let outline: Vec<Vec3> = s
                .points
                .iter()
                .chain(s.points.first())
                .map(|p| to_bevy(on_ground(*p, at_z(tool))))
                .collect();
            gizmos.linestrip(outline, colour);
        }
        for d in &live.dots {
            gizmos.sphere(
                Isometry3d::from_translation(to_bevy(*d + DVec3::Z * 0.5)),
                0.35,
                Color::srgb(0.3, 0.9, 0.3),
            );
        }
    }
    let Some(at) = tool.pointer else { return };
    let radius = b.size(tool.active).radius;
    // A lasso: a small ring at the pointer, the outline being what fills.
    if b.fill && b.stroke.is_none() {
        let ring: Vec<Vec3> = (0..=24)
            .map(|k| {
                let a = k as f64 / 24.0 * std::f64::consts::TAU;
                to_bevy(on_ground(at.truncate() + DVec2::from_angle(a) * 1.0, at.z))
            })
            .collect();
        gizmos.linestrip(ring, colour);
        return;
    }
    for (r, alpha) in [(radius, 1.0), (0.5 * radius, 0.45)] {
        let ring: Vec<Vec3> = (0..=64)
            .map(|k| {
                let a = k as f64 / 64.0 * std::f64::consts::TAU;
                to_bevy(on_ground(at.truncate() + DVec2::from_angle(a) * r, at.z))
            })
            .collect();
        gizmos.linestrip(ring, colour.with_alpha(alpha));
    }
}

/// The height to look for the ground from under the pointer.
fn at_z(tool: &Tool) -> f64 {
    tool.pointer.map_or(0.0, |p| p.z)
}

/// The ground layers a project starts painting with, by what they are made of.
const LAYER_KINDS: [(&str, &str, &str); 4] = [
    ("dirt", "dirt", "dirt"),
    ("gravel", "gravel", "gravel"),
    ("sand", "gravel", "gravel"),
    ("asphalt", "runoff", "asphalt"),
];

/// A kind of scatter to start from: a name, its built-in models with their weights, the
/// spacing and the sizes.
pub type ScatterKind = (&'static str, &'static [(&'static str, f64)], f64, [f64; 2]);

/// Kinds of scatters to start from.
pub const SCATTER_KINDS: [ScatterKind; 5] = [
    (
        "woods",
        &[("pine", 3.0), ("tree", 2.0), ("poplar", 1.0)],
        7.0,
        [0.8, 1.25],
    ),
    ("pines", &[("pine", 1.0)], 6.0, [0.75, 1.3]),
    ("bushes", &[("bush", 3.0), ("rock", 1.0)], 4.0, [0.7, 1.4]),
    ("long grass", &[("grass", 1.0)], 1.2, [0.7, 1.5]),
    ("rocks", &[("rock", 1.0)], 5.0, [0.5, 1.8]),
];

/// A new scatter of a kind, named so that it is free.
fn new_scatter(editor: &Editor, kind: usize) -> Scatter {
    let (name, models, spacing, scale) = SCATTER_KINDS[kind];
    let p = &editor.project;
    Scatter {
        name: crate::presets::free_name(name, |n| p.scatter.iter().any(|s| s.name == n)),
        models: models
            .iter()
            .map(|(m, w)| ScatterModel {
                model: open_racing_track_project::shapes::path(m),
                weight: *w,
            })
            .collect(),
        spacing,
        scale,
        tilt: if name == "rocks" { 0.6 } else { 0.1 },
        clearance: 3.0,
        max_slope: 35.0,
        collide: false,
        strokes: vec![],
        group: None,
    }
}

/// Adds a scatter of a kind of `SCATTER_KINDS`, and paints it next.
pub fn add_scatter(c: &mut Ctx, kind: usize) {
    let s = new_scatter(c.editor, kind);
    let name = s.name.clone();
    if c.editor.apply(vec![Op::PutScatter { scatter: s }], None) {
        c.tool.brush.scatter = Some(name.clone());
        c.tool.active = ToolKind::Scatter;
        c.editor.status = format!(
            "\"{name}\": drag over the ground to plant it (the Scatter tab sets its models)"
        );
    }
}

/// The active brush's settings: what it paints, its size and strength. Shown in the
/// view's header and the sidebar's Tool tab.
pub fn settings_ui(ui: &mut egui::Ui, c: &mut Ctx, compact: bool) {
    settle(c.editor, &mut c.tool.brush);
    let kind = c.tool.active;
    let b = &mut c.tool.brush;
    match kind {
        ToolKind::Sculpt => {
            let mut sculpt = b.sculpt;
            for s in Sculpt::ALL {
                ui.selectable_value(&mut sculpt, s, s.label())
                    .on_hover_text(s.tip());
            }
            b.sculpt = sculpt;
            if b.sculpt.by_height() {
                slider(ui, &mut b.height, 0.05..=30.0, "Height", " m", compact)
                    .on_hover_text("How far a stroke moves the ground where it acts fully");
            }
        }
        ToolKind::Paint => {
            let layers: Vec<String> = c
                .editor
                .project
                .terrain
                .layers
                .iter()
                .map(|l| l.name.clone())
                .collect();
            if ui
                .selectable_label(b.layer.is_none(), "Ground")
                .on_hover_text("The ground's own material, painted back (Ctrl with a layer)")
                .clicked()
            {
                b.layer = None;
                b.own = true;
            }
            for l in &layers {
                if ui
                    .selectable_label(b.layer.as_ref() == Some(l), l)
                    .clicked()
                {
                    b.layer = Some(l.clone());
                    b.own = false;
                }
            }
            if layers.len() < MAX_LAYERS {
                ui.menu_button("+ Layer", |ui| {
                    for (name, surface, material) in LAYER_KINDS {
                        if ui.button(name).clicked() {
                            add_layer(c, name, surface, material);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text("A material painted over the ground, with the grip of its surface");
            }
        }
        ToolKind::Scatter => {
            let names: Vec<String> = c
                .editor
                .project
                .scatter
                .iter()
                .map(|s| s.name.clone())
                .collect();
            egui::ComboBox::from_id_salt("scatter brush")
                .selected_text(b.scatter.as_deref().unwrap_or("none yet"))
                .show_ui(ui, |ui| {
                    for n in &names {
                        ui.selectable_value(&mut b.scatter, Some(n.clone()), n);
                    }
                });
            ui.menu_button("+ New", |ui| {
                for (k, (name, models, ..)) in SCATTER_KINDS.iter().enumerate() {
                    let what: Vec<&str> = models.iter().map(|m| m.0).collect();
                    if ui
                        .button(*name)
                        .on_hover_text(format!("Built-in {}", what.join(", ")))
                        .clicked()
                    {
                        add_scatter(c, k);
                        ui.close();
                    }
                }
            });
            let b = &mut c.tool.brush;
            ui.selectable_value(&mut b.erase, false, "Add");
            ui.selectable_value(&mut b.erase, true, "Wipe out");
        }
        _ => return,
    }
    let b = &mut c.tool.brush;
    ui.separator();
    ui.selectable_value(&mut b.fill, false, "Stroke")
        .on_hover_text("A drag paints along its path");
    ui.selectable_value(&mut b.fill, true, "Lasso fill")
        .on_hover_text("A drag outlines an area, filled fully; the radius softens its edge (L)");
    let b = &mut c.tool.brush;
    let by_height = kind == ToolKind::Sculpt && b.sculpt.by_height();
    let size = b.size_mut(kind);
    slider(ui, &mut size.radius, 0.5..=300.0, "Radius", " m", compact)
        .on_hover_text("F, or [ and ], in the view");
    if !by_height {
        let mut percent = size.strength * 100.0;
        if slider(ui, &mut percent, 1.0..=100.0, "Strength", " %", compact)
            .on_hover_text("Shift F in the view")
            .changed()
        {
            size.strength = percent / 100.0;
        }
    }
}

/// A slider, labelled in the sidebar and short in the header.
fn slider(
    ui: &mut egui::Ui,
    v: &mut f64,
    range: std::ops::RangeInclusive<f64>,
    label: &str,
    suffix: &str,
    compact: bool,
) -> egui::Response {
    let log = *range.end() / range.start().max(1e-3) > 50.0;
    let s = egui::Slider::new(v, range)
        .suffix(suffix)
        .logarithmic(log)
        .max_decimals(1);
    if compact {
        ui.label(label);
        ui.add_sized([110.0, ui.spacing().interact_size.y], s)
    } else {
        row(ui, label, |ui| ui.add(s))
    }
}

/// Adds a ground layer of a kind, and paints it next.
fn add_layer(c: &mut Ctx, name: &str, surface: &str, material: &str) {
    let p = &c.editor.project;
    let name = crate::presets::free_name(name, |n| p.terrain.layers.iter().any(|l| l.name == n));
    let pick = |want: &str, list: Vec<&String>| {
        list.iter()
            .find(|n| n.as_str() == want)
            .or(list.first())
            .map(|n| n.to_string())
            .unwrap_or_default()
    };
    let layer = GroundLayer {
        name: name.clone(),
        surface: pick(surface, p.surfaces.iter().map(|s| &s.name).collect()),
        material: pick(material, p.materials.iter().map(|m| &m.name).collect()),
    };
    if c.editor.apply(vec![Op::PutGroundLayer { layer }], None) {
        c.tool.brush.layer = Some(name.clone());
        c.tool.brush.own = false;
        c.editor.status = format!("ground layer \"{name}\": drag over the ground to paint it");
    }
}

/// The Tool tab's panel for a brush.
pub fn sidebar(ui: &mut egui::Ui, c: &mut Ctx) {
    let title = c.tool.active.label();
    section(ui, title, "sidebar brush", true, |ui| {
        ui.horizontal_wrapped(|ui| settings_ui(ui, c, false));
        ui.weak(match c.tool.active {
            ToolKind::Sculpt => "Drag over the terrain. Ctrl: the other way · Shift: smooth · F: radius. The ground by the roads stays where it meets them. A small terrain cell (Terrain tab) shows finer shapes.",
            ToolKind::Paint => "Drag over the terrain to paint the layer; each layer drives with the grip of its surface. Ctrl paints the ground's own material back.",
            _ => "Drag over the ground to plant the scatter's models; paint again for more. Ctrl wipes them out. They keep off roads, kerbs and steep ground. Its models, spacing and sizes are in the Scatter tab.",
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_racing_track_project::bake;

    /// An editor on a new project, and what a build of it knows.
    fn editor(name: &str) -> (Editor, Built, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("open-racing-brush-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let editor = Editor::open(dir.clone()).unwrap();
        let built = built(&editor);
        (editor, built, dir)
    }

    fn built(editor: &Editor) -> Built {
        let scene = bake::build(&editor.project);
        let surfaces: Vec<_> = editor.project.surfaces.iter().map(|s| s.props).collect();
        Built {
            ground: Some(Arc::new(scene.ground.build(&surfaces))),
            terrain: scene.terrain.clone(),
            count: 1,
            ..Default::default()
        }
    }

    /// A drag from `from` to `to` on the ground with the active brush, `ctrl` held.
    fn drag(
        editor: &mut Editor,
        tool: &mut Tool,
        built: &Built,
        from: DVec2,
        to: DVec2,
        ctrl: bool,
    ) {
        let mut buttons = ButtonInput::<MouseButton>::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        if ctrl {
            keys.press(KeyCode::ControlLeft);
        }
        let over = Some(Vec2::new(400.0, 300.0));
        buttons.press(MouseButton::Left);
        for k in 0..=10 {
            tool.pointer = Some(from.lerp(to, k as f64 / 10.0).extend(0.0));
            input(editor, tool, built, &buttons, &keys, over, over, true);
            buttons.clear();
        }
        buttons.release(MouseButton::Left);
        input(editor, tool, built, &buttons, &keys, over, over, true);
    }

    #[test]
    fn a_drag_is_one_stroke_saved_as_an_operation() {
        let (mut editor, built, dir) = editor("sculpt");
        let mut tool = Tool::default();
        tool.active = ToolKind::Sculpt;
        let (a, b) = (DVec2::new(100.0, 130.0), DVec2::new(200.0, 130.0));
        drag(&mut editor, &mut tool, &built, a, b, false);
        let s = &editor.project.terrain.sculpt;
        assert_eq!(s.len(), 1, "{}", editor.status);
        assert_eq!(s[0].brush, Brush::Raise);
        assert_eq!(s[0].strength, tool.brush.height);
        // A straight drag keeps its ends alone.
        assert_eq!(s[0].points, vec![a, b]);
        assert!(tool.brush.stroke.is_none());
        // Ctrl lowers; Flatten levels where the stroke began.
        drag(&mut editor, &mut tool, &built, a, b, true);
        assert_eq!(
            editor.project.terrain.sculpt[1].strength,
            -tool.brush.height
        );
        tool.brush.sculpt = Sculpt::Flatten;
        drag(&mut editor, &mut tool, &built, a, b, false);
        assert!(matches!(
            editor.project.terrain.sculpt[2].brush,
            Brush::Flatten(_)
        ));
        // One undo step each.
        editor.undo();
        assert_eq!(editor.project.terrain.sculpt.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_sculpting_stroke_reshapes_the_terrain_s_chunks_while_it_is_dragged() {
        let (editor, built, dir) = editor("live");
        let terrain = built.terrain.clone().unwrap();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_resource::<GroundPaint>()
            .add_systems(Update, live);
        let handles: Vec<Handle<Mesh>> = {
            let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
            terrain
                .chunks
                .iter()
                .map(|c| meshes.add(crate::preview::to_mesh(c.clone())))
                .collect()
        };
        for (i, h) in handles.iter().enumerate() {
            app.world_mut().spawn((TerrainChunk(i), Mesh3d(h.clone())));
        }
        let mut tool = Tool::default();
        tool.active = ToolKind::Sculpt;
        let at = DVec2::new(150.0, 130.0);
        let mut live = start(&editor, &tool, &built, at.extend(0.0), false, false).unwrap();
        live.stroke.points.push(at + DVec2::X * 30.0);
        tool.brush.stroke = Some(live);
        app.insert_resource(editor)
            .insert_resource(built)
            .insert_resource(tool);
        let top = |app: &App| {
            let meshes = app.world().resource::<Assets<Mesh>>();
            handles
                .iter()
                .filter_map(|h| meshes.get(h))
                .flat_map(|m| match m.attribute(Mesh::ATTRIBUTE_POSITION) {
                    Some(bevy::mesh::VertexAttributeValues::Float32x3(p)) => p.clone(),
                    _ => vec![],
                })
                // Near the stroke's middle; Bevy's (x, -z) is the ground's (x, y).
                .filter(|p| (p[0] - 165.0).abs() < 4.0 && (-p[2] - 130.0).abs() < 4.0)
                .map(|p| p[1])
                .fold(f32::MIN, f32::max)
        };
        let before = top(&app);
        app.update();
        // Bevy is Y up: the ground under the stroke is 2 m higher.
        let after = top(&app);
        assert!((after - before - 2.0).abs() < 0.05, "{before} → {after}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn painting_needs_a_layer_and_scattering_a_scatter() {
        let (mut editor, built, dir) = editor("paint");
        let mut tool = Tool::default();
        tool.active = ToolKind::Paint;
        let (a, b) = (DVec2::new(100.0, 130.0), DVec2::new(150.0, 130.0));
        drag(&mut editor, &mut tool, &built, a, b, false);
        assert!(editor.project.terrain.paint.is_empty());
        assert!(editor.status.contains("ground layer"), "{}", editor.status);
        // With a layer: it paints the first one; Ctrl the ground's own.
        assert!(editor.apply(
            vec![Op::PutGroundLayer {
                layer: GroundLayer {
                    name: "sand".into(),
                    surface: "gravel".into(),
                    material: "gravel".into(),
                },
            }],
            None
        ));
        let built = self::built(&editor);
        drag(&mut editor, &mut tool, &built, a, b, false);
        drag(&mut editor, &mut tool, &built, a, b, true);
        let paint = &editor.project.terrain.paint;
        assert_eq!(paint.len(), 2);
        assert_eq!(paint[0].layer.as_deref(), Some("sand"));
        assert_eq!(paint[1].layer, None);

        tool.active = ToolKind::Scatter;
        drag(&mut editor, &mut tool, &built, a, b, false);
        assert!(editor.status.contains("scatter"), "{}", editor.status);
        let s = new_scatter(&editor, 0);
        assert!(editor.apply(vec![Op::PutScatter { scatter: s }], None));
        drag(&mut editor, &mut tool, &built, a, b, false);
        drag(&mut editor, &mut tool, &built, a, b, true);
        let strokes = &editor.project.scatter[0].strokes;
        assert_eq!(
            strokes.iter().map(|s| s.brush).collect::<Vec<_>>(),
            vec![Brush::Paint, Brush::Erase]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
