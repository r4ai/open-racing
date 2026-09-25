//! The project being edited, its history and the selection. Every change goes through
//! `ops::Op`, as it does from `open-racing-trackctl`, and is saved right away, so that
//! `project.ron` on disk is always the truth: an agent editing the file and a person in
//! the editor work on the same thing. Changes made to the file from outside are loaded
//! as they happen.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use bevy::prelude::*;
use open_racing_track_project::ops::{self, Op};
use open_racing_track_project::{Node, PROJECT_FILE, Project};

/// Undo steps kept.
const HISTORY: usize = 200;
/// Edits with the same key this soon after each other undo as one (dragging a value).
const COALESCE: Duration = Duration::from_millis(800);

/// Something in the project that is selected and edited as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Road(usize),
    Spline(usize),
    Prop(usize),
}

/// The selected item and, as in Blender's edit mode, its selected nodes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub item: Option<Item>,
    /// Selected nodes of the item's line, the active one last.
    pub nodes: Vec<usize>,
}

impl Selection {
    pub fn road(&self) -> Option<usize> {
        match self.item {
            Some(Item::Road(r)) => Some(r),
            _ => None,
        }
    }

    pub fn spline(&self) -> Option<usize> {
        match self.item {
            Some(Item::Spline(s)) => Some(s),
            _ => None,
        }
    }

    pub fn prop(&self) -> Option<usize> {
        match self.item {
            Some(Item::Prop(p)) => Some(p),
            _ => None,
        }
    }

    /// The active node.
    pub fn node(&self) -> Option<usize> {
        self.nodes.last().copied()
    }

    /// Selects an item with none of its nodes.
    pub fn select(&mut self, item: Item) {
        self.item = Some(item);
        self.nodes.clear();
    }

    /// Selects one node of an item.
    pub fn select_node(&mut self, item: Item, node: usize) {
        self.item = Some(item);
        self.nodes = vec![node];
    }

    /// Adds a node to the selection, or takes it out if it is the active one already
    /// (Shift+click in Blender).
    pub fn toggle_node(&mut self, item: Item, node: usize) {
        if self.item != Some(item) {
            return self.select_node(item, node);
        }
        if self.node() == Some(node) {
            self.nodes.pop();
        } else {
            self.nodes.retain(|&n| n != node);
            self.nodes.push(node);
        }
    }
}

#[derive(Resource)]
pub struct Editor {
    pub dir: PathBuf,
    pub project: Project,
    undo: Vec<Project>,
    redo: Vec<Project>,
    /// Bumped on every change; the preview rebuilds when it moves.
    pub revision: u64,
    /// Modification time of `project.ron` when last written or read here.
    stamp: Option<SystemTime>,
    last_check: Instant,
    /// Key and time of the last edit, for merging undo steps.
    last_edit: Option<(String, Instant)>,
    pub selection: Selection,
    /// One line about what just happened.
    pub status: String,
    pub dragging: bool,
}

fn stamp(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir.join(PROJECT_FILE))
        .and_then(|m| m.modified())
        .ok()
}

impl Editor {
    /// Opens the project in `dir`, creating one if there is none.
    pub fn open(dir: PathBuf) -> Result<Self, String> {
        let project = if dir.join(PROJECT_FILE).exists() {
            Project::load(&dir).map_err(|e| e.to_string())?
        } else {
            let name = dir
                .file_name()
                .map_or("track".into(), |n| n.to_string_lossy().into_owned());
            let p = Project::new(&name);
            p.save(&dir).map_err(|e| e.to_string())?;
            p
        };
        let status = format!("opened {}", dir.display());
        Ok(Self {
            stamp: stamp(&dir),
            dir,
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 1,
            last_check: Instant::now(),
            last_edit: None,
            selection: Selection {
                item: Some(Item::Road(0)),
                nodes: Vec::new(),
            },
            status,
            dragging: false,
        })
    }

    /// Replaces the whole state with another project's.
    pub fn switch(&mut self, dir: PathBuf) {
        match Self::open(dir) {
            Ok(e) => {
                let revision = self.revision + 1;
                *self = e;
                self.revision = revision;
            }
            Err(e) => self.status = e,
        }
    }

