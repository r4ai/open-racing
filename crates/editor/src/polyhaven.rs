//! Poly Haven's CC0 models and textures in the editor: a browser in the Assets panel
//! (search, categories, pictures), and Poly Haven's plants first on the scatter
//! palette's shelves. Choosing one downloads and prepares it in the background (see
//! `open_racing_track_project::polyhaven`), its progress shown on its tile, then paints
//! it as a scatter, places it or makes a material of it. Once prepared, an asset is
//! used from the cache without the network.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bevy_egui::egui;
use open_racing_track::texture::{self, Image};
use open_racing_track_project::ops::Op;
use open_racing_track_project::polyhaven::{
    self as haven, Asset, AssetType, Client, ReadyModel, ReadyTexture,
};
use open_racing_track_project::project::Kind as PlantKind;

use crate::commands::Ctx;
use crate::vegetation::{BrushPreset, Category, Kind};

/// Poly Haven's plants and rocks on the palette's shelves, before the built-in kinds.
pub const PLANTS: [(&str, Category); 16] = [
    ("fir_sapling", Category::Trees),
    ("pine_sapling_medium", Category::Trees),
    ("fir_tree_01", Category::Trees),
    ("pine_tree_01", Category::Trees),
    ("island_tree_01", Category::Trees),
    ("jacaranda_tree", Category::Trees),
    ("fern_02", Category::Bushes),
    ("nettle_plant", Category::Bushes),
    ("grass_medium_01", Category::Grass),
    ("grass_medium_02", Category::Grass),
    ("grass_bermuda_01", Category::Grass),
    ("dandelion_01", Category::Grass),
    ("boulder_01", Category::Rocks),
    ("rock_moss_set_01", Category::Rocks),
    ("dead_tree_trunk", Category::Other),
    ("dry_branches_medium_01", Category::Other),
];

/// Pictures fetched at once.
const AT_ONCE: usize = 4;
/// Tiles the browser shows at most.
const SHOWN: usize = 240;

/// A result a thread of its own is working out.
struct Pending<T>(Arc<Mutex<Option<T>>>);

impl<T: Send + 'static> Pending<T> {
    fn spawn(work: impl FnOnce() -> T + Send + 'static) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        std::thread::spawn(move || {
            let result = work();
            if let Ok(mut s) = out.lock() {
                *s = Some(result);
            }
        });
        Self(slot)
    }

    fn take(&self) -> Option<T> {
        self.0.lock().ok()?.take()
    }
}

/// What to do with an asset once it is ready.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Then {
    /// Paint it as a scatter, a kind of this shelf.
    Scatter(Category),
    /// Place a copy of it with a click in the view.
    Place,
    /// Add a material of it.
    Material,
}

enum Ready {
    Model(ReadyModel),
    Texture(ReadyTexture),
}

/// An asset being downloaded and prepared: bytes done and in all.
struct Job {
    then: Then,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    result: Pending<Result<Ready, String>>,
}

enum Catalog {
    Loading(Pending<Result<Vec<Asset>, String>>),
    Ready(Vec<Asset>),
    Failed(String),
}

/// Poly Haven's catalogue, pictures and downloads, and the browser's state.
#[derive(Default)]
pub struct PolyHaven {
    client: Option<Result<Client, String>>,
    catalogs: HashMap<AssetType, Catalog>,
    pictures: HashMap<String, Option<egui::TextureHandle>>,
    fetching: HashMap<String, Pending<Option<Image>>>,
    jobs: HashMap<String, Job>,
    /// The browser: textures rather than models, the words searched for, the category
    /// shown and the asset chosen.
    pub textures: bool,
    pub query: String,
    pub category: Option<String>,
    pub selected: Option<String>,
}

impl PolyHaven {
    fn client(&mut self) -> Result<Client, String> {
        self.client
            .get_or_insert_with(|| Client::open().map_err(|e| e.to_string()))
            .clone()
    }

