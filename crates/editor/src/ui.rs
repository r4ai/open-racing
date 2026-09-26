//! The editor's window, laid out as Blender's: the top bar with the menus, the 3D view
//! with its header, toolbar (T) and sidebar (N), the outliner and the properties on the
//! right, the curves, assets and bake report below the view, and the status bar at the
//! bottom with what the mouse does, messages and statistics.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use open_racing_track_project::project::Side;
use open_racing_track_project::projects_dir;

use crate::assets::{self, Library};
use crate::commands::{self, Cmd, Ctx, entry};
use crate::curve_graph::{self, CurveGraph};
use crate::edit;
use crate::jobs::Jobs;
use crate::preview::{Built, Props};
use crate::profile::ProfileView;
use crate::state::{Editor, Item};
use crate::viewport::{EditorCamera, Hit, Orbit, Tool, ToolKind, View, ViewDir, ViewRect};
use crate::{menus, outliner, overlay, popups, properties, sidebar};

/// What the area below the 3D view shows.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum BottomTab {
    #[default]
    Curves,
    Checks,
    Assets,
    Report,
}

/// The tabs of the properties editor: the track's settings, then the selected item's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropTab {
    Track,
    Markers,
    Terrain,
    /// Woods, bushes and rocks painted over the ground.
    Scatter,
    Reference,
    /// Strip and wall types, materials and surfaces.
    Library,
    #[default]
    Object,
    Corners,
    Strips,
    Lines,
    Barriers,
    /// Rows of models beside the road.
    Rows,
}

impl PropTab {
    /// The tab called `name` in lower case, as the command line names it.
    pub fn named(name: &str) -> Option<Self> {
        use PropTab::*;
        [
            Track, Markers, Terrain, Scatter, Reference, Library, Object, Corners, Strips, Lines,
            Barriers, Rows,
        ]
        .into_iter()
        .find(|t| format!("{t:?}").eq_ignore_ascii_case(name))
    }
}

/// A part of a road the outliner asked the properties editor to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Strip(Side, usize),
    Line(usize),
    Barrier(usize),
    Row(usize),
}

/// A popup over everything, taking the keys and clicks until it closes.
pub enum Popup {
    /// F3: find a command by name.
    Search {
        at: Vec2,
        text: String,
        selected: usize,
    },
    /// F2: rename the active item.
    Rename { at: Vec2, item: Item, text: String },
    /// V: the handle types.
    Handles { at: Vec2 },
    /// `: the view pie.
    Pie { at: Vec2 },
    /// Ctrl F2: renames the selected items, finding and replacing in their names, or
    /// giving them all one name numbered.
    BatchRename {
        at: Vec2,
        find: String,
        replace: String,
    },
    /// M: the collection to move the selected splines and props to.
    Collection { at: Vec2, text: String },
    /// A few commands to choose from (Shift G, Ctrl M).
    Choose {
        at: Vec2,
        of: (&'static str, &'static [Cmd]),
    },
}

impl Popup {
    pub const SIMILAR: (&'static str, &'static [Cmd]) = (
        "Select Similar",
        &[
            Cmd::SelectSimilar(commands::Similar::Kind),
            Cmd::SelectSimilar(commands::Similar::Type),
            Cmd::SelectSimilar(commands::Similar::Material),
        ],
    );
    pub const MIRROR: (&'static str, &'static [Cmd]) =
        ("Mirror", &[Cmd::Mirror(true), Cmd::Mirror(false)]);

    pub fn search(at: Vec2) -> Self {
        Popup::Search {
            at,
            text: String::new(),
            selected: 0,
        }
    }
}

