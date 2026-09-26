//! Turns a project into a track package: meshes for the physics and the renderer, the
//! centreline of the main road, and the race layout from the markers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use glam::DVec3;
use open_racing_sim::GroundMesh;
use open_racing_sim::track::heading;
use open_racing_sim::{GridSlot, Layout, PitLane, Pose, Track, TrackDef, TrackPoint};
use open_racing_track::texture::{self, Mips};
use open_racing_track::{
    AlphaMode, Ground, Material, PatchKind, Texture, TrackPackage, Visual, VisualBuilder,
};

use crate::Error;
use crate::curve::Sampled;
use crate::model::{self, Model, Placement};
use crate::project::{Alpha, MaterialDef, Project, TextureSource};
use crate::road::{self, RoadBuild, Solid, SolidPart};
use crate::spline::{self, SplineBuild};
use crate::terrain::{self, TerrainBuild};

/// Spacing of the centreline's points, m. The simulation eases between them.
const CENTRELINE_SPACING: f64 = 4.0;

/// Everything the project's meshes are built into.
pub struct Scene {
    pub roads: Vec<RoadBuild>,
    pub terrain: Option<TerrainBuild>,
    pub splines: Vec<SplineBuild>,
    /// Every physics mesh: roads, terrain and splines.
    pub ground: Ground,
    /// What could not be built as the project asks, and why: elevation data that
    /// cannot be read.
    pub failed: Vec<String>,
}

impl Scene {
    /// Every rendered mesh but the terrain's and the walls models show.
    pub fn visual_parts(&self) -> impl Iterator<Item = &road::VisualPart> {
        let roads = self.roads.iter().flat_map(|b| &b.visual);
        roads.chain(self.splines.iter().flat_map(|b| &b.visual))
    }

    /// The walls models show.
    pub fn model_lines(&self) -> impl Iterator<Item = &road::ModelLine> {
        let roads = self.roads.iter().flat_map(|b| &b.models);
        roads.chain(self.splines.iter().flat_map(|b| &b.models))
    }
}

fn add_solids<'a>(ground: &mut Ground, parts: impl IntoIterator<Item = &'a SolidPart>) {
    for part in parts {
        let kind = match part.kind {
            Solid::Ground(s) => PatchKind::Ground(s as u16),
            Solid::Wall => PatchKind::Wall,
        };
        let m = &part.mesh;
        ground.add(kind, &m.positions, &m.normals, &m.indices);
    }
}

/// What the last build made, so that the next builds only what changed: the editor
/// rebuilds on every edit, and most edits leave most roads and the terrain as they
/// were.
#[derive(Default)]
pub struct BuildCache {
    /// The project's directory, where its elevation data is read from.
    pub dir: Option<PathBuf>,
    /// The elevation data read in, with the file's time and the place on the Earth it
    /// was read about.
    heights: Option<(
        PathBuf,
        Option<SystemTime>,
        Option<crate::geo::Geo>,
        Arc<crate::dem::Heights>,
    )>,
    /// The surfaces and materials the roads were built with.
    lists: Option<(Vec<crate::project::NamedSurface>, Vec<MaterialDef>)>,
    /// Each road as it was, and its build before overlaps were resolved.
    roads: Vec<(crate::project::Road, RoadBuild)>,
    /// The terrain's settings, the roads and elevation data it was built round, and
    /// the terrain.
    terrain: Option<TerrainKey>,
}

type TerrainKey = (
    crate::project::Terrain,
    Vec<crate::project::Road>,
    Option<Arc<crate::dem::Heights>>,
    Option<TerrainBuild>,
);

/// Builds the roads, then the terrain under them, then the splines over both. Without
/// the project's directory the terrain does not follow its elevation data.
pub fn build(project: &Project) -> Scene {
    build_with(project, &mut BuildCache::default())
}

/// `build` of the project in `dir`, the terrain following its elevation data.
pub fn build_in(project: &Project, dir: &Path) -> Scene {
    build_with(project, &mut BuildCache::at(dir))
}

impl BuildCache {
    /// A cache for building the project in `dir`.
    pub fn at(dir: &Path) -> Self {
        Self {
            dir: Some(dir.to_path_buf()),
            ..Default::default()
        }
    }

