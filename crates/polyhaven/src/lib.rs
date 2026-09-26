//! Poly Haven (<https://polyhaven.com>): a library of CC0 models, textures and HDRIs,
//! read through its public API (<https://api.polyhaven.com>).
//!
//! The catalogue and each asset's list of files are kept in a cache on disk and asked
//! for again after a day (the cached copy serves when the network does not). Files are
//! downloaded into the cache once, checked against their MD5, and used offline after.
//!
//! The assets are CC0 and need no attribution, but software offering them through the
//! API must say where they come from: show [`CREDIT`] wherever they are offered.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

pub const API: &str = "https://api.polyhaven.com";
/// Where the assets come from, to show wherever they are offered.
pub const CREDIT: &str = "Assets from Poly Haven (polyhaven.com), CC0";
/// How long the catalogue and lists of files are used before they are asked for again.
const FRESH: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug)]
pub enum Error {
    /// A request that failed: its URL and why.
    Http(String, String),
    Io(PathBuf, std::io::Error),
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(url, e) => write!(f, "{url}: {e}"),
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Invalid(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |e| Error::Io(path.to_path_buf(), e)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetType {
    Hdris,
    Textures,
    Models,
}

impl AssetType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hdris => "hdris",
            Self::Textures => "textures",
            Self::Models => "models",
        }
    }
}

/// An asset as the catalogue lists it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Asset {
    /// Its name in URLs: `fir_sapling`.
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// 0 for an HDRI, 1 a texture, 2 a model.
    #[serde(rename = "type", default)]
    pub type_number: u8,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Each author and what they did.
    #[serde(default)]
    pub authors: BTreeMap<String, String>,
    /// A model's triangles, in its most detailed form.
    #[serde(default)]
    pub polycount: Option<u64>,
    /// A model's size, or the size of the surface a texture covers once, mm.
    #[serde(default)]
    pub dimensions: Vec<f64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub thumbnail_url: String,
    #[serde(default)]
    pub download_count: u64,
}

impl Asset {
    pub fn asset_type(&self) -> AssetType {
        match self.type_number {
            0 => AssetType::Hdris,
            1 => AssetType::Textures,
            _ => AssetType::Models,
        }
    }

    /// Its page on the website.
    pub fn page(&self) -> String {
        format!("https://polyhaven.com/a/{}", self.id)
    }

    pub fn is_in(&self, category: &str) -> bool {
        self.categories
            .iter()
            .any(|c| c.eq_ignore_ascii_case(category))
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
    }

    /// Whether every word of `query` is in its id, name, tags or categories.
    pub fn matches(&self, query: &str) -> bool {
        let words: Vec<String> = [&self.id, &self.name]
            .into_iter()
            .chain(&self.tags)
            .chain(&self.categories)
            .map(|w| w.to_lowercase())
            .collect();
        query
            .split_whitespace()
            .map(str::to_lowercase)
            .all(|q| words.iter().any(|w| w.contains(&q)))
    }

    /// Who made it and where it is from, for a credits list.
    pub fn credit(&self) -> String {
        let authors: Vec<&str> = self.authors.keys().map(String::as_str).collect();
        format!(
            "{} by {} ({}, CC0)",
            self.name,
            authors.join(", "),
            self.page()
        )
    }
}

/// A file to download: a model's .gltf brings the files it includes, by their paths
/// relative to it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct File {
    pub url: String,
    #[serde(default)]
    pub md5: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub include: BTreeMap<String, File>,
}

impl File {
    /// The name the URL gives it.
    pub fn name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or(&self.url)
    }

    /// Its size and that of the files it includes, bytes.
    pub fn total_size(&self) -> u64 {
        self.size + self.include.values().map(File::total_size).sum::<u64>()
    }
}

/// An asset's files: for each map (`Diffuse`, `nor_gl`, `twigs_alpha`) or format
/// (`gltf`, `blend`), its resolutions (`1k`, `2k`) and file formats (`jpg`, `png`).
#[derive(Clone, Debug, Default)]
pub struct Files(BTreeMap<String, serde_json::Value>);

