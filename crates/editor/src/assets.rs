//! The project's textures and models: a list kept up to date with the files under
//! `assets/`, importing (file dialog or dropping files on the window), and the Assets
//! panel to see what uses what, make materials, place models, rename and delete.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use bevy::prelude::*;
use bevy::window::FileDragAndDrop;
use bevy_egui::egui;
use open_racing_track::texture;
use open_racing_track_project::assets::{self, Asset, Kind};
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::{Alpha, MaterialDef, Prop, TextureSource};

use crate::preview::Props;
use crate::state::{Editor, Item};
use crate::viewport::Tool;

/// Edge of the thumbnails, pixels.
const THUMB: usize = 48;

/// The asset files as last seen.
#[derive(Resource, Default)]
pub struct Library {
    pub list: Vec<Asset>,
    /// Bumped whenever a file is added, removed or changed, so that what shows it loads
    /// it again.
    pub revision: u64,
    /// Paths, sizes and times the list was made from.
    signature: Vec<(PathBuf, Option<u64>, Option<SystemTime>)>,
    last: Option<Instant>,
    dir: PathBuf,
}

impl Library {
    pub fn textures(&self) -> impl Iterator<Item = &Asset> {
        self.list.iter().filter(|a| a.kind == Kind::Texture)
    }

    pub fn models(&self) -> impl Iterator<Item = &Asset> {
        self.list.iter().filter(|a| a.kind == Kind::Model)
    }

    /// Looks at the files again now.
    pub fn refresh(&mut self) {
        self.last = None;
    }
}

/// Lists the files again every second, or at once after `refresh`.
pub fn watch(editor: Res<Editor>, mut library: ResMut<Library>) {
    let due = library
        .last
        .is_none_or(|t| t.elapsed() > Duration::from_secs(1));
    if !due && library.dir == editor.dir {
        return;
    }
    library.last = Some(Instant::now());
    library.dir = editor.dir.clone();
    let list = assets::list(&editor.project, &editor.dir);
    let signature: Vec<_> = list
        .iter()
        .map(|a| {
            let time = std::fs::metadata(editor.dir.join(&a.path))
                .and_then(|m| m.modified())
                .ok();
            (a.path.clone(), a.bytes, time)
        })
        .collect();
    if signature != library.signature {
        library.signature = signature;
        library.revision += 1;
    }
    library.list = list;
}

/// Imports files dropped on the window; a model dropped on the 3D view is placed where
/// it lands.
pub fn dropped(
    mut events: MessageReader<FileDragAndDrop>,
    mut editor: ResMut<Editor>,
    mut library: ResMut<Library>,
    tool: Res<Tool>,
) {
    for e in events.read() {
        let FileDragAndDrop::DroppedFile { path_buf, .. } = e else {
            continue;
        };
        match assets::import(&editor.dir, path_buf) {
            Ok(rel) => {
                library.refresh();
                editor.status = format!("imported {}", rel.display());
                if Kind::of(&rel) == Some(Kind::Model)
                    && let Some(at) = tool.pointer
                {
                    place(&mut editor, &rel, at);
                }
            }
            Err(e) => editor.status = e.to_string(),
        }
    }
}

/// Places a model as a new prop standing on the ground at `at`, and selects it.
pub fn place(editor: &mut Editor, model: &Path, at: glam::DVec3) {
    let stem = model
        .file_stem()
        .map_or("prop".into(), |s| s.to_string_lossy().into_owned());
    let name = (1..)
        .map(|i| {
            if i == 1 {
                stem.clone()
            } else {
                format!("{stem}.{i:03}")
            }
        })
        .find(|n| editor.project.props.iter().all(|p| &p.name != n))
        .expect("some name is free");
    let prop = Prop {
        name,
        model: model.to_path_buf(),
        pos: at,
        yaw: 0.0,
        scale: 1.0,
        drape: true,
        collide: false,
    };
    if editor.apply(vec![Op::PutProp { prop }], None) {
        editor
            .selection
            .select(Item::Prop(editor.project.props.len() - 1));
    }
}

/// Thumbnails of textures, made again when their files change.
#[derive(Default)]
pub struct Thumbnails {
    made: HashMap<PathBuf, (u64, Option<egui::TextureHandle>)>,
}

