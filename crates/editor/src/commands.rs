//! Everything the editor can do from a menu, a shortcut or the search (F3): each
//! command's name, shortcut, when it applies and what it does. Menus list commands, so
//! they show the same names and shortcuts everywhere.

use bevy::math::Vec2;
use bevy_egui::egui;
use open_racing_track_project::project::HandleMode;

use crate::edit;
use crate::jobs::Jobs;
use crate::presets::PRESETS;
use crate::preview::Built;
use crate::state::{Editor, Item};
use crate::ui::{BottomTab, Popup, Shell};
use crate::viewport::{
    self, Draw, DrawKind, Mode, Orbit, Tool, ToolKind, ViewDir, delete, frame_all, frame_selection,
    look,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Undo,
    Redo,
    Search,
    Rename,
    Bake,
    BakeDrive,
    Quit,
    View(ViewDir),
    ToggleOrtho,
    FrameSelected,
    FrameAll,
    ViewPie,
    ToggleToolbar,
    ToggleSidebar,
    ToggleMaximize,
    ToggleSnap,
    Shortcuts,
    SelectAll,
    SelectNone,
    SelectInvert,
    SelectMore,
    SelectLess,
    DrawRoad,
    DrawSpline(usize),
    PlaceProp,
    Grab,
    Rotate,
    Scale,
    /// A road's width at the selected nodes.
    Width,
    /// A road's bank at the selected nodes.
    Tilt,
    Extrude,
    Subdivide,
    Delete,
    Handles(HandleMode),
    HandleMenu,
    ToggleClosed,
    Duplicate,
    SetMain,
    UseTool(ToolKind),
}

/// What a command works on.
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub tool: &'a mut Tool,
    pub orbit: &'a mut Orbit,
    pub jobs: &'a mut Jobs,
    pub built: &'a Built,
    pub shell: &'a mut Shell,
    /// The pointer, in window coordinates: where transforms start and popups open.
    pub pointer: Vec2,
}

impl Cmd {
    /// Every command, for the search.
    pub fn all() -> Vec<Cmd> {
        use Cmd::*;
        let mut all = vec![
            Undo,
            Redo,
            Rename,
            Bake,
            BakeDrive,
            Quit,
            ToggleOrtho,
            FrameSelected,
            FrameAll,
            ViewPie,
            ToggleToolbar,
            ToggleSidebar,
            ToggleMaximize,
            ToggleSnap,
            Shortcuts,
            SelectAll,
            SelectNone,
            SelectInvert,
            SelectMore,
            SelectLess,
            DrawRoad,
            PlaceProp,
            Grab,
            Rotate,
            Scale,
            Width,
            Tilt,
            Extrude,
            Subdivide,
            Delete,
            Handles(HandleMode::Auto),
            Handles(HandleMode::Aligned),
            Handles(HandleMode::Free),
            ToggleClosed,
            Duplicate,
            SetMain,
        ];
        all.extend(ViewDir::ALL.map(View));
        all.extend((0..PRESETS.len()).map(DrawSpline));
        all.extend(ToolKind::ALL.map(UseTool));
        all
    }

    pub fn label(self) -> String {
        use Cmd::*;
        match self {
            Undo => "Undo".into(),
            Redo => "Redo".into(),
            Search => "Search…".into(),
            Rename => "Rename Active Item…".into(),
            Bake => "Bake".into(),
            BakeDrive => "Bake & Drive".into(),
            Quit => "Quit".into(),
            View(v) => format!("View {}", v.label()),
            ToggleOrtho => "Perspective/Orthographic".into(),
            FrameSelected => "Frame Selected".into(),
            FrameAll => "Frame All".into(),
            ViewPie => "View Pie…".into(),
            ToggleToolbar => "Toolbar".into(),
            ToggleSidebar => "Sidebar".into(),
            ToggleMaximize => "Maximize 3D View".into(),
            ToggleSnap => "Snapping".into(),
            Shortcuts => "Keyboard Shortcuts".into(),
            SelectAll => "Select All".into(),
            SelectNone => "Select None".into(),
            SelectInvert => "Invert Selection".into(),
            SelectMore => "Select More".into(),
            SelectLess => "Select Less".into(),
            DrawRoad => "Road".into(),
            DrawSpline(i) => PRESETS[i].label.into(),
            PlaceProp => "Prop (from Assets)…".into(),
            Grab => "Move".into(),
            Rotate => "Rotate".into(),
            Scale => "Scale".into(),
            Width => "Road Width".into(),
            Tilt => "Road Bank (Tilt)".into(),
            Extrude => "Extrude Node".into(),
            Subdivide => "Subdivide".into(),
            Delete => "Delete".into(),
            Handles(m) => format!("Handles: {}", handle_label(m)),
            HandleMenu => "Set Handle Type…".into(),
            ToggleClosed => "Toggle Closed Loop".into(),
            Duplicate => "Duplicate".into(),
            SetMain => "Make Main Road".into(),
            UseTool(t) => format!("Tool: {}", t.label()),
        }
    }

