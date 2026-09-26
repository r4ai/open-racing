//! Kinds of vegetation to paint, as a city builder's asset menu offers them: the
//! built-in ones (woods, pines, broadleaf trees, bushes, long grass, rocks) and those
//! saved from a project's scatters into the user's library, which keeps their model
//! and texture files and the materials they use, to paint in any project.
//!
//! The library is a folder laid out as a project's assets are (`assets/models`,
//! `assets/textures`), with `presets.ron` listing the kinds: `%APPDATA%/open-racing/
//! vegetation` on Windows, `~/.config/open-racing/vegetation` elsewhere.

use std::path::{Path, PathBuf};

use open_racing_track_project::assets;
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{
    HARDNESS, MaterialDef, MaterialSlot, Scatter, ScatterModel, TextureSource,
};
use serde::{Deserialize, Serialize};

use crate::state::Editor;

/// Which shelf of the palette a kind is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    #[default]
    Trees,
    Bushes,
    Grass,
    Rocks,
    Other,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Trees,
        Category::Bushes,
        Category::Grass,
        Category::Rocks,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Trees => "Trees",
            Category::Bushes => "Bushes",
            Category::Grass => "Grass",
            Category::Rocks => "Rocks",
            Category::Other => "Other",
        }
    }
}

/// The brush a kind paints with.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushPreset {
    pub radius: f64,
    pub strength: f64,
    #[serde(default = "hardness")]
    pub hardness: f64,
}

fn hardness() -> f64 {
    HARDNESS
}

/// A kind of vegetation: a scatter to start from, with the brush to paint it with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kind {
    pub name: String,
    #[serde(default)]
    pub category: Category,
    /// Its models (built-in, or files of the library), spacing, sizes, lean,
    /// clearance, slope and distances; no strokes.
    pub scatter: Scatter,
    /// The materials its models use in place of their own, their textures files of
    /// the library.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<MaterialDef>,
    pub brush: BrushPreset,
}

/// The built-in kinds.
pub fn builtin() -> Vec<Kind> {
    use Category::*;
    #[allow(clippy::type_complexity)]
    let kinds: [(&str, Category, &[(&str, f64)], f64, [f64; 2], f64, f64); 11] = [
        (
            "mixed woods",
            Trees,
            &[("pine", 3.0), ("tree", 2.0), ("poplar", 1.0)],
            7.0,
            [0.8, 1.25],
            30.0,
            0.7,
        ),
        (
            "pines",
            Trees,
            &[("pine", 1.0)],
            6.0,
            [0.75, 1.3],
            30.0,
            0.7,
        ),
        (
            "broadleaf trees",
            Trees,
            &[("tree", 1.0)],
            8.0,
            [0.8, 1.3],
            30.0,
            0.6,
        ),
        (
            "poplars",
            Trees,
            &[("poplar", 1.0)],
            5.0,
            [0.85, 1.15],
            15.0,
            0.8,
        ),
        (
            "bushes",
            Bushes,
            &[("bush", 3.0), ("rock", 1.0)],
            4.0,
            [0.7, 1.4],
            15.0,
            0.6,
        ),
        (
            "shrubs",
            Bushes,
            &[("bush", 1.0)],
            2.5,
            [0.45, 0.9],
            10.0,
            0.6,
        ),
        (
            "long grass",
            Grass,
            &[("grass", 1.0)],
            1.2,
            [0.7, 1.5],
            8.0,
            0.8,
        ),
        (
            "meadow",
            Grass,
            &[("grass", 5.0), ("bush", 1.0)],
            1.8,
            [0.6, 1.3],
            15.0,
            0.6,
        ),
        ("rocks", Rocks, &[("rock", 1.0)], 5.0, [0.5, 1.8], 12.0, 0.5),
        (
            "boulders",
            Rocks,
            &[("rock", 1.0)],
            9.0,
            [1.6, 3.0],
            20.0,
            0.4,
        ),
        (
            "traffic cones",
            Other,
            &[("cone", 1.0)],
            2.0,
            [1.0, 1.0],
            4.0,
            0.5,
        ),
    ];
    kinds
        .into_iter()
        .map(
            |(name, category, models, spacing, scale, radius, strength)| {
                let small = matches!(category, Grass);
                let rocks = matches!(category, Rocks);
                Kind {
                    name: name.into(),
                    category,
                    scatter: Scatter {
                        name: name.into(),
                        seed: 0,
                        models: models
                            .iter()
                            .map(|(m, w)| {
                                ScatterModel::new(open_racing_track_project::shapes::path(m), *w)
                            })
                            .collect(),
                        spacing,
                        scale,
                        tilt: if rocks { 0.6 } else { 0.1 },
                        clearance: if category == Other { 0.5 } else { 3.0 },
                        max_slope: 35.0,
                        collide: rocks && scale[1] > 2.0,
                        shadows: !small,
                        // Grass is lost to the eye long before a tree.
                        detail: if small { 40.0 } else { 150.0 },
                        draw: if small {
                            200.0
                        } else if matches!(category, Bushes | Other) {
                            700.0
                        } else {
                            2500.0
                        },
                        strokes: vec![],
                        removed: vec![],
                        placed: vec![],
                        group: None,
                    },
                    materials: vec![],
                    brush: BrushPreset {
                        radius,
                        strength,
                        hardness: if small { 0.3 } else { HARDNESS },
                    },
                }
            },
        )
        .collect()
}