/// Which areas are open, and the popups: what commands change about the window.
pub struct Shell {
    pub toolbar: bool,
    pub sidebar: bool,
    /// Ctrl + Space: the 3D view alone.
    pub maximized: bool,
    /// Copy the selection to the clipboard this frame.
    pub copy: bool,
    pub bottom_open: bool,
    pub bottom: BottomTab,
    pub tab: PropTab,
    pub sidebar_tab: sidebar::Tab,
    pub focus: Option<Focus>,
    /// The corner looked at last: its road and number.
    pub corner: Option<(usize, usize)>,
    /// The side kerbs and walls are laid along the selected nodes on.
    pub lay_side: crate::lay::LaySide,
    pub popup: Option<Popup>,
    /// The keyboard shortcuts window.
    pub shortcuts: bool,
    pub quit: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            toolbar: true,
            sidebar: true,
            maximized: false,
            copy: false,
            bottom_open: true,
            bottom: BottomTab::default(),
            tab: PropTab::default(),
            sidebar_tab: sidebar::Tab::default(),
            focus: None,
            corner: None,
            lay_side: Default::default(),
            popup: None,
            shortcuts: false,
            quit: false,
        }
    }
}

#[derive(Default)]
pub struct UiState {
    shell: Shell,
    new_project: String,
    profile: ProfileView,
    curve_graph: CurveGraph,
    assets: assets::Panel,
    properties: properties::State,
    outliner: outliner::State,
    styled: bool,
}

/// Tab is Blender's mode switch: egui must not take it as well to move its keyboard focus
/// from button to button (a focused button then holds every key the view reads), except
/// while text is typed.
pub fn keep_tab(
    mut contexts: Query<
        (&mut bevy_egui::EguiInput, &mut bevy_egui::EguiContext),
        With<bevy_egui::PrimaryEguiContext>,
    >,
) {
    for (mut input, mut ctx) in &mut contexts {
        if ctx.get_mut().text_edit_focused() {
            continue;
        }
        input.events.retain(|e| {
            !matches!(
                e,
                egui::Event::Key {
                    key: egui::Key::Tab,
                    ..
                }
            )
        });
    }
}

/// Blender's colours where egui's defaults differ most: the blue of what is selected.
fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        let v = &mut s.visuals;
        v.selection.bg_fill = egui::Color32::from_rgb(71, 114, 179);
        v.selection.stroke.color = egui::Color32::WHITE;
        v.panel_fill = egui::Color32::from_gray(43);
        v.window_fill = egui::Color32::from_gray(38);
        v.extreme_bg_color = egui::Color32::from_gray(29);
        v.faint_bg_color = egui::Color32::from_gray(50);
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
    });
}

