//! The files a project uses: textures and models, kept under `assets/` in the project's
//! directory, and what in the project refers to each. The files are the only record of
//! what assets there are; the project refers to them by their paths.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::Error;
use crate::ops::Op;
use crate::project::{MaterialDef, Project, TextureSource};

pub const TEXTURES: &str = "assets/textures";
pub const MODELS: &str = "assets/models";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Kind {
    Texture,
    Model,
}

impl Kind {
    /// What a file is, by its extension.
    pub fn of(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" | "dds" => Some(Self::Texture),
            "glb" | "gltf" => Some(Self::Model),
            _ => None,
        }
    }

    pub fn dir(self) -> &'static str {
        match self {
            Self::Texture => TEXTURES,
            Self::Model => MODELS,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Asset {
    /// Relative to the project's directory, with forward slashes.
    pub path: PathBuf,
    pub kind: Kind,
    /// Size of the file; `None` when it is referred to but missing.
    pub bytes: Option<u64>,
    /// What refers to it: "material asphalt", "prop grandstand".
    pub used_by: Vec<String>,
}

/// A path as the project stores it: relative, with forward slashes.
fn portable(path: &Path) -> PathBuf {
    PathBuf::from(
        path.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Every file the project refers to, with what refers to it.
pub fn references(project: &Project) -> Vec<(PathBuf, String)> {
    let mut refs = Vec::new();
    for m in &project.materials {
        for source in [&m.texture, &m.normal] {
            if let TextureSource::File(p) = source {
                refs.push((portable(p), format!("material {}", m.name)));
            }
        }
    }
    for p in &project.props {
        refs.push((portable(&p.model), format!("prop {}", p.name)));
    }
    if let Some(r) = &project.reference {
        refs.push((portable(&r.image), "reference image".into()));
    }
    refs
}

/// The asset files under `assets/`, and the files the project refers to anywhere,
/// sorted by kind and path. A folder holding a .gltf is that model's: its images and
/// other files are not listed on their own.
pub fn list(project: &Project, dir: &Path) -> Vec<Asset> {
    let mut found: BTreeMap<PathBuf, Asset> = BTreeMap::new();
    let mut stack = vec![dir.join("assets")];
    while let Some(d) = stack.pop() {
        let entries: Vec<_> = std::fs::read_dir(&d)
            .into_iter()
            .flatten()
            .flatten()
            .collect();
        let gltf = entries.iter().any(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("gltf"))
        });
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                if !gltf {
                    stack.push(path);
                }
                continue;
            }
            let Some(kind) = Kind::of(&path).filter(|&k| !gltf || k == Kind::Model) else {
                continue;
            };
            let rel = portable(path.strip_prefix(dir).unwrap_or(&path));
            let bytes = entry.metadata().ok().map(|m| m.len());
            found.insert(
                rel.clone(),
                Asset {
                    path: rel,
                    kind,
                    bytes,
                    used_by: vec![],
                },
            );
        }
    }
    for (path, user) in references(project) {
        let kind = Kind::of(&path).unwrap_or(Kind::Texture);
        let entry = found.entry(path.clone()).or_insert_with(|| Asset {
            bytes: std::fs::metadata(dir.join(&path)).ok().map(|m| m.len()),
            path,
            kind,
            used_by: vec![],
        });
        entry.used_by.push(user);
    }
    let mut assets: Vec<Asset> = found.into_values().collect();
    assets.sort_by(|a, b| (a.kind, &a.path).cmp(&(b.kind, &b.path)));
    assets
}

/// A file name in `dir` like `name` that is not taken yet.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map_or("asset".into(), |s| s.to_string_lossy());
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    (0..)
        .map(|i| {
            let base = if i == 0 {
                stem.to_string()
            } else {
                format!("{stem}-{i}")
            };
            match &ext {
                Some(e) => format!("{base}.{e}"),
                None => base,
            }
        })
        .map(|n| dir.join(n))
        .find(|p| !p.exists())
        .expect("some name is free")
}

fn copy(from: &Path, to: &Path) -> Result<(), Error> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io(parent.to_path_buf(), e))?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| Error::Io(from.to_path_buf(), e))
}

