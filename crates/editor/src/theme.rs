//! The editor's colours, one scheme everywhere as Blender's: what is selected is orange
//! (the active one brighter), unselected lines grey and unselected nodes dark, what
//! the pointer is over lighter; handles for widths and stretches have a colour of their
//! own so that they are never taken for the selection.

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
/// Stretches of barriers.
pub const BARRIER: Color = Color::srgb(0.7, 0.7, 0.95);
/// A road's first node, showing which way it runs.
pub const START: Color = Color::srgb(0.25, 0.9, 0.4);
pub const OFF_TRACK: Color = Color::srgb(1.0, 0.2, 0.2);
/// What a grabbed node or end has caught on.
pub const SNAP: Color = Color::srgb(1.0, 0.2, 0.85);

/// The same in the panels.
pub const SELECTED_UI: Color32 = Color32::from_rgb(255, 160, 40);
pub const SELECTED_OTHER_UI: Color32 = Color32::from_rgb(217, 107, 13);
pub const SELECTED_NODE_UI: Color32 = Color32::from_rgb(255, 115, 0);
pub const UNSELECTED_UI: Color32 = Color32::from_gray(215);
pub const STRIP_UI: Color32 = Color32::from_rgb(51, 217, 191);
