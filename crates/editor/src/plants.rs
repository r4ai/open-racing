//! Single copies of a scatter, as a city builder's tree tool places and moves them:
//! Plant puts one down where the pointer is clicked (a drag turns it), Select picks
//! copies, painted or planted, to move (G), turn (R), resize (S) or delete (X). A
//! painted copy that is moved or changed becomes a planted one where it stood, so that
//! painting again leaves it be.

use bevy::prelude::*;
use glam::{DVec2, DVec3};
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{Plant, Scatter};
use open_racing_track_project::scatter::{Copy, CopyId};
use open_racing_track_render::to_bevy;

use crate::preview::Built;
use crate::state::Editor;
use crate::theme;
use crate::viewport::{Tool, View};

/// What the Scatter tool's left button does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScatterMode {
    /// Strokes paint copies in (Ctrl wipes them out).
    #[default]
    Paint,
    /// A click plants one copy, a drag turns it as it goes down.
    Plant,
    /// Clicks and boxes pick copies to move, turn, resize or delete.
    Select,
}

impl ScatterMode {
    pub const ALL: [ScatterMode; 3] = [ScatterMode::Paint, ScatterMode::Plant, ScatterMode::Select];

    pub fn label(self) -> &'static str {
        match self {
            ScatterMode::Paint => "Paint",
            ScatterMode::Plant => "Plant",
            ScatterMode::Select => "Select",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            ScatterMode::Paint => "Drag to paint copies in, Ctrl to wipe them out (1)",
            ScatterMode::Plant => "Click to plant one copy; drag to turn it as it goes down (2)",
            ScatterMode::Select => {
                "Click or box copies, then G moves, R turns, S resizes, X deletes them (3)"
            }
        }
    }
}

/// What moving, turning or resizing copies does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Move,
    Turn,
    Resize,
}

/// Copies being moved, turned or resized, as a transform in the view.
pub struct Changing {
    pub what: Change,
    /// Where each is in the scatter's `placed`, and how it was.
    targets: Vec<(usize, Plant)>,
    /// Each as it is now.
    pub now: Vec<Plant>,
    /// The pointer on the ground and on screen when it began.
    start: DVec2,
    from: Vec2,
    /// What they turn about.
    pivot: DVec2,
}

/// The Scatter tool's single copies: those selected and what is being done to them.
#[derive(Default)]
pub struct Plants {
    /// The copies selected, of the scatter `of`.
    pub selected: Vec<CopyId>,
    pub of: Option<String>,
    /// The model Plant puts down: one of the scatter's, or each picked by weight.
    pub model: Option<usize>,
    /// The copy under the pointer.
    pub hover: Option<CopyId>,
    /// Plant: where the copy goes and where the button went down on screen.
    press: Option<(DVec2, Vec2)>,
    /// Select: where the button went down, and the box dragged out from there.
    pub boxing: Option<(Vec2, Vec2)>,
    pub changing: Option<Changing>,
    /// Copies planted so far, for their turns and sizes.
    count: u64,
}

/// How far the pointer moves on screen before a press is a drag, logical pixels.
const DRAG: f32 = 5.0;

/// A number in [0, 1), the same for the same inputs.
fn random(a: u64, b: u64) -> f64 {
    let mut h = a.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ b.wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    h ^= h >> 31;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 29;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// The scatter the tool works on, and its index.
fn scatter<'a>(editor: &'a Editor, tool: &Tool) -> Option<(usize, &'a Scatter)> {
    let name = tool.brush.scatter.as_deref()?;
    editor
        .project
        .scatter
        .iter()
        .enumerate()
        .find(|(_, s)| s.name == name)
}