impl Thumbnails {
    fn get(
        &mut self,
        ctx: &egui::Context,
        dir: &Path,
        a: &Asset,
        revision: u64,
    ) -> Option<egui::TextureHandle> {
        if let Some((r, t)) = self.made.get(&a.path)
            && *r == revision
        {
            return t.clone();
        }
        let image = std::fs::read(dir.join(&a.path))
            .ok()
            .and_then(|b| texture::decode(&b).ok());
        let handle = image.map(|img| {
            // Nearest texels of a small square.
            let mut px = Vec::with_capacity(THUMB * THUMB * 4);
            for y in 0..THUMB {
                for x in 0..THUMB {
                    let (sx, sy) = (x * img.width / THUMB, y * img.height / THUMB);
                    px.extend_from_slice(&img.pixels[(sy * img.width + sx) * 4..][..4]);
                }
            }
            let color = egui::ColorImage::from_rgba_unmultiplied([THUMB, THUMB], &px);
            ctx.load_texture(
                a.path.to_string_lossy(),
                color,
                egui::TextureOptions::LINEAR,
            )
        });
        self.made.insert(a.path.clone(), (revision, handle.clone()));
        handle
    }
}

/// State of the Assets panel.
#[derive(Default)]
pub struct Panel {
    thumbnails: Thumbnails,
    /// Asset being renamed, with the new name so far.
    renaming: Option<(PathBuf, String)>,
    /// Asset whose delete button was pressed once.
    deleting: Option<PathBuf>,
}

fn size(bytes: Option<u64>) -> String {
    match bytes {
        None => "missing".into(),
        Some(b) if b >= 1 << 20 => format!("{:.1} MiB", b as f64 / (1 << 20) as f64),
        Some(b) => format!("{} KiB", b.div_ceil(1024)),
    }
}

pub fn panel(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    library: &mut Library,
    props: &Props,
    tool: &mut Tool,
    state: &mut Panel,
) {
    ui.heading("Assets");
    ui.horizontal(|ui| {
        if ui.button("Import…").clicked()
            && let Some(files) = rfd::FileDialog::new()
                .add_filter("textures and models", &["png", "dds", "glb", "gltf"])
                .pick_files()
        {
            for f in files {
                match assets::import(&editor.dir, &f) {
                    Ok(rel) => editor.status = format!("imported {}", rel.display()),
                    Err(e) => editor.status = e.to_string(),
                }
            }
            library.refresh();
        }
        ui.weak("or drop files on the window");
    });
    ui.small(format!(
        "Kept in {}/assets. Textures: PNG or DDS; models: glTF (.glb, .gltf).",
        editor
            .dir
            .file_name()
            .map_or("".into(), |n| n.to_string_lossy())
    ));

    let list = library.list.clone();
    let revision = library.revision;
    for (kind, title) in [(Kind::Texture, "Textures"), (Kind::Model, "Models")] {
        ui.separator();
        ui.strong(title);
        let mut any = false;
        for a in list.iter().filter(|a| a.kind == kind) {
            any = true;
            ui.horizontal(|ui| {
                match kind {
                    Kind::Texture => match state.thumbnails.get(ui.ctx(), &editor.dir, a, revision)
                    {
                        Some(t) => {
                            ui.image((t.id(), egui::vec2(THUMB as f32, THUMB as f32)));
                        }
                        None => {
                            ui.add_sized([THUMB as f32, THUMB as f32], egui::Label::new("?"));
                        }
                    },
                    Kind::Model => {
                        ui.add_sized([THUMB as f32, 20.0], egui::Label::new("🗋"));
                    }
                }
                ui.vertical(|ui| asset_row(ui, editor, library, props, tool, state, a));
            });
        }
        if !any {
            ui.weak("none yet");
        }
    }
}