    /// The label the search matches and lists: with the menu it lives in.
    pub fn search_label(self) -> String {
        use Cmd::*;
        let menu = match self {
            DrawRoad | DrawSpline(_) | PlaceProp => "Add",
            SelectAll | SelectNone | SelectInvert | SelectMore | SelectLess => "Select",
            Grab | Rotate | Scale | Width | Tilt | Extrude | Subdivide | Delete | Handles(_)
            | HandleMenu | ToggleClosed | Duplicate | SetMain | Rename => "Edit",
            View(_) | ToggleOrtho | FrameSelected | FrameAll | ViewPie | ToggleToolbar
            | ToggleSidebar | ToggleMaximize | ToggleSnap => "View",
            _ => "",
        };
        if menu.is_empty() {
            self.label()
        } else {
            format!("{menu} › {}", self.label())
        }
    }

    pub fn shortcut(self) -> &'static str {
        use Cmd::*;
        match self {
            Undo => "Ctrl Z",
            Redo => "Ctrl Shift Z",
            Search => "F3",
            Rename => "F2",
            Quit => "Ctrl Q",
            View(v) => v.shortcut(),
            ToggleOrtho => "Numpad 5",
            FrameSelected => "Numpad . / F",
            FrameAll => "Home",
            ViewPie => "`",
            ToggleToolbar => "T",
            ToggleSidebar => "N",
            ToggleMaximize => "Ctrl Space",
            SelectAll => "A",
            SelectNone => "Alt A",
            SelectInvert => "Ctrl I",
            SelectMore => "Ctrl Numpad +",
            SelectLess => "Ctrl Numpad -",
            Grab => "G",
            Rotate => "R",
            Scale => "S",
            Width => "Alt S",
            Tilt => "Ctrl T",
            Extrude => "E",
            Delete => "X",
            HandleMenu => "V",
            ToggleClosed => "Alt C",
            Duplicate => "Shift D",
            _ => "",
        }
    }

    pub fn enabled(self, c: &Ctx) -> bool {
        use Cmd::*;
        let e = &*c.editor;
        let sel = &e.selection;
        let line = e.line().is_some();
        let node = line && sel.node().is_some();
        match self {
            Undo => e.can_undo(),
            Redo => e.can_redo(),
            Bake | BakeDrive => !c.jobs.running(),
            Rename | Grab | Rotate | Scale | Delete => sel.item.is_some(),
            FrameSelected => sel.item.is_some(),
            SelectAll | SelectNone | SelectInvert | SelectMore | SelectLess | Subdivide
            | ToggleClosed => line,
            Extrude | Handles(_) | HandleMenu => node,
            Width | Tilt => sel.road().is_some(),
            Duplicate => matches!(sel.item, Some(Item::Spline(_) | Item::Prop(_))),
            SetMain => e.road_name().is_some_and(|r| r != e.project.main_road),
            _ => true,
        }
    }

    /// Whether it is switched on, for commands that toggle.
    pub fn checked(self, c: &Ctx) -> Option<bool> {
        use Cmd::*;
        match self {
            ToggleOrtho => Some(c.orbit.ortho),
            ToggleToolbar => Some(c.shell.toolbar),
            ToggleSidebar => Some(c.shell.sidebar),
            ToggleMaximize => Some(c.shell.maximized),
            ToggleSnap => Some(c.tool.snap),
            UseTool(t) => Some(c.tool.active == t),
            _ => None,
        }
    }
}

pub fn handle_label(mode: HandleMode) -> &'static str {
    match mode {
        HandleMode::Auto => "Automatic",
        HandleMode::Aligned => "Aligned",
        HandleMode::Free => "Free",
    }
}

fn start_draw(tool: &mut Tool, kind: DrawKind) {
    tool.draw = Some(Draw {
        kind,
        points: Vec::new(),
    });
    tool.menu = None;
}

