//! The 3D preview: the project's meshes, rebuilt in the background whenever the project
//! changes, in the same materials the game renders them with, and its props, placed
//! from the project every frame so that they follow edits at once. Scatters' copies are
//! drawn by instancing as the game draws them, each copy an entity that is moved in
//! place when only it changed.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::image::CompressedImageFormatSupport;
use bevy::light::NotShadowCaster;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use open_racing_sim::GroundMesh;
use open_racing_track::{Instance, Level};
use open_racing_track_project::corners::{self, Corner};
use open_racing_track_project::curve::Sampled;
use open_racing_track_project::inspect::{self, Issue};
use open_racing_track_project::model::{Model, Placement};
use open_racing_track_project::project::{Kind, MaterialDef};
use open_racing_track_project::road::MeshData;
use open_racing_track_project::terrain::{PaintMask, TerrainBuild};
use open_racing_track_project::{Cache, Project, bake};
use open_racing_track_render::{self as render, ShapePart, TrackMaterial, to_bevy};

use crate::assets::Library;
use crate::state::{Editor, Item};

/// A mesh of the preview, despawned when it is rebuilt: of a road or spline, or the
/// terrain's.
#[derive(Component)]
pub struct PreviewMesh(Option<Item>);

/// A prop of the preview: `Project::props[i]`, showing the model at this path.
#[derive(Component)]
pub struct PreviewProp(usize, PathBuf);

/// A copy of a model of a row beside road `.0`.
#[derive(Component)]
pub struct PreviewRow(usize);

/// A chunk of the terrain: `TerrainBuild::chunks[.0]`. Brushes change its mesh while
/// they are dragged.
#[derive(Component)]
pub struct TerrainChunk(pub usize);

/// The copies of one of the models of the scatter named `.0`: their entities are its
/// children.
#[derive(Component)]
pub struct PreviewScatter(pub String);

/// The painted ground's mask as the renderer has it: brushes paint on it while they
/// are dragged.
#[derive(Resource, Default)]
pub struct GroundPaint {
    pub mask: Option<Handle<Image>>,
}

/// Textures and models, shared by the builds in the background and the main thread,
/// and what the last build made, for the next to build only what changed.
#[derive(Resource, Clone, Default)]
pub struct SharedCache(
    pub Arc<Mutex<Cache>>,
    pub Arc<Mutex<open_racing_track_project::BuildCache>>,
);

/// What the last finished build knows, for gizmos and picking.
#[derive(Resource, Default)]
pub struct Built {
    pub roads: Vec<Sampled>,
    pub splines: Vec<Sampled>,
    /// Everything solid, to find what the pointer is over.
    pub ground: Option<Arc<GroundMesh>>,
    /// What will not drive well, as of the last build.
    pub issues: Vec<Issue>,
    /// Each road's corners.
    pub corners: Vec<Vec<Corner>>,
    /// The terrain, for brushes to shape and paint.
    pub terrain: Option<Arc<TerrainBuild>>,
    /// Copies of each scatter's models standing now.
    pub scattered: Vec<usize>,
    /// Each scatter's copies as they stand, to pick and edit one by one.
    pub copies: Vec<Arc<Vec<open_racing_track_project::scatter::Copy>>>,
    /// How wide (from its upright axis) and how tall each scatter's models are, m.
    pub sizes: Vec<Vec<[f32; 2]>>,
    /// The colours of each scatter's models' own materials (linear), by model.
    pub colours: Vec<Vec<Vec<[f32; 4]>>>,
    /// The scatters' names, in the order of `copies` and `sizes`.
    pub names: Vec<String>,
    /// Where the roads' edges run, for scatters laid in rows along them.
    pub edges: Arc<open_racing_track_project::scatter::Edges>,
    /// The main road's centreline, as the game's track follows it.
    pub centreline: Option<open_racing_sim::TrackDef>,
    /// The editor's revision it was built from.
    pub revision: u64,
    /// Builds finished so far.
    pub count: u64,
    /// The names of the lines `roads` and `splines` are of, in their order.
    pub road_names: Vec<String>,
    pub spline_names: Vec<String>,
}

impl Built {
    /// The line of a road or spline as built.
    pub fn sampled(&self, item: Item) -> Option<&Sampled> {
        match item {
            Item::Road(r) => self.roads.get(r),
            Item::Spline(s) => self.splines.get(s),
            Item::Prop(_) => None,
        }
    }

    /// Puts the roads and splines as built in the order the project has them now. The
    /// build is of the project as it was: until the next one is done, a line deleted
    /// or added since would put the lines after it out of step, drawn and picked as
    /// others. Lines new since the build (and those after them) have none until then.
    pub fn align(&mut self, p: &Project) {
        let same = |names: &[String], now: &mut dyn Iterator<Item = &str>| {
            names.iter().map(String::as_str).eq(now)
        };
        if !same(
            &self.road_names,
            &mut p.roads.iter().map(|r| r.name.as_str()),
        ) {
            let roads: Vec<&str> = p.roads.iter().map(|r| r.name.as_str()).collect();
            let order = realign(&mut self.road_names, &roads);
            take_in(&mut self.roads, &order);
            take_in(&mut self.corners, &order);
        }
        if !same(
            &self.spline_names,
            &mut p.splines.iter().map(|s| s.name.as_str()),
        ) {
            let splines: Vec<&str> = p.splines.iter().map(|s| s.name.as_str()).collect();
            let order = realign(&mut self.spline_names, &splines);
            take_in(&mut self.splines, &order);
        }
    }
}

