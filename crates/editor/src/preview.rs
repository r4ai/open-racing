//! The 3D preview: the project's meshes, rebuilt in the background whenever the project
//! changes, in the same materials the game renders them with, and its props, placed
//! from the project every frame so that they follow edits at once.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy::image::CompressedImageFormatSupport;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use open_racing_sim::GroundMesh;
use open_racing_track_project::corners::{self, Corner};
use open_racing_track_project::curve::Sampled;
use open_racing_track_project::inspect::{self, Issue};
use open_racing_track_project::model::{Model, Placement};
use open_racing_track_project::project::MaterialDef;
use open_racing_track_project::road::MeshData;
use open_racing_track_project::terrain::{PaintMask, TerrainBuild};
use open_racing_track_project::{Cache, Project, bake};
use open_racing_track_render::{self as render, TrackMaterial, to_bevy};

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

/// Copies of models of scatter `.0`, merged.
#[derive(Component)]
pub struct PreviewScatter(pub usize);

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
    /// The editor's revision it was built from.
    pub revision: u64,
    /// Builds finished so far.
    pub count: u64,
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
    /// Each scatter's copies merged by model and level of detail, and its copies.
    scatter: Vec<ScatterPart>,
    scattered: Vec<usize>,
    copies: Vec<Arc<Vec<open_racing_track_project::scatter::Copy>>>,
    sizes: Vec<Vec<[f32; 2]>>,
    colours: Vec<Vec<Vec<[f32; 4]>>>,
    names: Vec<String>,
    revision: u64,
    /// Models that could not be read, and why.
    failed: Vec<String>,
    /// What the build could not make as asked (elevation data), and why.
    scene_failed: Vec<String>,
}

/// Copies of one of a scatter's models at one level of detail, merged by tile.
struct ScatterPart {
    /// Which scatter.
    scatter: usize,
    /// Names the model's materials for the renderer: its path near, its far model's
    /// own far.
    look: String,
    model: Arc<Model>,
    /// The project's materials used for some of the model's own: (its material, the
    /// project's).
    materials: Vec<(usize, usize)>,
    meshes: Vec<open_racing_track::Mesh>,
}

#[derive(Resource, Default)]
pub struct Rebuild {
    task: Option<Task<Meshes>>,
    /// Revision the running or last started build is of.
    started: u64,
    /// Materials the handles were made from, and the asset files' revision then.
    materials: Vec<MaterialDef>,
    assets: u64,
    handles: Vec<Handle<TrackMaterial>>,
    /// The materials of models walls and scatters show, made once per model and asset
    /// revision.
    wall_looks: HashMap<String, Vec<Handle<TrackMaterial>>>,
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
        lod: None,
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
    let (mut scatter, mut scattered, mut all_copies, mut sizes, mut colours) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (k, s) in project.scatter.iter().enumerate() {
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
        use open_racing_track_project::scatter::Level;
        let mut parts: HashMap<(usize, bool), Vec<open_racing_track::Mesh>> = HashMap::new();
        for (m, level, mesh) in open_racing_track_project::scatter::meshes(s, &near, &far, &copies)
        {
            parts
                .entry((m, level == Level::Far))
                .or_default()
                .push(mesh);
        }
        all_copies.push(Arc::new(copies));
        let mut keys: Vec<_> = parts.keys().copied().collect();
        keys.sort_unstable();
        for key @ (m, is_far) in keys {
            let def = &s.models[m];
            let (look, model, materials) = if is_far {
                let model = far[m].clone().expect("a far level has a far model");
                (format!("far {:p}", Arc::as_ptr(&model)), model, vec![])
            } else {
                let slots = def
                    .materials
                    .iter()
                    .filter_map(|x| Some((x.slot, project.material_index(&x.material)?)))
                    .collect();
                (def.model.display().to_string(), near[m].clone(), slots)
            };
            scatter.push(ScatterPart {
                scatter: k,
                look,
                model,
                materials,
                meshes: parts.remove(&key).unwrap_or_default(),
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
        revision,
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
    old: Query<Entity, With<PreviewMesh>>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TrackMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Materials change rarely; their textures are prepared again only when their files
    // change.
    if state.materials != editor.project.materials || state.assets != library.revision {
        let mut cache = cache.0.lock().expect("cache");
        match bake::materials(&editor.project, &editor.dir, &mut cache) {
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
        state.materials = editor.project.materials.clone();
        state.assets = library.revision;
        state.wall_looks.clear();
        state.ground = None;
    }

    if let Some(task) = &mut state.task
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
        // The scatters' copies, each tile of each level faded in and out by its
        // distance.
        for part in done.scatter {
            let looks = state
                .wall_looks
                .entry(part.look)
                .or_insert_with(|| {
                    render::add_materials(
                        &part.model.look,
                        render::formats(formats.as_deref()),
                        16,
                        &mut materials,
                        &mut images,
                    )
                })
                .clone();
            for mut m in part.meshes {
                let slot = m.material as usize;
                let handle = part
                    .materials
                    .iter()
                    .find(|(s, _)| *s == slot)
                    .and_then(|(_, i)| state.handles.get(*i))
                    .or(looks.get(slot))
                    .cloned()
                    .unwrap_or(fallback.clone());
                let shadows = m.cast_shadows;
                let lod = render::level_of_detail(&mut m);
                let mut e = commands.spawn((
                    PreviewMesh(None),
                    PreviewScatter(part.scatter),
                    Mesh3d(meshes.add(render::to_mesh(m))),
                    MeshMaterial3d(handle),
                ));
                if let Some(lod) = lod {
                    e.insert(lod);
                }
                if !shadows {
                    e.insert(NotShadowCaster);
                }
            }
        }
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
        built.revision = done.revision;
        built.count += 1;
    }

    if state.task.is_none() && state.started != editor.revision {
        let (project, revision) = (editor.project.clone(), editor.revision);
        let (cache, dir) = (cache.clone(), editor.dir.clone());
        state.started = revision;
        state.task = Some(
            AsyncComputeTaskPool::get().spawn(async move { build(project, cache, dir, revision) }),
        );
    }
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
    mut meshes: Query<
        (&PreviewMesh, Option<&PreviewScatter>, &mut Visibility),
        Without<PreviewProp>,
    >,
    mut props: Query<(&PreviewProp, &mut Visibility), Without<PreviewMesh>>,
) {
    let want = |shown: bool| {
        if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    for (m, scatter, mut v) in &mut meshes {
        let shown = match m.0 {
            Some(item) => editor.visible(item),
            None => editor.shown.local.is_none(),
        } && scatter.is_none_or(|s| {
            tool.overlays.scatter
                && editor
                    .project
                    .scatter
                    .get(s.0)
                    .is_none_or(|x| !editor.shown.hidden_scatter.contains(&x.name))
        });
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
