//! The palette under the view while the Scatter tool is on, as a city builder's asset
//! menu: shelves of kinds of vegetation (trees, bushes, grass, rocks and others, the
//! user's own, and the project's scatters), each with a picture of its model; a click
//! paints it. Pictures of models are made in the background, for the Assets panel too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bevy_egui::egui;
use open_racing_track::texture::Image;
use open_racing_track_project::Project;
use open_racing_track_project::project::ScatterModel;

use crate::commands::Ctx;
use crate::plants::ScatterMode;
use crate::preview::SharedCache;
use crate::theme;
use crate::vegetation::{self, Category, Kind};
use crate::viewport::ToolKind;

/// Edge of the pictures, pixels.
const PICTURE: usize = 96;
/// Edge of a tile's picture on screen, logical pixels.
const TILE: f32 = 64.0;
/// Pictures made at once in the background.
const AT_ONCE: usize = 3;

/// Which shelf the palette shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shelf {
    /// The project's scatters.
    #[default]
    Project,
    Kind(Category),
    /// The user's own kinds, from the library.
    Mine,
}

/// Pictures of models, made once each in the background.
#[derive(Default)]
pub struct Pictures {
    made: HashMap<String, Option<egui::TextureHandle>>,
    making: HashMap<String, Task<Option<Image>>>,
}

#[derive(Resource)]
pub struct Palette {
    pub shelf: Shelf,
    /// The user's kinds as last read, or why they could not be.
    mine: Option<Result<Vec<Kind>, String>>,
    pictures: Pictures,
    cache: SharedCache,
    /// The asset files' revision the pictures of project files were made at.
    assets: u64,
    /// Poly Haven's plants on the shelves, and the Assets panel's browser.
    pub polyhaven: crate::polyhaven::PolyHaven,
}

impl FromWorld for Palette {
    fn from_world(world: &mut World) -> Self {
        Self {
            shelf: Shelf::default(),
            mine: None,
            pictures: Pictures::default(),
            cache: world.resource::<SharedCache>().clone(),
            assets: 0,
            polyhaven: Default::default(),
        }
    }
}

impl Palette {
    /// The user's kinds, read the first time.
    fn mine(&mut self) -> &Result<Vec<Kind>, String> {
        self.mine
            .get_or_insert_with(|| match vegetation::library_dir() {
                Some(dir) => vegetation::load(&dir),
                None => Ok(vec![]),
            })
    }

    /// Reads the user's library again (after a kind was saved or deleted).
    pub fn reload(&mut self) {
        self.mine = None;
    }

    /// Forgets the pictures of files that changed.
    pub fn assets_changed(&mut self, revision: u64) {
        if self.assets != revision {
            self.assets = revision;
            self.pictures
                .made
                .retain(|k, _| k.contains(open_racing_track_project::shapes::PREFIX));
        }
    }

    /// A picture of a model of `dir` (with the project's materials for its own where
    /// it says, given `project`), made in the background the first time it is asked
    /// for.
    pub fn picture(
        &mut self,
        ctx: &egui::Context,
        dir: &Path,
        m: &ScatterModel,
        project: Option<&Project>,
    ) -> Option<egui::TextureHandle> {
        let key = format!("{}|{}|{:?}", dir.display(), m.model.display(), m.materials);
        if let Some(t) = self.pictures.made.get(&key) {
            return t.clone();
        }
        if let Some(task) = self.pictures.making.get_mut(&key) {
            let image = check_ready(task)?;
            self.pictures.making.remove(&key);
            let texture = image.map(|img| {
                let colour =
                    egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.pixels);
                ctx.load_texture(&key, colour, egui::TextureOptions::LINEAR)
            });
            self.pictures.made.insert(key, texture.clone());
            return texture;
        }
        if self.pictures.making.len() >= AT_ONCE {
            return None;
        }
        let (cache, dir, m) = (self.cache.0.clone(), dir.to_path_buf(), m.clone());
        let project = project.filter(|_| !m.materials.is_empty()).cloned();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let model = cache.lock().ok()?.model(&dir, &m.model).ok()?;
            let paints = match &project {
                Some(p) => cache.lock().ok()?.paints(p, &dir, &model, &m).ok()?,
                None => open_racing_track_project::impostor::paints(&model),
            };
            Some(open_racing_track_project::impostor::thumbnail(
                &model, &paints, PICTURE,
            ))
        });
        self.pictures.making.insert(key, task);
        None
    }
}