#[allow(clippy::too_many_arguments)]
pub fn ui(
    mut contexts: EguiContexts,
    mut editor: ResMut<Editor>,
    mut jobs: ResMut<Jobs>,
    mut orbit: ResMut<Orbit>,
    mut rect: ResMut<ViewRect>,
    mut tool: ResMut<Tool>,
    built: Res<Built>,
    mut library: ResMut<Library>,
    props: Res<Props>,
    reference: Res<crate::reference::Shown>,
    start: crate::Start,
    mut cmds: Commands,
    mut state: Local<UiState>,
    camera: Single<(&Camera, &GlobalTransform), With<EditorCamera>>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    if !state.styled {
        style(&ctx);
        state.styled = true;
    }
    // The window starts out tiny on some systems; the panels need room.
    if ctx.viewport_rect().width() < 800.0 || ctx.viewport_rect().height() < 500.0 {
        rect.rect = None;
        return Ok(());
    }
    let (cam, t) = *camera;
    let view = View { cam, t };
    let pointer = ctx
        .pointer_latest_pos()
        .map_or(Vec2::ZERO, |p| Vec2::new(p.x, p.y));
    let over_view = rect.rect.is_some_and(|r| r.contains(pointer));
    let UiState {
        shell,
        new_project,
        profile,
        curve_graph,
        assets: asset_panel,
        properties: prop_state,
        outliner: out_state,
        ..
    } = &mut *state;
    let mut c = Ctx {
        editor: &mut editor,
        tool: &mut tool,
        orbit: &mut orbit,
        jobs: &mut jobs,
        built: &built,
        shell,
        pointer,
    };
    let (start_corner, start_tab, start_curves, start_distance, on_ground) = start;
    if c.built.count > 0 {
        if let Some(n) = start_corner {
            crate::corners::step_to(&mut c, n.0);
            cmds.remove_resource::<crate::StartCorner>();
        }
        if let Some(t) = start_tab {
            c.shell.tab = t.0;
            cmds.remove_resource::<crate::StartTab>();
        }
        if let Some(s) = start_curves {
            c.shell.bottom = BottomTab::Curves;
            c.shell.bottom_open = true;
            curve_graph.show(c.editor, s.0);
            cmds.remove_resource::<crate::StartCurves>();
        }
        if let Some(d) = start_distance {
            c.orbit.distance = d.0;
            cmds.remove_resource::<crate::StartDistance>();
        }
        if on_ground.is_some() {
            let p = open_racing_track_render::from_bevy(c.orbit.focus);
            if let Some(h) = c
                .built
                .ground
                .as_ref()
                .and_then(|g| g.raycast_down(p.with_z(1e4), 2e4))
            {
                c.orbit.focus = open_racing_track_render::to_bevy(h.point);
            }
            cmds.remove_resource::<crate::StartOnGround>();
        }
    }
    commands::shortcuts(&ctx, &mut c, over_view);
    c.tool.outliner_hover = None;
    c.tool.landforms = c.shell.tab == PropTab::Terrain && !c.shell.maximized;
    // What the panels change about the active item, the other selected items of its
    // kind take too.
    let active = c.editor.selection.item;
    let before = active
        .filter(|_| crate::batch::followers(c.editor) > 0)
        .and_then(|i| crate::batch::snapshot(c.editor, i));

    let mut root = egui::Ui::new(
        ctx.clone(),
        "root".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    // The 3D view's sky fills the window under the panels: whatever the panels leave
    // uncovered beside the view (a pixel between two of them) is painted as a panel.
    let backdrop = root.painter().add(egui::Shape::Noop);

    egui::Panel::top("top bar").show(&mut root, |ui| top_bar(ui, &mut c, new_project));
    egui::Panel::bottom("status bar").show(&mut root, |ui| status_bar(ui, &c));

    if !c.shell.maximized {
        egui::Panel::right("right column")
            .resizable(true)
            .default_size(390.0)
            .size_range(260.0..=700.0)
            .frame(egui::Frame::NONE)
            .show(&mut root, |ui| {
                egui::Panel::top("outliner")
                    .resizable(true)
                    .default_size(260.0)
                    .size_range(80.0..=900.0)
                    .show(ui, |ui| outliner::show(ui, &mut c, out_state));
                egui::CentralPanel::default().show(ui, |ui| {
                    properties::show(ui, &mut c, prop_state, &library, &reference);
                });
            });
        let mut open = c.shell.bottom_open;
        // As tall as the user drags it, leaving the view no more than its header and a
        // strip below; what does not fit scrolls, so nothing in it holds the edge.
        let tallest = (root.available_height() - 70.0).max(60.0);
        egui::Panel::bottom("bottom area")
            .resizable(true)
            .default_size(230.0)
            .size_range(60.0..=tallest)
            .show_collapsible(&mut root, &mut open, |ui| {
                bottom_area(
                    ui,
                    &mut c,
                    profile,
                    curve_graph,
                    asset_panel,
                    &mut library,
                    &props,
                );
            });
        c.shell.bottom_open = open;
    }
    // A drag in a graph that is no longer shown would never end.
    if c.shell.maximized || !c.shell.bottom_open || c.shell.bottom != BottomTab::Curves {
        profile.stop(c.editor);
        curve_graph.stop(c.editor);
    }

    menus::header(&mut root, &mut c);
    menus::tool_settings(&mut root, &mut c);
    let mut open = c.shell.sidebar;
    egui::Panel::right("sidebar")
        .resizable(true)
        .default_size(250.0)
        .size_range(200.0..=500.0)
        .show_collapsible(&mut root, &mut open, |ui| sidebar::show(ui, &mut c));
    c.shell.sidebar = open;

    if let (Some(item), Some(before)) = (active, before)
        && c.editor.selection.item == Some(item)
        && !c.editor.dragging()
    {
        let ops = crate::batch::spread_ops(c.editor, item, &before);
        let n = ops.len();
        if n > 0 && c.editor.apply_along(ops) {
            c.editor.status = format!("and the same to {n} more selected");
        }
    }

    // What is left is the 3D view.
    let free = root.available_rect_before_wrap();
    // The panels' resize handles reach into the view: clicks there resize, not select.
    let inner = free.shrink(ctx.global_style().interaction.resize_grab_radius_side + 1.0);
    rect.rect = Some(Rect::new(
        inner.min.x,
        inner.min.y,
        inner.max.x,
        inner.max.y,
    ));
    root.painter().set(
        backdrop,
        around(ctx.viewport_rect(), free, root.visuals().panel_fill),
    );
    overlay::view(&ctx, free, &mut c, view);
    menus::overlay(&ctx, &mut c);
    popups::show(&ctx, &mut c);
    if std::mem::take(&mut c.shell.copy)
        && let Some((text, n)) = crate::clipboard::copy(c.editor)
    {
        ctx.copy_text(text);
        c.editor.status = format!("{n} copied: Ctrl V pastes them, here or in another project");
    }
    c.tool.blocked = c.shell.popup.is_some();
    rect.ui_dragging = ctx.egui_is_using_pointer();
    rect.ui_busy = rect.ui_dragging || on_panel_edge(&ctx);
    if c.shell.quit {
        exit.write(AppExit::Success);
    }
    Ok(())
}

/// Rectangles filling `outer` round `hole`.
fn around(outer: egui::Rect, hole: egui::Rect, fill: egui::Color32) -> egui::Shape {
    let hole = hole.intersect(outer);
    let rects = [
        egui::Rect::from_x_y_ranges(outer.x_range(), outer.top()..=hole.top()),
        egui::Rect::from_x_y_ranges(outer.x_range(), hole.bottom()..=outer.bottom()),
        egui::Rect::from_x_y_ranges(outer.left()..=hole.left(), hole.y_range()),
        egui::Rect::from_x_y_ranges(hole.right()..=outer.right(), hole.y_range()),
    ];
    egui::Shape::Vec(
        rects
            .into_iter()
            .filter(|r| r.is_positive())
            .map(|r| egui::Shape::rect_filled(r, 0.0, fill))
            .collect(),
    )
}

/// Whether the pointer is over a panel's edge, which a press there would resize: the UI
/// shows a resize cursor.
fn on_panel_edge(ctx: &egui::Context) -> bool {
    use egui::CursorIcon as C;
    matches!(
        ctx.output(|o| o.cursor_icon),
        C::ResizeHorizontal
            | C::ResizeVertical
            | C::ResizeColumn
            | C::ResizeRow
            | C::ResizeEast
            | C::ResizeWest
            | C::ResizeNorth
            | C::ResizeSouth
            | C::ResizeNeSw
            | C::ResizeNwSe
    )
}

fn list_projects() -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<_> = std::fs::read_dir(projects_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(open_racing_track_project::PROJECT_FILE).is_file())
        .collect();
    dirs.sort();
    dirs
}

