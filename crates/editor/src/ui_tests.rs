//! Every panel drawn without a window, for every properties tab, every sidebar tab and
//! every kind of selection, on a project with corner kits, strip and wall types, a
//! model wall and a prop: no panel may panic, and what they show must stay drawable
//! as the project changes under them.

use bevy::math::Vec2;
use bevy_egui::egui;
use glam::DVec3;
use open_racing_track_project::corners::{self, Kit};
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{ModelRun, Prop, Shape};

use crate::assets::Library;
use crate::commands::Ctx;
use crate::curve_graph::{self, CurveGraph};
use crate::jobs::Jobs;
use crate::preview::Built;
use crate::profile::ProfileView;
use crate::reference::Shown;
use crate::state::{Editor, Item};
use crate::ui::{PropTab, Shell};
use crate::viewport::{Orbit, Tool};
use crate::{outliner, properties, sidebar};

/// A project with something in every panel.
fn furnished(name: &str) -> (Editor, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("open-racing-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Editor::open(dir.clone()).unwrap();
    let (smp, cs) = corners::of_road(&e.project, 0);
    let kit = Kit {
        outside: Some(("gravel".into(), 12.0)),
        wall: Some(("tyre wall".into(), 20.0)),
        ..Kit::kerbs(&e.project, None, 1.5)
    };
    let mut ops: Vec<Op> = cs
        .iter()
        .flat_map(|c| corners::kit_ops(&e.project, "circuit", &smp, &cs, c, &kit))
        .collect();
    let mut wall = crate::presets::named(&e.project, "guard rail")
        .unwrap()
        .spline(
            &e.project,
            vec![DVec3::new(0.0, -30.0, 0.0), DVec3::new(80.0, -30.0, 0.0)],
        )
        .unwrap();
    if let Shape::Wall { model, .. } = &mut wall.shape {
        *model = Some(ModelRun {
            model: "assets/models/missing.glb".into(),
            length: 4.0,
            bend: true,
            flip: false,
        });
    }
    wall.style = None;
    ops.push(Op::PutSpline { spline: wall });
    ops.push(Op::PutRow {
        road: "circuit".into(),
        row: open_racing_track_project::project::PropRow {
            name: "trees".into(),
            model: "assets/models/tree.glb".into(),
            side: open_racing_track_project::project::Side::Left,
            offset: 20.0,
            spacing: 15.0,
            ranges: vec![open_racing_track_project::project::Range { from: 1.0, to: 2.0 }],
            at: vec![],
            yaw: 0.0,
            scale: 1.0,
            jitter: Default::default(),
            drape: true,
            collide: false,
        },
    });
    let mut terrain = e.project.terrain.clone();
    terrain
        .landforms
        .push(open_racing_track_project::project::Landform {
            name: "hill".into(),
            center: glam::DVec2::new(200.0, 150.0),
            to: None,
            radius: 20.0,
            falloff: 20.0,
            kind: open_racing_track_project::project::LandformKind::Raise(4.0),
        });
    ops.push(Op::SetTerrain { terrain });
    // A kerb type in two colours with a model, and a kerb with nodes and a step.
    let mut paint = e.project.materials[0].clone();
    paint.name = "blue kerb".into();
    paint.texture = open_racing_track_project::project::TextureSource::Builtin(
        open_racing_track_project::project::BuiltinTexture::Stripes([30, 70, 200], [240, 205, 30]),
    );
    ops.push(Op::PutMaterial { material: paint });
    let mut style = e.project.strip_style("kerb").unwrap().clone();
    style.name = "blue kerb".into();
    style.material = "blue kerb".into();
    style.profile =
        open_racing_track_project::project::Profile::Shape(vec![[0.0, 0.04], [1.0, 0.06]]);
    style.model = Some(ModelRun {
        model: "assets/models/kerb.glb".into(),
        length: 2.0,
        bend: true,
        flip: false,
    });
    let mut strip = style.strip(
        "keyed",
        vec![open_racing_track_project::project::Range { from: 2.0, to: 4.0 }],
    );
    strip.keys = vec![
        open_racing_track_project::project::StripKey {
            u: 2.5,
            width: 1.2,
            height: 1.0,
        },
        open_racing_track_project::project::StripKey {
            u: 3.0,
            width: 2.5,
            height: 1.5,
        },
    ];
    ops.push(Op::PutStripStyle { style });
    ops.push(Op::PutStrip {
        road: "circuit".into(),
        side: open_racing_track_project::project::Side::Right,
        strip,
        at: Some(0),
    });
    // Sculpting, a painted ground layer, woods, a gravel area and a kerb whose width
    // changes at a node.
    let stroke = |brush, points: &[(f64, f64)]| open_racing_track_project::project::Stroke {
        brush,
        radius: 20.0,
        strength: 1.0,
        points: points
            .iter()
            .map(|&(x, y)| glam::DVec2::new(x, y))
            .collect(),
        fill: false,
        hardness: open_racing_track_project::project::HARDNESS,
    };
    use open_racing_track_project::ops::StrokeTarget;
    use open_racing_track_project::project::Brush;
    ops.push(Op::AddStroke {
        to: StrokeTarget::Sculpt,
        stroke: stroke(Brush::Raise, &[(150.0, 130.0), (200.0, 140.0)]),
    });
    ops.push(Op::PutGroundLayer {
        layer: open_racing_track_project::project::GroundLayer {
            name: "dirt".into(),
            surface: "dirt".into(),
            material: "dirt".into(),
        },
    });
    ops.push(Op::AddStroke {
        to: StrokeTarget::Paint(Some("dirt".into())),
        stroke: stroke(Brush::Paint, &[(150.0, 100.0)]),
    });
    ops.push(Op::PutScatter {
        scatter: open_racing_track_project::project::Scatter {
            name: "woods".into(),
            strokes: vec![stroke(Brush::Paint, &[(-300.0, -100.0)])],
            placed: vec![open_racing_track_project::project::Plant {
                model: 0,
                pos: glam::DVec2::new(-250.0, -60.0),
                yaw: 0.5,
                scale: 1.5,
            }],
            spacing: 8.0,
            ..crate::vegetation::builtin().remove(1).scatter
        },
    });
    let area = crate::presets::list(&e.project)
        .into_iter()
        .find(|p| p.is_area())
        .unwrap()
        .spline(
            &e.project,
            vec![
                DVec3::new(300.0, 60.0, 0.0),
                DVec3::new(340.0, 60.0, 0.0),
                DVec3::new(330.0, 90.0, 0.0),
            ],
        )
        .unwrap();
    ops.push(Op::PutSpline { spline: area });
    ops.push(Op::PutProp {
        prop: Prop {
            group: Some("stands".into()),
            name: "stand".into(),
            model: "assets/models/stand.glb".into(),
            pos: DVec3::new(100.0, 40.0, 0.0),
            yaw: 0.0,
            scale: 1.0,
            drape: true,
            collide: false,
        },
    });
    assert!(e.apply(ops, None), "{}", e.status);
    (e, dir)
}

fn built_of(e: &Editor) -> Built {
    let scene = open_racing_track_project::bake::build(&e.project);
    Built {
        corners: (0..e.project.roads.len())
            .map(|i| corners::of_road(&e.project, i).1)
            .collect(),
        roads: scene.roads.into_iter().map(|b| b.sampled).collect(),
        splines: scene.splines.into_iter().map(|b| b.sampled).collect(),
        count: 1,
        ..Default::default()
    }
}

/// Draws `f` in a frame of a window-less egui, twice, so that panels opened by the
/// first frame are drawn open in the second.
fn draw(ctx: &egui::Context, mut f: impl FnMut(&mut egui::Ui)) {
    for _ in 0..2 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 900.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| f(ui));
        out.textures_delta.clear();
    }
}