    /// The assets of a type, asked for the first time they are wanted; `Err(None)`
    /// while they come.
    fn catalog(&mut self, kind: AssetType) -> Result<&[Asset], Option<String>> {
        if !self.catalogs.contains_key(&kind) {
            let client = self.client().map_err(Some)?;
            let pending = Pending::spawn(move || client.catalog(kind).map_err(|e| e.to_string()));
            self.catalogs.insert(kind, Catalog::Loading(pending));
        }
        let entry = self.catalogs.get_mut(&kind).expect("inserted above");
        if let Catalog::Loading(p) = entry
            && let Some(result) = p.take()
        {
            *entry = match result {
                Ok(list) => Catalog::Ready(list),
                Err(e) => Catalog::Failed(e),
            };
        }
        match entry {
            Catalog::Ready(list) => Ok(list),
            Catalog::Loading(_) => Err(None),
            Catalog::Failed(e) => Err(Some(e.clone())),
        }
    }

    /// An asset's name as the catalogue has it, or made from its id.
    pub fn name(&self, id: &str) -> String {
        self.catalogs
            .values()
            .find_map(|c| match c {
                Catalog::Ready(list) => list.iter().find(|a| a.id == id),
                _ => None,
            })
            .map_or_else(|| id.replace('_', " "), |a| a.name.clone())
    }

