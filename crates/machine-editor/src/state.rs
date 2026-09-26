//! What the editor edits: a library, changed only through operations (each saved at
//! once and undoable), and what is selected.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use open_racing_machine_project::Library;
use open_racing_machine_project::ops::{self, Op};

/// Undo steps kept.
const HISTORY: usize = 200;
/// Edits with the same key within this time make one undo step (a drag).
const COALESCE: Duration = Duration::from_millis(800);
/// How often the files are checked for changes made elsewhere.
const WATCH: Duration = Duration::from_millis(500);

/// The editor's workspaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Assembly,
    Engine,
    Suspension,
    Tyres,
    Aero,
    Interior,
}

impl Workspace {
    pub const ALL: [Workspace; 6] = [
        Workspace::Assembly,
        Workspace::Engine,
        Workspace::Suspension,
        Workspace::Tyres,
        Workspace::Aero,
        Workspace::Interior,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Workspace::Assembly => "Assembly",
            Workspace::Engine => "Engine",
            Workspace::Suspension => "Suspension",
            Workspace::Tyres => "Tyres",
            Workspace::Aero => "Aero",
            Workspace::Interior => "Interior",
        }
    }

    pub fn named(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|w| w.label().eq_ignore_ascii_case(s))
    }
}

/// What is selected.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    /// The machine the Assembly workspace shows.
    pub machine: Option<String>,
    /// A placed part of it, by instance name.
    pub placed: Option<String>,
    /// A part of the library (the one placed, or one picked in the library).
    pub part: Option<String>,
    /// The engine the Engine workspace develops: an engine part (on its bench) or
    /// `machine/<name>`.
    pub engine: Option<String>,
    /// An element (pipe or volume) of an intake or exhaust network: (part, name).
    pub element: Option<(String, String)>,
}

struct Step {
    label: String,
    before: Library,
    key: Option<String>,
    at: Instant,
}

#[derive(Resource)]
pub struct Editor {
    pub lib: Library,
    pub selection: Selection,
    pub workspace: Workspace,
    /// Bumped by every change: views rebuild when it moves.
    pub revision: u64,
    pub status: String,
    undo: Vec<Step>,
    redo: Vec<Step>,
    last_watch: Instant,
    /// Writes to disk (false in tests).
    pub save: bool,
}

impl Editor {
    pub fn new(lib: Library) -> Self {
        let mut e = Self {
            lib,
            selection: Selection::default(),
            workspace: Workspace::Assembly,
            revision: 1,
            status: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_watch: Instant::now(),
            save: true,
        };
        e.default_selection();
        e
    }

    /// Opens a library directory.
    pub fn open(dir: std::path::PathBuf) -> Result<Self, String> {
        let lib = Library::open(&dir)?;
        let mut e = Self::new(lib);
        e.lib.dir = dir;
        Ok(e)
    }

    /// Picks the first machine and engine when nothing is.
    pub fn default_selection(&mut self) {
        let s = &mut self.selection;
        if s.machine
            .as_ref()
            .is_none_or(|m| !self.lib.machines.contains_key(m))
        {
            s.machine = self.lib.machines.keys().next().cloned();
            s.placed = None;
        }
        if s.engine.is_none() {
            s.engine = s
                .machine
                .as_ref()
                .map(|m| format!("machine/{m}"))
                .or_else(|| {
                    self.lib
                        .parts
                        .keys()
                        .find(|r| r.kind == open_racing_machine_project::Kind::Engine)
                        .map(|r| r.to_string())
                });
        }
    }

