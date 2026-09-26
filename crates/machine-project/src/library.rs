//! A library of parts and machines, in memory, and its files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::machine::Machine;
use crate::part::{Kind, Part, PartRef};

/// The parts and machines of a library directory.
#[derive(Clone, Debug, Default)]
pub struct Library {
    pub dir: PathBuf,
    pub parts: BTreeMap<PartRef, Part>,
    pub machines: BTreeMap<String, Machine>,
    /// Text of each file as last read or written, to write only what changed, remove
    /// what was deleted, and tell others' changes from ours.
    files: BTreeMap<PathBuf, String>,
}

/// Something the library holds: a part or a machine.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    Part(PartRef),
    Machine(String),
}

impl Target {
    /// `kind/name` for a part, `machine/name` for a machine.
    pub fn parse(s: &str) -> Result<Self, String> {
        if let Some(n) = s.strip_prefix("machine/") {
            crate::check_name(n)?;
            Ok(Target::Machine(n.into()))
        } else {
            PartRef::parse(s).map(Target::Part)
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Part(p) => p.fmt(f),
            Target::Machine(m) => write!(f, "machine/{m}"),
        }
    }
}

fn pretty() -> ron::ser::PrettyConfig {
    ron::ser::PrettyConfig::default()
        .depth_limit(12)
        .struct_names(false)
        .new_line("\n".to_string())
        .indentor("    ".to_string())
}

/// A part as the text of its file.
pub fn part_text(p: &Part) -> String {
    ron::ser::to_string_pretty(p, pretty()).expect("parts serialise") + "\n"
}

pub fn machine_text(m: &Machine) -> String {
    ron::ser::to_string_pretty(m, pretty()).expect("machines serialise") + "\n"
}

pub fn parse_part(src: &str) -> Result<Part, String> {
    ron::from_str(src).map_err(|e| e.to_string())
}

pub fn parse_machine(src: &str) -> Result<Machine, String> {
    ron::from_str(src).map_err(|e| e.to_string())
}

impl Library {
    /// An empty library in `dir` (nothing is written until `save`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            ..Default::default()
        }
    }

    /// Reads every part and machine under `dir` (a missing directory is an empty
    /// library).
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, String> {
        let mut lib = Self::new(dir);
        for kind in Kind::ALL {
            let d = lib.dir.join("parts").join(kind.dir());
            for (name, path) in ron_files(&d)? {
                let src = read(&path)?;
                let part = parse_part(&src).map_err(|e| format!("{}: {e}", path.display()))?;
                if part.kind() != kind {
                    return Err(format!(
                        "{}: a {} part in the {} directory",
                        path.display(),
                        part.kind().dir(),
                        kind.dir()
                    ));
                }
                lib.parts.insert(PartRef { kind, name }, part);
                lib.files.insert(path, src);
            }
        }
        for (name, path) in ron_files(&lib.dir.join("machines"))? {
            let src = read(&path)?;
            let m = parse_machine(&src).map_err(|e| format!("{}: {e}", path.display()))?;
            lib.machines.insert(name, m);
            lib.files.insert(path, src);
        }
        Ok(lib)
    }

    pub fn part_path(&self, r: &PartRef) -> PathBuf {
        self.dir
            .join("parts")
            .join(r.kind.dir())
            .join(format!("{}.ron", r.name))
    }

    pub fn machine_path(&self, name: &str) -> PathBuf {
        self.dir.join("machines").join(format!("{name}.ron"))
    }

    pub fn path_of(&self, t: &Target) -> PathBuf {
        match t {
            Target::Part(p) => self.part_path(p),
            Target::Machine(m) => self.machine_path(m),
        }
    }

    /// A part by reference (`kind/name`).
    pub fn part(&self, r: &str) -> Result<&Part, String> {
        let r = PartRef::parse(r)?;
        self.parts.get(&r).ok_or_else(|| format!("no part {r}"))
    }

    pub fn machine(&self, name: &str) -> Result<&Machine, String> {
        self.machines
            .get(name)
            .ok_or_else(|| format!("no machine \"{name}\""))
    }

    /// The text every file should hold.
    fn texts(&self) -> BTreeMap<PathBuf, String> {
        let mut t = BTreeMap::new();
        for (r, p) in &self.parts {
            t.insert(self.part_path(r), part_text(p));
        }
        for (n, m) in &self.machines {
            t.insert(self.machine_path(n), machine_text(m));
        }
        t
    }

    /// Files that differ from the library in memory (written, or to be removed).
    pub fn changed(&self) -> Vec<PathBuf> {
        let now = self.texts();
        let mut out: Vec<PathBuf> = now
            .iter()
            .filter(|(p, s)| self.files.get(*p) != Some(s))
            .map(|(p, _)| p.clone())
            .collect();
        out.extend(self.files.keys().filter(|p| !now.contains_key(*p)).cloned());
        out
    }

    /// Writes the files that changed and removes those of deleted parts and machines.
    /// Each file is written whole (to a temporary name, then renamed). Returns the paths.
    pub fn save(&mut self) -> Result<Vec<PathBuf>, String> {
        let now = self.texts();
        let mut touched = Vec::new();
        for (path, text) in &now {
            if self.files.get(path) == Some(text) {
                continue;
            }
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            let tmp = path.with_extension("ron.tmp");
            std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
            touched.push(path.clone());
        }
        let gone: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|p| !now.contains_key(*p))
            .cloned()
            .collect();
        for p in gone {
            if p.exists() {
                std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            touched.push(p);
        }
        self.files = now;
        Ok(touched)
    }

    /// Whether any file on disk differs from what was last read or written: someone else
    /// changed the library.
    pub fn changed_on_disk(&self) -> bool {
        let on_disk = Library::open(&self.dir)
            .map(|l| l.files)
            .unwrap_or_default();
        on_disk != self.files
    }

    /// Takes the in-memory state of `other` as this library's (after edits).
    pub fn adopt(&mut self, other: Library) {
        self.parts = other.parts;
        self.machines = other.machines;
    }

    /// Marks the current contents as what is on disk (after loading from elsewhere).
    pub fn mark_saved(&mut self) {
        self.files = self.texts();
    }
}

fn read(p: &Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// `(stem, path)` of the `.ron` files in a directory, sorted.
fn ron_files(d: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let Ok(rd) = std::fs::read_dir(d) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("ron")
            && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
        {
            out.push((stem.to_string(), p.clone()));
        }
    }
    out.sort();
    Ok(out)
}