/// Copies a texture or model into the project's `assets/`, under a name not taken yet,
/// and returns its path relative to the project. A .gltf brings the buffers and images
/// it refers to along, in a folder of its own.
pub fn import(dir: &Path, file: &Path) -> Result<PathBuf, Error> {
    let kind = Kind::of(file).ok_or_else(|| {
        Error::Invalid(format!(
            "{}: not a texture (.png, .dds) or model (.glb, .gltf)",
            file.display()
        ))
    })?;
    let name = file
        .file_name()
        .map_or("asset".into(), |n| n.to_string_lossy().into_owned());
    let target_dir = dir.join(kind.dir());
    let is_gltf = file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gltf"));
    let target = if is_gltf {
        // Its own folder, so that the files it refers to keep their relative paths.
        let stem = file
            .file_stem()
            .map_or("model".into(), |s| s.to_string_lossy());
        free_name(&target_dir, &stem).join(&name)
    } else {
        free_name(&target_dir, &name)
    };
    copy(file, &target)?;
    if is_gltf {
        let src = std::fs::read_to_string(file).map_err(|e| Error::Io(file.to_path_buf(), e))?;
        let json: serde_json::Value = serde_json::from_str(&src)
            .map_err(|e| Error::Invalid(format!("{}: {e}", file.display())))?;
        let from = file.parent().unwrap_or(Path::new("."));
        let to = target.parent().expect("inside its folder");
        for list in ["buffers", "images"] {
            for uri in json[list]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| b["uri"].as_str())
                .filter(|u| !u.starts_with("data:"))
            {
                let rel = urlencoding::decode(uri)
                    .map_err(|e| Error::Invalid(format!("{}: {e}", file.display())))?
                    .into_owned();
                copy(&from.join(&rel), &to.join(&rel))?;
            }
        }
    }
    Ok(portable(target.strip_prefix(dir).unwrap_or(&target)))
}

/// Operations that make the project refer to `to` wherever it refers to `from`.
pub fn repoint(project: &Project, from: &Path, to: &Path) -> Vec<Op> {
    let (from, to) = (portable(from), portable(to));
    let swap = |s: &TextureSource| match s {
        TextureSource::File(p) if portable(p) == from => Some(TextureSource::File(to.clone())),
        _ => None,
    };
    let mut ops = Vec::new();
    for m in &project.materials {
        let (texture, normal) = (swap(&m.texture), swap(&m.normal));
        if texture.is_some() || normal.is_some() {
            ops.push(Op::PutMaterial {
                material: MaterialDef {
                    texture: texture.unwrap_or_else(|| m.texture.clone()),
                    normal: normal.unwrap_or_else(|| m.normal.clone()),
                    ..m.clone()
                },
            });
        }
    }
    for p in &project.props {
        if portable(&p.model) == from {
            let mut p = p.clone();
            p.model = to.clone();
            ops.push(Op::PutProp { prop: p });
        }
    }
    if let Some(r) = &project.reference
        && portable(&r.image) == from
    {
        ops.push(Op::SetReference {
            reference: Some(crate::project::Reference {
                image: to.clone(),
                ..r.clone()
            }),
        });
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_list_and_repoint() {
        let dir = std::env::temp_dir().join(format!("open-racing-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("wall.png");
        std::fs::write(&src, b"\x89PNG not really").unwrap();
        let project_dir = dir.join("p");
        let a = import(&project_dir, &src).unwrap();
        let b = import(&project_dir, &src).unwrap();
        assert_eq!(a, PathBuf::from("assets/textures/wall.png"));
        assert_eq!(b, PathBuf::from("assets/textures/wall-1.png"));

        let mut p = Project::new("t");
        p.materials[0].texture = TextureSource::File(a.clone());
        let assets = list(&p, &project_dir);
        let used = |path: &Path| &assets.iter().find(|x| x.path == path).unwrap().used_by;
        assert_eq!(assets.len(), 2);
        assert_eq!(used(&a), &vec!["material asphalt".to_string()]);
        assert!(used(&b).is_empty());

        let ops = repoint(&p, &a, &b);
        crate::ops::apply_all(&mut p, &ops).unwrap();
        assert_eq!(p.materials[0].texture, TextureSource::File(b));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