impl Files {
    pub fn get(&self, map: &str, res: &str, format: &str) -> Option<File> {
        serde_json::from_value(self.0.get(map)?.get(res)?.get(format)?.clone()).ok()
    }

    /// The maps and formats there are.
    pub fn maps(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// The resolutions a map or format comes in, smallest first.
    pub fn resolutions(&self, map: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .0
            .get(map)
            .and_then(|m| m.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        out.sort_by_key(|r| r.trim_end_matches('k').parse::<u32>().unwrap_or(u32::MAX));
        out
    }
}

/// An asset's files in the cache.
#[derive(Clone, Debug)]
pub struct Download {
    /// The folder they are in.
    pub dir: PathBuf,
    /// A model's .gltf, or a texture's colour map.
    pub main: PathBuf,
    /// The other maps by their name in `Files`: for a model its alpha maps (in the folder
    /// of its textures, next to the colour maps they belong to), for a texture its
    /// `nor_gl`, `arm` and `Alpha`.
    pub maps: BTreeMap<String, PathBuf>,
}

/// Where the cache is kept unless `OPEN_RACING_POLYHAVEN` says: the user's cache folder.
pub fn default_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("OPEN_RACING_POLYHAVEN") {
        return Some(PathBuf::from(dir));
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_CACHE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("open-racing").join("polyhaven"))
}

/// The API and the cache.
#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    dir: PathBuf,
}

