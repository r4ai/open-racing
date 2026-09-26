//! Poly Haven's models and textures in a project: downloaded into the cache (see the
//! open-racing-polyhaven crate), prepared there once (see `prepare`), and brought into
//! the project's assets like any other file, so that the project bakes without the
//! network or the cache.

use std::path::{Path, PathBuf};

pub use open_racing_polyhaven::{self as api, Asset, AssetType, CREDIT, Client};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::assets;
use crate::prepare::{self, Prepared};
use crate::project::{Alpha, Kind, Layout, MaterialDef, Scatter, ScatterModel, TextureSource};

/// The resolution downloaded unless asked otherwise: plants are seen at a distance, and
/// most of the ground from a car.
pub const RES: &str = "1k";
/// Changes whenever preparing makes different files, so that those made before are
/// made again.
const VERSION: u32 = 2;

/// Plants and rocks offered first, light enough to scatter by the thousand once
/// prepared, and what each is.
pub const NATURE: [(&str, Kind); 16] = [
    ("fir_sapling", Kind::Evergreen),
    ("pine_sapling_medium", Kind::Evergreen),
    ("fir_tree_01", Kind::Evergreen),
    ("pine_tree_01", Kind::Evergreen),
    ("island_tree_01", Kind::Deciduous),
    ("jacaranda_tree", Kind::Deciduous),
    ("fern_02", Kind::Deciduous),
    ("nettle_plant", Kind::Deciduous),
    ("dandelion_01", Kind::Grass),
    ("grass_medium_01", Kind::Grass),
    ("grass_medium_02", Kind::Grass),
    ("grass_bermuda_01", Kind::Grass),
    ("boulder_01", Kind::Rigid),
    ("rock_moss_set_01", Kind::Rigid),
    ("dead_tree_trunk", Kind::Rigid),
    ("dry_branches_medium_01", Kind::Rigid),
];

fn api(e: api::Error) -> Error {
    Error::Invalid(e.to_string())
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |e| Error::Io(path.to_path_buf(), e)
}

/// What kind of thing an asset is, by its categories and tags.
pub fn kind_of(asset: &Asset) -> Kind {
    if let Some((_, k)) = NATURE.iter().find(|(id, _)| *id == asset.id) {
        return *k;
    }
    let conifer = ["conifer", "coniferous", "pine", "fir", "spruce", "needles"];
    if asset.is_in("grass") || asset.has_tag("grass") {
        Kind::Grass
    } else if conifer.iter().any(|t| asset.has_tag(t)) {
        Kind::Evergreen
    } else if ["trees", "plants", "flowers", "potted plants", "succulent"]
        .iter()
        .any(|c| asset.is_in(c))
    {
        Kind::Deciduous
    } else {
        Kind::Rigid
    }
}

/// A model of Poly Haven's prepared in the cache: one model for each of its variants.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadyModel {
    version: u32,
    pub id: String,
    pub name: String,
    pub res: String,
    pub kind: Kind,
    pub credit: String,
    pub models: Vec<Prepared>,
}

/// A model, downloaded and prepared unless it is in the cache already. `progress`
/// hears the bytes downloaded and the bytes in all.
pub fn model(
    client: &Client,
    id: &str,
    res: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ReadyModel, Error> {
    let dir = client.dir().join("prepared").join(id).join(res);
    let record = dir.join("model.json");
    if let Some(ready) = std::fs::read_to_string(&record)
        .ok()
        .and_then(|t| serde_json::from_str::<ReadyModel>(&t).ok())
        .filter(|r| r.version == VERSION && r.models.iter().all(|m| m.path.is_file()))
    {
        return Ok(ready);
    }
    let asset = client.asset(id).map_err(api)?;
    let download = client
        .download(id, AssetType::Models, res, progress)
        .map_err(api)?;
    let kind = kind_of(&asset);
    let _ = std::fs::remove_dir_all(&dir);
    let copyright = format!("{} (CC0)", asset.credit());
    let models = prepare::prepare(&download.main, &dir, id, kind, &copyright)?;
    let ready = ReadyModel {
        version: VERSION,
        id: id.into(),
        name: asset.name.clone(),
        res: res.into(),
        kind,
        credit: asset.credit(),
        models,
    };
    let text = serde_json::to_string_pretty(&ready).map_err(|e| Error::Invalid(e.to_string()))?;
    std::fs::write(&record, text).map_err(io(&record))?;
    Ok(ready)
}

/// Whether a model is prepared in the cache, so that using it needs no download.
pub fn is_ready(client: &Client, id: &str, res: &str) -> bool {
    client
        .dir()
        .join("prepared")
        .join(id)
        .join(res)
        .join("model.json")
        .is_file()
}

/// Brings a prepared model's files into the project at `dir`, once, and returns the
/// paths to refer to each by.
pub fn import_model(dir: &Path, ready: &ReadyModel) -> Result<Vec<PathBuf>, Error> {
    ready
        .models
        .iter()
        .map(|m| assets::import_once(dir, &m.path))
        .collect()
}

/// A scatter of a prepared model's variants at `paths` (as `import_model` gives them,
/// or anywhere), each as likely, spaced and drawn as far as suits its kind and size.
pub fn scatter(name: &str, ready: &ReadyModel, paths: &[PathBuf]) -> Scatter {
    // glTF's frame: Y up.
    let height = ready.models.iter().map(|m| m.size[1]).fold(0.0, f32::max) as f64;
    let width = ready
        .models
        .iter()
        .map(|m| m.size[0].max(m.size[2]))
        .fold(0.0, f32::max) as f64;
    let kind = ready.kind;
    let small = height < 1.5;
    let (spacing, detail, draw) = match kind {
        Kind::Grass => ((width * 1.5).max(0.5), 40.0, 200.0),
        Kind::Rigid => (
            (width * 2.0).max(1.0),
            150.0,
            if small { 500.0 } else { 1500.0 },
        ),
        _ if small => ((width * 1.2).max(0.5), 60.0, 400.0),
        _ => ((width * 0.9).max(2.0), 150.0, 2500.0),
    };
    Scatter {
        name: name.into(),
        seed: 0,
        models: paths
            .iter()
            .map(|p| ScatterModel {
                kind: Some(kind),
                ..ScatterModel::new(p.clone(), 1.0)
            })
            .collect(),
        spacing,
        scale: [0.8, 1.25],
        tilt: if kind == Kind::Rigid { 0.6 } else { 0.1 },
        clearance: if small { 1.0 } else { 3.0 },
        max_slope: 35.0,
        collide: kind == Kind::Rigid && width > 1.5,
        shadows: kind != Kind::Grass,
        detail,
        draw,
        variety: crate::project::VARIETY,
        layout: Layout::Grid,
        strokes: vec![],
        removed: vec![],
        placed: vec![],
        group: None,
    }
}

/// A texture of Poly Haven's ready in the cache: its colour (with alpha where it has
/// one), normal map and the size one repetition covers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadyTexture {
    version: u32,
    pub id: String,
    pub name: String,
    pub res: String,
    pub credit: String,
    pub color: PathBuf,
    pub normal: Option<PathBuf>,
    pub roughness: f32,
    /// Cut out by its alpha: leaves, fences.
    pub alpha: bool,
    /// Metres one repetition covers, across and along.
    pub tile: [f32; 2],
}

