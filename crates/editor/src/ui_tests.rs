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
use crate::jobs::Jobs;
use crate::preview::Built;
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
    let ctx = egui::Context::default();
    let tabs = [
        PropTab::Track,
        PropTab::Markers,
        PropTab::Terrain,
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
