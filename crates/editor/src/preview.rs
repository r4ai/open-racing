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
    /// Models that could not be read, and why.
    failed: Vec<String>,
    /// What the build could not make as asked (elevation data), and why.
    scene_failed: Vec<String>,
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
    /// The materials of models walls show, made once per model and asset revision.
    wall_looks: HashMap<PathBuf, Vec<Handle<TrackMaterial>>>,
}

fn to_mesh(m: MeshData) -> Mesh {
    render::to_mesh(open_racing_track::Mesh {
        material: 0,
        cast_shadows: false,
        positions: m.positions,
        normals: m.normals,
        uvs: m.uvs,
        indices: m.indices,
    })
}

fn build(project: Project, cache: SharedCache, dir: PathBuf) -> Meshes {
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
    let terrain = project
        .material_index(&project.terrain.material)
        .unwrap_or(0);
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
    let mut meshes: Vec<_> = parts
        .map(|(item, p)| {
            let mesh = to_mesh(p.mesh.clone());
            (Some(item), p.material, mesh, p.cast_shadows)
        })
        .collect();
    if let Some(t) = scene.terrain {
        meshes.extend(
            t.chunks
                .into_iter()
                .map(|m| (None, terrain, to_mesh(m), false)),
        );
    }
    let surfaces: Vec<_> = project.surfaces.iter().map(|s| s.props).collect();
    Meshes {
        ground: Some(Arc::new(scene.ground.build(&surfaces))),
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
        for (item, path, model, copies) in done.walls {
            let looks = state.wall_looks.entry(path).or_insert_with(|| {
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
        built.count += 1;
    }

    if state.task.is_none() && state.started != editor.revision {
        let (project, revision) = (editor.project.clone(), editor.revision);
        let (cache, dir) = (cache.clone(), editor.dir.clone());
        state.started = revision;
        state.task =
            Some(AsyncComputeTaskPool::get().spawn(async move { build(project, cache, dir) }));
    }
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
    let wanted: HashSet<&PathBuf> = project.props.iter().map(|p| &p.model).collect();
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
    mut meshes: Query<(&PreviewMesh, &mut Visibility), Without<PreviewProp>>,
    mut props: Query<(&PreviewProp, &mut Visibility), Without<PreviewMesh>>,
) {
    let want = |shown: bool| {
        if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    for (m, mut v) in &mut meshes {
        let shown = match m.0 {
            Some(item) => editor.visible(item),
            None => editor.shown.local.is_none(),
        };
        v.set_if_neq(want(shown));
    }
    for (p, mut v) in &mut props {
        v.set_if_neq(want(editor.visible(Item::Prop(p.0))));
    }
}