    /// Applies operations as one undo step (merged with the previous one when `key` is the
    /// same and it was moments ago), saves, and reports failure in the status line.
    pub fn apply(&mut self, list: Vec<Op>, key: Option<&str>) -> bool {
        if list.is_empty() {
            return true;
        }
        let before = self.lib.clone();
        if let Err(e) = ops::apply_all(&mut self.lib, &list) {
            self.status = e;
            return false;
        }
        let label = describe(&list);
        let merge = key.is_some()
            && self
                .undo
                .last()
                .is_some_and(|s| s.key.as_deref() == key && s.at.elapsed() < COALESCE);
        if merge {
            let s = self.undo.last_mut().expect("checked");
            s.at = Instant::now();
        } else {
            self.undo.push(Step {
                label: label.clone(),
                before,
                key: key.map(String::from),
                at: Instant::now(),
            });
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.status = label;
        self.changed();
        true
    }

    fn changed(&mut self) {
        self.revision += 1;
        if self.save
            && let Err(e) = self.lib.save()
        {
            self.status = format!("not saved: {e}");
        }
        self.default_selection();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            let now = std::mem::replace(&mut self.lib, s.before.clone());
            self.keep_files(&now);
            self.status = format!("undone: {}", s.label);
            self.redo.push(Step { before: now, ..s });
            self.changed();
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            let now = std::mem::replace(&mut self.lib, s.before.clone());
            self.keep_files(&now);
            self.status = format!("redone: {}", s.label);
            self.undo.push(Step { before: now, ..s });
            self.changed();
        }
    }

    /// Carries what is on disk over to a library taken from the history.
    fn keep_files(&mut self, current: &Library) {
        let parts = std::mem::take(&mut self.lib.parts);
        let machines = std::mem::take(&mut self.lib.machines);
        let mut l = current.clone();
        l.parts = parts;
        l.machines = machines;
        self.lib = l;
    }

    /// Reloads the library when its files changed on disk (another editor, or an agent
    /// with `machinectl`), as an undoable step.
    pub fn watch(&mut self) {
        if !self.save || self.last_watch.elapsed() < WATCH {
            return;
        }
        self.last_watch = Instant::now();
        if !self.lib.changed_on_disk() {
            return;
        }
        match Library::open(&self.lib.dir) {
            Ok(fresh) => {
                let before = std::mem::replace(&mut self.lib, fresh);
                self.undo.push(Step {
                    label: "Reload from disk".into(),
                    before,
                    key: None,
                    at: Instant::now(),
                });
                self.redo.clear();
                self.status = "reloaded: the files changed on disk".into();
                self.revision += 1;
                self.default_selection();
            }
            Err(e) => self.status = format!("the files changed on disk and do not load: {e}"),
        }
    }
}

/// A label for an undo step.
fn describe(list: &[Op]) -> String {
    match list {
        [Op::Set { target, path, .. }] => format!("{target}: {path}"),
        [op] => format!("{} {}", op.kind(), op.targets().join(", ")),
        _ => format!("{} changes", list.len()),
    }
}

pub fn watch_files(mut editor: ResMut<Editor>) {
    editor.watch();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        let mut e = Editor::new(open_racing_machine_project::samples::library("test"));
        e.save = false;
        e
    }

    #[test]
    fn edits_undo_and_redo() {
        let mut e = editor();
        let set = |v: f64| Op::Set {
            target: "engine/i4_2l_na".into(),
            path: "design.Engine.spec.compression_ratio".into(),
            value: serde_json::json!(v),
        };
        let cr = |e: &Editor| {
            let open_racing_machine_project::part::Design::Engine(en) =
                &e.lib.part("engine/i4_2l_na").unwrap().design
            else {
                panic!()
            };
            en.spec.compression_ratio
        };
        assert!(e.apply(vec![set(12.0)], Some("cr")));
        assert!(e.apply(vec![set(12.5)], Some("cr")));
        assert_eq!(cr(&e), 12.5);
        e.undo();
        assert_eq!(cr(&e), 11.0, "a drag is one step");
        e.redo();
        assert_eq!(cr(&e), 12.5);
        assert!(
            !e.apply(vec![set(0.5)], None),
            "a compression ratio below one is refused"
        );
        assert_eq!(cr(&e), 12.5);
    }
}
