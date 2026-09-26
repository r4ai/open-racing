//! The project being edited, its history and the selection. Every change goes through
//! `ops::Op`, as it does from `open-racing-trackctl`, and is saved right away, so that
//! `project.ron` on disk is always the truth: an agent editing the file and a person in
//! the editor work on the same thing. Changes made to the file from outside are loaded
//! as they happen.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use bevy::prelude::*;
use open_racing_track_project::ops::{self, Op};
use open_racing_track_project::project::Road;
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

/// The selected item and, as in Blender's edit mode, its selected nodes; in object mode,
/// other items selected with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// The active item: the properties show it.
    pub item: Option<Item>,
    /// Selected nodes of the item's line, the active one last.
    pub nodes: Vec<usize>,
    /// Other items selected with the active one (Shift + click, a box), moved,
    /// turned, scaled and deleted with it.
    pub others: Vec<Item>,
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
        self.others.clear();
    }

    /// Selects one node of an item.
    pub fn select_node(&mut self, item: Item, node: usize) {
        self.item = Some(item);
        self.nodes = vec![node];
        self.others.clear();
    }

    /// Adds an item to the selection and makes it the active one, or takes it out if
    /// it is the active one already (Shift + click in Blender's object mode).
    pub fn toggle_item(&mut self, item: Item) {
        self.nodes.clear();
        if self.item == Some(item) {
            self.item = self.others.pop();
        } else {
            self.others.retain(|&o| o != item);
            if let Some(active) = self.item.replace(item) {
                self.others.push(active);
            }
        }
    }

    /// Adds an item to the selection: the active one if nothing is selected yet.
    pub fn add(&mut self, item: Item) {
        if self.item.is_none() {
            self.item = Some(item);
            self.nodes.clear();
            self.others.retain(|&o| o != item);
        } else if !self.has(item) {
            self.others.push(item);
        }
    }

    /// Takes an item out of the selection; the next selected becomes the active one.
    pub fn remove(&mut self, item: Item) {
        self.others.retain(|&o| o != item);
        if self.item == Some(item) {
            self.item = self.others.pop();
            self.nodes.clear();
        }
    }

    /// Selects these items and nothing else, the first one active.
    pub fn set_items(&mut self, items: impl IntoIterator<Item = Item>) {
        *self = Self::default();
        for item in items {
            self.add(item);
        }
    }

    /// Whether an item is selected, active or not.
    pub fn has(&self, item: Item) -> bool {
        self.item == Some(item) || self.others.contains(&item)
    }

    /// Every selected item, the active one first.
    pub fn items(&self) -> Vec<Item> {
        self.item
            .into_iter()
            .chain(self.others.iter().copied())
            .collect()
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

/// An item by its kind and name, which stay the same as the lists change.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Named {
    Road(String),
    Spline(String),
    Prop(String),
}

/// What the view shows, as Blender's hiding (H), the outliner's locks and local view
/// (numpad /). Not part of the track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub hidden: HashSet<Named>,
    /// Shown but not picked in the view.
    pub locked: HashSet<Named>,
    /// Local view: only these are shown.
    pub local: Option<HashSet<Named>>,
    /// Collections hidden, and locked.
    pub hidden_groups: HashSet<String>,
    pub locked_groups: HashSet<String>,
    /// Scatters hidden, by name.
    pub hidden_scatter: HashSet<String>,
}

/// A step of the undo history: the project to go back (or on) to, and what the step
/// did, for the history's list.
struct Step {
    project: Project,
    what: String,
}

/// What a list of operations does, in a few words.
fn describe(ops: &[Op]) -> String {
    let Some(first) = ops.first().map(|o| o.kind()) else {
        return "Edit".into();
    };
    match ops.len() {
        1 => first.to_string(),
        n if ops.iter().all(|o| o.kind() == first) => format!("{first} ×{n}"),
        n => format!("{first} and {} more", n - 1),
    }
}