/// The copy whose trunk is nearest `at` on screen, if one is near enough.
pub fn under(view: View, copies: &[Copy], sizes: &[[f32; 2]], at: Vec2) -> Option<CopyId> {
    let mut best: Option<(f32, CopyId)> = None;
    for c in copies {
        let [r, h] = sizes.get(c.model).copied().unwrap_or([1.0, 2.0]);
        let (r, h) = (r as f64 * c.scale, h as f64 * c.scale);
        let (Some(base), Some(top)) = (view.screen(c.pos), view.screen(c.pos + c.up * h)) else {
            continue;
        };
        let middle = c.pos + c.up * (0.5 * h);
        let reach = view
            .screen(middle)
            .zip(view.screen(middle + DVec3::X * r))
            .map_or(0.0, |(a, b)| a.distance(b));
        let d = distance_to_segment(at, base, top);
        if d <= (0.6 * reach).max(8.0) && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, c.id));
        }
    }
    best.map(|(_, id)| id)
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// A plant of scatter `s` at `at`: its model as chosen or by weight, a turn and a size
/// of its own.
fn new_plant(s: &Scatter, model: Option<usize>, at: DVec2, yaw: Option<f64>, n: u64) -> Plant {
    let seed = s.seed();
    let model = model.filter(|&m| m < s.models.len()).unwrap_or_else(|| {
        let total: f64 = s.models.iter().map(|m| m.weight).sum();
        let mut pick = random(seed, n * 3) * total;
        s.models
            .iter()
            .position(|m| {
                pick -= m.weight;
                pick < 0.0
            })
            .unwrap_or(0)
    });
    Plant {
        model,
        pos: round2(at),
        yaw: yaw.unwrap_or_else(|| random(seed, n * 3 + 1) * std::f64::consts::TAU),
        scale: s.scale[0] + (s.scale[1] - s.scale[0]) * random(seed, n * 3 + 2),
    }
}

/// A place kept to the centimetre, for the file.
fn round2(p: DVec2) -> DVec2 {
    (p * 100.0).round() / 100.0
}

/// The pointer in the view with the Scatter tool planting or selecting. Returns
/// whether it took the input (always: the tool owns the left button and its keys).
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
    let Some((k, s)) = scatter(editor, tool) else {
        tool.hint = "Add a scatter to plant or select (the palette below, or + New)".into();
        return true;
    };
    let s = s.clone();
    let b = built_index(built, &s.name);
    let copies = b
        .and_then(|b| built.copies.get(b).cloned())
        .unwrap_or_default();
    let sizes = b
        .and_then(|b| built.sizes.get(b).cloned())
        .unwrap_or_default();
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let mode = tool.brush.mode;
    let plants = &mut tool.brush.plants;
    if plants.of.as_deref() != Some(s.name.as_str()) {
        *plants = Plants {
            model: None,
            count: plants.count,
            of: Some(s.name.clone()),
            ..Default::default()
        };
    }

    // Moving, turning or resizing: every input is the transform's.
    if plants.changing.is_some() {
        change(editor, tool, k, buttons, keys, anywhere);
        return true;
    }
    // What stands no more is not selected, once the view shows the project as it is.
    if !editor.dragging() && built.revision == editor.revision {
        plants
            .selected
            .retain(|id| copies.iter().any(|c| c.id == *id));
    }
    plants.hover = over.and_then(|at| under(view, &copies, &sizes, at));

    match mode {
        ScatterMode::Plant => {
            if over.is_some()
                && buttons.just_pressed(MouseButton::Left)
                && let (Some(p), Some(at)) = (tool.pointer, over)
            {
                if ctrl {
                    // Ctrl + click takes out the copy under the pointer.
                    if let Some(id) = plants.hover {
                        editor.apply(
                            vec![Op::RemoveCopies {
                                scatter: s.name.clone(),
                                copies: vec![id],
                            }],
                            None,
                        );
                    }
                } else {
                    plants.press = Some((p.truncate(), at));
                }
            }
            if let Some((at, from)) = plants.press
                && !buttons.pressed(MouseButton::Left)
            {
                plants.press = None;
                // Dragged: it faces the way the pointer went.
                let yaw = anywhere
                    .filter(|p| p.distance(from) > DRAG)
                    .and(tool.pointer)
                    .map(|p| {
                        let d = p.truncate() - at;
                        d.y.atan2(d.x) - std::f64::consts::FRAC_PI_2
                    });
                plants.count += 1;
                let plant = new_plant(&s, plants.model, at, yaw, plants.count);
                if editor.apply(
                    vec![Op::PlantCopies {
                        scatter: s.name.clone(),
                        plants: vec![plant],
                    }],
                    None,
                ) {
                    let name = crate::assets::model_name(&s.models[plant.model].model);
                    editor.status = format!("planted a {name} of {}", s.name);
                }
            }
            tool.hint = format!(
                "Click to plant {} · drag to turn it · Ctrl + click takes one out · 3 selects",
                match plants.model {
                    Some(m) if m < s.models.len() => crate::assets::model_name(&s.models[m].model),
                    _ => format!("{} (models by weight)", s.name),
                }
            );
        }
        ScatterMode::Select | ScatterMode::Paint => {
            if over.is_some() && buttons.just_pressed(MouseButton::Left) {
                plants.boxing = over.map(|a| (a, a));
            }
            if let (Some((_, end)), Some(at)) = (&mut plants.boxing, anywhere) {
                *end = at;
            }
            if let Some((from, to)) = plants.boxing
                && !buttons.pressed(MouseButton::Left)
            {
                plants.boxing = None;
                let picked: Vec<CopyId> = if from.distance(to) <= DRAG {
                    plants.hover.into_iter().collect()
                } else {
                    let rect = Rect::from_corners(from, to);
                    copies
                        .iter()
                        .filter(|c| view.screen(c.pos).is_some_and(|p| rect.contains(p)))
                        .map(|c| c.id)
                        .collect()
                };
                if shift {
                    for id in picked {
                        match plants.selected.iter().position(|x| *x == id) {
                            Some(i) if from.distance(to) <= DRAG => {
                                plants.selected.remove(i);
                            }
                            Some(_) => {}
                            None => plants.selected.push(id),
                        }
                    }
                } else {
                    plants.selected = picked;
                }
            }
            let n = plants.selected.len();
            tool.hint = if n == 0 {
                "Click or drag a box round copies to select them · Shift adds · A selects all"
                    .into()
            } else {
                format!(
                    "{n} selected · G move · R turn · S resize · X delete · Shift + click adds or drops · Alt A none"
                )
            };
        }
    }

    if !keys_free || over.is_none() {
        return true;
    }
    let pressed = |k| keys.just_pressed(k);
    let plants = &mut tool.brush.plants;
    if pressed(KeyCode::KeyA) && alt {
        plants.selected.clear();
    } else if pressed(KeyCode::KeyA) && !ctrl && !shift {
        plants.selected = copies.iter().map(|c| c.id).collect();
    } else if (pressed(KeyCode::KeyX) || pressed(KeyCode::Delete)) && !plants.selected.is_empty() {
        let ids = std::mem::take(&mut plants.selected);
        let n = ids.len();
        if editor.apply(
            vec![Op::RemoveCopies {
                scatter: s.name.clone(),
                copies: ids,
            }],
            None,
        ) {
            editor.status = format!("took {n} out of {}", s.name);
        }
    } else if let Some(what) = [
        (KeyCode::KeyG, Change::Move),
        (KeyCode::KeyR, Change::Turn),
        (KeyCode::KeyS, Change::Resize),
    ]
    .into_iter()
    .find(|(key, _)| pressed(*key) && !ctrl && !alt)
    .map(|(_, w)| w)
    {
        start(editor, tool, built, k, what, over, &copies);
    }
    true
}

