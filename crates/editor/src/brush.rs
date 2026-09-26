//! Brushes in the 3D view, as Blender's sculpting and texture painting: Sculpt Terrain
//! shapes the ground, Paint Ground paints its layers (dirt, gravel, sand), and Scatter
//! paints woods, bushes and rocks over it. A drag is one stroke, saved as an operation
//! when the button is let go; while it is dragged the view shows what it does.
//!
//! Ctrl turns a brush round (lowers, paints the ground's own material back, wipes
//! models out), Shift smooths; F sets the radius, Shift F the strength and Ctrl F the
//! hardness (how much of the radius acts fully) by moving the mouse, [ and ] the
//! radius in steps. The Scatter tool also plants and selects single copies (1, 2, 3:
//! paint, plant, select; see `plants`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy_egui::egui;
use glam::{DVec2, DVec3};
use open_racing_track_project::ops::{Op, StrokeTarget};
use open_racing_track_project::project::{
    Brush, GroundLayer, HARDNESS, LayerStroke, MAX_LAYERS, Stroke,
};
use open_racing_track_project::terrain::{Grid, PaintMask, TerrainBuild};
use open_racing_track_render::to_bevy;

use crate::commands::Ctx;
use crate::plants::{Plants, ScatterMode};
use crate::preview::{Built, GroundPaint, TerrainChunk};
use crate::properties::{row, section};
use crate::state::Editor;
use crate::viewport::{Tool, ToolKind, View};

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
    /// 0 to 1: the share of the radius a stroke acts fully within.
    pub hardness: f64,
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
    /// What the Scatter tool's left button does: paint, plant or select.
    pub mode: ScatterMode,
    /// Single copies planted and selected with the Scatter tool.
    pub plants: Plants,
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
                    hardness: HARDNESS,
                },
                Size {
                    radius: 6.0,
                    strength: 1.0,
                    hardness: HARDNESS,
                },
                Size {
                    radius: 25.0,
                    strength: 0.7,
                    hardness: HARDNESS,
                },
            ],
            height: 2.0,
            layer: None,
            own: false,
            scatter: None,
            erase: false,
            fill: false,
            mode: ScatterMode::Paint,
            plants: Plants::default(),
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
            mode: self.mode,
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
    Hardness,
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
            hardness: size.hardness,
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