/// Lays a new road along a centreline file the user picks, and selects it.
fn import_centreline(c: &mut Ctx) {
    use open_racing_track_project::centreline;
    let Some(file) = rfd::FileDialog::new()
        .add_filter(
            "centrelines",
            &["gpx", "kml", "geojson", "json", "csv", "txt"],
        )
        .pick_file()
    else {
        return;
    };
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let line = match std::fs::read_to_string(&file)
        .map_err(|e| e.to_string())
        .and_then(|src| centreline::read(&name, &src).map_err(|e| e.to_string()))
    {
        Ok(line) => line,
        Err(e) => {
            c.editor.status = format!("not imported: {e}");
            return;
        }
    };
    let stem = file
        .file_stem()
        .map_or("imported".into(), |s| s.to_string_lossy().into_owned());
    let road = crate::presets::unique_name(&c.editor.project, stem.trim());
    let ops = centreline::road_ops(&c.editor.project, &line, &road, 1.0);
    if c.editor.apply(ops, None) {
        let r = c.editor.project.roads.len() - 1;
        c.editor.selection.select(Item::Road(r));
        crate::viewport::frame_selection(c.editor, c.orbit);
        let nodes = c.editor.project.roads[r].nodes.len();
        c.editor.status = format!(
            "laid \"{road}\" along {} points with {nodes} nodes{}",
            line.points.len(),
            if line.closed {
                "; Make Main Road to race on it"
            } else {
                " (an open line)"
            }
        );
    }
}