fn asset_row(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    library: &mut Library,
    props: &Props,
    tool: &mut Tool,
    state: &mut Panel,
    a: &Asset,
) {
    let name = a.path.to_string_lossy().into_owned();
    ui.label(egui::RichText::new(&name).strong());
    let mut info = size(a.bytes);
    if let Some(t) = props.triangles.get(&a.path) {
        info += &format!(", {t} triangles");
    }
    if let Some(e) = props.failed.get(&a.path) {
        ui.colored_label(egui::Color32::from_rgb(255, 110, 90), e);
    }
    ui.horizontal(|ui| {
        ui.weak(info);
        if a.used_by.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(230, 160, 60), "unused");
        } else {
            ui.label(format!("used by {}", a.used_by.join(", ")));
        }
    });
    if a.bytes.is_none() {
        ui.colored_label(egui::Color32::from_rgb(255, 110, 90), "the file is missing");
        return;
    }

    // Renaming.
    if let Some((path, text)) = &mut state.renaming
        && *path == a.path
    {
        let (mut done, mut cancel) = (false, false);
        ui.horizontal(|ui| {
            let r = ui.text_edit_singleline(text);
            done = ui.button("Rename").clicked()
                || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            cancel = ui.button("Cancel").clicked();
        });
        if cancel {
            state.renaming = None;
        } else if done && let Some((from, text)) = state.renaming.take() {
            rename(editor, library, &from, &text);
        }
        return;
    }

    ui.horizontal(|ui| {
        match a.kind {
            Kind::Texture => {
                if ui
                    .small_button("New material")
                    .on_hover_text("A material with this texture")
                    .clicked()
                {
                    new_material(editor, &a.path);
                }
            }
            Kind::Model => {
                if ui
                    .small_button("Place")
                    .on_hover_text("Click in the view to place it (Esc cancels)")
                    .clicked()
                {
                    tool.place = Some(a.path.clone());
                }
            }
        }
        if ui.small_button("Rename").clicked() {
            let file = a
                .path
                .file_name()
                .map_or(String::new(), |n| n.to_string_lossy().into_owned());
            state.renaming = Some((a.path.clone(), file));
        }
        if a.used_by.is_empty() {
            let armed = state.deleting.as_ref() == Some(&a.path);
            let label = if armed { "Really delete?" } else { "Delete" };
            if ui.small_button(label).clicked() {
                if armed {
                    state.deleting = None;
                    match std::fs::remove_file(editor.dir.join(&a.path)) {
                        Ok(()) => editor.status = format!("deleted {}", a.path.display()),
                        Err(e) => editor.status = format!("{}: {e}", a.path.display()),
                    }
                    library.refresh();
                } else {
                    state.deleting = Some(a.path.clone());
                }
            }
        }
    });
}

/// Renames an asset's file within its folder and points what used it at the new name.
fn rename(editor: &mut Editor, library: &mut Library, from: &Path, name: &str) {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) {
        editor.status = "a file name, without folders".into();
        return;
    }
    let to = from.with_file_name(name);
    if Kind::of(&to) != Kind::of(from) {
        editor.status = "keep the file's extension".into();
        return;
    }
    let (full_from, full_to) = (editor.dir.join(from), editor.dir.join(&to));
    if full_to.exists() {
        editor.status = format!("{} exists", to.display());
        return;
    }
    if let Err(e) = std::fs::rename(&full_from, &full_to) {
        editor.status = format!("{}: {e}", from.display());
        return;
    }
    let ops = assets::repoint(&editor.project, from, &to);
    if !ops.is_empty() {
        editor.apply(ops, None);
    }
    editor.status = format!("renamed to {}", to.display());
    library.refresh();
}

/// A material using a texture, named after its file, and shown in the inspector.
fn new_material(editor: &mut Editor, texture: &Path) {
    let stem = texture
        .file_stem()
        .map_or("material".into(), |s| s.to_string_lossy().into_owned());
    let name = (1..)
        .map(|i| {
            if i == 1 {
                stem.clone()
            } else {
                format!("{stem} {i}")
            }
        })
        .find(|n| editor.project.material_index(n).is_none())
        .expect("some name is free");
    let material = MaterialDef {
        name: name.clone(),
        color: [1.0; 3],
        texture: TextureSource::File(texture.to_path_buf()),
        tile: [2.0, 2.0],
        roughness: 0.8,
        reflectance: 0.5,
        double_sided: false,
        normal: TextureSource::None,
        alpha: Alpha::Opaque,
    };
    if editor.apply(vec![Op::PutMaterial { material }], None) {
        editor.status = format!("added material \"{name}\"");
    }
}
