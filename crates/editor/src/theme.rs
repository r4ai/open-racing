//! The editor's colours, one scheme everywhere as Blender's: what is selected is orange
//! (the active one brighter) and drawn bold, unselected lines grey and unselected nodes
//! dark, what the pointer is over lighter (and ringed, for nodes and handles); handles
//! for widths and stretches have a colour of their own so that they are never taken for
//! the selection. `Look` and `NodeLook` say which of these anything is drawn with.

use bevy::color::Color;
use bevy_egui::egui::Color32;

/// The terrain's landforms in the view.
pub const LANDFORM: Color = Color::srgb(0.55, 0.85, 0.45);
/// Painted lines of the selected road, their stretches and handles.
pub const PAINT: Color = Color::srgb(0.95, 0.95, 0.85);

/// The selected item, and the lines of what is selected.
pub const SELECTED: Color = Color::srgb(1.0, 0.63, 0.16);
/// Items selected with the active one, darker as Blender's.
pub const SELECTED_OTHER: Color = Color::srgb(0.85, 0.42, 0.05);
/// Selected nodes.
pub const SELECTED_NODE: Color = Color::srgb(1.0, 0.45, 0.0);
/// The active node.
pub const ACTIVE_NODE: Color = Color::WHITE;
/// Nodes not selected.
pub const NODE: Color = Color::srgb(0.06, 0.06, 0.06);
/// Lines of items not selected.
pub const UNSELECTED: Color = Color::srgba(0.85, 0.87, 0.9, 0.45);
/// What the pointer is over.
pub const HOVER: Color = Color::srgb(1.0, 0.95, 0.7);
/// Stretches of strips, and handles for widths.
pub const STRIP: Color = Color::srgb(0.2, 0.85, 0.75);
/// Nodes of strips (their widths and heights along the road).
pub const STRIP_KEY: Color = Color::srgb(0.45, 1.0, 0.9);
/// Stretches of barriers.
pub const BARRIER: Color = Color::srgb(0.7, 0.7, 0.95);
/// A road's first node, showing which way it runs.
pub const START: Color = Color::srgb(0.25, 0.9, 0.4);
pub const OFF_TRACK: Color = Color::srgb(1.0, 0.2, 0.2);
/// What a grabbed node or end has caught on.
pub const SNAP: Color = Color::srgb(1.0, 0.2, 0.85);
/// Rings round dots that set them off from what is behind them.
pub const RIM_DARK: Color = Color::srgba(0.0, 0.0, 0.0, 0.85);
pub const RIM_LIGHT: Color = Color::srgba(0.92, 0.92, 0.92, 0.9);

/// The same in the panels.
pub const SELECTED_UI: Color32 = Color32::from_rgb(255, 160, 40);
pub const SELECTED_OTHER_UI: Color32 = Color32::from_rgb(217, 107, 13);
pub const SELECTED_NODE_UI: Color32 = Color32::from_rgb(255, 115, 0);
pub const ACTIVE_NODE_UI: Color32 = Color32::WHITE;
pub const UNSELECTED_UI: Color32 = Color32::from_gray(215);
pub const HOVER_UI: Color32 = Color32::from_rgb(255, 242, 178);
pub const STRIP_UI: Color32 = Color32::from_rgb(51, 217, 191);

/// How an item stands in the selection, and so how it is drawn everywhere: in the
/// view, its label and the outliner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// The active item: the properties show it.
    Active,
    /// Selected with the active one.
    Selected,
    /// Not selected, under the pointer (in the view or the outliner).
    Hovered,
    Normal,
}

impl Look {
    pub fn of(active: bool, selected: bool, hovered: bool) -> Self {
        if active {
            Look::Active
        } else if selected {
            Look::Selected
        } else if hovered {
            Look::Hovered
        } else {
            Look::Normal
        }
    }

    pub fn selected(self) -> bool {
        matches!(self, Look::Active | Look::Selected)
    }

    /// Drawn with bold lines: what is selected, and what a click would select.
    pub fn bold(self) -> bool {
        self != Look::Normal
    }

    /// Its lines in the view.
    pub fn line(self) -> Color {
        match self {
            Look::Active => SELECTED,
            Look::Selected => SELECTED_OTHER,
            Look::Hovered => HOVER,
            Look::Normal => UNSELECTED,
        }
    }

    /// Its name, over the view and in the panels.
    pub fn text(self) -> Color32 {
        match self {
            Look::Active => SELECTED_UI,
            Look::Selected => SELECTED_OTHER_UI,
            Look::Hovered => HOVER_UI,
            Look::Normal => UNSELECTED_UI,
        }
    }
}

/// How a node of the line being edited stands, as Blender's vertices: the active one
/// white, the other selected orange, the rest dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeLook {
    Active,
    Selected,
    /// A road's first node, showing which way it runs.
    Start,
    Normal,
}

impl NodeLook {
    pub fn of(active: bool, selected: bool, start: bool) -> Self {
        if active {
            NodeLook::Active
        } else if selected {
            NodeLook::Selected
        } else if start {
            NodeLook::Start
        } else {
            NodeLook::Normal
        }
    }

    pub fn selected(self) -> bool {
        matches!(self, NodeLook::Active | NodeLook::Selected)
    }

    /// Its dot's colour and the ring round it that sets it off from the ground: dark
    /// round light dots, light round dark ones.
    pub fn colours(self) -> (Color, Color) {
        match self {
            NodeLook::Active => (ACTIVE_NODE, SELECTED_NODE),
            NodeLook::Selected => (SELECTED_NODE, RIM_DARK),
            NodeLook::Start => (START, RIM_DARK),
            NodeLook::Normal => (NODE, RIM_LIGHT),
        }
    }

    /// Its size, the selected ones larger.
    pub fn size(self) -> f32 {
        if self.selected() { 1.15 } else { 0.9 }
    }

    /// Its number over the view.
    pub fn text(self) -> Color32 {
        match self {
            NodeLook::Active => ACTIVE_NODE_UI,
            NodeLook::Selected => SELECTED_NODE_UI,
            NodeLook::Start | NodeLook::Normal => UNSELECTED_UI,
        }
    }
}