    /// The project's elevation data, read again only when its file changes.
    fn heights(&mut self, project: &Project) -> Result<Option<Arc<crate::dem::Heights>>, Error> {
        let (Some(file), Some(dir)) = (&project.terrain.heights, &self.dir) else {
            return Ok(None);
        };
        let path = dir.join(file);
        let time = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Some((p, t, g, h)) = &self.heights
            && *p == path
            && *t == time
            && *g == project.geo
        {
            return Ok(Some(h.clone()));
        }
        let h = Arc::new(crate::dem::read_file(&path, project.geo)?);
        self.heights = Some((path, time, project.geo, h.clone()));
        Ok(Some(h))
    }
}

/// `build`, keeping what is the same as last time from `cache`.
pub fn build_with(project: &Project, cache: &mut BuildCache) -> Scene {
    use rayon::prelude::*;
    let lists = (project.surfaces.clone(), project.materials.clone());
    if cache.lists.as_ref() != Some(&lists) {
        cache.lists = Some(lists);
        cache.roads.clear();
        cache.terrain = None;
    }
    let mut failed = Vec::new();
    let heights = cache.heights(project).unwrap_or_else(|e| {
        failed.push(format!("terrain: {e}"));
        None
    });
    let old = std::mem::take(&mut cache.roads);
    let fresh: Vec<RoadBuild> = (0..project.roads.len())
        .into_par_iter()
        .map(|i| {
            let road = &project.roads[i];
            match old.iter().find(|(r, _)| r == road) {
                Some((_, b)) => b.clone(),
                None => road::build(project, i),
            }
        })
        .collect();
    cache.roads = project
        .roads
        .iter()
        .cloned()
        .zip(fresh.iter().cloned())
        .collect();
    let mut roads = fresh;
    crate::overlap::resolve(&mut roads);
    let same_heights = |h: &Option<Arc<crate::dem::Heights>>| match (h, &heights) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    };
    let terrain = match &cache.terrain {
        Some((t, r, h, built))
            if *t == project.terrain && *r == project.roads && same_heights(h) =>
        {
            built.clone()
        }
        _ => {
            let built = terrain::build(project, &roads, heights.as_deref());
            cache.terrain = Some((
                project.terrain.clone(),
                project.roads.clone(),
                heights.clone(),
                built.clone(),
            ));
            built
        }
    };

    let mut ground = Ground::default();
    add_solids(&mut ground, roads.iter().flat_map(|b| &b.solid));
    if let Some(t) = &terrain {
        let s = &t.solid;
        let surface = project.surface_index(&project.terrain.surface).unwrap_or(0);
        ground.add(
            PatchKind::Ground(surface as u16),
            &s.positions,
            &s.normals,
            &s.indices,
        );
    }
    let under = project
        .splines
        .iter()
        .any(|s| s.drape)
        .then(|| ground.build(&surface_props(project)));
    let splines: Vec<SplineBuild> = (0..project.splines.len())
        .map(|i| spline::build(project, i, under.as_ref()))
        .collect();
    add_solids(&mut ground, splines.iter().flat_map(|b| &b.solid));
    Scene {
        roads,
        terrain,
        splines,
        ground,
        failed,
    }
}

fn surface_props(project: &Project) -> Vec<open_racing_sim::SurfaceProps> {
    project.surfaces.iter().map(|s| s.props).collect()
}