    fn push_undo(&mut self) {
        self.undo.push(self.project.clone());
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Applies operations as one undo step, saves, and reports failures in the status.
    /// Edits with the same `key` in quick succession merge into one step.
    pub fn apply(&mut self, ops: Vec<Op>, key: Option<&str>) -> bool {
        let now = Instant::now();
        let merge = key.is_some_and(|k| {
            self.last_edit
                .as_ref()
                .is_some_and(|(last, t)| last == k && now - *t < COALESCE)
        });
        let before = self.project.clone();
        match ops::apply_all(&mut self.project, &ops) {
            Ok(()) => {
                if !merge && !self.dragging {
                    self.undo.push(before);
                    if self.undo.len() > HISTORY {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
                self.last_edit = key.map(|k| (k.to_string(), now));
                self.revision += 1;
                if !self.dragging {
                    self.save();
                }
                self.clamp_selection();
                true
            }
            Err(e) => {
                self.status = e.to_string();
                false
            }
        }
    }

    /// Starts a drag: the edits until `end_drag` undo as one step and are saved at the
    /// end.
    pub fn begin_drag(&mut self) {
        if !self.dragging {
            self.push_undo();
            self.dragging = true;
        }
    }

    pub fn end_drag(&mut self) {
        if self.dragging {
            self.dragging = false;
            self.save();
        }
    }

    /// Ends a drag by putting everything back as it was when it began.
    pub fn cancel_drag(&mut self) {
        if self.dragging {
            self.dragging = false;
            if let Some(p) = self.undo.pop() {
                self.project = p;
                self.revision += 1;
                self.clamp_selection();
            }
        }
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project, p));
            self.changed("undone");
        }
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project, p));
            self.changed("redone");
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn changed(&mut self, what: &str) {
        self.revision += 1;
        self.save();
        self.clamp_selection();
        self.status = what.into();
    }

    fn clamp_selection(&mut self) {
        if let Some(Item::Prop(p)) = self.selection.item {
            if p >= self.project.props.len() {
                self.selection = Selection::default();
            }
            return;
        }
        let count = self.line().map(|(_, nodes, _)| nodes.len());
        let s = &mut self.selection;
        match count {
            Some(n) => s.nodes.retain(|&i| i < n),
            None => *s = Selection::default(),
        }
    }

    pub fn save(&mut self) {
        match self.project.save(&self.dir) {
            Ok(()) => self.stamp = stamp(&self.dir),
            Err(e) => self.status = format!("not saved: {e}"),
        }
    }

    /// Loads `project.ron` again if something else changed it.
    pub fn watch(&mut self) {
        if self.dragging || self.last_check.elapsed() < Duration::from_millis(400) {
            return;
        }
        self.last_check = Instant::now();
        let now = stamp(&self.dir);
        if now.is_none() || now == self.stamp {
            return;
        }
        self.stamp = now;
        match Project::load(&self.dir) {
            Ok(p) if p != self.project => {
                self.push_undo();
                self.project = p;
                self.changed("reloaded: project.ron changed on disk");
            }
            Ok(_) => {}
            Err(e) => self.status = format!("project.ron changed on disk but is invalid: {e}"),
        }
    }

    /// Name of the selected road.
    pub fn road_name(&self) -> Option<String> {
        self.selection
            .road()
            .and_then(|r| self.project.roads.get(r))
            .map(|r| r.name.clone())
    }

    /// The selected road or spline: its name, nodes and whether it is closed.
    pub fn line(&self) -> Option<(&str, &[Node], bool)> {
        item_line(&self.project, self.selection.item?)
    }
}

/// A road's or spline's name, nodes and whether it is closed.
pub fn item_line(project: &Project, item: Item) -> Option<(&str, &[Node], bool)> {
    match item {
        Item::Road(r) => project
            .roads
            .get(r)
            .map(|r| (r.name.as_str(), r.nodes.as_slice(), r.closed)),
        Item::Spline(s) => project
            .splines
            .get(s)
            .map(|s| (s.name.as_str(), s.nodes.as_slice(), s.closed)),
        Item::Prop(_) => None,
    }
}

pub fn watch_file(mut editor: ResMut<Editor>) {
    editor.watch();
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use open_racing_track_project::NodeHandles;
    use open_racing_track_project::project::HandleMode;

    #[test]
    fn handle_drag_is_one_undo_step_and_redoes() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "open-racing-editor-handles-{}-{stamp}",
            std::process::id()
        ));
        let mut editor = Editor::open(dir.clone()).unwrap();
        let before = editor.project.roads[0].nodes[0];
        editor.begin_drag();
        for length in [3.0, 4.0] {
            assert!(editor.apply(
                vec![Op::SetNodeHandles {
                    line: "circuit".into(),
                    index: 0,
                    mode: HandleMode::Free,
                    incoming: DVec3::new(-2.0, 1.0, 0.0),
                    outgoing: DVec3::X * length,
                }],
                None
            ));
        }
        editor.end_drag();
        assert_eq!(
            editor.project.roads[0].nodes[0].handles,
            NodeHandles::Free {
                incoming: DVec3::new(-2.0, 1.0, 0.0),
                outgoing: DVec3::X * 4.0
            }
        );
        editor.undo();
        assert_eq!(editor.project.roads[0].nodes[0], before);
        editor.redo();
        assert_eq!(
            editor.project.roads[0].nodes[0].handles,
            NodeHandles::Free {
                incoming: DVec3::new(-2.0, 1.0, 0.0),
                outgoing: DVec3::X * 4.0
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