    /// An asset's picture, fetched in the background the first time it is asked for.
    pub fn picture(&mut self, ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.pictures.get(id) {
            return t.clone();
        }
        if let Some(p) = self.fetching.get(id) {
            let image = p.take()?;
            self.fetching.remove(id);
            let texture = image.map(|img| {
                let colour =
                    egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.pixels);
                ctx.load_texture(
                    format!("polyhaven {id}"),
                    colour,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.pictures.insert(id.to_string(), texture.clone());
            return texture;
        }
        if self.fetching.len() >= AT_ONCE {
            return None;
        }
        let client = self.client().ok()?;
        let asset = Asset {
            id: id.to_string(),
            ..Default::default()
        };
        let pending = Pending::spawn(move || {
            let path = client.thumbnail(&asset).ok()?;
            texture::decode(&std::fs::read(path).ok()?).ok()
        });
        self.fetching.insert(id.to_string(), pending);
        None
    }

    /// How far an asset's download has got, 0 to 1, while it is being got.
    pub fn progress(&self, id: &str) -> Option<f32> {
        let job = self.jobs.get(id)?;
        let total = job.total.load(Ordering::Relaxed);
        Some(if total == 0 {
            0.0
        } else {
            job.done.load(Ordering::Relaxed) as f32 / total as f32
        })
    }

    /// Whether a model is prepared already, so that using it takes no download.
    pub fn is_ready(&mut self, id: &str) -> bool {
        self.client()
            .is_ok_and(|c| haven::is_ready(&c, id, haven::RES))
    }

    /// Gets an asset in the background, and does `then` with it when it is ready.
    pub fn get(&mut self, id: &str, kind: AssetType, then: Then) -> Result<(), String> {
        if self.jobs.contains_key(id) {
            return Ok(());
        }
        let client = self.client()?;
        let (done, total) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let (d, t, id_owned) = (done.clone(), total.clone(), id.to_string());
        let result = Pending::spawn(move || {
            let mut progress = |a: u64, b: u64| {
                d.store(a, Ordering::Relaxed);
                t.store(b, Ordering::Relaxed);
            };
            match kind {
                AssetType::Textures => {
                    haven::texture(&client, &id_owned, haven::RES, &mut progress)
                        .map(Ready::Texture)
                }
                _ => haven::model(&client, &id_owned, haven::RES, &mut progress).map(Ready::Model),
            }
            .map_err(|e| e.to_string())
        });
        self.jobs.insert(
            id.to_string(),
            Job {
                then,
                done,
                total,
                result,
            },
        );
        Ok(())
    }

    /// Does what was asked with each asset that became ready.
    pub fn finish(&mut self, c: &mut Ctx) {
        let done: Vec<(String, Then, Result<Ready, String>)> = self
            .jobs
            .iter()
            .filter_map(|(id, job)| Some((id.clone(), job.then, job.result.take()?)))
            .collect();
        for (id, then, result) in done {
            self.jobs.remove(&id);
            match result {
                Ok(ready) => use_ready(c, ready, then),
                Err(e) => c.editor.status = format!("{id} from Poly Haven: {e}"),
            }
        }
    }
}

/// Uses an asset that is ready as asked.
fn use_ready(c: &mut Ctx, ready: Ready, then: Then) {
    let result = match (ready, then) {
        (Ready::Model(m), Then::Scatter(category)) => {
            // The scatter refers to the prepared files in the cache; adding it brings
            // them into the project.
            let paths: Vec<PathBuf> = m.models.iter().map(|x| x.path.clone()).collect();
            let kind = Kind {
                name: m.name.clone(),
                category,
                scatter: haven::scatter(&m.name, &m, &paths),
                materials: vec![],
                brush: brush(m.kind),
            };
            crate::brush::add_scatter(c, &kind, None);
            Ok(())
        }
        (Ready::Model(m), _) => haven::import_model(&c.editor.dir, &m).map(|paths| {
            if let Some(first) = paths.first() {
                c.tool.place = Some(first.clone());
                c.editor.status = format!(
                    "{}: click in the view to place it ({} of its variants are in the Assets panel)",
                    m.name,
                    paths.len()
                );
            }
        }),
        (Ready::Texture(t), _) => {
            let name = crate::presets::free_name(&t.id, |n| {
                c.editor.project.material_index(n).is_some()
            });
            haven::import_texture(&c.editor.dir, &t, &name).map(|material| {
                if c.editor.apply(vec![Op::PutMaterial { material }], None) {
                    c.editor.status = format!("added material \"{name}\" of {}", t.name);
                }
            })
        }
    };
    if let Err(e) = result {
        c.editor.status = e.to_string();
    }
}

/// The brush to paint a kind of plant with.
fn brush(kind: PlantKind) -> BrushPreset {
    let (radius, strength, hardness) = match kind {
        PlantKind::Grass => (8.0, 0.8, 0.3),
        PlantKind::Rigid => (12.0, 0.5, 0.8),
        _ => (25.0, 0.7, 0.8),
    };
    BrushPreset {
        radius,
        strength,
        hardness,
    }
}

/// How far a download has got, or that it is done and the asset being prepared.
fn label(progress: f32) -> String {
    if progress >= 1.0 {
        "Preparing…".into()
    } else {
        format!("{:.0}%", progress * 100.0)
    }
}

/// Draws how far a tile's download has got over it, or a mark that it needs one.
pub fn badge(ui: &egui::Ui, rect: egui::Rect, progress: Option<f32>, ready: bool) {
    let painter = ui.painter_at(rect);
    let corner = rect.right_top() + egui::vec2(-12.0, 12.0);
    match progress {
        Some(p) => {
            let bar = egui::Rect::from_min_size(
                rect.left_bottom() + egui::vec2(4.0, -26.0),
                egui::vec2((rect.width() - 8.0) * p.clamp(0.02, 1.0), 4.0),
            );
            painter.rect_filled(bar, 2.0, crate::theme::SELECTED_UI);
            painter.text(
                rect.center() - egui::vec2(0.0, 8.0),
                egui::Align2::CENTER_CENTER,
                label(p),
                egui::FontId::proportional(13.0),
                egui::Color32::WHITE,
            );
        }
        None if !ready => {
            painter.circle_filled(corner, 8.0, egui::Color32::from_black_alpha(160));
            painter.text(
                corner,
                egui::Align2::CENTER_CENTER,
                "⬇",
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
        }
        None => {}
    }
}

/// The browser, in the Assets panel.
pub fn browser(ui: &mut egui::Ui, ph: &mut PolyHaven, status: &mut String) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut ph.textures, false, "Models");
        ui.selectable_value(&mut ph.textures, true, "Textures");
        ui.separator();
        ui.add(
            egui::TextEdit::singleline(&mut ph.query)
                .hint_text("Search: fir, grass, rock, asphalt…")
                .desired_width(220.0),
        );
    });
    let kind = if ph.textures {
        AssetType::Textures
    } else {
        AssetType::Models
    };
    let list: Vec<Asset> = match ph.catalog(kind) {
        Ok(list) => list.to_vec(),
        Err(None) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading Poly Haven's catalogue…");
            });
            return;
        }
        Err(Some(e)) => {
            ui.colored_label(egui::Color32::from_rgb(255, 110, 90), e);
            return;
        }
    };
    // The categories, most assets first.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for a in &list {
        for c in a.categories.iter().filter(|c| !c.starts_with("collection")) {
            *counts.entry(c.as_str()).or_default() += 1;
        }
    }
    let mut categories: Vec<(&str, usize)> = counts.into_iter().collect();
    categories.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut ph.category, None, "All");
        for (c, n) in categories.iter().take(16) {
            ui.selectable_value(&mut ph.category, Some(c.to_string()), format!("{c} {n}"));
        }
    });
    let shown: Vec<&Asset> = list
        .iter()
        .filter(|a| a.matches(&ph.query))
        .filter(|a| ph.category.as_ref().is_none_or(|c| a.is_in(c)))
        .collect();
    if let Some(id) = ph.selected.clone()
        && let Some(a) = list.iter().find(|a| a.id == id)
    {
        details(ui, ph, a, kind, status);
    }
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        for a in shown.iter().take(SHOWN) {
            let picture = ph.picture(ui.ctx(), &a.id);
            let selected = ph.selected.as_deref() == Some(a.id.as_str());
            let r = crate::palette::tile(ui, picture, &a.name, selected);
            badge(ui, r.rect, ph.progress(&a.id), true);
            if r.clicked() {
                ph.selected = Some(a.id.clone());
            }
        }
    });
    if shown.len() > SHOWN {
        ui.weak(format!(
            "and {} more: search to narrow them down",
            shown.len() - SHOWN
        ));
    }
    if shown.is_empty() {
        ui.weak("Nothing matches.");
    }
    ui.separator();
    ui.hyperlink_to(haven::CREDIT, "https://polyhaven.com");
}

