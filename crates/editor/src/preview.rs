//! The 3D preview: the project's meshes, rebuilt in the background whenever the project
//! changes, in the same materials the game renders them with.

use bevy::image::CompressedImageFormatSupport;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use open_racing_track_project::curve::Sampled;
use open_racing_track_project::project::MaterialDef;
use open_racing_track_project::road::MeshData;
use open_racing_track_project::{Project, Textures, bake};
use open_racing_track_render::{self as render, TrackMaterial};

use crate::state::Editor;

/// A mesh of the preview, despawned when it is rebuilt.
#[derive(Component)]
pub struct PreviewMesh;

/// What the last finished build knows about the roads, for gizmos and picking.
#[derive(Resource, Default)]
pub struct Built {
    pub roads: Vec<Sampled>,
    /// Builds finished so far.
    pub count: u64,
}

#[derive(Default)]
struct Meshes {
    roads: Vec<Sampled>,
    /// (material, mesh, casts shadows)
    meshes: Vec<(usize, Mesh, bool)>,
}

#[derive(Resource, Default)]
pub struct Rebuild {
    task: Option<Task<Meshes>>,
    /// Revision the running or last started build is of.
    started: u64,
    /// Materials the handles were made from.
    materials: Vec<MaterialDef>,
    handles: Vec<Handle<TrackMaterial>>,
    textures: Option<Textures>,
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

fn build(project: Project) -> Meshes {
    let scene = bake::build(&project);
    let terrain = project
        .material_index(&project.terrain.material)
        .unwrap_or(0);
    let mut meshes: Vec<_> = scene
        .visual_parts()
        .map(|p| (p.material, to_mesh(p.mesh.clone()), p.cast_shadows))
        .collect();
    if let Some(t) = scene.terrain {
        meshes.extend(t.chunks.into_iter().map(|m| (terrain, to_mesh(m), false)));
    }
    let roads = scene.roads.into_iter().map(|b| b.sampled).collect();
    Meshes { roads, meshes }
}

#[allow(clippy::too_many_arguments)]
pub fn rebuild(
    mut commands: Commands,
    editor: Res<Editor>,
    mut state: ResMut<Rebuild>,
    mut built: ResMut<Built>,
    old: Query<Entity, With<PreviewMesh>>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TrackMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Materials change rarely; their textures are prepared once.
    if state.materials != editor.project.materials {
        let mut textures = state.textures.take().unwrap_or_default();
        match bake::materials(&editor.project, &editor.dir, &mut textures) {
            Ok(visual) => {
                state.handles = render::add_materials(
                    &visual,
                    render::formats(formats.as_deref()),
                    16,
                    &mut materials,
                    &mut images,
                );
                state.materials = editor.project.materials.clone();
                // Rebuild so meshes pick up the new handles.
                state.started = 0;
            }
            Err(e) => warn!("materials: {e}"),
        }
        state.textures = Some(textures);
    }

    if let Some(task) = &mut state.task
        && let Some(done) = check_ready(task)
    {
        state.task = None;
        for e in &old {
            commands.entity(e).despawn();
        }
        let fallback = state.handles.first().cloned().unwrap_or_default();
        for (material, mesh, shadows) in done.meshes {
            let handle = state
                .handles
                .get(material)
                .cloned()
                .unwrap_or(fallback.clone());
            let mut e = commands.spawn((
                PreviewMesh,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(handle),
            ));
            if !shadows {
                e.insert(NotShadowCaster);
            }
        }
        built.roads = done.roads;
        built.count += 1;
    }

    if state.task.is_none() && state.started != editor.revision {
        let (project, revision) = (editor.project.clone(), editor.revision);
        state.started = revision;
        state.task = Some(AsyncComputeTaskPool::get().spawn(async move { build(project) }));
    }
}
