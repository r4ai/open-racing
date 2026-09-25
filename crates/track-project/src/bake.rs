//! Turns a project into a track package: meshes for the physics and the renderer, the
//! centreline of the main road, and the race layout from the markers.

use std::collections::HashMap;
use std::path::Path;

use glam::DVec3;
use open_racing_sim::track::heading;
use open_racing_sim::{GridSlot, Layout, PitLane, Pose, Track, TrackDef, TrackPoint};
use open_racing_track::texture::{self, Mips};
use open_racing_track::{
    Ground, Material, PatchKind, Texture, TrackPackage, Visual, VisualBuilder,
};

use crate::Error;
use crate::curve::Sampled;
use crate::project::{MaterialDef, Project, TextureSource};
use crate::road::{self, RoadBuild, Solid};
use crate::terrain::{self, TerrainBuild};

/// Spacing of the centreline's points, m. The simulation eases between them.
const CENTRELINE_SPACING: f64 = 4.0;

/// Everything the project's meshes are built into.
pub struct Scene {
    pub roads: Vec<RoadBuild>,
    pub terrain: Option<TerrainBuild>,
}

pub fn build(project: &Project) -> Scene {
    let roads: Vec<RoadBuild> = (0..project.roads.len())
        .map(|i| road::build(project, i))
        .collect();
    let terrain = terrain::build(project, &roads);
    Scene { roads, terrain }
}

/// Prepared textures, kept between bakes so that each is encoded once.
#[derive(Default)]
pub struct Textures {
    cache: HashMap<TextureSource, Vec<u8>>,
}

impl Textures {
    /// The DDS of a texture; files are read from `dir`.
    pub fn get(&mut self, source: &TextureSource, dir: &Path) -> Result<Option<&[u8]>, Error> {
        if *source == TextureSource::None {
            return Ok(None);
        }
        if !self.cache.contains_key(source) {
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
            self.cache.insert(source.clone(), dds);
        }
        Ok(self.cache.get(source).map(Vec::as_slice))
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
    textures: &mut Textures,
    visual: &mut VisualBuilder,
) -> Result<(), Error> {
    for m in &project.materials {
        let MaterialDef {
            color,
            texture,
            roughness,
            reflectance,
            double_sided,
            ..
        } = m;
        let texture = textures.get(texture, dir)?.map(|data| {
            visual.add_texture(Texture {
                data: data.to_vec(),
            })
        });
        let [r, g, b] = color.map(srgb_to_linear);
        visual.add_material(Material {
            base_color: [r, g, b, 1.0],
            base_color_texture: texture,
            roughness: *roughness,
            reflectance: *reflectance,
            double_sided: *double_sided,
            ..Default::default()
        });
    }
    Ok(())
}

/// The project's materials only, for previews that add their own meshes.
pub fn materials(project: &Project, dir: &Path, textures: &mut Textures) -> Result<Visual, Error> {
    let mut visual = VisualBuilder::new();
    add_materials(project, dir, textures, &mut visual)?;
    Ok(visual.build())
}

/// Bakes the project into a package. Textures are read from the project's directory
/// `dir`.
pub fn bake(project: &Project, dir: &Path, textures: &mut Textures) -> Result<TrackPackage, Error> {
    project.validate()?;
    let scene = build(project);

    let mut ground = Ground::default();
    let mut visual = VisualBuilder::new();
    add_materials(project, dir, textures, &mut visual)?;
    for b in &scene.roads {
        for part in &b.solid {
            let kind = match part.kind {
                Solid::Ground(s) => PatchKind::Ground(s as u16),
                Solid::Wall => PatchKind::Wall,
            };
            let m = &part.mesh;
            ground.add(kind, &m.positions, &m.normals, &m.indices);
        }
        for part in &b.visual {
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
    }
    if let Some(t) = &scene.terrain {
        let s = &t.solid;
        ground.add(
            PatchKind::Ground(project.terrain.surface as u16),
            &s.positions,
            &s.normals,
            &s.indices,
        );
        for m in &t.chunks {
            visual.add_mesh(
                project.terrain.material as u32,
                false,
                &m.positions,
                &m.normals,
                &m.uvs,
                &m.indices,
            );
        }
    }

    let centreline = centreline(project, &scene.roads[project.main_road]);
    let layout = layout(project, &scene, &centreline)?;
    Ok(TrackPackage {
        centreline,
        surfaces: project.surfaces.iter().map(|s| s.props).collect(),
        layout,
        ground,
        visual: Some(visual.build()),
    })
}

/// The main road's centre, starting at the start/finish line.
fn centreline(project: &Project, main: &RoadBuild) -> TrackDef {
    let road = &project.roads[project.main_road];
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
    let main = &scene.roads[project.main_road].sampled;
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
        let road = &project.roads[p.road];
        let lane = &scene.roads[p.road].sampled;
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