/// Puts nodes on the ground of elevation data: the selected line's, or every line's.
fn import_heights(c: &mut Ctx) {
    use open_racing_track_project::dem;
    let Some(file) = rfd::FileDialog::new()
        .add_filter(
            "elevation data",
            &["tif", "tiff", "asc", "xyz", "csv", "txt"],
        )
        .pick_file()
    else {
        return;
    };
    let heights = match dem::read_file(&file, c.editor.project.geo) {
        Ok(h) => h,
        Err(e) => {
            c.editor.status = format!("not read: {e}");
            return;
        }
    };
    let lines: Vec<String> = c
        .editor
        .line()
        .map(|(n, ..)| vec![n.to_string()])
        .unwrap_or_default();
    let (ops, moved) = dem::node_ops(&c.editor.project, &heights, &lines, 0.0);
    if moved == 0 {
        c.editor.status = "the elevation data does not cover these nodes".into();
    } else if c.editor.apply(ops, None) {
        c.editor.status = format!(
            "{moved} nodes put on the ground; Smooth Heights evens out the data's roughness"
        );
    }
}

/// Opens another project, dropping what the tools were doing in this one.
fn open_project(c: &mut Ctx, dir: std::path::PathBuf) {
    let before = c.editor.dir.clone();
    c.editor.switch(dir);
    if c.editor.dir != before {
        c.tool.reset();
        c.jobs.forget();
        c.orbit.walk = None;
        c.orbit.replay = None;
        c.shell.popup = None;
        c.shell.focus = None;
        c.shell.corner = None;
        crate::viewport::frame_all(c.editor, c.orbit);
    }
}