#[derive(Resource)]
pub struct Editor {
    pub dir: PathBuf,
    pub project: Project,
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    /// Bumped on every change; the preview rebuilds when it moves.
    pub revision: u64,
    /// Modification time of `project.ron` when last written or read here.
    stamp: Option<SystemTime>,
    last_check: Instant,
    /// When `project.ron` was last copied into the backups.
    last_backup: Option<Instant>,
    /// Key and time of the last edit, for merging undo steps.
    last_edit: Option<(String, Instant)>,
    pub selection: Selection,
    /// One line about what just happened.
    pub status: String,
    /// A drag in progress: the project as it was when it began, and what it has done so
    /// far. It becomes an undo step when it ends, if it changed anything.
    drag: Option<Step>,
    pub shown: Shown,
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
        let item = (!project.roads.is_empty()).then_some(Item::Road(0));
        Ok(Self {
            stamp: stamp(&dir),
            dir,
            project,
            undo: VecDeque::new(),
            redo: Vec::new(),
            revision: 1,
            last_check: Instant::now(),
            last_backup: None,
            last_edit: None,
            selection: Selection {
                item,
                ..Default::default()
            },
            status,
            drag: None,
            shown: Shown::default(),
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

    fn push_undo(&mut self, before: Project, what: &str) {
        self.undo.push_back(Step {
            project: before,
            what: what.to_string(),
        });
        if self.undo.len() > HISTORY {
            self.undo.pop_front();
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
        if let Err(e) = ops::apply_all(&mut self.project, &ops) {
            self.status = e.to_string();
            return false;
        }
        match &mut self.drag {
            // A drag is named after what it does.
            Some(drag) => drag.what = describe(&ops),
            None if !merge => self.push_undo(before, &describe(&ops)),
            None => {}
        }
        self.last_edit = key.map(|k| (k.to_string(), now));
        self.edited();
        true
    }

    /// Applies operations as part of the last undo step: what follows from an edit
    /// just made, such as the same change made to other selected items.
    pub fn apply_along(&mut self, ops: Vec<Op>) -> bool {
        if let Err(e) = ops::apply_all(&mut self.project, &ops) {
            self.status = e.to_string();
            return false;
        }
        self.edited();
        true
    }

    /// After an edit: the preview rebuilds, the file is written (at the end of a drag)
    /// and the selection keeps to what exists.
    fn edited(&mut self) {
        self.revision += 1;
        if !self.dragging() {
            self.save();
        }
        self.clamp_selection();
    }

    /// Whether a drag is in progress.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Starts a drag: the edits until `end_drag` undo as one step and are saved at the
    /// end.
    pub fn begin_drag(&mut self) {
        if self.drag.is_none() {
            self.drag = Some(Step {
                project: self.project.clone(),
                what: "Drag".into(),
            });
            self.last_edit = None;
        }
    }

    /// Ends a drag: what it changed is one undo step, and saved. A drag that changed
    /// nothing (a click on a node) leaves the history as it was.
    pub fn end_drag(&mut self) {
        if let Some(step) = self.drag.take()
            && step.project != self.project
        {
            self.push_undo(step.project, &step.what);
            self.save();
        }
    }

    /// Ends a drag by putting everything back as it was when it began.
    pub fn cancel_drag(&mut self) {
        if let Some(step) = self.drag.take()
            && step.project != self.project
        {
            self.project = step.project;
            self.revision += 1;
            self.clamp_selection();
        }
    }

    /// Steps back; not while dragging, whose start is the step it would undo.
    pub fn undo(&mut self) {
        if !self.can_undo() {
            return;
        }
        if let Some(step) = self.undo.pop_back() {
            let now = std::mem::replace(&mut self.project, step.project);
            self.status = format!("undone: {}", step.what);
            self.redo.push(Step {
                project: now,
                what: step.what,
            });
            let status = std::mem::take(&mut self.status);
            self.changed(&status);
        }
    }

    pub fn redo(&mut self) {
        if !self.can_redo() {
            return;
        }
        if let Some(step) = self.redo.pop() {
            let now = std::mem::replace(&mut self.project, step.project);
            let status = format!("redone: {}", step.what);
            self.undo.push_back(Step {
                project: now,
                what: step.what,
            });
            self.changed(&status);
        }
    }

    /// What each step that undoes did, the oldest first, and each that redoes, the
    /// next first.
    pub fn history(&self) -> (Vec<&str>, Vec<&str>) {
        (
            self.undo.iter().map(|s| s.what.as_str()).collect(),
            self.redo.iter().rev().map(|s| s.what.as_str()).collect(),
        )
    }

    /// Undoes or redoes until `steps` steps are left to undo.
    pub fn go_to(&mut self, steps: usize) {
        while self.undo.len() > steps && self.can_undo() {
            self.undo();
        }
        while self.undo.len() < steps && self.can_redo() {
            self.redo();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.dragging() && !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.dragging() && !self.redo.is_empty()
    }

    fn changed(&mut self, what: &str) {
        // The next edit is a step of its own, whatever its key.
        self.last_edit = None;
        self.revision += 1;
        self.save();
        self.clamp_selection();
        self.status = what.into();
    }

    /// Drops from the selection what no longer exists: after an undo, a reload or a
    /// deletion the lists may be shorter.
    fn clamp_selection(&mut self) {
        let p = &self.project;
        let exists = |item: Item| match item {
            Item::Road(r) => r < p.roads.len(),
            Item::Spline(s) => s < p.splines.len(),
            Item::Prop(i) => i < p.props.len(),
        };
        let s = &mut self.selection;
        s.others.retain(|&o| exists(o) && Some(o) != s.item);
        s.others.dedup();
        match s.item {
            Some(item) if exists(item) => {}
            Some(_) => {
                // The active one went: the next selected takes its place.
                s.item = s.others.pop();
                s.nodes.clear();
            }
            None => s.nodes.clear(),
        }
        let count = self.line().map_or(0, |(_, nodes, _)| nodes.len());
        self.selection.nodes.retain(|&i| i < count);
    }

    pub fn save(&mut self) {
        self.backup();
        match self.project.save(&self.dir) {
            Ok(()) => self.stamp = stamp(&self.dir),
            Err(e) => self.status = format!("not saved: {e}"),
        }
    }

    /// Loads `project.ron` again if something else changed it.
    pub fn watch(&mut self) {
        if self.dragging() || self.last_check.elapsed() < Duration::from_millis(400) {
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
                let before = std::mem::replace(&mut self.project, p);
                self.push_undo(before, "Reload from disk");
                self.changed("reloaded: project.ron changed on disk");
            }
            Ok(_) => {}
            Err(e) => self.status = format!("project.ron changed on disk but is invalid: {e}"),
        }
    }

    /// The selected road: its place in the list, and the road.
    pub fn road(&self) -> Option<(usize, &Road)> {
        let r = self.selection.road()?;
        Some((r, self.project.roads.get(r)?))
    }

    /// Name of the selected road.
    pub fn road_name(&self) -> Option<String> {
        self.road().map(|(_, r)| r.name.clone())
    }

    /// The selected road or spline: its name, nodes and whether it is closed.
    pub fn line(&self) -> Option<(&str, &[Node], bool)> {
        item_line(&self.project, self.selection.item?)
    }

    /// The nodes an edit of the selected line works on: the selected ones, or all of them
    /// with none selected.
    pub fn picked_nodes(&self) -> Vec<usize> {
        let count = self.line().map_or(0, |(_, nodes, _)| nodes.len());
        if self.selection.nodes.is_empty() {
            (0..count).collect()
        } else {
            let sel = self.selection.nodes.iter().copied();
            sel.filter(|&n| n < count).collect()
        }
    }
}

impl Editor {
    /// An item's kind and name.
    pub fn named(&self, item: Item) -> Option<Named> {
        let p = &self.project;
        Some(match item {
            Item::Road(r) => Named::Road(p.roads.get(r)?.name.clone()),
            Item::Spline(s) => Named::Spline(p.splines.get(s)?.name.clone()),
            Item::Prop(i) => Named::Prop(p.props.get(i)?.name.clone()),
        })
    }

    /// The collection an item is kept in.
    pub fn group(&self, item: Item) -> Option<&str> {
        let p = &self.project;
        match item {
            Item::Road(_) => None,
            Item::Spline(s) => p.splines.get(s)?.group.as_deref(),
            Item::Prop(i) => p.props.get(i)?.group.as_deref(),
        }
    }

    /// Whether the view shows an item: not hidden, nor its collection, and in local
    /// view if there is one.
    pub fn visible(&self, item: Item) -> bool {
        let Some(n) = self.named(item) else {
            return false;
        };
        !self.shown.hidden.contains(&n)
            && self
                .group(item)
                .is_none_or(|g| !self.shown.hidden_groups.contains(g))
            && self.shown.local.as_ref().is_none_or(|l| l.contains(&n))
    }

    /// Whether a click or a box in the view can select an item.
    pub fn pickable(&self, item: Item) -> bool {
        self.visible(item)
            && self
                .named(item)
                .is_some_and(|n| !self.shown.locked.contains(&n))
            && self
                .group(item)
                .is_none_or(|g| !self.shown.locked_groups.contains(g))
    }

    /// The collections splines and props are kept in, in order of name.
    pub fn groups(&self) -> Vec<String> {
        let p = &self.project;
        let mut g: Vec<String> = p
            .splines
            .iter()
            .filter_map(|s| s.group.clone())
            .chain(p.props.iter().filter_map(|x| x.group.clone()))
            .collect();
        g.sort();
        g.dedup();
        g
    }

    /// Puts the selected splines and props in collection `group` (none: out of any), as
    /// one step.
    pub fn set_group(&mut self, group: Option<String>) {
        let p = &self.project;
        let mut ops = Vec::new();
        for item in self.selection.items() {
            match item {
                Item::Spline(s) => {
                    if let Some(sp) = p.splines.get(s).filter(|sp| sp.group != group) {
                        let mut sp = sp.clone();
                        sp.group = group.clone();
                        ops.push(Op::PutSpline { spline: sp });
                    }
                }
                Item::Prop(i) => {
                    if let Some(x) = p.props.get(i).filter(|x| x.group != group) {
                        let mut x = x.clone();
                        x.group = group.clone();
                        ops.push(Op::PutProp { prop: x });
                    }
                }
                Item::Road(_) => {}
            }
        }
        let n = ops.len();
        if n > 0 && self.apply(ops, None) {
            self.status = match &group {
                Some(g) => format!("{n} put in \"{g}\""),
                None => format!("{n} taken out of their collections"),
            };
        }
    }

    pub fn is_hidden(&self, item: Item) -> bool {
        self.named(item)
            .is_some_and(|n| self.shown.hidden.contains(&n))
    }

    pub fn is_locked(&self, item: Item) -> bool {
        self.named(item)
            .is_some_and(|n| self.shown.locked.contains(&n))
    }

    /// Every road, spline and prop.
    pub fn all_items(&self) -> Vec<Item> {
        let p = &self.project;
        (0..p.roads.len())
            .map(Item::Road)
            .chain((0..p.splines.len()).map(Item::Spline))
            .chain((0..p.props.len()).map(Item::Prop))
            .collect()
    }

    /// H: hides what is selected.
    pub fn hide_selected(&mut self) {
        let sel = self.selection.items();
        self.hide(&sel);
    }

    /// Shift H: hides everything but what is selected.
    pub fn hide_unselected(&mut self) {
        let sel = self.selection.items();
        let others: Vec<Item> = self
            .all_items()
            .into_iter()
            .filter(|i| !sel.contains(i))
            .collect();
        self.hide(&others);
    }

    /// Hides items, dropping them from the selection.
    pub fn hide(&mut self, items: &[Item]) {
        let names: Vec<Named> = items.iter().filter_map(|&i| self.named(i)).collect();
        let n = names.len();
        self.shown.hidden.extend(names);
        for &i in items {
            self.selection.remove(i);
        }
        self.status = format!("{n} hidden (Alt H shows them again)");
    }

    /// Hides or shows one item, as the outliner's eye.
    pub fn toggle_hidden(&mut self, item: Item) {
        if let Some(n) = self.named(item)
            && !self.shown.hidden.remove(&n)
        {
            self.hide(&[item]);
        }
    }

    pub fn toggle_locked(&mut self, item: Item) {
        if let Some(n) = self.named(item)
            && !self.shown.locked.remove(&n)
        {
            self.shown.locked.insert(n);
        }
    }

    /// Shows everything hidden again, selecting it, as Blender's Alt H.
    pub fn reveal(&mut self) {
        let hidden = std::mem::take(&mut self.shown.hidden);
        let items: Vec<Item> = self
            .all_items()
            .into_iter()
            .filter(|&i| self.named(i).is_some_and(|n| hidden.contains(&n)))
            .collect();
        if !items.is_empty() {
            self.selection.nodes.clear();
            for &i in &items {
                self.selection.add(i);
            }
        }
        self.status = format!("{} shown again", items.len());
    }

    /// Local view: only the selected items, or back to everything.
    pub fn toggle_local(&mut self) -> bool {
        if self.shown.local.take().is_some() {
            return false;
        }
        let items = self.selection.items();
        if items.is_empty() {
            self.status = "select what to look at on its own (numpad /)".into();
            return false;
        }
        self.shown.local = Some(items.iter().filter_map(|&i| self.named(i)).collect());
        true
    }
}

/// Folder in a project's directory the backups are kept in.
pub const BACKUPS: &str = ".backups";
/// How often `project.ron` is backed up while it changes, and how many copies are kept.
const BACKUP_EVERY: Duration = Duration::from_secs(5 * 60);
const BACKUPS_KEPT: usize = 40;

impl Editor {
    /// Copies `project.ron` as it is into the backups, at most every few minutes, before
    /// it is written again; the oldest copies go.
    fn backup(&mut self) {
        if self.last_backup.is_some_and(|t| t.elapsed() < BACKUP_EVERY) {
            return;
        }
        self.last_backup = Some(Instant::now());
        let from = self.dir.join(PROJECT_FILE);
        if !from.is_file() {
            return;
        }
        let dir = self.dir.join(BACKUPS);
        let secs = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let copied = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::copy(&from, dir.join(format!("project-{secs}.ron"))));
        if let Err(e) = copied {
            self.status = format!("backup failed: {e}");
            return;
        }
        let list = backups(&self.dir);
        for (path, _) in list.iter().skip(BACKUPS_KEPT) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Replaces the project with a backup, as a step that undoes.
    pub fn restore(&mut self, path: &Path) {
        let project = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|src| ron::from_str::<Project>(&src).map_err(|e| e.to_string()))
            .and_then(|p| p.validate().map(|()| p).map_err(|e| e.to_string()));
        match project {
            Ok(p) => {
                let before = std::mem::replace(&mut self.project, p);
                self.push_undo(before, "Restore backup");
                self.changed("restored a backup (Ctrl Z undoes it)");
            }
            Err(e) => self.status = format!("{}: {e}", path.display()),
        }
    }
}

/// The backups of the project in `dir`, newest first, with when each was made.
pub fn backups(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut list: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(dir.join(BACKUPS))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "ron"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .collect();
    list.sort_by_key(|b| std::cmp::Reverse(b.1));
    list
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

    #[test]
    fn hiding_locking_and_local_view_follow_names() {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-shown-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        assert!(editor.apply(
            vec![Op::AddRoad {
                name: "pit".into(),
                closed: false,
                nodes: vec![DVec3::ZERO, DVec3::X * 50.0],
                like: None,
            }],
            None
        ));
        editor.selection.select(Item::Road(1));
        editor.hide(&[Item::Road(1)]);
        assert!(!editor.visible(Item::Road(1)) && editor.visible(Item::Road(0)));
        assert_eq!(editor.selection.item, None, "hidden, so not selected");
        // Renaming another road does not change what is hidden.
        assert!(editor.apply(
            vec![Op::RenameRoad {
                road: "circuit".into(),
                to: "gp".into()
            }],
            None
        ));
        assert!(!editor.visible(Item::Road(1)));
        editor.reveal();
        assert!(editor.visible(Item::Road(1)));
        assert_eq!(
            editor.selection.item,
            Some(Item::Road(1)),
            "shown and selected"
        );
        editor.toggle_locked(Item::Road(0));
        assert!(editor.visible(Item::Road(0)) && !editor.pickable(Item::Road(0)));
        // Local view shows the selection alone.
        assert!(editor.toggle_local());
        assert!(editor.visible(Item::Road(1)) && !editor.visible(Item::Road(0)));
        assert!(!editor.toggle_local());
        assert!(editor.visible(Item::Road(0)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn collections_hide_lock_and_take_the_selection() {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-groups-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        for (name, y) in [("a", 0.0), ("b", 10.0), ("c", 20.0)] {
            let spline = crate::presets::named(&editor.project, "kerb")
                .unwrap()
                .spline(
                    &editor.project,
                    vec![DVec3::new(0.0, y, 0.0), DVec3::new(9.0, y, 0.0)],
                )
                .unwrap();
            let spline = open_racing_track_project::project::Spline {
                name: name.into(),
                ..spline
            };
            assert!(editor.apply(vec![Op::PutSpline { spline }], None));
        }
        editor.selection.select(Item::Spline(0));
        editor.selection.others = vec![Item::Spline(1), Item::Road(0)];
        editor.set_group(Some("T1 kerbs".into()));
        assert_eq!(editor.groups(), vec!["T1 kerbs".to_string()]);
        assert_eq!(editor.group(Item::Spline(1)), Some("T1 kerbs"));
        assert_eq!(editor.group(Item::Spline(2)), None);
        editor.shown.hidden_groups.insert("T1 kerbs".into());
        assert!(!editor.visible(Item::Spline(0)) && editor.visible(Item::Spline(2)));
        editor.shown.hidden_groups.clear();
        editor.shown.locked_groups.insert("T1 kerbs".into());
        assert!(editor.visible(Item::Spline(0)) && !editor.pickable(Item::Spline(0)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_history_names_steps_and_goes_back_and_on_to_any() {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        let rename = |to: &str| {
            vec![Op::SetName {
                name: to.to_string(),
            }]
        };
        assert!(editor.apply(rename("a"), None));
        let moves = (0..3)
            .map(|i| Op::MoveNode {
                line: "circuit".into(),
                index: i,
                pos: DVec3::new(i as f64, 1.0, 0.0),
            })
            .collect();
        assert!(editor.apply(moves, None));
        assert!(editor.apply(rename("b"), None));
        assert_eq!(
            editor.history().0,
            vec!["SetName", "MoveNode ×3", "SetName"]
        );
        editor.go_to(1);
        assert_eq!(editor.project.name, "a");
        assert_eq!(editor.history().1, vec!["MoveNode ×3", "SetName"]);
        editor.go_to(3);
        assert_eq!(editor.project.name, "b");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saving_keeps_a_backup_that_restores() {
        let dir =
            std::env::temp_dir().join(format!("open-racing-editor-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        let before = editor.project.clone();
        assert!(editor.apply(
            vec![Op::SetName {
                name: "renamed".into()
            }],
            None
        ));
        let list = backups(&dir);
        assert_eq!(list.len(), 1, "the first save backs up what was there");
        editor.restore(&list[0].0);
        assert_eq!(editor.project, before);
        editor.undo();
        assert_eq!(editor.project.name, "renamed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_drag_that_changes_nothing_leaves_the_history_alone() {
        let dir = std::env::temp_dir().join(format!(
            "open-racing-editor-still-drag-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        let rename = |to: &str| vec![Op::SetName { name: to.into() }];
        assert!(editor.apply(rename("a"), None));
        editor.undo();
        // A click on a node: a drag that moved nothing.
        editor.begin_drag();
        editor.end_drag();
        assert!(!editor.can_undo(), "no empty step");
        assert!(editor.can_redo(), "the undone edit still redoes");
        // A drag cancelled half way puts the project back and leaves no step either.
        editor.begin_drag();
        assert!(editor.apply(rename("b"), None));
        editor.cancel_drag();
        assert_ne!(editor.project.name, "b");
        assert!(editor.can_redo());
        // A drag that moved something is one step, named after what it did.
        editor.begin_drag();
        assert!(editor.apply(rename("c"), None));
        assert!(editor.apply(rename("d"), None));
        editor.end_drag();
        assert_eq!(editor.history().0, vec!["SetName"]);
        editor.undo();
        assert_ne!(editor.project.name, "d");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn undo_waits_for_a_drag_and_ends_merging() {
        let dir = std::env::temp_dir().join(format!(
            "open-racing-editor-undo-merge-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut editor = Editor::open(dir.clone()).unwrap();
        let crown = |c: f64| {
            vec![Op::SetRoad {
                road: "circuit".into(),
                closed: None,
                crown: Some(c),
                surface: None,
                material: None,
                resolution: None,
            }]
        };
        assert!(editor.apply(crown(0.1), Some("crown")));
        editor.begin_drag();
        assert!(editor.apply(crown(0.15), None));
        assert!(!editor.can_undo(), "no undo in the middle of a drag");
        editor.undo();
        assert!(editor.dragging());
        editor.end_drag();
        editor.undo();
        assert_eq!(editor.project.roads[0].crown, 0.1);

        // An edit with the same key right after an undo is a step of its own.
        assert!(editor.apply(crown(0.3), Some("crown")));
        editor.undo();
        assert!(editor.apply(crown(0.2), Some("crown")));
        assert!(!editor.can_redo(), "the undone edit is gone");
        editor.undo();
        assert_eq!(editor.project.roads[0].crown, 0.1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