/// Where each of the lines named `now` was among those named `names`, the first ones
/// only up to one that was not there (a line new since). A line renamed in place (the
/// lists as long as they were) keeps its place. `names` becomes the names of the lines
/// kept.
fn realign(names: &mut Vec<String>, now: &[&str]) -> Vec<usize> {
    let mut order = Vec::new();
    for (i, name) in now.iter().enumerate() {
        match names.iter().position(|n| n == name) {
            Some(j) => order.push(j),
            None if names.len() == now.len() => order.push(i),
            None => break,
        }
    }
    *names = now[..order.len()].iter().map(|n| n.to_string()).collect();
    order
}

/// Keeps the elements of `list` at the places `order` names, in that order, up to the
/// first it has not.
fn take_in<T>(list: &mut Vec<T>, order: &[usize]) {
    let mut old: Vec<Option<T>> = std::mem::take(list).into_iter().map(Some).collect();
    *list = order
        .iter()
        .map_while(|&j| old.get_mut(j).and_then(Option::take))
        .collect();
}

#[derive(Default)]
struct Meshes {
    roads: Vec<Sampled>,
    splines: Vec<Sampled>,
    ground: Option<Arc<GroundMesh>>,
    issues: Vec<Issue>,
    corners: Vec<Vec<Corner>>,
    /// (whose, material, mesh, casts shadows)
    meshes: Vec<(Option<Item>, usize, Mesh, bool)>,
    /// Walls models show: whose, the model, and its copies along them.
    walls: Vec<(Item, PathBuf, Arc<Model>, Vec<open_racing_track::Mesh>)>,
    terrain: Option<Arc<TerrainBuild>>,
    /// The copies of each of each scatter's models, and the copies as they stand.
    scatter: Vec<ScatterPart>,
    scattered: Vec<usize>,
    copies: Vec<Arc<Vec<open_racing_track_project::scatter::Copy>>>,
    sizes: Vec<Vec<[f32; 2]>>,
    colours: Vec<Vec<Vec<[f32; 4]>>>,
    names: Vec<String>,
    edges: Arc<open_racing_track_project::scatter::Edges>,
    centreline: Option<open_racing_sim::TrackDef>,
    revision: u64,
    road_names: Vec<String>,
    spline_names: Vec<String>,
    /// Models that could not be read, and why.
    failed: Vec<String>,
    /// What the build could not make as asked (elevation data), and why.
    scene_failed: Vec<String>,
}

/// The copies of one of a scatter's models, to draw by instancing.
struct ScatterPart {
    /// The scatter's name, and which of its models.
    scatter: String,
    model: usize,
    /// Names the model's look for the renderer: its path, its kind of plant, the
    /// project's materials used for some of its own, and whether it casts shadows.
    near_look: String,
    near: Arc<Model>,
    kind: Kind,
    /// The project's materials used for some of the model's own: (its material, the
    /// project's).
    materials: Vec<(usize, usize)>,
    /// Its far model, and the name of its look.
    far: Option<(String, Arc<Model>)>,
    /// The distances the model's levels of detail and the far model show at.
    levels: Vec<Level>,
    shadows: bool,
    copies: Vec<Instance>,
}

#[derive(Resource, Default)]
pub struct Rebuild {
    task: Option<Task<Meshes>>,
    /// Revision the running or last started build is of.
    started: u64,
    /// Materials the handles were made from, and the asset files' revision then.
    materials: Vec<MaterialDef>,
    assets: u64,
    /// The materials' textures being prepared in the background: the main thread would
    /// wait on the cache while a build holds it (a new scatter's models being made).
    looking: Option<Task<Result<open_racing_track::Visual, String>>>,
    handles: Vec<Handle<TrackMaterial>>,
    /// The materials of models walls and scatters show, made once per model and asset
    /// revision.
    wall_looks: HashMap<String, Vec<Handle<TrackMaterial>>>,
    /// Counts the times the materials were made again: copies drawn with the old ones
    /// are spawned again.
    looks: u64,
    /// The painted ground's material, made again when the mask or the layers change.
    ground: Option<GroundLook>,
}

/// The painted ground's material and what it was made of.
struct GroundLook {
    mask: Arc<PaintMask>,
    layers: Vec<String>,
    handle: Handle<TrackMaterial>,
}

pub(crate) fn to_mesh(m: MeshData) -> Mesh {
    render::to_mesh(open_racing_track::Mesh {
        material: 0,
        cast_shadows: false,
        positions: m.positions,
        normals: m.normals,
        uvs: m.uvs,
        indices: m.indices,
    })
}