/// A tile of the palette: the picture, the name under it. Returns its response.
pub(crate) fn tile(
    ui: &mut egui::Ui,
    picture: Option<egui::TextureHandle>,
    name: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(TILE + 8.0, TILE + 22.0), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let fill = if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().extreme_bg_color
    };
    painter.rect_filled(rect, 4.0, fill);
    let image = egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(TILE, TILE));
    match picture {
        Some(t) => {
            painter.image(
                t.id(),
                image,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.text(
                image.center(),
                egui::Align2::CENTER_CENTER,
                "…",
                egui::FontId::proportional(18.0),
                ui.visuals().weak_text_color(),
            );
        }
    }
    let mut label = name.to_string();
    if label.chars().count() > 11 {
        label = label.chars().take(10).collect::<String>() + "…";
    }
    painter.text(
        egui::pos2(rect.center().x, rect.max.y - 9.0),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(11.0),
        ui.visuals().text_color(),
    );
    if selected {
        painter.rect_stroke(
            rect.shrink(1.0),
            4.0,
            egui::Stroke::new(2.0, theme::SELECTED_UI),
            egui::StrokeKind::Inside,
        );
    }
    response.on_hover_text(name)
}

/// The palette, over the bottom of the view while the Scatter tool is on.
pub fn show(root: &mut egui::Ui, c: &mut Ctx, palette: &mut Palette) {
    if c.tool.active != ToolKind::Scatter {
        return;
    }
    egui::Panel::bottom("scatter palette").show(root, |ui| {
        ui.horizontal(|ui| {
            let shelves = std::iter::once((Shelf::Project, "In this project"))
                .chain(Category::ALL.map(|c| (Shelf::Kind(c), c.label())))
                .chain(std::iter::once((Shelf::Mine, "★ Mine")));
            for (shelf, label) in shelves {
                ui.selectable_value(&mut palette.shelf, shelf, label);
            }
            ui.separator();
            if ui
                .small_button("From a model file…")
                .on_hover_text("A scatter of a glTF model of your own (copied into the project's assets)")
                .clicked()
                && let Some(file) = rfd::FileDialog::new()
                    .add_filter("glTF models", &["glb", "gltf"])
                    .pick_file()
            {
                match open_racing_track_project::assets::import_once(&c.editor.dir, &file) {
                    Ok(path) => scatter_of_model(c, &path),
                    Err(e) => c.editor.status = e.to_string(),
                }
                palette.shelf = Shelf::Project;
            }
        });
        egui::ScrollArea::horizontal()
            .id_salt("palette tiles")
            .show(ui, |ui| {
                ui.horizontal(|ui| match palette.shelf {
                    Shelf::Project => project_shelf(ui, c, palette),
                    Shelf::Kind(category) => {
                        let kinds: Vec<Kind> = vegetation::builtin()
                            .into_iter()
                            .filter(|k| k.category == category)
                            .collect();
                        let mine: Vec<Kind> = palette
                            .mine()
                            .as_ref()
                            .map(|m| m.iter().filter(|k| k.category == category).cloned().collect())
                            .unwrap_or_default();
                        crate::polyhaven::shelf(ui, c, &mut palette.polyhaven, category);
                        ui.separator();
                        kinds_shelf(ui, c, palette, &kinds, None);
                        if !mine.is_empty() {
                            ui.separator();
                            let library = vegetation::library_dir();
                            kinds_shelf(ui, c, palette, &mine, library.as_deref());
                        }
                    }
                    Shelf::Mine => match palette.mine().clone() {
                        Ok(kinds) if kinds.is_empty() => {
                            ui.weak("None yet: a scatter's \"Save to my library…\" (the Scatter tab, or a right click on its tile) keeps it here, with its models and materials, for any project.");
                        }
                        Ok(kinds) => {
                            let library = vegetation::library_dir();
                            kinds_shelf(ui, c, palette, &kinds, library.as_deref());
                        }
                        Err(e) => {
                            ui.colored_label(egui::Color32::from_rgb(255, 110, 90), e);
                        }
                    },
                });
            });
    });
}

