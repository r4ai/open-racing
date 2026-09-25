//! Track projects: the editable source of a circuit, and its baking into the track
//! package the simulator reads.
//!
//! A project describes a track the way it is built rather than as meshes: roads laid
//! along splines with their width, banking and crown, strips beside them (kerbs,
//! run-off, gravel, grass) limited to stretches of the road, painted lines, barriers,
//! and markers for the start line, sectors, grid and pit lane. [`bake::bake`] builds
//! the meshes the physics and the renderer need from it; the open-racing-editor app
//! edits it.
//!
//! The crate does not depend on any renderer, so projects can be baked and checked from
//! tests and scripts.

pub mod assets;
pub mod bake;
pub mod builtin;
pub mod centreline;
pub mod corners;
pub mod curve;
pub mod dem;
pub mod geo;
pub mod inspect;
pub mod model;
pub mod ops;
pub mod overlap;
pub mod pitlane;
pub mod preview;
pub mod project;
pub mod road;
pub mod spline;
pub mod terrain;
pub mod validate;

use std::path::PathBuf;

pub use bake::{Cache, Scene, bake};
pub use project::*;

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, Box<ron::error::SpannedError>),
    Invalid(String),
    Package(open_racing_track::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Parse(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Invalid(msg) => f.write_str(msg),
            Self::Package(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<open_racing_track::Error> for Error {
    fn from(e: open_racing_track::Error) -> Self {
        Self::Package(e)
    }
}

/// Where projects are kept by default: `<content>/track-src/`.
pub fn projects_dir() -> PathBuf {
    open_racing_track::content_dir().join("track-src")
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use open_racing_sim::Surface;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("open-racing-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn project_round_trips() {
        let dir = temp_dir("round-trip");
        let p = Project::new("oval");
        p.save(&dir).unwrap();
        assert_eq!(Project::load(&dir).unwrap(), p);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn curve_controls_round_trip() {
        let mut p = Project::new("curve-round-trip");
        p.roads[0].nodes[1].handles = NodeHandles::Free {
            incoming: DVec3::new(-1.0, 2.0, 0.0),
            outgoing: DVec3::new(4.0, 0.0, 0.0),
        };
        p.roads[0].width_left.keys.push(Key {
            u: 1.0,
            value: 6.0,
            slope_in: 0.2,
            slope_out: 0.0,
        });
        let dir = temp_dir("curve-round-trip");
        p.save(&dir).unwrap();
        assert_eq!(Project::load(&dir).unwrap(), p);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edited_curves_bake_and_drive() {
        let mut p = Project::new("edited-curves");
        p.roads[0].nodes[0].handles = NodeHandles::Aligned {
            outgoing: DVec3::new(30.0, 0.0, 0.0),
            incoming_length: 25.0,
        };
        p.roads[0].width_left.set(1.0, 6.5);
        p.roads[0].width_left.keys[0].slope_out = 0.2;
        p.roads[0].width_left.keys[1].slope_in = 0.1;
        p.roads[0].bank.set(1.0, 0.03);
        p.roads[0].bank.keys[1].slope_out = -0.01;
        let package = bake(&p, Path::new("."), &mut Cache::default()).unwrap();
        let report = validate::check(&package, true);
        assert!(report.ok(), "{report}");
    }

    #[test]
    fn baked_package_drives() {
        let project = Project::new("oval");
        let dir = temp_dir("bake");
        let package = bake(&project, &dir, &mut Cache::default()).unwrap();
        package.save(&dir).unwrap();
        let package = open_racing_track::TrackPackage::load(&dir, true).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let track = package.build_track().unwrap();

        // Surfaces across the start straight: asphalt, then the grass (no kerbs on the
        // straight), and a wall 20 m beyond the edge.
        let s = 100.0;
        let at = |d: f64| {
            let smp = track.sample_at(s);
            track.query(smp.pos + smp.lateral * d + DVec3::Z * 0.5, 0)
        };
        assert_eq!(at(0.0).surface, Surface::Asphalt);
        assert_eq!(at(5.5).surface, Surface::Asphalt);
        assert_eq!(at(-8.0).surface, Surface::Grass);
        assert_eq!(
            at(40.0).surface,
            Surface::Grass,
            "terrain beyond the strips"
        );
        let smp = track.sample_at(s);
        assert!(
            track
                .wall_contact(smp.pos + smp.lateral * 26.0 + DVec3::Z * 0.5, 0.3)
                .is_some()
        );

        // Race layout.
        assert_eq!(track.layout.sectors.len(), 2);
        assert_eq!(track.layout.grid.len(), 12);
        let pole = track.layout.start();
        assert!(pole.s > track.length - 20.0 && pole.d > 0.0);

        let report = validate::check(&package, true);
        assert!(report.ok(), "{report}");
    }

    #[test]
    fn kerbs_sit_in_the_corners() {
        let project = Project::new("oval");
        let package = bake(&project, Path::new("."), &mut Cache::default()).unwrap();
        let track = package.build_track().unwrap();
        // The middle of the first corner: node 3, on the outside (right) and inside.
        let main = curve::Sampled::new(&project.roads[0], 2.0);
        let p = bake::frame_at_u(&main, 3.0).pos;
        let c = track.locate(p, track.nearest_index(p));
        let smp = track.sample_at(c.s);
        let at = |d: f64| {
            track
                .query(smp.pos + smp.lateral * d + DVec3::Z * 0.5, 0)
                .surface
        };
        assert_eq!(at(6.6), Surface::Kerb);
        assert_eq!(at(-6.6), Surface::Kerb);
    }

    #[test]
    fn pit_lane_is_paved_beside_the_straight() {
        let mut project = Project::new("oval");
        let ops = ops::parse(
            r#"[
                AddRoad(name: "pit", closed: false,
                        nodes: [(20, -8, 0), (120, -22, 0), (230, -22, 0), (330, -8, 0)]),
                SetPit(pit: Some((road: "pit", speed_limit: 22.2, boxes: [1.2, 1.5],
                                  box_side: Right, box_offset: 4))),
            ]"#,
        )
        .unwrap();
        ops::apply_all(&mut project, &ops).unwrap();
        let package = bake(&project, Path::new("."), &mut Cache::default()).unwrap();
        let track = package.build_track().unwrap();
        let ground = track.ground.as_ref().unwrap();
        // Along the lane, across its width.
        let lane = curve::Sampled::new(&project.roads[1], 2.0);
        for f in lane.frames.iter().step_by(10) {
            for d in [-5.0, 0.0, 5.0] {
                let p = f.pos + f.lateral * d + DVec3::Z * 2.0;
                let hit = ground.raycast_down(p, 3.0).unwrap();
                assert_eq!(hit.surface.kind, Surface::Asphalt, "s = {}, d = {d}", f.s);
            }
        }
        // No wall across the lane.
        assert!(
            track
                .wall_contact(glam::DVec3::new(180.0, -25.0, 0.5), 0.5)
                .is_none()
        );
        let pit = track.layout.pit.as_ref().unwrap();
        assert_eq!(pit.boxes.len(), 2);
        // The lane spans the start line: in before it, out after it.
        let (entry, exit) = (pit.entry.unwrap(), pit.exit.unwrap());
        assert!(
            entry > track.length - 150.0 && exit < 250.0,
            "{entry} {exit}"
        );
    }

    #[test]
    fn roads_stay_on_top_of_the_ground_over_hills() {
        // The oval over a crest, a sag and banked turns, with and without strips.
        for strips in [true, false] {
            let mut project = Project::new("hills");
            let road = &mut project.roads[0];
            for (node, z) in road
                .nodes
                .iter_mut()
                .zip([0.0, 12.0, 4.0, -6.0, 8.0, 20.0, 3.0, -4.0, 6.0, 1.0])
            {
                node.pos.z = z;
            }
            road.bank = project::StationCurve::constant(0.08);
            if !strips {
                road.left.clear();
                road.right.clear();
                road.barriers.clear();
            }
            let package = bake(&project, Path::new("."), &mut Cache::default()).unwrap();
            let track = package.build_track().unwrap();
            let ground = track.ground.as_ref().unwrap();
            let main = curve::Sampled::new(&project.roads[0], 0.5);
            for f in &main.frames {
                for d in [-5.9, -4.0, -2.0, 0.0, 2.0, 4.0, 5.9] {
                    let on_road = f.pos + f.lateral * d;
                    let hit = ground.raycast_down(on_road + DVec3::Z * 2.0, 4.0).unwrap();
                    assert_eq!(
                        hit.surface.kind,
                        Surface::Asphalt,
                        "strips {strips}, s = {}, d = {d}",
                        f.s
                    );
                    // The crown lifts the middle by up to 5 cm.
                    let dz = hit.point.z - on_road.z;
                    assert!((-0.02..0.08).contains(&dz), "s = {}, d = {d}: {dz}", f.s);
                }
            }
        }
    }

    #[test]
    fn splines_drape_kerbs_and_stand_walls_anywhere() {
        let mut project = Project::new("oval");
        // Lift the far end of the straight so that draping has something to follow.
        project.roads[0].nodes[1].pos.z = 6.0;
        let ops = ops::parse(
            r#"[
                PutSpline(spline: (name: "chicane kerb", closed: false, drape: true,
                    nodes: [(pos: (60, 5, 50)), (pos: (120, 5, 50)), (pos: (180, 5, 50))], resolution: 1,
                    shape: Band(width: 1.5, profile: Crown(0.04), surface: "kerb",
                                material: "kerb", lift: 0.02))),
                PutSpline(spline: (name: "infield wall", closed: false, drape: true,
                    nodes: [(pos: (100, 60, 0)), (pos: (160, 60, 0))], resolution: 2,
                    shape: Wall(height: 1.2, thickness: 0.4, material: "concrete"))),
                AddNode(line: "infield wall", pos: (220, 70, 0)),
            ]"#,
        )
        .unwrap();
        ops::apply_all(&mut project, &ops).unwrap();
        assert_eq!(project.splines[1].nodes.len(), 3);

        let scene = bake::build(&project);
        let ground = scene
            .ground
            .build(&project.surfaces.iter().map(|s| s.props).collect::<Vec<_>>());
        let road = curve::Sampled::new(&project.roads[0], 1.0);
        for x in [70.0, 120.0, 170.0] {
            let p = DVec3::new(x, 5.0, 20.0);
            let hit = ground.raycast_down(p, 0.0).unwrap();
            assert_eq!(hit.surface.kind, Surface::Kerb, "x = {x}");
            // On the road (under the kerb's middle), not at the nodes' 50 m.
            let f = road.frames[road.nearest(p)];
            assert!(
                (hit.point.z - f.pos.z).abs() < 0.3,
                "x = {x}: {}",
                hit.point.z
            );
        }
        // The wall stands on the terrain along its last segment too.
        let p = DVec3::new(190.0, 65.0, 0.0);
        let floor = ground.raycast_down(p.with_z(50.0), 0.0).unwrap().point;
        assert!(ground.wall_contact(floor + DVec3::Z * 0.6, 0.5).is_some());
        assert!(
            ground
                .wall_contact(floor + DVec3::new(0.0, 3.0, 0.6), 0.5)
                .is_none()
        );
    }

    /// A 2 m square standing upright, facing glTF's +Z (the simulation's −Y), as a
    /// .gltf with its buffer beside it.
    fn write_panel(dir: &Path) {
        std::fs::create_dir_all(dir.join("assets/models")).unwrap();
        let mut bin = Vec::new();
        for v in [[0f32, 0., 0.], [2., 0., 0.], [2., 2., 0.], [0., 2., 0.]] {
            bin.extend(v.iter().flat_map(|x| x.to_le_bytes()));
        }
        for i in [0u16, 1, 2, 0, 2, 3] {
            bin.extend(i.to_le_bytes());
        }
        std::fs::write(dir.join("assets/models/panel.bin"), &bin).unwrap();
        let gltf = r#"{
            "asset": {"version": "2.0"},
            "scene": 0, "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}],
            "buffers": [{"uri": "panel.bin", "byteLength": 60}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 48},
                            {"buffer": 0, "byteOffset": 48, "byteLength": 12}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
                 "min": [0, 0, 0], "max": [2, 2, 0]},
                {"bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR"}]
        }"#;
        std::fs::write(dir.join("assets/models/panel.gltf"), gltf).unwrap();
    }

    #[test]
    fn props_stand_where_placed_and_block_when_solid() {
        let dir = temp_dir("props");
        write_panel(&dir);
        let mut project = Project::new("oval");
        let ops = ops::parse(
            r#"[
                PutProp(prop: (name: "board", model: "assets/models/panel.gltf",
                    pos: (100, 60, 30), yaw: 0, scale: 2, drape: true, collide: true)),
                MoveProp(name: "board", yaw: Some(0.5)),
            ]"#,
        )
        .unwrap();
        ops::apply_all(&mut project, &ops).unwrap();
        let package = bake(&project, &dir, &mut Cache::default()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let track = package.build_track().unwrap();
        // Turned half a radian, 4 m wide and high, standing on the terrain.
        let along = DVec3::new(0.5f64.cos(), 0.5f64.sin(), 0.0);
        let floor = track
            .ground
            .as_ref()
            .unwrap()
            .raycast_down(DVec3::new(100.0, 60.0, 50.0), 0.0)
            .unwrap()
            .point;
        let mid = floor + along * 2.0 + DVec3::Z * 2.0;
        assert!(track.wall_contact(mid, 0.3).is_some());
        assert!(
            track.wall_contact(mid + DVec3::Z * 3.0, 0.3).is_none(),
            "4 m high"
        );
        assert!(
            track
                .wall_contact(floor + along * 5.0 + DVec3::Z, 0.3)
                .is_none(),
            "4 m wide"
        );
        let visual = package.visual.unwrap();
        assert!(visual.meshes.iter().any(|m| {
            m.positions
                .iter()
                .any(|p| (p[2] as f64 - floor.z - 4.0).abs() < 1e-3)
        }));
    }

    use std::path::Path;
}