fn build(project: Project, cache: SharedCache, dir: PathBuf, revision: u64) -> Meshes {
    let scene = {
        let mut built = cache.1.lock().expect("build cache");
        built.dir = Some(dir.clone());
        bake::build_with(&project, &mut built)
    };
    let cache = cache.0;
    let (mut walls, mut failed) = (Vec::new(), Vec::new());
    let lines = scene
        .roads
        .iter()
        .enumerate()
        .flat_map(|(i, b)| b.models.iter().map(move |l| (Item::Road(i), l)))
        .chain(
            scene
                .splines
                .iter()
                .enumerate()
                .flat_map(|(i, b)| b.models.iter().map(move |l| (Item::Spline(i), l))),
        );
    for (item, line) in lines {
        let model = cache.lock().expect("cache").model(&dir, &line.run.model);
        match model {
            Ok(m) => {
                let copies = open_racing_track_project::model::along(&m, line);
                walls.push((item, line.run.model.clone(), m, copies));
            }
            Err(e) => failed.push(e.to_string()),
        }
    }
    let issues = inspect::issues(&project, &scene);
    let scene_failed = scene.failed.clone();
    let corners = project
        .roads
        .iter()
        .zip(&scene.roads)
        .map(|(road, b)| {
            let start = if road.name == project.main_road {
                b.sampled.s_at(project.markers.start)
            } else {
                0.0
            };
            corners::find(&b.sampled, start)
        })
        .collect();
    let parts = scene
        .roads
        .iter()
        .enumerate()
        .flat_map(|(i, b)| b.visual.iter().map(move |p| (Item::Road(i), p)))
        .chain(
            scene
                .splines
                .iter()
                .enumerate()
                .flat_map(|(i, b)| b.visual.iter().map(move |p| (Item::Spline(i), p))),
        );
    let meshes: Vec<_> = parts
        .map(|(item, p)| {
            let mesh = to_mesh(p.mesh.clone());
            (Some(item), p.material, mesh, p.cast_shadows)
        })
        .collect();
    let surfaces: Vec<_> = project.surfaces.iter().map(|s| s.props).collect();
    let ground = Arc::new(scene.ground.build(&surfaces));
    // The scatters' copies, on the ground as it is now, near and far.
    let keepout = open_racing_track_project::scatter::Keepout::new(&scene.roads);
    let month = bake::plant_month(&project);
    let (mut scatter, mut scattered, mut all_copies, mut sizes, mut colours) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for s in &project.scatter {
        let models = cache
            .lock()
            .expect("cache")
            .scatter_models(&project, &dir, s);
        let (near, far) = match models {
            Ok(m) => m,
            Err(e) => {
                failed.push(e.to_string());
                scattered.push(0);
                all_copies.push(Arc::new(Vec::new()));
                sizes.push(Vec::new());
                colours.push(Vec::new());
                continue;
            }
        };
        let copies = open_racing_track_project::scatter::copies(s, &keepout, &ground);
        scattered.push(copies.len());
        let lists = open_racing_track_project::scatter::instances(s, month, &copies);
        sizes.push(
            near.iter()
                .map(|m| {
                    let [lo, hi] = m.bounds;
                    [
                        lo.truncate().abs().max(hi.truncate().abs()).max_element(),
                        hi.z,
                    ]
                })
                .collect(),
        );
        colours.push(
            near.iter()
                .map(|m| m.look.materials.iter().map(|x| x.base_color).collect())
                .collect(),
        );
        all_copies.push(Arc::new(copies));
        for (m, (def, list)) in s.models.iter().zip(lists).enumerate() {
            let materials: Vec<(usize, usize)> = def
                .materials
                .iter()
                .filter_map(|x| Some((x.slot, project.material_index(&x.material)?)))
                .collect();
            let kind = def.kind();
            scatter.push(ScatterPart {
                scatter: s.name.clone(),
                model: m,
                near_look: format!(
                    "{}|{kind:?}|{materials:?}|{}",
                    def.model.display(),
                    s.shadows
                ),
                near: near[m].clone(),
                kind,
                materials,
                far: far[m].clone().map(|f| {
                    let look = format!("far {:p}|{kind:?}|{}", Arc::as_ptr(&f), s.shadows);
                    (look, f)
                }),
                levels: open_racing_track_project::scatter::fades(
                    s,
                    near[m].lods.len(),
                    far[m].is_some(),
                ),
                shadows: s.shadows,
                copies: list,
            });
        }
    }
    Meshes {
        ground: Some(ground),
        terrain: scene.terrain.clone(),
        scatter,
        scattered,
        copies: all_copies,
        sizes,
        colours,
        names: project.scatter.iter().map(|s| s.name.clone()).collect(),
        edges: Arc::new(keepout.edges.clone()),
        centreline: project
            .road_index(&project.main_road)
            .and_then(|i| scene.roads.get(i))
            .map(|main| bake::centreline(&project, main)),
        revision,
        road_names: project.roads.iter().map(|r| r.name.clone()).collect(),
        spline_names: project.splines.iter().map(|s| s.name.clone()).collect(),
        roads: scene.roads.into_iter().map(|b| b.sampled).collect(),
        splines: scene.splines.into_iter().map(|b| b.sampled).collect(),
        issues,
        corners,
        meshes,
        walls,
        failed,
        scene_failed,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn rebuild(
    mut commands: Commands,
    editor: Res<Editor>,
    library: Res<Library>,
    cache: Res<SharedCache>,
    mut state: ResMut<Rebuild>,
    mut built: ResMut<Built>,
    mut paint: ResMut<GroundPaint>,
    mut scattered: ResMut<Scattered>,
    old: Query<Entity, With<PreviewMesh>>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TrackMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Materials change rarely; their textures are prepared again, in the background,
    // only when their files change.
    if state.looking.is_none()
        && (state.materials != editor.project.materials || state.assets != library.revision)
    {
        state.materials = editor.project.materials.clone();
        state.assets = library.revision;
        let (project, dir, cache) = (editor.project.clone(), editor.dir.clone(), cache.0.clone());
        state.looking = Some(AsyncComputeTaskPool::get().spawn(async move {
            let mut cache = cache.lock().map_err(|e| e.to_string())?;
            bake::materials(&project, &dir, &mut cache).map_err(|e| e.to_string())
        }));
    }
    if let Some(task) = &mut state.looking
        && let Some(done) = check_ready(task)
    {
        state.looking = None;
        match done {
            Ok(visual) => {
                state.handles = render::add_materials(
                    &visual,
                    render::formats(formats.as_deref()),
                    16,
                    &mut materials,
                    &mut images,
                );
                // Rebuild so meshes pick up the new handles.
                state.started = 0;
            }
            Err(e) => warn!("materials: {e}"),
        }
        state.wall_looks.clear();
        state.looks += 1;
        scattered.shapes.clear();
        state.ground = None;
    }

    // A build waits for the materials being made, so as not to show it in the old ones.
    if state.looking.is_none()
        && let Some(task) = &mut state.task
        && let Some(done) = check_ready(task)
    {
        state.task = None;
        for e in &old {
            commands.entity(e).despawn();
        }
        let fallback = state.handles.first().cloned().unwrap_or_default();
        for (item, material, mesh, shadows) in done.meshes {
            let handle = state
                .handles
                .get(material)
                .cloned()
                .unwrap_or(fallback.clone());
            let mut e = commands.spawn((
                PreviewMesh(item),
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(handle),
            ));
            if !shadows {
                e.insert(NotShadowCaster);
            }
        }
        // The terrain, a chunk at a time, for brushes to change.
        if let Some(t) = &done.terrain {
            let p = &editor.project;
            let handle = match &t.mask {
                Some(mask) => {
                    ground_look(&mut state, p, mask, &mut materials, &mut images, &mut paint)
                }
                None => {
                    paint.mask = None;
                    let own = p.material_index(&p.terrain.material).unwrap_or(0);
                    state.handles.get(own).cloned().unwrap_or(fallback.clone())
                }
            };
            for (i, chunk) in t.chunks.iter().enumerate() {
                commands.spawn((
                    PreviewMesh(None),
                    TerrainChunk(i),
                    Mesh3d(meshes.add(to_mesh(chunk.clone()))),
                    MeshMaterial3d(handle.clone()),
                    NotShadowCaster,
                ));
            }
        }
        // Models along walls, whose item they are.
        for (item, path, model, copies) in done.walls {
            let looks = state
                .wall_looks
                .entry(path.display().to_string())
                .or_insert_with(|| {
                    render::add_materials(
                        &model.look,
                        render::formats(formats.as_deref()),
                        16,
                        &mut materials,
                        &mut images,
                    )
                });
            for m in copies {
                let handle = looks
                    .get(m.material as usize)
                    .cloned()
                    .unwrap_or(fallback.clone());
                commands.spawn((
                    PreviewMesh(Some(item)),
                    Mesh3d(meshes.add(render::to_mesh(m))),
                    MeshMaterial3d(handle),
                ));
            }
        }
        // The scatters' copies: near, each faded in and out by its own distance; far,
        // merged by tile.
        let formats = render::formats(formats.as_deref());
        let mut seen = HashSet::new();
        for part in done.scatter {
            let key = (part.scatter.clone(), part.model);
            seen.insert(key.clone());
            let mut make = |model: &Model,
                            level: &[open_racing_track::Mesh],
                            look: &str,
                            slots: &[(usize, usize)]| {
                let parts = scattered.shapes.entry(look.to_string()).or_insert_with(|| {
                    shape_parts(
                        model,
                        level,
                        look,
                        part.kind,
                        slots,
                        part.shadows,
                        &mut state,
                        formats,
                        &mut meshes,
                        &mut materials,
                        &mut images,
                    )
                });
                parts.clone()
            };
            // Each level of the model with its own name, and the model's materials.
            let near: Vec<(Vec<ShapePart>, Level)> = std::iter::once(&part.near.meshes)
                .chain(&part.near.lods)
                .zip(&part.levels)
                .enumerate()
                .map(|(i, (meshes, level))| {
                    let look = match i {
                        0 => part.near_look.clone(),
                        i => format!("{}|lod {i}", part.near_look),
                    };
                    (make(&part.near, meshes, &look, &part.materials), *level)
                })
                .collect();
            let far = part.far.as_ref().map(|(look, far)| Far {
                parts: make(far, &far.meshes, look, &[]),
                model: far.clone(),
                level: *part.levels.last().expect("a level at least"),
                tiles: HashMap::new(),
            });
            let look = format!(
                "{}|{:?}|{:?}|{}",
                part.near_look,
                part.far.as_ref().map(|f| &f.0),
                part.levels,
                state.looks
            );
            scattered.show(
                &mut commands,
                &mut meshes,
                key,
                look,
                near,
                far,
                part.copies,
            );
        }
        scattered.keep(&mut commands, &seen);
        let mut issues = done.issues;
        issues.extend(done.scene_failed.into_iter().map(|text| Issue {
            text,
            road: None,
            s: None,
        }));
        issues.extend(done.failed.into_iter().map(|text| Issue {
            text,
            road: None,
            s: None,
        }));
        built.roads = done.roads;
        built.splines = done.splines;
        built.ground = done.ground;
        built.issues = issues;
        built.corners = done.corners;
        built.terrain = done.terrain;
        built.scattered = done.scattered;
        built.copies = done.copies;
        built.sizes = done.sizes;
        built.colours = done.colours;
        built.names = done.names;
        built.edges = done.edges;
        built.centreline = done.centreline;
        built.revision = done.revision;
        built.road_names = done.road_names;
        built.spline_names = done.spline_names;
        built.count += 1;
    }
    built.align(&editor.project);

    if state.task.is_none() && state.started != editor.revision {
        let (project, revision) = (editor.project.clone(), editor.revision);
        let (cache, dir) = (cache.clone(), editor.dir.clone());
        state.started = revision;
        state.task = Some(
            AsyncComputeTaskPool::get().spawn(async move { build(project, cache, dir, revision) }),
        );
    }
}

/// The name of a model's own materials among those made for models: its path (or
/// what names its far model) and its kind of plant, without the project's materials
/// used for some of them or its level of detail.
fn model_look(look: &str) -> String {
    look.split('|').take(2).collect::<Vec<_>>().join("|")
}

/// Meshes of a model (those of one of its levels of detail) ready to draw copies of:
/// the model's own materials as a plant of `kind`, made once for the model and the
/// kind, with the project's used for some of them (`slots`: the model's material, the
/// project's), made the plant's too.
#[allow(clippy::too_many_arguments)]
fn shape_parts(
    model: &Model,
    level: &[open_racing_track::Mesh],
    look: &str,
    kind: Kind,
    slots: &[(usize, usize)],
    shadows: bool,
    state: &mut Rebuild,
    formats: bevy::image::CompressedImageFormats,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<TrackMaterial>,
    images: &mut Assets<Image>,
) -> Vec<ShapePart> {
    let plant = open_racing_track_project::scatter::varied_look(model, kind);
    let own = state
        .wall_looks
        .entry(model_look(look))
        .or_insert_with(|| render::add_materials(&plant, formats, 16, materials, images))
        .clone();
    let fallback = state.handles.first().cloned().unwrap_or_default();
    level
        .iter()
        .map(|m| {
            let slot = m.material as usize;
            let project = slots
                .iter()
                .find(|(s, _)| *s == slot)
                .and_then(|(_, i)| state.handles.get(*i))
                .and_then(|h| {
                    let as_plant = plant.materials.get(slot)?.varies;
                    let mut material = materials.get(h)?.clone();
                    render::set_varies(&mut material, as_plant);
                    Some(materials.add(material))
                });
            let material = project
                .or(own.get(slot).cloned())
                .unwrap_or(fallback.clone());
            ShapePart {
                mesh: meshes.add(render::to_mesh(m.clone())),
                material,
                cast_shadows: m.cast_shadows && shadows,
            }
        })
        .collect()
}

/// The copies of the scatters' models as the view shows them: an entity for each
/// model of each scatter, and under it one for each copy of each of the model's
/// meshes, and one for each tile of its far model's copies, as the game draws them.
#[derive(Resource, Default)]
pub struct Scattered {
    /// By the scatter's name and which of its models.
    groups: HashMap<(String, usize), Group>,
    /// Each model's meshes ready to draw copies of, by their look.
    shapes: HashMap<String, Vec<ShapePart>>,
}

struct Group {
    entity: Entity,
    /// What the copies are drawn with: the model's looks, levels, and materials.
    look: String,
    /// The model's levels of detail, each drawn at every copy.
    near: Vec<(Vec<ShapePart>, Level)>,
    far: Option<Far>,
    copies: Vec<Instance>,
    /// The entities of each copy of the model.
    entities: Vec<Vec<Entity>>,
}

/// The far model's copies, merged by tile.
struct Far {
    parts: Vec<ShapePart>,
    model: Arc<Model>,
    level: Level,
    /// The entities of each tile.
    tiles: HashMap<[i32; 2], Vec<Entity>>,
}

impl Scattered {
    /// Shows the copies of a model of a scatter: moves those that moved, spawns those
    /// that are new and despawns those gone, and merges the tiles of the far model
    /// again where they changed; all of them again if they are drawn with something
    /// else now.
    #[allow(clippy::too_many_arguments)]
    fn show(
        &mut self,
        commands: &mut Commands,
        meshes: &mut Assets<Mesh>,
        key: (String, usize),
        look: String,
        near: Vec<(Vec<ShapePart>, Level)>,
        far: Option<Far>,
        copies: Vec<Instance>,
    ) {
        if let Some(g) = self.groups.get(&key)
            && g.look != look
        {
            commands.entity(g.entity).despawn();
            self.groups.remove(&key);
        }
        let g = self.groups.entry(key.clone()).or_insert_with(|| Group {
            entity: commands
                .spawn((
                    PreviewScatter(key.0.clone()),
                    Transform::default(),
                    Visibility::default(),
                ))
                .id(),
            look,
            near,
            far,
            copies: Vec::new(),
            entities: Vec::new(),
        });
        let mut dirty = std::collections::BTreeSet::new();
        for (i, c) in copies.iter().enumerate() {
            match g.copies.get(i) {
                Some(old) if old == c => {}
                Some(old) => {
                    let t = render::instance_transform(c);
                    for &e in &g.entities[i] {
                        commands
                            .entity(e)
                            .insert((t, MeshTag(u32::from_le_bytes(c.tint))));
                    }
                    dirty.extend([render::tile_of(old), render::tile_of(c)]);
                }
                None => {
                    let entities = spawn_copy(commands, g.entity, &g.near, c);
                    g.entities.push(entities);
                    dirty.insert(render::tile_of(c));
                }
            }
        }
        for gone in g.entities.drain(copies.len()..) {
            for e in gone {
                commands.entity(e).despawn();
            }
        }
        dirty.extend(g.copies.iter().skip(copies.len()).map(render::tile_of));
        g.copies = copies;
        let Some(far) = &mut g.far else {
            return;
        };
        let mut in_tile: HashMap<[i32; 2], Vec<Instance>> = HashMap::new();
        for c in &g.copies {
            let t = render::tile_of(c);
            if dirty.contains(&t) {
                in_tile.entry(t).or_default().push(*c);
            }
        }
        for tile in dirty {
            for e in far.tiles.remove(&tile).into_iter().flatten() {
                commands.entity(e).despawn();
            }
            let Some(these) = in_tile.get(&tile) else {
                continue;
            };
            let (at, merged) = render::merge_tile(&far.model.meshes, these, tile);
            let entities = merged
                .into_iter()
                .zip(&far.parts)
                .map(|(m, part)| {
                    let mesh = meshes.add(m);
                    render::spawn_tile(commands, mesh, part, at, far.level, ChildOf(g.entity))
                })
                .collect();
            far.tiles.insert(tile, entities);
        }
    }

    /// Despawns the copies of the models not in `seen`.
    fn keep(&mut self, commands: &mut Commands, seen: &HashSet<(String, usize)>) {
        self.groups.retain(|key, g| {
            let keep = seen.contains(key);
            if !keep {
                commands.entity(g.entity).despawn();
            }
            keep
        });
    }
}

/// The entities of one copy: each mesh of each of the model's levels of detail, under
/// `group`.
fn spawn_copy(
    commands: &mut Commands,
    group: Entity,
    levels: &[(Vec<ShapePart>, Level)],
    c: &Instance,
) -> Vec<Entity> {
    let t = render::instance_transform(c);
    levels
        .iter()
        .flat_map(|(parts, level)| {
            parts
                .iter()
                .map(move |part| (part, render::visibility_range(*level)))
        })
        .map(|(part, range)| {
            let mut e = commands.spawn((
                Mesh3d(part.mesh.clone()),
                MeshMaterial3d(part.material.clone()),
                t,
                MeshTag(u32::from_le_bytes(c.tint)),
                ChildOf(group),
            ));
            if let Some(range) = &range {
                e.insert(range.clone());
            }
            if !part.cast_shadows {
                e.insert(NotShadowCaster);
            }
            e.id()
        })
        .collect()
}

/// The painted ground's material: the ground's own texture and each layer's, blended
/// by the mask, made again only when they change. Its mask is kept for brushes.
fn ground_look(
    state: &mut Rebuild,
    p: &Project,
    mask: &Arc<PaintMask>,
    materials: &mut Assets<TrackMaterial>,
    images: &mut Assets<Image>,
    paint: &mut GroundPaint,
) -> Handle<TrackMaterial> {
    let layers: Vec<String> = std::iter::once(p.terrain.material.clone())
        .chain(p.terrain.layers.iter().map(|l| l.material.clone()))
        .collect();
    if let Some(g) = &state.ground
        && Arc::ptr_eq(&g.mask, mask)
        && g.layers == layers
    {
        return g.handle.clone();
    }
    let image = mask_image(mask);
    let mask_handle = match &paint.mask {
        // The same image, painted afresh: the material need not change.
        Some(h) if images.get(h).is_some_and(|i| i.size() == image.size()) => {
            if let Some(mut i) = images.get_mut(h) {
                *i = image;
            }
            h.clone()
        }
        _ => images.add(image),
    };
    paint.mask = Some(mask_handle.clone());
    let texture = |m: &str| {
        let i = p.material_index(m)?;
        let def = &p.materials[i];
        let t = materials
            .get(state.handles.get(i)?)?
            .base
            .base_color_texture
            .clone()?;
        Some((t, 1.0 / def.tile[0].max(1e-3)))
    };
    let mut slots: [Option<(Handle<Image>, f32)>; 4] = Default::default();
    for (slot, m) in slots.iter_mut().zip(&layers) {
        *slot = texture(m);
    }
    let own = p.material_index(&p.terrain.material);
    let base = own
        .and_then(|i| state.handles.get(i))
        .and_then(|h| materials.get(h))
        .map(|m| m.base.clone())
        .unwrap_or_default();
    let handle = materials.add(render::layered_material(base, mask_handle, slots));
    state.ground = Some(GroundLook {
        mask: mask.clone(),
        layers,
        handle: handle.clone(),
    });
    handle
}

/// The mask as an image the renderer blends by, kept on the CPU too for brushes to
/// paint on.
pub fn mask_image(mask: &PaintMask) -> Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let mut image = Image::new(
        Extent3d {
            width: mask.width as u32,
            height: mask.height as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        mask.rgba.clone(),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        ..ImageSamplerDescriptor::linear()
    });
    image
}

/// A model ready to show: its meshes with their materials.
struct Shown {
    parts: Vec<(Handle<Mesh>, Handle<TrackMaterial>, bool)>,
}

/// A model read in the background, or why it could not be.
type Loaded = (PathBuf, Result<Arc<Model>, String>);

#[derive(Resource, Default)]
pub struct Props {
    models: HashMap<PathBuf, Shown>,
    /// Loaded models, with the triangle counts the asset list shows.
    pub triangles: HashMap<PathBuf, usize>,
    /// Models that failed to load, and why.
    pub failed: HashMap<PathBuf, String>,
    loading: Option<Task<Vec<Loaded>>>,
    assets: u64,
}

/// Loads the models props use, keeps an entity for each prop and places it where the
/// project says.
#[allow(clippy::too_many_arguments)]
pub fn props(
    mut commands: Commands,
    editor: Res<Editor>,
    library: Res<Library>,
    cache: Res<SharedCache>,
    built: Res<Built>,
    mut state: ResMut<Props>,
    mut shown: Query<(Entity, &PreviewProp, &mut Transform)>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TrackMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let project = &editor.project;
    // Changed files load again.
    if state.assets != library.revision && state.loading.is_none() {
        state.assets = library.revision;
        state.models.clear();
        state.failed.clear();
        for (e, ..) in &shown {
            commands.entity(e).despawn();
        }
        return;
    }

    if let Some(task) = &mut state.loading
        && let Some(done) = check_ready(task)
    {
        state.loading = None;
        for (path, model) in done {
            match model {
                Ok(m) => {
                    let handles = render::add_materials(
                        &m.look,
                        render::formats(formats.as_deref()),
                        16,
                        &mut materials,
                        &mut images,
                    );
                    let parts = m
                        .meshes
                        .iter()
                        .map(|mesh| {
                            let material = handles
                                .get(mesh.material as usize)
                                .cloned()
                                .unwrap_or_default();
                            (
                                meshes.add(render::to_mesh(mesh.clone())),
                                material,
                                mesh.cast_shadows,
                            )
                        })
                        .collect();
                    state.triangles.insert(path.clone(), m.triangles);
                    state.models.insert(path, Shown { parts });
                }
                Err(e) => {
                    state.failed.insert(path, e);
                }
            }
        }
    }

    // Models to load.
    let rows = project
        .roads
        .iter()
        .flat_map(|r| r.rows.iter().map(|w| &w.model));
    let wanted: HashSet<&PathBuf> = project.props.iter().map(|p| &p.model).chain(rows).collect();
    let missing: Vec<PathBuf> = wanted
        .into_iter()
        .filter(|p| !state.models.contains_key(*p) && !state.failed.contains_key(*p))
        .cloned()
        .collect();
    if !missing.is_empty() && state.loading.is_none() {
        let (cache, dir) = (cache.0.clone(), editor.dir.clone());
        state.loading = Some(AsyncComputeTaskPool::get().spawn(async move {
            missing
                .into_iter()
                .map(|path| {
                    let m = cache
                        .lock()
                        .expect("cache")
                        .model(&dir, &path)
                        .map_err(|e| e.to_string());
                    (path, m)
                })
                .collect()
        }));
    }

    // One entity per prop, showing its model; placed every frame.
    let mut have = vec![false; project.props.len()];
    for (e, p, mut t) in &mut shown {
        match project.props.get(p.0).filter(|prop| prop.model == p.1) {
            Some(prop) if !have[p.0] => {
                have[p.0] = true;
                let at = Placement::of(prop, built.ground.as_deref());
                *t = Transform {
                    translation: to_bevy(at.pos),
                    rotation: Quat::from_rotation_y(at.yaw as f32),
                    scale: Vec3::splat(at.scale as f32),
                };
            }
            _ => commands.entity(e).despawn(),
        }
    }
    for (i, prop) in project.props.iter().enumerate() {
        if have[i] {
            continue;
        }
        let Some(model) = state.models.get(&prop.model) else {
            continue;
        };
        commands
            .spawn((
                PreviewProp(i, prop.model.clone()),
                Transform::default(),
                Visibility::default(),
            ))
            .with_children(|c| {
                for (mesh, material, shadows) in &model.parts {
                    let mut e = c.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone())));
                    if !shadows {
                        e.insert(NotShadowCaster);
                    }
                }
            });
    }
}