/// The selected copies as they stand.
fn chosen<'a>(selected: &[CopyId], copies: &'a [Copy]) -> Vec<&'a Copy> {
    copies.iter().filter(|c| selected.contains(&c.id)).collect()
}

/// Begins moving, turning or resizing the selected copies: painted ones become
/// planted ones where they stand, so that they can be changed one by one.
fn start(
    editor: &mut Editor,
    tool: &mut Tool,
    _built: &Built,
    k: usize,
    what: Change,
    over: Option<Vec2>,
    copies: &[Copy],
) {
    let chosen = chosen(&tool.brush.plants.selected, copies);
    let (Some(p), Some(from)) = (tool.pointer, over) else {
        return;
    };
    if chosen.is_empty() {
        return;
    }
    let name = editor.project.scatter[k].name.clone();
    editor.begin_drag();
    let mut next = editor.project.scatter[k].placed.len();
    let mut painted = Vec::new();
    let mut targets = Vec::new();
    for c in &chosen {
        let plant = Plant {
            pos: round2(c.pos.truncate()),
            ..c.plant()
        };
        match c.id {
            CopyId::Placed(i) => targets.push((i, plant)),
            CopyId::Cell(_) => {
                painted.push((c.id, plant));
                targets.push((next, plant));
                next += 1;
            }
        }
    }
    if !painted.is_empty()
        && !editor.apply(
            vec![Op::SetCopies {
                scatter: name,
                copies: painted,
            }],
            None,
        )
    {
        editor.cancel_drag();
        return;
    }
    let pivot = targets.iter().map(|(_, p)| p.pos).sum::<DVec2>() / targets.len() as f64;
    let plants = &mut tool.brush.plants;
    plants.selected = targets.iter().map(|(i, _)| CopyId::Placed(*i)).collect();
    plants.changing = Some(Changing {
        what,
        now: targets.iter().map(|(_, p)| *p).collect(),
        targets,
        start: p.truncate(),
        from,
        pivot,
    });
}