fn top_bar(ui: &mut egui::Ui, c: &mut Ctx, new_project: &mut String) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            ui.weak(c.editor.dir.display().to_string());
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(new_project)
                        .hint_text("project name")
                        .desired_width(140.0),
                );
                if ui.button("New / Open").clicked() && !new_project.trim().is_empty() {
                    open_project(c, projects_dir().join(new_project.trim()));
                    ui.close();
                }
            });
            ui.menu_button("Open Project", |ui| {
                let dirs = list_projects();
                if dirs.is_empty() {
                    ui.weak("no projects yet");
                }
                for dir in dirs {
                    let name = dir
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if ui.button(name).clicked() {
                        open_project(c, dir);
                        ui.close();
                    }
                }
            });
            ui.menu_button("Restore Backup", |ui| {
                let list = crate::state::backups(&c.editor.dir);
                if list.is_empty() {
                    ui.weak("no backups yet: one is kept every 5 minutes of editing");
                }
                let now = std::time::SystemTime::now();
                for (path, time) in list {
                    let ago = now.duration_since(time).map_or(0, |d| d.as_secs());
                    let label = match ago {
                        0..=119 => format!("{ago} s ago"),
                        120..=7199 => format!("{} min ago", ago / 60),
                        _ => format!("{} h ago", ago / 3600),
                    };
                    if ui
                        .button(label)
                        .on_hover_text(path.display().to_string())
                        .clicked()
                    {
                        c.editor.restore(&path);
                        ui.close();
                    }
                }
            });
            ui.separator();
            if ui
                .button("Import Centreline…")
                .on_hover_text(
                    "Lay a road along a real circuit: a GPS track (.gpx), a KML line, a GeoJSON \
                     line (OpenStreetMap) or a CSV of x, y[, z] metres or lon, lat[, ele]",
                )
                .clicked()
            {
                ui.close();
                import_centreline(c);
            }
            if ui
                .button("Heights from Elevation Data…")
                .on_hover_text(
                    "Put the nodes of the selected road or spline (or of every one) on the ground of elevation data: a GeoTIFF (.tif), an ESRI ASCII grid (.asc) or x y z points (.xyz, .csv), in metres, longitudes and latitudes or UTM. The Terrain tab makes the ground follow it too",
                )
                .clicked()
            {
                ui.close();
                import_heights(c);
            }
            ui.separator();
            entry(ui, c, Cmd::Bake);
            entry(ui, c, Cmd::BakeDrive);
            ui.separator();
            entry(ui, c, Cmd::Quit);
        });
        ui.menu_button("Edit", |ui| {
            entry(ui, c, Cmd::Undo);
            entry(ui, c, Cmd::Redo);
            ui.menu_button("Undo History", |ui| undo_history(ui, c));
            ui.separator();
            entry(ui, c, Cmd::Search);
            entry(ui, c, Cmd::Rename);
            entry(ui, c, Cmd::BatchRename);
            ui.separator();
            entry(ui, c, Cmd::Duplicate);
            entry(ui, c, Cmd::Copy);
            ui.add_enabled(false, egui::Button::new("Paste").shortcut_text("Ctrl V"))
                .on_disabled_hover_text("Ctrl V in the window pastes copied items or operations");
            entry(ui, c, Cmd::Delete);
        });
        ui.menu_button("View", |ui| {
            entry(ui, c, Cmd::ToggleToolbar);
            entry(ui, c, Cmd::ToggleSidebar);
            let label = if c.shell.bottom_open {
                "✔ Bottom Area"
            } else {
                "    Bottom Area"
            };
            if ui.button(label).clicked() {
                c.shell.bottom_open = !c.shell.bottom_open;
                ui.close();
            }
            entry(ui, c, Cmd::ToggleMaximize);
            ui.separator();
            for v in ViewDir::ALL {
                entry(ui, c, Cmd::View(v));
            }
            ui.separator();
            entry(ui, c, Cmd::FrameSelected);
            entry(ui, c, Cmd::FrameAll);
        });
        ui.menu_button("Help", |ui| {
            entry(ui, c, Cmd::Shortcuts);
            entry(ui, c, Cmd::Search);
        });
        ui.separator();
        ui.strong(&c.editor.project.name);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let idle = !c.jobs.running();
            if ui
                .add_enabled(idle, egui::Button::new("▶ Bake & Drive"))
                .on_hover_text("Bake the track, check it with a test lap and drive it")
                .clicked()
            {
                commands::run(Cmd::BakeDrive, c);
            }
            if ui
                .add_enabled(idle, egui::Button::new("Bake"))
                .on_hover_text("Bake the track into a package and check it")
                .clicked()
            {
                commands::run(Cmd::Bake, c);
            }
            if c.jobs.running() {
                ui.spinner();
            }
        });
    });
}

/// The steps that undo, newest at the top, and those that redo; a click goes back (or
/// on) to just after that step.
fn undo_history(ui: &mut egui::Ui, c: &mut Ctx) {
    ui.set_min_width(220.0);
    let (done, undone) = c.editor.history();
    let (done, undone): (Vec<String>, Vec<String>) = (
        done.into_iter().map(str::to_string).collect(),
        undone.into_iter().map(str::to_string).collect(),
    );
    let mut go = None;
    egui::ScrollArea::vertical()
        .max_height(420.0)
        .show(ui, |ui| {
            for (i, what) in undone.iter().enumerate().rev() {
                if ui
                    .button(egui::RichText::new(what).weak())
                    .on_hover_text("Redo up to here")
                    .clicked()
                {
                    go = Some(done.len() + i + 1);
                }
            }
            let now = egui::RichText::new(format!(
                "▶ {}",
                done.last().map_or("Original", String::as_str)
            ))
            .strong();
            ui.label(now);
            for (i, what) in done.iter().enumerate().rev().skip(1) {
                if ui
                    .button(what)
                    .on_hover_text("Go back to just after this")
                    .clicked()
                {
                    go = Some(i + 1);
                }
            }
            if !done.is_empty() && ui.button("Original").clicked() {
                go = Some(0);
            }
        });
    if let Some(steps) = go {
        c.editor.go_to(steps);
        ui.close();
    }
}