/// The chosen asset: what it is, who made it, and what to do with it.
fn details(ui: &mut egui::Ui, ph: &mut PolyHaven, a: &Asset, kind: AssetType, status: &mut String) {
    ui.separator();
    ui.horizontal(|ui| {
        ui.strong(&a.name);
        ui.hyperlink_to("polyhaven.com", a.page());
    });
    let authors: Vec<&str> = a.authors.keys().map(String::as_str).collect();
    let mut info = format!("by {}", authors.join(", "));
    if let Some(t) = a.polycount {
        info += &format!(", {t} triangles as made (fewer once prepared)");
    }
    ui.weak(info);
    let mut ask = |ph: &mut PolyHaven, then: Then| {
        if let Err(e) = ph.get(&a.id, kind, then) {
            *status = e;
        } else {
            *status = format!("getting {} from Poly Haven…", a.name);
        }
    };
    ui.horizontal(|ui| {
        if let Some(p) = ph.progress(&a.id) {
            ui.add(
                egui::ProgressBar::new(p)
                    .desired_width(160.0)
                    .text(label(p))
                    .animate(p >= 1.0),
            );
            return;
        }
        match kind {
            AssetType::Textures => {
                if ui
                    .button("Add as a material")
                    .on_hover_text("Its colour and normal maps, tiled at its real size")
                    .clicked()
                {
                    ask(ph, Then::Material);
                }
            }
            _ => {
                let category = category_of(haven::kind_of(a));
                if ui
                    .button("Scatter")
                    .on_hover_text("Paint copies of it over the ground, its variants mixed")
                    .clicked()
                {
                    ask(ph, Then::Scatter(category));
                }
                if ui
                    .button("Place")
                    .on_hover_text("Click in the view to place a copy")
                    .clicked()
                {
                    ask(ph, Then::Place);
                }
            }
        }
    });
}

/// The palette's shelf for a kind of plant.
pub fn category_of(kind: PlantKind) -> Category {
    match kind {
        PlantKind::Evergreen | PlantKind::Deciduous => Category::Trees,
        PlantKind::Grass => Category::Grass,
        PlantKind::Rigid => Category::Rocks,
        PlantKind::Crowd => Category::People,
    }
}

/// Poly Haven's plants of a shelf of the palette: a click gets one, if need be, and
/// paints it.
pub fn shelf(ui: &mut egui::Ui, c: &mut Ctx, ph: &mut PolyHaven, category: Category) {
    for (id, _) in PLANTS.iter().filter(|(_, cat)| *cat == category) {
        let picture = ph.picture(ui.ctx(), id);
        let name = ph.name(id);
        let r = crate::palette::tile(ui, picture, &name, false);
        let ready = ph.is_ready(id);
        badge(ui, r.rect, ph.progress(id), ready);
        let r = r.on_hover_text(if ready {
            format!("{name}, from Poly Haven (CC0)")
        } else {
            format!("{name}, from Poly Haven (CC0): a click downloads and prepares it once")
        });
        if r.clicked() {
            match ph.get(id, AssetType::Models, Then::Scatter(category)) {
                Ok(()) if !ready => {
                    c.editor.status = format!("getting {name} from Poly Haven…");
                }
                Ok(()) => {}
                Err(e) => c.editor.status = e,
            }
        }
    }
}
