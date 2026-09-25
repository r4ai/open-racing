//! The project being edited, its history and the selection. Every change goes through
//! `ops::Op`, as it does from `open-racing-trackctl`, and is saved right away, so that
//! `project.ron` on disk is always the truth: an agent editing the file and a person in
//! the editor work on the same thing. Changes made to the file from outside are loaded
//! as they happen.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use bevy::prelude::*;
use open_racing_track_project::ops::{self, Op};
use open_racing_track_project::{PROJECT_FILE, Project};

/// Undo steps kept.
const HISTORY: usize = 200;
/// Edits with the same key this soon after each other undo as one (dragging a value).
const COALESCE: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub road: Option<usize>,
    pub node: Option<usize>,
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
                road: Some(0),
                node: None,
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
        let s = &mut self.selection;
        if s.road.is_some_and(|r| r >= self.project.roads.len()) {
            *s = Selection::default();
        }
        if let (Some(r), Some(n)) = (s.road, s.node)
            && n >= self.project.roads[r].nodes.len()
        {
            s.node = None;
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
            .road
            .and_then(|r| self.project.roads.get(r))
            .map(|r| r.name.clone())
    }
}

pub fn watch_file(mut editor: ResMut<Editor>) {
    editor.watch();
}