/// The problems the last build found; clicking one with a place looks at it.
fn checks(ui: &mut egui::Ui, c: &mut Ctx) {
    if c.built.issues.is_empty() {
        ui.label(
            "No problems found: corners, grades, banking, crossings and markers look drivable.",
        );
        ui.weak("Bake to drive a test lap as well.");
        return;
    }
    ui.weak("Updated as you edit. Click one to look at it.");
    for issue in c.built.issues.clone() {
        let place = issue
            .road
            .as_deref()
            .and_then(|r| c.editor.project.road_index(r))
            .zip(issue.s);
        let text = egui::RichText::new(format!("⚠ {}", issue.text))
            .color(egui::Color32::from_rgb(255, 190, 90));
        let resp = if place.is_some() {
            ui.add(egui::Button::new(text).frame(false))
                .on_hover_text("Select the road and look here")
        } else {
            ui.label(text)
        };
        if let Some((r, s)) = place
            && resp.clicked()
        {
            c.editor.selection.select(Item::Road(r));
            if let Some(smp) = c.built.roads.get(r) {
                c.orbit.focus = open_racing_track_render::to_bevy(smp.frame_at(s).pos);
                c.orbit.distance = c.orbit.distance.min(150.0);
            }
        }
    }
}

/// What the mouse does over what the pointer is on, Blender's status bar hints.
fn mouse_hints(c: &Ctx) -> String {
    let t = &*c.tool;
    if t.modal.is_some() || t.draw.is_some() || t.place.is_some() || t.active.is_brush() {
        return t.hint.clone();
    }
    let name = |item| {
        edit::item_name(&c.editor.project, item)
            .unwrap_or("")
            .to_string()
    };
    let tool = match t.active {
        _ if !t.edit && t.active == ToolKind::Select => {
            "Click: select · Drag: move it, or a box · Tab: edit nodes"
        }
        ToolKind::AddNode => "Click: add node",
        ToolKind::Measure => match t.measure.len() {
            1 => "Click: measure to here",
            _ => "Click: measure from here",
        },
        _ => "Click: select · Drag: box",
    };
    match t.hover {
        Some(Hit::Node(item, n)) => format!(
            "Node {n} of {}  ·  Click: select · Shift: extend · Drag: move · Right: menu",
            name(item)
        ),
        Some(Hit::Handle(..)) => "Handle  ·  Drag: move · Alt+click: automatic".into(),
        Some(Hit::Gizmo(axis)) => format!(
            "Drag: {} {}",
            t.active.label(),
            match axis {
                crate::viewport::Axis::Free => "freely".to_string(),
                a => format!("along {a:?}"),
            }
        ),
        Some(Hit::Marker(_)) => "Marker  ·  Drag: slide it along the road · Right: menu".into(),
        Some(Hit::Landform(..)) => {
            "Landform  ·  Drag: move its middle or end, or its edge for its radius".into()
        }
        Some(Hit::Range(_)) => {
            "Stretch end  ·  Drag: move it (catches on nodes and corners; Ctrl: free) · Right: remove".into()
        }
        Some(Hit::Reach(_)) => "Stretch's outer edge  ·  Drag: its width, or a wall's distance".into(),
        Some(Hit::Edge(..)) => {
            "Road edge  ·  Drag: the width on this side at the selected nodes".into()
        }
        Some(Hit::StripKey(k)) => format!(
            "Node {} of a strip  ·  Drag: along the road and out (its width) · Z while dragging: its height · Right: remove",
            k.key
        ),
        Some(Hit::Strip(..)) => {
            "Strip  ·  Ctrl+click: a node here, to change its width or height nearby · Right: menu".into()
        }
        Some(Hit::Line(r, i)) => format!(
            "Line {}  ·  Drag: move it across (catches on the centre, the edges and other lines; Ctrl: free) · Right: menu",
            c.editor
                .project
                .roads
                .get(r)
                .and_then(|road| road.lines.get(i))
                .map_or("", |l| l.name.as_str())
        ),
        Some(Hit::Body(item)) => format!(
            "{} {}  ·  {tool} · Right: insert node, markers",
            edit::item_kind(item),
            name(item)
        ),
        None => format!("{tool} · Middle: orbit · Shift Middle: pan · Wheel: zoom · Right: menu"),
    }
}