/// Prepared textures and loaded models, kept between bakes so that each is read and
/// encoded once, and again only when its file changes.
#[derive(Default)]
pub struct Cache {
    textures: HashMap<TextureSource, (Option<SystemTime>, Vec<u8>)>,
    models: HashMap<PathBuf, (Option<SystemTime>, Arc<Model>)>,
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Cache {
    /// The DDS of a texture; files are read from `dir`.
    pub fn texture(&mut self, source: &TextureSource, dir: &Path) -> Result<Option<&[u8]>, Error> {
        let stamp = match source {
            TextureSource::None => return Ok(None),
            TextureSource::Builtin(_) => None,
            TextureSource::File(p) => modified(&dir.join(p)),
        };
        if self.textures.get(source).is_none_or(|(s, _)| *s != stamp) {
            let dds = match source {
                TextureSource::None => unreachable!(),
                TextureSource::Builtin(t) => texture::encode(crate::builtin::image(*t)),
                TextureSource::File(p) => {
                    let path = dir.join(p);
                    let bytes = std::fs::read(&path).map_err(|e| Error::Io(path.clone(), e))?;
                    texture::prepare(&bytes, Mips::Complete)
                        .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?
                }
            };
            self.textures.insert(source.clone(), (stamp, dds));
        }
        Ok(self.textures.get(source).map(|(_, d)| d.as_slice()))
    }

    /// A model, read from `path` relative to `dir`.
    pub fn model(&mut self, dir: &Path, path: &Path) -> Result<Arc<Model>, Error> {
        let full = dir.join(path);
        let stamp = modified(&full);
        if let Some((s, m)) = self.models.get(path)
            && *s == stamp
        {
            return Ok(m.clone());
        }
        let m = Arc::new(model::load(&full)?);
        self.models.insert(path.to_path_buf(), (stamp, m.clone()));
        Ok(m)
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Adds the project's materials, in order, with their textures: material `i` of the
/// project is material `i` of the visual.
pub fn add_materials(
    project: &Project,
    dir: &Path,
    cache: &mut Cache,
    visual: &mut VisualBuilder,
) -> Result<(), Error> {
    for m in &project.materials {
        let MaterialDef {
            color,
            texture,
            roughness,
            reflectance,
            double_sided,
            normal,
            alpha,
            ..
        } = m;
        let mut add = |source| -> Result<Option<u32>, Error> {
            Ok(cache.texture(source, dir)?.map(|data| {
                visual.add_texture(Texture {
                    data: data.to_vec(),
                })
            }))
        };
        let (texture, normal) = (add(texture)?, add(normal)?);
        let [r, g, b] = color.map(srgb_to_linear);
        visual.add_material(Material {
            base_color: [r, g, b, 1.0],
            base_color_texture: texture,
            normal_texture: normal,
            roughness: *roughness,
            reflectance: *reflectance,
            double_sided: *double_sided,
            alpha_mode: match *alpha {
                Alpha::Opaque => AlphaMode::Opaque,
                Alpha::Mask(c) => AlphaMode::Mask(c),
                Alpha::Blend => AlphaMode::Blend,
            },
            ..Default::default()
        });
    }
    Ok(())
}

/// The project's materials only, for previews that add their own meshes.
pub fn materials(project: &Project, dir: &Path, cache: &mut Cache) -> Result<Visual, Error> {
    let mut visual = VisualBuilder::new();
    add_materials(project, dir, cache, &mut visual)?;
    Ok(visual.build())
}

/// Adds a model's textures and materials to `visual`; returns the new index of each
/// of its materials.
pub fn add_look(model: &Model, visual: &mut VisualBuilder) -> Vec<u32> {
    let textures: Vec<u32> = model
        .look
        .textures
        .iter()
        .map(|t| visual.add_texture(t.clone()))
        .collect();
    let remap = |i: Option<u32>| i.map(|i| textures[i as usize]);
    model
        .look
        .materials
        .iter()
        .map(|m| {
            visual.add_material(Material {
                base_color_texture: remap(m.base_color_texture),
                normal_texture: remap(m.normal_texture),
                surface_texture: remap(m.surface_texture),
                ..m.clone()
            })
        })
        .collect()
}

/// Adds the props: their meshes to `visual`, and those cars collide with to `ground`.
pub fn add_props(
    project: &Project,
    dir: &Path,
    cache: &mut Cache,
    under: Option<&GroundMesh>,
    visual: &mut VisualBuilder,
    ground: &mut Ground,
) -> Result<(), Error> {
    let mut looks: HashMap<&Path, Vec<u32>> = HashMap::new();
    for prop in &project.props {
        let model = cache.model(dir, &prop.model)?;
        let materials = looks
            .entry(prop.model.as_path())
            .or_insert_with(|| add_look(&model, visual));
        let at = Placement::of(prop, under);
        for m in &model.meshes {
            let positions: Vec<[f32; 3]> = m.positions.iter().map(|&p| at.point(p)).collect();
            let normals: Vec<[f32; 3]> = m.normals.iter().map(|&n| at.normal(n)).collect();
            visual.add_mesh(
                materials[m.material as usize],
                m.cast_shadows,
                &positions,
                &normals,
                &m.uvs,
                &m.indices,
            );
            if prop.collide {
                ground.add(PatchKind::Wall, &positions, &[], &m.indices);
            }
        }
    }
    Ok(())
}

/// Adds the walls models show: each model's look once, and its copies along the walls.
pub fn add_model_walls(
    scene: &Scene,
    dir: &Path,
    cache: &mut Cache,
    visual: &mut VisualBuilder,
) -> Result<(), Error> {
    let mut looks: HashMap<PathBuf, Vec<u32>> = HashMap::new();
    for line in scene.model_lines() {
        let model = cache.model(dir, &line.run.model)?;
        let materials = looks
            .entry(line.run.model.clone())
            .or_insert_with(|| add_look(&model, visual));
        for m in model::along(&model, line) {
            visual.add_mesh(
                materials[m.material as usize],
                m.cast_shadows,
                &m.positions,
                &m.normals,
                &m.uvs,
                &m.indices,
            );
        }
    }
    Ok(())
}

/// Bakes the project into a package. Textures and models are read from the project's directory
/// `dir`.
pub fn bake(project: &Project, dir: &Path, cache: &mut Cache) -> Result<TrackPackage, Error> {
    project.validate()?;
    let mut scene = build_in(project, dir);
    if let Some(e) = scene.failed.first() {
        return Err(Error::Invalid(e.clone()));
    }

    let mut visual = VisualBuilder::new();
    add_materials(project, dir, cache, &mut visual)?;
    let under = project
        .props
        .iter()
        .any(|p| p.drape)
        .then(|| scene.ground.build(&surface_props(project)));
    add_props(
        project,
        dir,
        cache,
        under.as_ref(),
        &mut visual,
        &mut scene.ground,
    )?;
    add_model_walls(&scene, dir, cache, &mut visual)?;
    for part in scene.visual_parts() {
        let m = &part.mesh;
        visual.add_mesh(
            part.material as u32,
            part.cast_shadows,
            &m.positions,
            &m.normals,
            &m.uvs,
            &m.indices,
        );
    }
    if let Some(t) = &scene.terrain {
        let material = project
            .material_index(&project.terrain.material)
            .unwrap_or(0);
        for m in &t.chunks {
            visual.add_mesh(
                material as u32,
                false,
                &m.positions,
                &m.normals,
                &m.uvs,
                &m.indices,
            );
        }
    }

    let centreline = centreline(project, &scene.roads[project.main_index()]);
    let layout = layout(project, &scene, &centreline)?;
    Ok(TrackPackage {
        centreline,
        surfaces: surface_props(project),
        layout,
        ground: scene.ground,
        visual: Some(visual.build()),
    })
}

/// The main road's centre, starting at the start/finish line.
fn centreline(project: &Project, main: &RoadBuild) -> TrackDef {
    let road = &project.roads[project.main_index()];
    let sampled = &main.sampled;
    let start = sampled.s_at(project.markers.start);
    let count = (sampled.length / CENTRELINE_SPACING).round().max(8.0) as usize;
    let period = road.period();
    let mut reach: f64 = 0.0;
    let points = (0..count)
        .map(|k| {
            let f = sampled.frame_at(start + k as f64 * sampled.length / count as f64);
            let i = sampled.nearest(f.pos);
            let [(left, _), (right, _)] = main.edges(i);
            reach = reach.max(left - f.width_left).max(-right - f.width_right);
            TrackPoint {
                pos: (f.pos + f.normal * road.crown).into(),
                width_left: f.width_left,
                width_right: f.width_right,
                bank: road.bank.eval(f.u, period, true),
            }
        })
        .collect();
    let barriers = road
        .barriers
        .iter()
        .map(|b| b.offset + b.thickness)
        .fold(0.0, f64::max);
    TrackDef {
        name: project.name.clone(),
        points,
        kerb_width: 1.0,
        kerb_height: 0.0,
        // The invisible barrier stands beyond everything built.
        runoff_width: reach.max(barriers) + 5.0,
        spacing: 1.0,
    }
}

fn layout(project: &Project, scene: &Scene, centreline: &TrackDef) -> Result<Layout, Error> {
    let track = Track::new(centreline).map_err(|e| Error::Invalid(e.to_string()))?;
    let locate = |p: DVec3| track.locate(p, track.nearest_index(p));
    let main = &scene.roads[project.main_index()].sampled;
    let m = &project.markers;

    let mut sectors: Vec<f64> = m
        .sectors
        .iter()
        .map(|&u| locate(main.frame_at(main.s_at(u)).pos).s)
        .filter(|&s| s > 1.0 && s < track.length - 1.0)
        .collect();
    sectors.sort_by(f64::total_cmp);
    sectors.dedup_by(|a, b| (*a - *b).abs() < 1.0);

    let grid = (0..m.grid.count)
        .map(|i| {
            let side = if i % 2 == 0 { 1.0 } else { -1.0 };
            GridSlot {
                s: track.wrap_s(-(m.grid.behind + i as f64 * m.grid.spacing)),
                d: m.grid.pole.sign() * side * m.grid.stagger,
            }
        })
        .collect();

    let pit = m.pit.as_ref().map(|p| {
        let i = project.road_index(&p.road).expect("validated");
        let road = &project.roads[i];
        let lane = &scene.roads[i].sampled;
        let boxes = p
            .boxes
            .iter()
            .map(|&u| {
                let f = lane.frame_at(lane.s_at(u));
                let left = DVec3::Z.cross(f.tangent).normalize_or(DVec3::Y);
                Pose {
                    pos: (f.pos + left * (p.box_side.sign() * p.box_offset)).into(),
                    heading: heading(f.tangent),
                }
            })
            .collect();
        let end = |i: usize| road.nodes.get(i).map(|n| locate(n.pos).s);
        PitLane {
            speed_limit: p.speed_limit,
            entry: end(0),
            exit: end(road.nodes.len().wrapping_sub(1)),
            boxes,
        }
    });
    Ok(Layout { sectors, grid, pit })
}

/// A sampled road's frame at spline parameter `u`, for the editor's markers.
pub fn frame_at_u(sampled: &Sampled, u: f64) -> crate::curve::Frame {
    sampled.frame_at(sampled.s_at(u))
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::ops::{Op, apply_all};

    fn meshes(s: &Scene) -> Vec<Vec<[f32; 3]>> {
        s.visual_parts()
            .map(|p| p.mesh.positions.clone())
            .chain(
                s.terrain
                    .iter()
                    .flat_map(|t| t.chunks.iter().map(|c| c.positions.clone())),
            )
            .collect()
    }

    #[test]
    fn a_cached_build_is_the_same_as_a_fresh_one() {
        let mut p = Project::new("t");
        apply_all(
            &mut p,
            &[Op::AddRoad {
                name: "pit".into(),
                closed: false,
                nodes: vec![DVec3::new(0.0, -40.0, 0.0), DVec3::new(200.0, -40.0, 0.0)],
                like: Some("circuit".into()),
            }],
        )
        .unwrap();
        let mut cache = BuildCache::default();
        let _ = build_with(&p, &mut cache);
        // One road changes, the other does not; then only the terrain's settings.
        let edits = [
            Op::MoveNode {
                line: "pit".into(),
                index: 1,
                pos: DVec3::new(210.0, -45.0, 1.0),
            },
            Op::SetTerrain {
                terrain: crate::project::Terrain {
                    margin: 120.0,
                    ..p.terrain.clone()
                },
            },
        ];
        for op in edits {
            apply_all(&mut p, std::slice::from_ref(&op)).unwrap();
            let cached = build_with(&p, &mut cache);
            let fresh = build(&p);
            assert_eq!(meshes(&cached), meshes(&fresh), "after {op:?}");
        }
    }
}