/// Where the user's library is kept.
pub fn library_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("OPEN_RACING_LIBRARY") {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("open-racing").join("vegetation"))
}

const PRESETS: &str = "presets.ron";

/// The kinds saved in the library at `dir`; none if there is none.
pub fn load(dir: &Path) -> Result<Vec<Kind>, String> {
    match std::fs::read_to_string(dir.join(PRESETS)) {
        Ok(text) => {
            ron::from_str(&text).map_err(|e| format!("{}: {e}", dir.join(PRESETS).display()))
        }
        Err(_) => Ok(vec![]),
    }
}

fn store(dir: &Path, kinds: &[Kind]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = ron::ser::to_string_pretty(kinds, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())?;
    std::fs::write(dir.join(PRESETS), text).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Copies what a file of `from` refers to into `to`, unless it is built in: the
/// path it has there.
fn bring(from: &Path, to: &Path, path: &Path) -> Result<PathBuf, String> {
    if open_racing_track_project::shapes::name(path).is_some() {
        return Ok(path.to_path_buf());
    }
    assets::import_once(to, &from.join(path)).map_err(|e| e.to_string())
}

/// A scatter's models and materials brought from the directory `from` into `to`: the
/// files copied, the materials renamed where `taken` says their names are, and the
/// models pointed at both.
fn bring_scatter(
    s: &Scatter,
    materials: &[MaterialDef],
    from: &Path,
    to: &Path,
    taken: impl Fn(&MaterialDef) -> Option<String>,
) -> Result<(Scatter, Vec<MaterialDef>), String> {
    let mut s = s.clone();
    let mut brought = Vec::new();
    let mut renamed: Vec<(String, String)> = Vec::new();
    for m in materials {
        let mut m = m.clone();
        for t in [&mut m.texture, &mut m.normal] {
            if let TextureSource::File(p) = t {
                *p = bring(from, to, p)?;
            }
        }
        if let Some(name) = taken(&m) {
            renamed.push((m.name.clone(), name.clone()));
            m.name = name;
        }
        brought.push(m);
    }
    for m in &mut s.models {
        m.model = bring(from, to, &m.model)?;
        if let Some(far) = &mut m.far {
            *far = bring(from, to, far)?;
        }
        for slot in &mut m.materials {
            if let Some((_, to)) = renamed.iter().find(|(a, _)| *a == slot.material) {
                slot.material = to.clone();
            }
        }
    }
    Ok((s, brought))
}

/// Adds a scatter of a kind to the project, named after it, with its files and
/// materials brought in from the library at `library` (built-in kinds need none).
/// Returns the scatter's name.
pub fn add(editor: &mut Editor, kind: &Kind, library: Option<&Path>) -> Result<String, String> {
    let p = &editor.project;
    let name = crate::presets::free_name(&kind.name, |n| p.scatter.iter().any(|s| s.name == n));
    let from = library.map_or_else(|| editor.dir.clone(), Path::to_path_buf);
    let (mut scatter, materials) =
        bring_scatter(&kind.scatter, &kind.materials, &from, &editor.dir, |m| {
            // A material of the same name and look is used as it is; another of the same
            // name is not replaced.
            match p.materials.iter().find(|x| x.name == m.name) {
                Some(x) if x == m => None,
                Some(_) => Some(crate::presets::free_name(&m.name, |n| {
                    p.material_index(n).is_some()
                })),
                None => None,
            }
        })?;
    scatter.name = name.clone();
    scatter.strokes.clear();
    scatter.removed.clear();
    scatter.placed.clear();
    scatter.seed = 0;
    let mut ops: Vec<Op> = materials
        .into_iter()
        .filter(|m| !editor.project.materials.contains(m))
        .map(|material| Op::PutMaterial { material })
        .collect();
    ops.push(Op::PutScatter { scatter });
    if editor.apply(ops, None) {
        Ok(name)
    } else {
        Err(editor.status.clone())
    }
}

/// Saves a scatter of the project as a kind in the library at `library`, its model
/// and texture files and materials with it, replacing a kind of the same name.
pub fn save(
    editor: &Editor,
    scatter: &str,
    name: &str,
    category: Category,
    brush: BrushPreset,
    library: &Path,
) -> Result<(), String> {
    let p = &editor.project;
    let s = p
        .scatter
        .iter()
        .find(|s| s.name == scatter)
        .ok_or_else(|| format!("no scatter \"{scatter}\""))?;
    let used: Vec<MaterialDef> = s
        .models
        .iter()
        .flat_map(|m| &m.materials)
        .filter_map(|slot: &MaterialSlot| {
            p.material_index(&slot.material)
                .map(|i| p.materials[i].clone())
        })
        .fold(Vec::new(), |mut all, m| {
            if !all.contains(&m) {
                all.push(m);
            }
            all
        });
    let (mut s, materials) = bring_scatter(s, &used, &editor.dir, library, |_| None)?;
    s.name = name.to_string();
    s.strokes.clear();
    s.removed.clear();
    s.placed.clear();
    s.seed = 0;
    s.group = None;
    let mut kinds = load(library)?;
    let kind = Kind {
        name: name.to_string(),
        category,
        scatter: s,
        materials,
        brush,
    };
    match kinds.iter_mut().find(|k| k.name == name) {
        Some(k) => *k = kind,
        None => kinds.push(kind),
    }
    store(library, &kinds)
}

/// Removes a kind from the library. Its files stay: other kinds may use them.
pub fn delete(library: &Path, name: &str) -> Result<(), String> {
    let mut kinds = load(library)?;
    kinds.retain(|k| k.name != name);
    store(library, &kinds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "open-racing-vegetation-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn built_in_kinds_are_valid_scatters() {
        let (dir, _) = (temp("builtin"), ());
        let mut editor = Editor::open(dir.clone()).unwrap();
        for kind in builtin() {
            let name = add(&mut editor, &kind, None).unwrap();
            assert_eq!(name, kind.name);
        }
        // The same kind again: a name of its own.
        let again = add(&mut editor, &builtin()[0], None).unwrap();
        assert_eq!(again, "mixed woods.001");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_saved_kind_takes_its_model_and_materials_to_another_project() {
        let (a, b, library) = (temp("a"), temp("b"), temp("library"));
        let mut one = Editor::open(a.clone()).unwrap();
        // A model file of the project, and a material of its own used for its leaves.
        let model = one.dir.join("assets/models/oak.glb");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(&model, b"glb bytes").unwrap();
        let mut kind = builtin().remove(2);
        kind.scatter.models[0].model = "assets/models/oak.glb".into();
        kind.scatter.models[0].materials = vec![MaterialSlot {
            slot: 1,
            material: "leaves".into(),
        }];
        let leaves = MaterialDef {
            name: "leaves".into(),
            ..one.project.materials[2].clone()
        };
        assert!(one.apply(
            vec![Op::PutMaterial {
                material: leaves.clone()
            }],
            None
        ));
        let name = add(&mut one, &kind, None).unwrap();
        let brush = BrushPreset {
            radius: 12.0,
            strength: 0.5,
            hardness: 0.8,
        };
        save(&one, &name, "oaks", Category::Trees, brush, &library).unwrap();
        assert!(library.join("assets/models/oak.glb").is_file());

        let kinds = load(&library).unwrap();
        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].brush, brush);
        assert_eq!(kinds[0].materials, vec![leaves.clone()]);
        let mut two = Editor::open(b.clone()).unwrap();
        let added = add(&mut two, &kinds[0], Some(&library)).unwrap();
        let s = two
            .project
            .scatter
            .iter()
            .find(|s| s.name == added)
            .unwrap();
        assert_eq!(s.models[0].model, PathBuf::from("assets/models/oak.glb"));
        assert!(two.dir.join("assets/models/oak.glb").is_file());
        assert!(two.project.materials.contains(&leaves));
        // Again: nothing copied twice.
        add(&mut two, &kinds[0], Some(&library)).unwrap();
        assert!(!two.dir.join("assets/models/oak-1.glb").exists());
        delete(&library, "oaks").unwrap();
        assert!(load(&library).unwrap().is_empty());
        for d in [a, b, library] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }
}