/// A texture, downloaded unless it is in the cache already.
pub fn texture(
    client: &Client,
    id: &str,
    res: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ReadyTexture, Error> {
    use open_racing_track::texture;
    let dir = client.dir().join("prepared").join(id).join(res);
    let record = dir.join("texture.json");
    if let Some(ready) = std::fs::read_to_string(&record)
        .ok()
        .and_then(|t| serde_json::from_str::<ReadyTexture>(&t).ok())
        .filter(|r| r.version == VERSION && r.color.is_file())
    {
        return Ok(ready);
    }
    let asset = client.asset(id).map_err(api)?;
    let download = client
        .download(id, AssetType::Textures, res, progress)
        .map_err(api)?;
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let read = |p: &Path| -> Result<texture::Image, Error> {
        let bytes = std::fs::read(p).map_err(io(p))?;
        texture::decode(&bytes).map_err(|e| Error::Invalid(format!("{}: {e}", p.display())))
    };
    // The colour map, with the alpha map's red as its alpha where there is one.
    let (color, alpha) = match download.maps.get("Alpha") {
        Some(a) => {
            let mut image = read(&download.main)?;
            let alpha = read(a)?;
            prepare::put_alpha(&mut image, &alpha);
            let path = dir.join(format!("{id}_color_{res}.png"));
            std::fs::write(&path, prepare::png(&image, true)?).map_err(io(&path))?;
            (path, true)
        }
        None => (download.main.clone(), false),
    };
    let roughness = match download.maps.get("arm") {
        Some(arm) => prepare::channel_mean(&read(arm)?, 1),
        None => 0.8,
    };
    let size = |i: usize| {
        asset
            .dimensions
            .get(i)
            .map_or(2.0, |mm| (*mm / 1000.0) as f32)
    };
    let ready = ReadyTexture {
        version: VERSION,
        id: id.into(),
        name: asset.name.clone(),
        res: res.into(),
        credit: asset.credit(),
        color,
        normal: download.maps.get("nor_gl").cloned(),
        roughness,
        alpha,
        tile: [size(0), size(1)],
    };
    let text = serde_json::to_string_pretty(&ready).map_err(|e| Error::Invalid(e.to_string()))?;
    std::fs::write(&record, text).map_err(io(&record))?;
    Ok(ready)
}

/// Brings a texture's files into the project at `dir`, once, and returns a material of
/// it named `name`.
pub fn import_texture(dir: &Path, ready: &ReadyTexture, name: &str) -> Result<MaterialDef, Error> {
    let color = assets::import_once(dir, &ready.color)?;
    let normal = match &ready.normal {
        Some(n) => TextureSource::File(assets::import_once(dir, n)?),
        None => TextureSource::None,
    };
    Ok(MaterialDef {
        name: name.into(),
        color: [1.0; 3],
        texture: TextureSource::File(color),
        tile: ready.tile,
        roughness: ready.roughness,
        reflectance: 0.5,
        double_sided: ready.alpha,
        normal,
        alpha: if ready.alpha {
            Alpha::Mask(0.5)
        } else {
            Alpha::Opaque
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_category_and_tag() {
        let asset = |id: &str, categories: &[&str], tags: &[&str]| Asset {
            id: id.into(),
            categories: categories.iter().map(|s| s.to_string()).collect(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert_eq!(
            kind_of(&asset("x", &["nature", "trees"], &["coniferous"])),
            Kind::Evergreen
        );
        assert_eq!(
            kind_of(&asset("x", &["nature", "trees"], &["oak"])),
            Kind::Deciduous
        );
        assert_eq!(kind_of(&asset("x", &["nature", "grass"], &[])), Kind::Grass);
        assert_eq!(kind_of(&asset("x", &["rocks"], &[])), Kind::Rigid);
        assert_eq!(kind_of(&asset("boulder_01", &["plants"], &[])), Kind::Rigid);
    }
}