#[test]
fn every_panel_draws_for_every_selection() {
    let (mut editor, dir) = furnished("panels");
    let built = built_of(&editor);
    let (mut tool, mut orbit, mut jobs, mut shell) = (
        Tool::default(),
        Orbit::default(),
        Jobs::default(),
        Shell::default(),
    );
    let (library, shown) = (Library::default(), Shown::default());
    let (mut props_state, mut outliner_state) =
        (properties::State::default(), outliner::State::default());
    let (mut elevation, mut curves) = (ProfileView::default(), CurveGraph::default());
    let ctx = egui::Context::default();
    let tabs = [
        PropTab::Track,
        PropTab::Markers,
        PropTab::Terrain,
        PropTab::Scatter,
        PropTab::Reference,
        PropTab::Library,
        PropTab::Object,
        PropTab::Corners,
        PropTab::Strips,
        PropTab::Lines,
        PropTab::Barriers,
        PropTab::Rows,
    ];
    let selections = [
        None,
        Some((Item::Road(0), vec![])),
        Some((Item::Road(0), vec![1, 2])),
        Some((Item::Spline(0), vec![0])),
        Some((Item::Spline(1), vec![])),
        Some((Item::Prop(0), vec![])),
    ];
    for selection in selections {
        for tab in tabs {
            for corner in [None, Some((0, 1))] {
                editor.selection = Default::default();
                if let Some((item, nodes)) = &selection {
                    editor.selection.select(*item);
                    editor.selection.nodes = nodes.clone();
                }
                shell.tab = tab;
                shell.corner = corner;
                let mut c = Ctx {
                    editor: &mut editor,
                    tool: &mut tool,
                    orbit: &mut orbit,
                    jobs: &mut jobs,
                    built: &built,
                    shell: &mut shell,
                    pointer: Vec2::ZERO,
                };
                draw(&ctx, |ui| {
                    properties::show(ui, &mut c, &mut props_state, &library, &shown);
                    outliner::show(ui, &mut c, &mut outliner_state);
                    for t in [sidebar::Tab::Item, sidebar::Tab::Tool, sidebar::Tab::View] {
                        c.shell.sidebar_tab = t;
                        sidebar::show(ui, &mut c);
                    }
                    // Every brush's settings, as the Tool tab and the header show them.
                    for active in crate::viewport::ToolKind::ALL {
                        c.tool.active = active;
                        c.shell.sidebar_tab = sidebar::Tab::Tool;
                        sidebar::show(ui, &mut c);
                        ui.horizontal(|ui| crate::brush::settings_ui(ui, &mut c, true));
                    }
                    c.tool.active = crate::viewport::ToolKind::Select;
                    // Every graph of the Curves area.
                    for shown in [
                        curve_graph::Shown::Elevation,
                        curve_graph::Shown::Left,
                        curve_graph::Shown::Right,
                        curve_graph::Shown::Bank,
                    ] {
                        curves.show(c.editor, shown);
                        curve_graph::panel(ui, &mut c, &mut elevation, &mut curves);
                    }
                });
            }
        }
    }
    // Drawing changes nothing by itself.
    assert!(!editor.can_redo());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn panels_draw_after_the_project_shrinks_under_the_selection() {
    let (mut editor, dir) = furnished("shrink");
    let built = built_of(&editor);
    // Selected, then gone: the panels still draw what is left.
    editor.selection.select(Item::Spline(0));
    editor.selection.nodes = vec![1];
    editor.selection.others = vec![Item::Prop(0), Item::Road(0)];
    let name = editor.project.splines[0].name.clone();
    assert!(editor.apply(
        vec![
            Op::RemoveSpline { name },
            Op::RemoveProp {
                name: "stand".into()
            }
        ],
        None
    ));
    let (mut tool, mut orbit, mut jobs, mut shell) = (
        Tool::default(),
        Orbit::default(),
        Jobs::default(),
        Shell::default(),
    );
    let ctx = egui::Context::default();
    let mut c = Ctx {
        editor: &mut editor,
        tool: &mut tool,
        orbit: &mut orbit,
        jobs: &mut jobs,
        built: &built,
        shell: &mut shell,
        pointer: Vec2::ZERO,
    };
    let (mut ps, mut os) = (properties::State::default(), outliner::State::default());
    draw(&ctx, |ui| {
        properties::show(ui, &mut c, &mut ps, &Library::default(), &Shown::default());
        outliner::show(ui, &mut c, &mut os);
        sidebar::show(ui, &mut c);
    });
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn every_popup_draws() {
    use crate::commands::{Cmd, Similar};
    use crate::ui::Popup;
    let (mut editor, dir) = furnished("popups");
    let built = built_of(&editor);
    let (mut tool, mut orbit, mut jobs, mut shell) = (
        Tool::default(),
        Orbit::default(),
        Jobs::default(),
        Shell::default(),
    );
    editor.selection.select(Item::Spline(0));
    editor.selection.others = vec![Item::Prop(0)];
    let at = Vec2::new(400.0, 300.0);
    let popups = [
        Popup::search(at),
        Popup::Handles { at },
        Popup::Pie { at },
        Popup::Choose {
            at,
            of: Popup::SIMILAR,
        },
        Popup::Choose {
            at,
            of: Popup::MIRROR,
        },
        Popup::Collection {
            at,
            text: "new".into(),
        },
        Popup::BatchRename {
            at,
            find: "".into(),
            replace: "kerb".into(),
        },
    ];
    let ctx = egui::Context::default();
    for popup in popups {
        shell.popup = Some(popup);
        let mut c = Ctx {
            editor: &mut editor,
            tool: &mut tool,
            orbit: &mut orbit,
            jobs: &mut jobs,
            built: &built,
            shell: &mut shell,
            pointer: at,
        };
        draw(&ctx, |ui| crate::popups::show(ui.ctx(), &mut c));
        assert!(
            shell.popup.is_some(),
            "stays open until something is chosen"
        );
    }
    // The commands behind them run on this selection.
    let mut c = Ctx {
        editor: &mut editor,
        tool: &mut tool,
        orbit: &mut orbit,
        jobs: &mut jobs,
        built: &built,
        shell: &mut shell,
        pointer: at,
    };
    for cmd in [
        Cmd::SelectSimilar(Similar::Kind),
        Cmd::Mirror(true),
        Cmd::Mirror(false),
    ] {
        assert!(cmd.enabled(&c), "{cmd:?}");
        crate::commands::run(cmd, &mut c);
    }
    assert!(c.editor.selection.items().len() >= 2);
    std::fs::remove_dir_all(dir).unwrap();
}