/// A transform of the selected copies under way: the pointer sets it, a click or
/// Enter keeps it, a right click or Esc puts them back.
fn change(
    editor: &mut Editor,
    tool: &mut Tool,
    k: usize,
    buttons: &ButtonInput<MouseButton>,
    keys: &ButtonInput<KeyCode>,
    anywhere: Option<Vec2>,
) {
    let pointer = tool.pointer;
    let plants = &mut tool.brush.plants;
    let c = plants.changing.as_mut().expect("a change");
    let dx = anywhere.map_or(0.0, |a| (a.x - c.from.x) as f64);
    let delta = pointer.map_or(DVec2::ZERO, |p| p.truncate() - c.start);
    let (turn, size) = (dx * 0.01, 2f64.powf(dx / 200.0));
    c.now = c
        .targets
        .iter()
        .map(|(_, p)| match c.what {
            Change::Move => Plant {
                pos: round2(p.pos + delta),
                ..*p
            },
            Change::Turn => Plant {
                pos: round2(c.pivot + DVec2::from_angle(turn).rotate(p.pos - c.pivot)),
                yaw: p.yaw + turn,
                ..*p
            },
            Change::Resize => Plant {
                scale: ((p.scale * size * 100.0).round() / 100.0).clamp(0.05, 20.0),
                ..*p
            },
        })
        .collect();
    let name = editor.project.scatter[k].name.clone();
    let copies: Vec<_> = c
        .targets
        .iter()
        .zip(&c.now)
        .map(|((i, _), p)| (CopyId::Placed(*i), *p))
        .collect();
    let n = copies.len();
    editor.apply(
        vec![Op::SetCopies {
            scatter: name.clone(),
            copies,
        }],
        None,
    );
    tool.hint = match c.what {
        Change::Move => format!("Moving {n}: {:.1} m, {:.1} m", delta.x, delta.y),
        Change::Turn => format!("Turning {n}: {:.0}°", turn.to_degrees()),
        Change::Resize => format!("Resizing {n}: ×{size:.2}"),
    } + " · click or Enter keeps it · right click or Esc puts them back";
    if buttons.just_pressed(MouseButton::Left)
        || keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter])
    {
        plants.changing = None;
        editor.end_drag();
        editor.status = format!("changed {n} of {name}");
    } else if buttons.just_pressed(MouseButton::Right) || keys.just_pressed(KeyCode::Escape) {
        plants.changing = None;
        editor.cancel_drag();
        editor.status = "put back".into();
    }
}

/// Sets something of the selected copies from the tool settings: their model, size or
/// turn. Painted ones become planted ones where they stand.
pub fn set(editor: &mut Editor, tool: &mut Tool, built: &Built, f: impl Fn(&mut Plant)) {
    let Some((_, s)) = scatter(editor, tool) else {
        return;
    };
    let name = s.name.clone();
    let mut next = s.placed.len();
    let copies = built_index(built, &name)
        .and_then(|b| built.copies.get(b).cloned())
        .unwrap_or_default();
    let mut ops = Vec::new();
    let mut now = Vec::new();
    for c in chosen(&tool.brush.plants.selected, &copies) {
        let mut plant = Plant {
            pos: round2(c.pos.truncate()),
            ..c.plant()
        };
        f(&mut plant);
        ops.push((c.id, plant));
        now.push(match c.id {
            CopyId::Placed(i) => CopyId::Placed(i),
            CopyId::Cell(_) => {
                next += 1;
                CopyId::Placed(next - 1)
            }
        });
    }
    if ops.is_empty() {
        return;
    }
    if editor.apply(
        vec![Op::SetCopies {
            scatter: name.clone(),
            copies: ops,
        }],
        Some(&format!("copies of {name}")),
    ) {
        tool.brush.plants.selected = now;
    }
}