/// Hides the meshes and props of items hidden or outside local view; the terrain shows
/// outside local view only.
pub fn show_items(
    editor: Res<Editor>,
    tool: Res<crate::viewport::Tool>,
    mut meshes: Query<&mut Visibility, (Without<PreviewProp>, Without<PreviewScatter>)>,
    items: Query<(Entity, &PreviewMesh)>,
    mut scatters: Query<(&PreviewScatter, &mut Visibility), Without<PreviewMesh>>,
    mut props: Query<
        (&PreviewProp, &mut Visibility),
        (Without<PreviewMesh>, Without<PreviewScatter>),
    >,
) {
    let want = |shown: bool| {
        if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    for (e, m) in &items {
        let shown = match m.0 {
            Some(item) => editor.visible(item),
            None => editor.shown.local.is_none(),
        };
        if let Ok(mut v) = meshes.get_mut(e) {
            v.set_if_neq(want(shown));
        }
    }
    for (s, mut v) in &mut scatters {
        let shown = editor.shown.local.is_none()
            && tool.overlays.scatter
            && !editor.shown.hidden_scatter.contains(&s.0);
        v.set_if_neq(want(shown));
    }
    for (p, mut v) in &mut props {
        v.set_if_neq(want(editor.visible(Item::Prop(p.0))));
    }
}

/// Shows the copies of the rows of models beside the roads, placed again whenever the
/// project, the build or the models loaded change.
#[allow(clippy::too_many_arguments)]
pub fn rows(
    mut commands: Commands,
    editor: Res<Editor>,
    built: Res<Built>,
    state: Res<Props>,
    shown: Query<Entity, With<PreviewRow>>,
    mut rows: Query<(&PreviewRow, &mut Visibility)>,
    mut last: Local<(u64, u64, usize)>,
) {
    let key = (editor.revision, built.count, state.models.len());
    if *last != key {
        *last = key;
        for e in &shown {
            commands.entity(e).despawn();
        }
        let p = &editor.project;
        for (i, (road, smp)) in p.roads.iter().zip(&built.roads).enumerate() {
            for row in &road.rows {
                let Some(model) = state.models.get(&row.model) else {
                    continue;
                };
                for prop in open_racing_track_project::rows::copies(road, smp, row) {
                    let at = Placement::of(&prop, built.ground.as_deref());
                    commands
                        .spawn((
                            PreviewRow(i),
                            Transform {
                                translation: to_bevy(at.pos),
                                rotation: Quat::from_rotation_y(at.yaw as f32),
                                scale: Vec3::splat(at.scale as f32),
                            },
                            Visibility::default(),
                        ))
                        .with_children(|c| {
                            for (mesh, material, shadows) in &model.parts {
                                let mut e = c.spawn((
                                    Mesh3d(mesh.clone()),
                                    MeshMaterial3d(material.clone()),
                                ));
                                if !shadows {
                                    e.insert(NotShadowCaster);
                                }
                            }
                        });
                }
            }
        }
        return;
    }
    for (r, mut v) in &mut rows {
        let shown = editor.visible(Item::Road(r.0));
        v.set_if_neq(if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aligned(built: &[&str], now: &[&str]) -> (Vec<usize>, Vec<String>) {
        let mut names = built.iter().map(|n| n.to_string()).collect();
        let order = realign(&mut names, now);
        (order, names)
    }

    #[test]
    fn built_lines_follow_the_project_until_the_next_build() {
        // One deleted: those after it move up.
        assert_eq!(aligned(&["a", "b", "c"], &["a", "c"]).0, vec![0, 2]);
        // One renamed in place keeps its place.
        assert_eq!(aligned(&["a", "b", "c"], &["a", "x", "c"]).0, vec![0, 1, 2]);
        // One new before others: it and those after it wait for the build.
        let (order, names) = aligned(&["a", "c"], &["a", "b", "c"]);
        assert_eq!(order, vec![0]);
        assert_eq!(names, vec!["a"]);
        let mut list = vec!["A", "B", "C"];
        take_in(&mut list, &[2, 0]);
        assert_eq!(list, vec!["C", "A"]);
        let mut list = vec!["A"];
        take_in(&mut list, &[0, 0, 5]);
        assert_eq!(list, vec!["A"]);
    }
}