/// The brush under the pointer: strokes, F, Shift F and Ctrl F, [ and ]; with the
/// Scatter tool, 1, 2 and 3 switch between painting, planting and selecting copies.
/// Called by the view's input while a brush tool is active; returns whether it took
/// the input (a stroke or a setting under way, or the Scatter tool planting or
/// selecting).
#[allow(clippy::too_many_arguments)]
pub fn input(
    editor: &mut Editor,
    tool: &mut Tool,
    built: &Built,
    view: View,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    over: Option<Vec2>,
    anywhere: Option<Vec2>,
    keys_free: bool,
) -> bool {
    settle(editor, &mut tool.brush);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let kind = tool.active;
    let busy = tool.brush.stroke.is_some()
        || tool.brush.adjust.is_some()
        || tool.brush.plants.changing.is_some();
    if kind == ToolKind::Scatter && keys_free && over.is_some() && !busy {
        for (key, mode) in [
            (KeyCode::Digit1, ScatterMode::Paint),
            (KeyCode::Digit2, ScatterMode::Plant),
            (KeyCode::Digit3, ScatterMode::Select),
        ] {
            if keys.just_pressed(key) {
                tool.brush.mode = mode;
            }
        }
    }
    if kind == ToolKind::Scatter && tool.brush.mode != ScatterMode::Paint {
        return crate::plants::input(
            editor, tool, built, view, buttons, keys, over, anywhere, keys_free,
        );
    }
    // Setting the radius or strength with the mouse.
    if let Some(a) = &tool.brush.adjust {
        let dx = anywhere.map_or(0.0, |p| (p.x - a.from.x) as f64);
        let (what, start) = (a.what, a.start);
        let size = tool.brush.size_mut(kind);
        match what {
            Setting::Radius => size.radius = (start * 2f64.powf(dx / 150.0)).clamp(0.5, 500.0),
            Setting::Strength => size.strength = (start + dx / 300.0).clamp(0.01, 1.0),
            Setting::Hardness => size.hardness = (start + dx / 300.0).clamp(0.0, 1.0),
        }
        let size = *size;
        tool.hint = match what {
            Setting::Radius => format!("Radius {:.1} m", size.radius),
            Setting::Strength => format!("Strength {:.0} %", size.strength * 100.0),
            Setting::Hardness => format!("Hardness {:.0} %", size.hardness * 100.0),
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
                Setting::Hardness => size.hardness = start,
            }
            tool.brush.adjust = None;
        }
        return true;
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
        return true;
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
        return tool.brush.stroke.is_some();
    }
    if keys.just_pressed(KeyCode::KeyL) {
        tool.brush.fill = !tool.brush.fill;
    }
    if keys.just_pressed(KeyCode::KeyF) {
        let what = if ctrl {
            Setting::Hardness
        } else if shift {
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
                Setting::Hardness => size.hardness,
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
    tool.brush.stroke.is_some() || tool.brush.adjust.is_some()
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
    let modes = if tool.active == ToolKind::Scatter {
        " · 2 plants, 3 selects single copies"
    } else {
        ""
    };
    format!(
        "{drag} {what} · radius {:.1} m (F, [ ]) · strength {:.0} % (Shift F) · hardness {:.0} % (Ctrl F) · {turn} · {lasso}{modes}",
        size.radius,
        size.strength * 100.0,
        size.hardness * 100.0
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
    stroke.hardness = round(stroke.hardness);
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
                .map(|c| c.pos)
                .filter(|p| p.cmpge(lo).all() && p.cmple(hi).all())
                .take(4000)
                .map(|p| {
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
pub fn draw(
    tool: &Tool,
    built: &Built,
    gizmos: &mut Gizmos,
    bold: &mut Gizmos<crate::viewport::Bold>,
) {
    if !tool.active.is_brush() || tool.modal.is_some() {
        return;
    }
    if tool.active == ToolKind::Scatter && tool.brush.mode != ScatterMode::Paint {
        crate::plants::draw(tool, built, gizmos, bold);
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
    let size = b.size(tool.active);
    let radius = size.radius;
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
    for (r, alpha) in [(radius, 1.0), (size.hardness * radius, 0.45)] {
        if r < 0.05 {
            continue;
        }
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

/// Adds a scatter of a kind of vegetation (see `vegetation`), with the files it
/// needs from `library`, and paints it next with the kind's brush.
pub fn add_scatter(c: &mut Ctx, kind: &crate::vegetation::Kind, library: Option<&std::path::Path>) {
    match crate::vegetation::add(c.editor, kind, library) {
        Ok(name) => {
            c.tool.brush.scatter = Some(name.clone());
            c.tool.active = ToolKind::Scatter;
            c.tool.brush.mode = ScatterMode::Paint;
            c.tool.brush.erase = false;
            let size = c.tool.brush.size_mut(ToolKind::Scatter);
            (size.radius, size.strength, size.hardness) =
                (kind.brush.radius, kind.brush.strength, kind.brush.hardness);
            c.editor.status = format!(
                "\"{name}\": drag over the ground to plant it (the Scatter tab sets its models)"
            );
        }
        Err(e) => c.editor.status = e,
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
                for kind in crate::vegetation::builtin() {
                    if ui
                        .button(&kind.name)
                        .on_hover_text(format!("Built-in {}", kind.category.label().to_lowercase()))
                        .clicked()
                    {
                        add_scatter(c, &kind, None);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("More kinds, and your own, are on the palette below the view");
            ui.separator();
            let b = &mut c.tool.brush;
            for m in ScatterMode::ALL {
                ui.selectable_value(&mut b.mode, m, m.label())
                    .on_hover_text(m.tip());
            }
            ui.separator();
            match b.mode {
                ScatterMode::Paint => {
                    ui.selectable_value(&mut b.erase, false, "Add");
                    ui.selectable_value(&mut b.erase, true, "Wipe out");
                }
                ScatterMode::Plant => {
                    plant_ui(ui, c);
                    return;
                }
                ScatterMode::Select => {
                    select_ui(ui, c);
                    return;
                }
            }
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
    let mut percent = size.hardness * 100.0;
    if slider(ui, &mut percent, 0.0..=100.0, "Hardness", " %", compact)
        .on_hover_text(
            "How much of the radius acts fully: 0 eases out from the middle, 100 is a hard edge (Ctrl F in the view)",
        )
        .changed()
    {
        size.hardness = percent / 100.0;
    }
}

/// The scatter being planted: the model each click puts down.
fn plant_ui(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(s) = c
        .tool
        .brush
        .scatter
        .as_ref()
        .and_then(|n| c.editor.project.scatter.iter().find(|s| &s.name == n))
    else {
        return;
    };
    let models: Vec<String> = s
        .models
        .iter()
        .map(|m| crate::assets::model_name(&m.model))
        .collect();
    let p = &mut c.tool.brush.plants;
    ui.label("Model");
    egui::ComboBox::from_id_salt("plant model")
        .selected_text(match p.model {
            Some(m) if m < models.len() => models[m].clone(),
            _ => "any, by weight".into(),
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut p.model, None, "any, by weight");
            for (i, m) in models.iter().enumerate() {
                ui.selectable_value(&mut p.model, Some(i), m);
            }
        });
    ui.weak("click to plant · drag to turn · Ctrl + click takes one out");
}

/// The copies selected: how many, and their model, size and turn to set at once.
fn select_ui(ui: &mut egui::Ui, c: &mut Ctx) {
    let Some(s) = c
        .tool
        .brush
        .scatter
        .as_ref()
        .and_then(|n| c.editor.project.scatter.iter().find(|s| &s.name == n))
        .cloned()
    else {
        return;
    };
    let selected = c.tool.brush.plants.selected.clone();
    let n = selected.len();
    ui.label(format!("{n} selected"));
    if !s.removed.is_empty()
        && ui
            .small_button(format!("Bring back {}", s.removed.len()))
            .on_hover_text("The painted copies taken out, back where they were painted")
            .clicked()
    {
        let mut back = s.clone();
        back.removed.clear();
        c.editor.apply(vec![Op::PutScatter { scatter: back }], None);
    }
    if n == 0 {
        ui.weak("click or box copies · A all");
        return;
    }
    let chosen: Vec<_> = c
        .built
        .names
        .iter()
        .position(|x| *x == s.name)
        .and_then(|b| c.built.copies.get(b))
        .map(|copies| {
            copies
                .iter()
                .filter(|x| selected.contains(&x.id))
                .copied()
                .collect()
        })
        .unwrap_or_else(Vec::new);
    let Some(first) = chosen.first().copied() else {
        return;
    };
    let models: Vec<String> = s
        .models
        .iter()
        .map(|m| crate::assets::model_name(&m.model))
        .collect();
    let mut model = first.model;
    let same = chosen.iter().all(|x| x.model == model);
    egui::ComboBox::from_id_salt("selected model")
        .selected_text(if same {
            models.get(model).cloned().unwrap_or_default()
        } else {
            "several".into()
        })
        .show_ui(ui, |ui| {
            for (i, m) in models.iter().enumerate() {
                ui.selectable_value(&mut model, i, m);
            }
        });
    if model != first.model || (!same && chosen.iter().any(|x| x.model != model)) {
        crate::plants::set(c.editor, c.tool, c.built, |p| p.model = model);
    }
    let mut size = first.scale;
    if ui
        .add(
            egui::DragValue::new(&mut size)
                .speed(0.01)
                .range(0.05..=20.0)
                .prefix("size ×"),
        )
        .on_hover_text("The size of each selected copy (S in the view resizes them together)")
        .changed()
    {
        crate::plants::set(c.editor, c.tool, c.built, |p| p.scale = size);
    }
    let mut turn = first.yaw.to_degrees().rem_euclid(360.0);
    if ui
        .add(
            egui::DragValue::new(&mut turn)
                .speed(1.0)
                .range(0.0..=360.0)
                .suffix("°"),
        )
        .on_hover_text("The turn of each selected copy (R in the view turns them together)")
        .changed()
    {
        crate::plants::set(c.editor, c.tool, c.built, |p| p.yaw = turn.to_radians());
    }
    if ui
        .small_button("Delete")
        .on_hover_text("X in the view")
        .clicked()
    {
        let copies = std::mem::take(&mut c.tool.brush.plants.selected);
        c.editor.apply(
            vec![Op::RemoveCopies {
                scatter: s.name.clone(),
                copies,
            }],
            None,
        );
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
            _ => "Paint (1): drag over the ground to plant the scatter's models; paint again for more, Ctrl wipes them out. Plant (2): a click puts one copy down. Select (3): pick copies to move (G), turn (R), resize (S) or delete (X). Painted copies keep off roads, kerbs and steep ground. The palette below the view holds more kinds and your own; the Scatter tab sets models, distances and materials.",
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
        let (cam, t) = (Camera::default(), GlobalTransform::default());
        let view = View { cam: &cam, t: &t };
        buttons.press(MouseButton::Left);
        for k in 0..=10 {
            tool.pointer = Some(from.lerp(to, k as f64 / 10.0).extend(0.0));
            input(editor, tool, built, view, &buttons, &keys, over, over, true);
            buttons.clear();
        }
        buttons.release(MouseButton::Left);
        input(editor, tool, built, view, &buttons, &keys, over, over, true);
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
        let s = crate::vegetation::builtin().remove(0).scatter;
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