impl Client {
    /// A client keeping its cache in `dir`.
    pub fn new(dir: PathBuf) -> Self {
        // The API asks for a user agent naming the software.
        let agent = ureq::AgentBuilder::new()
            .user_agent(&format!(
                "open-racing/{} (track editor)",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(60))
            .build();
        Self { agent, dir }
    }

    /// A client with its cache in the default place.
    pub fn open() -> Result<Self, Error> {
        default_dir()
            .map(Self::new)
            .ok_or_else(|| Error::Invalid("no folder for Poly Haven's cache".into()))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn get(&self, url: &str) -> Result<ureq::Response, Error> {
        self.agent
            .get(url)
            .call()
            .map_err(|e| Error::Http(url.into(), e.to_string()))
    }

    /// JSON from `url`, kept at `cache` and used from there while it is fresh, or when
    /// the request fails.
    fn json(&self, url: &str, cache: &Path) -> Result<serde_json::Value, Error> {
        let parse = |text: &str| {
            serde_json::from_str(text).map_err(|e| Error::Invalid(format!("{url}: {e}")))
        };
        let stored = std::fs::read_to_string(cache).ok();
        let fresh = std::fs::metadata(cache)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < FRESH);
        if fresh && let Some(text) = &stored {
            return parse(text);
        }
        let fetched = self.get(url).and_then(|r| {
            let mut text = String::new();
            r.into_reader()
                .read_to_string(&mut text)
                .map_err(|e| Error::Http(url.into(), e.to_string()))?;
            Ok(text)
        });
        match (fetched, stored) {
            (Ok(text), _) => {
                let value = parse(&text)?;
                if let Some(parent) = cache.parent() {
                    std::fs::create_dir_all(parent).map_err(io(parent))?;
                }
                std::fs::write(cache, &text).map_err(io(cache))?;
                Ok(value)
            }
            (Err(_), Some(text)) => parse(&text),
            (Err(e), None) => Err(e),
        }
    }

    /// The assets of a type, most downloaded first.
    pub fn catalog(&self, kind: AssetType) -> Result<Vec<Asset>, Error> {
        let url = format!("{API}/assets?type={}", kind.name());
        let cache = self
            .dir
            .join("catalog")
            .join(format!("{}.json", kind.name()));
        let list: BTreeMap<String, Asset> = serde_json::from_value(self.json(&url, &cache)?)
            .map_err(|e| Error::Invalid(format!("{url}: {e}")))?;
        let mut assets: Vec<Asset> = list.into_iter().map(|(id, a)| Asset { id, ..a }).collect();
        assets.sort_by_key(|a| std::cmp::Reverse(a.download_count));
        Ok(assets)
    }

    /// One asset.
    pub fn asset(&self, id: &str) -> Result<Asset, Error> {
        let url = format!("{API}/info/{id}");
        let cache = self.dir.join("info").join(format!("{id}.json"));
        let asset: Asset = serde_json::from_value(self.json(&url, &cache)?)
            .map_err(|e| Error::Invalid(format!("{url}: {e}")))?;
        Ok(Asset {
            id: id.into(),
            ..asset
        })
    }

    /// Which files an asset has.
    pub fn files(&self, id: &str) -> Result<Files, Error> {
        let url = format!("{API}/files/{id}");
        let cache = self.dir.join("files").join(format!("{id}.json"));
        serde_json::from_value(self.json(&url, &cache)?)
            .map(Files)
            .map_err(|e| Error::Invalid(format!("{url}: {e}")))
    }

    /// Its picture, 256 pixels square, as a PNG in the cache.
    pub fn thumbnail(&self, asset: &Asset) -> Result<PathBuf, Error> {
        let path = self.dir.join("thumbs").join(format!("{}.png", asset.id));
        if !path.is_file() {
            let url = if asset.thumbnail_url.is_empty() {
                format!(
                    "https://cdn.polyhaven.com/asset_img/thumbs/{}.png?width=256&height=256",
                    asset.id
                )
            } else {
                asset.thumbnail_url.clone()
            };
            let mut bytes = Vec::new();
            self.get(&url)?
                .into_reader()
                .read_to_end(&mut bytes)
                .map_err(|e| Error::Http(url.clone(), e.to_string()))?;
            write_new(&path, &bytes)?;
        }
        Ok(path)
    }

    /// Downloads `file` to `to` unless it is there already, checking its MD5; tells
    /// `progress` each chunk's size.
    pub fn fetch(
        &self,
        file: &File,
        to: &Path,
        progress: &mut dyn FnMut(u64),
    ) -> Result<(), Error> {
        if to.is_file() {
            return Ok(());
        }
        let parent = to.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent).map_err(io(parent))?;
        // Written aside and moved into place once whole and checked, so that a file in
        // the cache is always complete.
        let part = to.with_extension("part");
        let mut out = std::fs::File::create(&part).map_err(io(&part))?;
        let mut reader = self.get(&file.url)?.into_reader();
        let mut hash = md5::Context::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| Error::Http(file.url.clone(), e.to_string()))?;
            if n == 0 {
                break;
            }
            hash.consume(&buf[..n]);
            out.write_all(&buf[..n]).map_err(io(&part))?;
            progress(n as u64);
        }
        drop(out);
        let md5 = format!("{:x}", hash.finalize());
        if !file.md5.is_empty() && !md5.eq_ignore_ascii_case(&file.md5) {
            let _ = std::fs::remove_file(&part);
            return Err(Error::Invalid(format!(
                "{}: damaged in download (MD5 {md5}, expected {})",
                file.url, file.md5
            )));
        }
        std::fs::rename(&part, to).map_err(io(to))
    }

    /// Downloads an asset at a resolution (`1k`, `2k`, …) into the cache, unless it is
    /// there already: a model as glTF with its alpha maps, a texture as its colour,
    /// normal (OpenGL's convention), ARM and alpha maps. `progress` hears the bytes
    /// done and the bytes in all.
    pub fn download(
        &self,
        id: &str,
        kind: AssetType,
        res: &str,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Download, Error> {
        let files = self.files(id)?;
        let dir = self.dir.join("raw").join(id).join(res);
        let missing = |what: &str| {
            let there = files.resolutions(what).join(", ");
            Error::Invalid(format!("{id} has no {what} at {res} (there are: {there})"))
        };
        // Each file to fetch and where to.
        let mut jobs: Vec<(File, PathBuf)> = Vec::new();
        let mut maps = BTreeMap::new();
        let main = match kind {
            AssetType::Models => {
                let gltf = files
                    .get("gltf", res, "gltf")
                    .ok_or_else(|| missing("gltf"))?;
                let main = dir.join(gltf.name());
                for (rel, f) in &gltf.include {
                    jobs.push((f.clone(), dir.join(rel)));
                }
                // glTF has nowhere for a separate alpha map, so the files leave out the
                // alpha of leaves; it is in maps of their own, fetched beside the colour
                // maps they belong to.
                let textures = gltf
                    .include
                    .keys()
                    .find_map(|rel| {
                        Path::new(rel)
                            .parent()
                            .filter(|p| !p.as_os_str().is_empty())
                    })
                    .map_or(dir.clone(), |p| dir.join(p));
                for map in files.maps().filter(|m| m.to_lowercase().ends_with("alpha")) {
                    if let Some(f) = files
                        .get(map, res, "png")
                        .or_else(|| files.get(map, res, "jpg"))
                    {
                        let to = textures.join(f.name());
                        maps.insert(map.to_string(), to.clone());
                        jobs.push((f, to));
                    }
                }
                jobs.push((gltf.clone(), main.clone()));
                main
            }
            AssetType::Textures => {
                let diffuse = files
                    .get("Diffuse", res, "jpg")
                    .ok_or_else(|| missing("Diffuse"))?;
                let main = dir.join(diffuse.name());
                jobs.push((diffuse, main.clone()));
                for map in ["nor_gl", "arm", "Alpha"] {
                    if let Some(f) = files
                        .get(map, res, "png")
                        .filter(|_| map == "Alpha")
                        .or_else(|| files.get(map, res, "jpg"))
                    {
                        let to = dir.join(f.name());
                        maps.insert(map.to_string(), to.clone());
                        jobs.push((f, to));
                    }
                }
                main
            }
            AssetType::Hdris => {
                let hdri = files
                    .get("hdri", res, "hdr")
                    .ok_or_else(|| missing("hdri"))?;
                let main = dir.join(hdri.name());
                jobs.push((hdri, main.clone()));
                main
            }
        };
        let total: u64 = jobs
            .iter()
            .filter(|(_, to)| !to.is_file())
            .map(|(f, _)| f.size)
            .sum();
        let mut done = 0;
        progress(0, total);
        for (file, to) in &jobs {
            self.fetch(file, to, &mut |n| {
                done += n;
                progress(done, total);
            })?;
        }
        Ok(Download { dir, main, maps })
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    std::fs::write(path, bytes).map_err(io(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: &str = r#"{
        "gltf": {"1k": {"gltf": {"url": "https://dl.polyhaven.org/x/fern_1k.gltf", "md5": "a", "size": 10,
            "include": {"textures/fern_diff_1k.jpg": {"url": "https://dl.polyhaven.org/x/fern_diff_1k.jpg", "md5": "b", "size": 20}}}}},
        "Alpha": {"1k": {"png": {"url": "https://dl.polyhaven.org/x/fern_alpha_1k.png", "md5": "c", "size": 5}},
                  "2k": {"png": {"url": "https://dl.polyhaven.org/x/fern_alpha_2k.png", "md5": "d", "size": 9}}},
        "tonemapped": {"url": "https://dl.polyhaven.org/x/t.jpg", "md5": "e", "size": 1}
    }"#;

    #[test]
    fn reads_files_of_every_shape() {
        let files = Files(serde_json::from_str(FILES).unwrap());
        let gltf = files.get("gltf", "1k", "gltf").unwrap();
        assert_eq!(gltf.name(), "fern_1k.gltf");
        assert_eq!(gltf.total_size(), 30);
        assert_eq!(files.resolutions("Alpha"), ["1k", "2k"]);
        assert!(files.get("tonemapped", "1k", "jpg").is_none());
    }

    #[test]
    fn matches_every_word() {
        let a = Asset {
            id: "fir_sapling".into(),
            name: "Fir Sapling".into(),
            tags: vec!["coniferous".into()],
            categories: vec!["trees".into()],
            ..Default::default()
        };
        assert!(a.matches("fir tree"));
        assert!(a.matches("Conifer"));
        assert!(!a.matches("fir palm"));
    }
}