/// The selected copies, the one under the pointer, and the copy about to be planted.
pub fn draw(
    tool: &Tool,
    built: &Built,
    gizmos: &mut Gizmos,
    bold: &mut Gizmos<crate::viewport::Bold>,
) {
    let Some(name) = tool.brush.scatter.as_deref() else {
        return;
    };
    let plants = &tool.brush.plants;
    if plants.of.as_deref() != Some(name) {
        return;
    }
    let Some(k) = built_index(built, name) else {
        return;
    };
    let copies = &built.copies[k];
    let sizes = built.sizes.get(k).map(Vec::as_slice).unwrap_or_default();
    let ground = built.ground.as_deref();
    let size = |model: usize| sizes.get(model).copied().unwrap_or([1.0, 2.0]);
    /// A ring round a copy's foot and a line up its height.
    fn ring<G: GizmoConfigGroup>(g: &mut Gizmos<G>, at: DVec3, r: f64, h: f64, colour: Color) {
        let points: Vec<Vec3> = (0..=32)
            .map(|i| {
                let a = i as f64 / 32.0 * std::f64::consts::TAU;
                to_bevy(at + (DVec2::from_angle(a) * r).extend(0.15))
            })
            .collect();
        g.linestrip(points, colour);
        g.line(to_bevy(at), to_bevy(at + DVec3::Z * h), colour);
    }
    let on_ground = |p: DVec2| {
        ground
            .and_then(|g| g.raycast_down(p.extend(1e4), 2e4))
            .map_or(p.extend(0.0), |h| h.point)
    };
    if let Some(c) = &plants.changing {
        for p in &c.now {
            let [r, h] = size(p.model);
            ring(
                bold,
                on_ground(p.pos),
                (r as f64 * p.scale).max(0.3),
                h as f64 * p.scale,
                theme::SELECTED,
            );
        }
        return;
    }
    for c in copies.iter() {
        let selected = plants.selected.contains(&c.id);
        let hover = plants.hover == Some(c.id);
        if !selected && !hover {
            continue;
        }
        let [r, h] = size(c.model);
        let colour = if selected {
            theme::SELECTED
        } else {
            theme::HOVER
        };
        ring(
            bold,
            c.pos,
            (r as f64 * c.scale).max(0.3),
            h as f64 * c.scale,
            colour,
        );
    }
    // Plant: where the next copy goes.
    if tool.brush.mode == ScatterMode::Plant
        && let Some(p) = tool.pointer
    {
        let at = plants.press.map_or(p.truncate(), |(a, _)| a);
        let [r, h] = plants.model.map_or([1.0, 2.0], size);
        ring(
            gizmos,
            on_ground(at),
            (r as f64).max(0.3),
            h as f64,
            Color::srgb(0.45, 1.0, 0.45),
        );
        if plants.press.is_some() {
            gizmos.line(
                to_bevy(on_ground(at) + DVec3::Z * 0.3),
                to_bevy(p + DVec3::Z * 0.3),
                Color::srgb(0.45, 1.0, 0.45),
            );
        }
    }
}