fn status_bar(ui: &mut egui::Ui, c: &Ctx) {
    ui.horizontal(|ui| {
        let hint = mouse_hints(c);
        let highlight = c.tool.modal.is_some() || c.tool.draw.is_some() || c.tool.place.is_some();
        let text = if highlight {
            egui::RichText::new(hint).color(egui::Color32::from_rgb(255, 215, 30))
        } else {
            egui::RichText::new(hint).weak()
        };
        let h = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width() * 0.55, h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.add(egui::Label::new(text).truncate()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let p = &c.editor.project;
            let nodes: usize = p.roads.iter().map(|r| r.nodes.len()).sum::<usize>()
                + p.splines.iter().map(|s| s.nodes.len()).sum::<usize>();
            let length = p
                .road_index(&p.main_road)
                .and_then(|i| c.built.roads.get(i))
                .map_or(String::new(), |s| format!(" · Lap {:.0} m", s.length));
            ui.weak(format!(
                "Roads {} · Splines {} · Props {} · Nodes {nodes}{length}",
                p.roads.len(),
                p.splines.len(),
                p.props.len()
            ));
            // Where the pointer is on the Earth.
            if let (Some(g), Some(p)) = (c.editor.project.geo, c.tool.pointer) {
                let (lon, lat) = g.to_geo(p.truncate());
                ui.weak(format!("{lat:.6}°, {lon:.6}°"));
            }
            ui.separator();
            ui.add(egui::Label::new(&c.editor.status).truncate())
                .on_hover_text(&c.editor.status);
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn bottom_area(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    profile: &mut ProfileView,
    curve_graph: &mut CurveGraph,
    asset_panel: &mut assets::Panel,
    library: &mut Library,
    props: &Props,
) {
    ui.horizontal(|ui| {
        let failed = c.jobs.passed == Some(false);
        let issues = c.built.issues.len();
        let checks = if issues == 0 {
            "✔ Checks".to_string()
        } else {
            format!("⚠ Checks ({issues})")
        };
        for (tab, label) in [
            (BottomTab::Curves, "📈 Curves".to_string()),
            (BottomTab::Checks, checks),
            (BottomTab::Assets, "📦 Assets".to_string()),
            (
                BottomTab::Report,
                if failed {
                    "⚠ Bake Report".to_string()
                } else {
                    "📋 Bake Report".to_string()
                },
            ),
        ] {
            ui.selectable_value(&mut c.shell.bottom, tab, label);
        }
    });
    ui.separator();
    match c.shell.bottom {
        BottomTab::Curves => {
            // The wheel zooms the graphs; only the bar scrolls.
            egui::ScrollArea::vertical()
                .id_salt("curves")
                .auto_shrink([false, false])
                .scroll_source(egui::containers::scroll_area::ScrollSource::SCROLL_BAR)
                .show(ui, |ui| curve_graph::panel(ui, c, profile, curve_graph));
        }
        BottomTab::Checks => {
            egui::ScrollArea::vertical()
                .id_salt("checks")
                .auto_shrink([false, false])
                .show(ui, |ui| checks(ui, c));
        }
        BottomTab::Assets => {
            egui::ScrollArea::vertical()
                .id_salt("assets")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    assets::panel(ui, c.editor, library, props, c.tool, asset_panel);
                });
        }
        BottomTab::Report => {
            egui::ScrollArea::vertical()
                .id_salt("report")
                .auto_shrink([false, false])
                .show(ui, |ui| match &c.jobs.report {
                    Some(r) => {
                        ui.monospace(r);
                    }
                    None => {
                        ui.label("Bake to check the track and drive a test lap.");
                    }
                });
        }
    }
}