/// The project's scatters: a click paints one.
fn project_shelf(ui: &mut egui::Ui, c: &mut Ctx, palette: &mut Palette) {
    let project = c.editor.project.clone();
    if project.scatter.is_empty() {
        ui.weak("No scatters yet: pick a kind from a shelf to paint it.");
    }
    for s in &project.scatter {
        let picture = s
            .models
            .first()
            .and_then(|m| palette.picture(ui.ctx(), &c.editor.dir, m, Some(&project)));
        let on = c.tool.brush.scatter.as_deref() == Some(s.name.as_str());
        let r = tile(ui, picture, &s.name, on);
        if r.clicked() {
            c.tool.brush.scatter = Some(s.name.clone());
            if c.tool.brush.mode == ScatterMode::Select {
                c.tool.brush.mode = ScatterMode::Paint;
            }
        }
        r.context_menu(|ui| {
            ui.menu_button("Save to my library as", |ui| {
                for category in Category::ALL {
                    if ui.button(category.label()).clicked() {
                        save(c, palette, &s.name, category);
                        ui.close();
                    }
                }
            });
            if ui.button("Select its copies").clicked() {
                c.tool.brush.scatter = Some(s.name.clone());
                c.tool.brush.mode = ScatterMode::Select;
                ui.close();
            }
            if ui.button("Remove").clicked() {
                c.editor.apply(
                    vec![open_racing_track_project::ops::Op::RemoveScatter {
                        name: s.name.clone(),
                    }],
                    None,
                );
                ui.close();
            }
        });
    }
}

/// Kinds of vegetation: a click adds a scatter of one to the project and paints it.
/// `library` holds the files of the user's own kinds.
fn kinds_shelf(
    ui: &mut egui::Ui,
    c: &mut Ctx,
    palette: &mut Palette,
    kinds: &[Kind],
    library: Option<&Path>,
) {
    let dir = library.map_or_else(|| c.editor.dir.clone(), Path::to_path_buf);
    for kind in kinds {
        let picture = kind
            .scatter
            .models
            .first()
            .and_then(|m| palette.picture(ui.ctx(), &dir, m, None));
        let r = tile(ui, picture, &kind.name, false);
        if r.clicked() {
            crate::brush::add_scatter(c, kind, library);
            palette.shelf = Shelf::Project;
        }
        if let Some(library) = library {
            r.context_menu(|ui| {
                if ui.button("Delete from my library").clicked() {
                    if let Err(e) = vegetation::delete(library, &kind.name) {
                        c.editor.status = e;
                    }
                    palette.reload();
                    ui.close();
                }
            });
        }
    }
}

/// Saves a scatter of the project to the user's library, with the brush as it is.
fn save(c: &mut Ctx, palette: &mut Palette, scatter: &str, category: Category) {
    let size = c.tool.brush.size(ToolKind::Scatter);
    let brush = vegetation::BrushPreset {
        radius: size.radius,
        strength: size.strength,
        hardness: size.hardness,
    };
    let result = vegetation::library_dir()
        .ok_or_else(|| "no folder for the library".to_string())
        .and_then(|dir| vegetation::save(c.editor, scatter, scatter, category, brush, &dir));
    c.editor.status = match result {
        Ok(()) => format!("saved \"{scatter}\" to your library"),
        Err(e) => format!("not saved: {e}"),
    };
    palette.reload();
}

/// Adds a scatter of one model of the project's (a tree of the user's, say), shaped
/// as a wood, and paints it.
pub fn scatter_of_model(c: &mut Ctx, model: &Path) {
    let mut kind = vegetation::builtin().remove(2);
    kind.name = crate::assets::model_name(model);
    kind.scatter.models = vec![ScatterModel::new(PathBuf::from(model), 1.0)];
    crate::brush::add_scatter(c, &kind, None);
}