pub fn run(cmd: Cmd, c: &mut Ctx) {
    use Cmd::*;
    if !cmd.enabled(c) {
        return;
    }
    let at = c.pointer;
    match cmd {
        Undo => c.editor.undo(),
        Redo => c.editor.redo(),
        Search => c.shell.popup = Some(Popup::search(at)),
        Rename => {
            if let Some(item) = c.editor.selection.item {
                let text = edit::item_name(&c.editor.project, item)
                    .unwrap_or_default()
                    .to_string();
                c.shell.popup = Some(Popup::Rename { at, item, text });
            }
        }
        Bake => c.jobs.bake(&c.editor.project, &c.editor.dir, false),
        BakeDrive => c.jobs.bake(&c.editor.project, &c.editor.dir, true),
        Quit => c.shell.quit = true,
        View(v) => look(c.orbit, v),
        ToggleOrtho => {
            c.orbit.ortho = !c.orbit.ortho;
            c.orbit.auto_ortho = false;
        }
        FrameSelected => frame_selection(c.editor, c.orbit),
        FrameAll => frame_all(c.editor, c.orbit),
        ViewPie => c.shell.popup = Some(Popup::Pie { at }),
        ToggleToolbar => c.shell.toolbar = !c.shell.toolbar,
        ToggleSidebar => c.shell.sidebar = !c.shell.sidebar,
        ToggleMaximize => c.shell.maximized = !c.shell.maximized,
        ToggleSnap => c.tool.snap = !c.tool.snap,
        Shortcuts => c.shell.shortcuts = !c.shell.shortcuts,
        SelectAll => viewport::select_all(c.editor),
        SelectNone => c.editor.selection.nodes.clear(),
        SelectInvert => edit::select_invert(c.editor),
        SelectMore => edit::select_more(c.editor),
        SelectLess => edit::select_less(c.editor),
        DrawRoad => start_draw(c.tool, DrawKind::Road),
        DrawSpline(i) => start_draw(c.tool, DrawKind::Spline(i)),
        PlaceProp => {
            c.shell.maximized = false;
            c.shell.bottom_open = true;
            c.shell.bottom = BottomTab::Assets;
            c.editor.status = "pick a model in Assets and press Place".into();
        }
        Grab => viewport::start_modal(c.editor, c.tool, c.built, Mode::Grab, None, at, false),
        Rotate => viewport::start_modal(c.editor, c.tool, c.built, Mode::Rotate, None, at, false),
        Scale => viewport::start_modal(c.editor, c.tool, c.built, Mode::Scale, None, at, false),
        Width => viewport::start_modal(c.editor, c.tool, c.built, Mode::Width, None, at, false),
        Tilt => viewport::start_modal(c.editor, c.tool, c.built, Mode::Tilt, None, at, false),
        Extrude => viewport::extrude(c.editor, c.tool, c.built, at),
        Subdivide => edit::subdivide(c.editor, c.built),
        Delete => delete(c.editor),
        Handles(m) => edit::set_handles(c.editor, m),
        HandleMenu => c.shell.popup = Some(Popup::Handles { at }),
        ToggleClosed => edit::toggle_closed(c.editor),
        Duplicate => viewport::duplicate(c.editor, c.tool, c.built, at),
        SetMain => edit::set_main(c.editor),
        UseTool(t) => c.tool.active = t,
    }
}

/// A menu entry for a command, with its shortcut; runs it and closes the menu when
/// clicked.
pub fn entry(ui: &mut egui::Ui, c: &mut Ctx, cmd: Cmd) -> bool {
    let clicked = button_as(ui, c, cmd, &cmd.label());
    if clicked {
        ui.close();
    }
    clicked
}

/// A button for a command in a panel.
pub fn button(ui: &mut egui::Ui, c: &mut Ctx, cmd: Cmd) -> bool {
    button_as(ui, c, cmd, &cmd.label())
}

/// A button for a command under another name, with its shortcut.
pub fn button_as(ui: &mut egui::Ui, c: &mut Ctx, cmd: Cmd, label: &str) -> bool {
    let enabled = cmd.enabled(c);
    let text = match cmd.checked(c) {
        Some(true) => format!("✔ {label}"),
        Some(false) => format!("    {label}"),
        None => label.to_string(),
    };
    let button = egui::Button::new(text).shortcut_text(cmd.shortcut());
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        run(cmd, c);
    }
    clicked
}

/// The shortcuts the egui side handles: popups and panels. The view's own keys (G, R,
/// S, E, X, A, numpad…) are read by `viewport::input`.
pub fn shortcuts(ctx: &egui::Context, c: &mut Ctx, over_view: bool) {
    if ctx.egui_wants_keyboard_input() || c.tool.modal.is_some() || c.shell.popup.is_some() {
        return;
    }
    use egui::{Key, Modifiers};
    let pressed =
        |key, mods: Modifiers| ctx.input(|i| i.key_pressed(key) && i.modifiers.matches_exact(mods));
    let none = Modifiers::NONE;
    let run_if = |cmd: Cmd, key, mods, c: &mut Ctx| {
        if pressed(key, mods) {
            run(cmd, c);
        }
    };
    run_if(Cmd::Undo, Key::Z, Modifiers::COMMAND, c);
    run_if(Cmd::Redo, Key::Z, Modifiers::COMMAND | Modifiers::SHIFT, c);
    run_if(Cmd::Redo, Key::Y, Modifiers::COMMAND, c);
    run_if(Cmd::Quit, Key::Q, Modifiers::COMMAND, c);
    run_if(Cmd::Search, Key::F3, none, c);
    run_if(Cmd::Rename, Key::F2, none, c);
    run_if(Cmd::ToggleMaximize, Key::Space, Modifiers::COMMAND, c);
    if c.tool.draw.is_some() {
        return;
    }
    run_if(Cmd::ToggleToolbar, Key::T, none, c);
    run_if(Cmd::ToggleSidebar, Key::N, none, c);
    if over_view {
        run_if(Cmd::HandleMenu, Key::V, none, c);
        run_if(Cmd::ToggleClosed, Key::C, Modifiers::ALT, c);
        run_if(Cmd::SelectInvert, Key::I, Modifiers::COMMAND, c);
        run_if(Cmd::ViewPie, Key::Backtick, none, c);
    }
}