/// Where scatter `name` is among those the last build made.
fn built_index(built: &Built, name: &str) -> Option<usize> {
    built.names.iter().position(|n| n == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_racing_track_project::project::ScatterModel;

    fn woods() -> Scatter {
        Scatter {
            name: "woods".into(),
            seed: 0,
            models: vec![
                ScatterModel::new("builtin:pine".into(), 3.0),
                ScatterModel::new("builtin:tree".into(), 1.0),
            ],
            spacing: 6.0,
            scale: [0.8, 1.2],
            tilt: 0.0,
            clearance: 3.0,
            max_slope: 35.0,
            collide: false,
            shadows: true,
            detail: 150.0,
            draw: 2000.0,
            variety: open_racing_track_project::project::VARIETY,
            strokes: vec![],
            removed: vec![],
            placed: vec![],
            group: None,
        }
    }

    #[test]
    fn planted_copies_vary_and_keep_to_the_scatter_s_sizes() {
        let s = woods();
        let a = new_plant(&s, None, DVec2::new(1.234, 5.678), None, 1);
        let b = new_plant(&s, None, DVec2::ZERO, None, 2);
        assert_eq!(a.pos, DVec2::new(1.23, 5.68));
        assert!(a.yaw != b.yaw);
        for p in [a, b] {
            assert!((0.8..=1.2).contains(&p.scale), "{}", p.scale);
            assert!(p.model < 2);
        }
        // A chosen model, a given turn.
        let c = new_plant(&s, Some(1), DVec2::ZERO, Some(0.5), 3);
        assert_eq!((c.model, c.yaw), (1, 0.5));
        // By weight: most are pines.
        let pines = (0..400)
            .filter(|n| new_plant(&s, None, DVec2::ZERO, None, *n).model == 0)
            .count();
        assert!((250..350).contains(&pines), "{pines}");
    }

    /// An editor on a new project with woods painted by the start, and what a build of
    /// it knows.
    fn painted(name: &str) -> (Editor, Built, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("open-racing-plants-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        let mut s = woods();
        s.strokes.push(open_racing_track_project::project::Stroke {
            brush: open_racing_track_project::project::Brush::Paint,
            radius: 20.0,
            strength: 1.0,
            points: vec![DVec2::new(100.0, 60.0)],
            fill: false,
            hardness: 0.5,
        });
        assert!(editor.apply(vec![Op::PutScatter { scatter: s }], None));
        let built = build(&editor);
        (editor, built, dir)
    }

    fn build(editor: &Editor) -> Built {
        let scene = open_racing_track_project::bake::build(&editor.project);
        let surfaces: Vec<_> = editor.project.surfaces.iter().map(|s| s.props).collect();
        let ground = scene.ground.build(&surfaces);
        let keepout = open_racing_track_project::scatter::Keepout::new(&scene.roads);
        let s = &editor.project.scatter[0];
        Built {
            copies: vec![std::sync::Arc::new(
                open_racing_track_project::scatter::copies(s, &keepout, &ground),
            )],
            sizes: vec![vec![[3.0, 11.0], [3.0, 9.0]]],
            names: vec![s.name.clone()],
            revision: editor.revision,
            ground: Some(std::sync::Arc::new(ground)),
            count: 1,
            ..Default::default()
        }
    }

    /// One frame of the view's input with these keys just pressed and the left button
    /// as given.
    fn frame(
        editor: &mut Editor,
        tool: &mut Tool,
        built: &Built,
        keys: &[KeyCode],
        click: bool,
        at: DVec2,
    ) {
        let mut k = ButtonInput::<KeyCode>::default();
        for key in keys {
            k.press(*key);
        }
        let mut b = ButtonInput::<MouseButton>::default();
        if click {
            b.press(MouseButton::Left);
        }
        let (cam, t) = (Camera::default(), GlobalTransform::default());
        let view = View { cam: &cam, t: &t };
        let over = Some(Vec2::new(400.0, 300.0));
        tool.pointer = Some(at.extend(0.0));
        input(editor, tool, built, view, &b, &k, over, over, true);
    }

    #[test]
    fn selected_copies_move_as_planted_ones_and_are_deleted() {
        let (mut editor, built, dir) = painted("select");
        let mut tool = Tool::default();
        tool.active = crate::viewport::ToolKind::Scatter;
        tool.brush.scatter = Some("woods".into());
        tool.brush.mode = ScatterMode::Select;
        let standing = built.copies[0].len();
        assert!(standing > 5, "{standing}");
        // A selects them all; G, the pointer 10 m east, a click: moved.
        let at = DVec2::new(100.0, 60.0);
        frame(&mut editor, &mut tool, &built, &[KeyCode::KeyA], false, at);
        assert_eq!(tool.brush.plants.selected.len(), standing);
        frame(&mut editor, &mut tool, &built, &[KeyCode::KeyG], false, at);
        assert!(tool.brush.plants.changing.is_some());
        frame(
            &mut editor,
            &mut tool,
            &built,
            &[],
            false,
            at + DVec2::X * 10.0,
        );
        frame(
            &mut editor,
            &mut tool,
            &built,
            &[],
            true,
            at + DVec2::X * 10.0,
        );
        assert!(tool.brush.plants.changing.is_none());
        let s = &editor.project.scatter[0];
        assert_eq!(s.placed.len(), standing);
        assert_eq!(s.removed.len(), standing);
        for (c, p) in built.copies[0].iter().zip(&s.placed) {
            assert!((p.pos - (c.pos.truncate() + DVec2::X * 10.0)).length() < 0.02);
        }
        // One undo step puts them all back.
        editor.undo();
        assert!(editor.project.scatter[0].placed.is_empty());
        editor.redo();
        // X deletes the selected: planted ones are gone.
        let built = build(&editor);
        tool.brush.plants.selected = vec![CopyId::Placed(0), CopyId::Placed(1)];
        frame(&mut editor, &mut tool, &built, &[KeyCode::KeyX], false, at);
        assert_eq!(editor.project.scatter[0].placed.len(), standing - 2);

        // Plant: a click puts one copy down where it was clicked.
        tool.brush.mode = ScatterMode::Plant;
        let there = DVec2::new(60.0, 80.0);
        frame(&mut editor, &mut tool, &built, &[], true, there);
        frame(&mut editor, &mut tool, &built, &[], false, there);
        let s = &editor.project.scatter[0];
        assert_eq!(s.placed.len(), standing - 1);
        assert_eq!(s.placed.last().unwrap().pos, there);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
