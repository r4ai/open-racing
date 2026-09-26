//! Converts track and car folders in the Assetto Corsa format into open-racing track and
//! car packages (see `car` for cars).
//!
//! Conversion runs ahead of time, on files the user supplies; the runtime only reads
//! the resulting package. Protected (encrypted) models are rejected, and packed data is
//! not unpacked.
//!
//! A track folder contains KN5 models listed in `models.ini` (or `models_<layout>.ini`
//! for multi-layout tracks), and per layout `ai/fast_lane.ai` and `data/surfaces.ini`.
//! Physical meshes are recognised by name: a digit prefix followed by a surface key,
//! e.g. `1ROAD_05` or `2KERB`, and `WALL` for solid walls.
//!
//! The AI line is used only as a geometric reference: it becomes the package's
//! centreline, which provides track coordinates for progress, observations and lap
//! timing. The tyres ride on the road meshes.

mod ai;
pub mod car;
mod car_physics;
mod ini;
mod json;
mod kn5;
mod lut;
mod material;
mod reader;

use std::path::{Path, PathBuf};

use glam::{DVec3, Vec3};
use open_racing_sim::track::heading;
use open_racing_sim::{
    GridSlot, Layout, PitLane, Pose, Surface, SurfaceProps, Track, TrackDef, TrackPoint,
};
use open_racing_track::{Ground, PatchKind, TrackPackage, VisualBuilder};

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Format(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Format(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {}

/// Converts the files' Y-up coordinates to the simulation's Z-up world.
fn to_sim(v: DVec3) -> DVec3 {
    DVec3::new(v.x, -v.z, v.y)
}

/// Spacing of the centreline control points taken from the AI line, in m.
const CONTROL_SPACING: f64 = 4.0;
/// Largest gap between the last and first AI point of a closed circuit, in m.
const MAX_LOOP_GAP: f64 = 40.0;
const DEFAULT_HALF_WIDTH: f64 = 5.0;
/// Name prefixes of the game's marker objects (grid, pit, timing, audio and reverb),
/// which it never renders. Other `AC_` objects, such as start lights, are rendered.
const MARKER_PREFIXES: [&str; 7] = [
    "AC_START_",
    "AC_PIT_",
    "AC_TIME_",
    "AC_HOTLAP_START_",
    "AC_AB_",
    "AC_AUDIO_",
    "AC_REVERB_",
];

fn is_marker(mesh_name: &str) -> bool {
    MARKER_PREFIXES.iter().any(|p| mesh_name.starts_with(p))
}

/// A surface type from `surfaces.ini`.
#[derive(Clone, Debug, PartialEq)]
struct SurfaceDef {
    key: String,
    friction: f64,
    damping: f64,
    valid_track: bool,
    /// How much dirt tyres pick up (`DIRT_ADDITIVE`), when given.
    dirt: Option<f64>,
    /// Rolling sound (`WAV`), which tells kerbs, grass and sand apart when the key does not.
    wav: String,
}

/// Surfaces a track may use without defining them itself.
fn builtin_surfaces() -> Vec<SurfaceDef> {
    let s = |key: &str, friction, damping, valid_track| SurfaceDef {
        key: key.into(),
        friction,
        damping,
        valid_track,
        dirt: None,
        wav: String::new(),
    };
    vec![
        s("ROAD", 1.0, 0.0, true),
        s("KERB", 0.95, 0.0, true),
        s("GRASS", 0.6, 0.06, false),
        s("SAND", 0.5, 0.15, false),
        s("GRAVEL", 0.55, 0.1, false),
        s("DIRT", 0.7, 0.04, false),
    ]
}

/// Parses `surfaces.ini` and merges it over the built-in surfaces.
fn surfaces(ini_src: Option<&str>) -> Vec<SurfaceDef> {
    let mut out = builtin_surfaces();
    for sec in ini_src.map(ini::parse).unwrap_or_default() {
        if !sec.name.starts_with("SURFACE") {
            continue;
        }
        let Some(key) = sec.get("KEY").filter(|k| !k.is_empty()) else {
            continue;
        };
        let def = SurfaceDef {
            key: key.to_ascii_uppercase(),
            friction: sec.get_f64("FRICTION").unwrap_or(1.0),
            damping: sec.get_f64("DAMPING").unwrap_or(0.0),
            valid_track: sec.get_f64("IS_VALID_TRACK").unwrap_or(0.0) != 0.0,
            dirt: sec.get_f64("DIRT_ADDITIVE"),
            wav: sec.get("WAV").unwrap_or_default().to_ascii_lowercase(),
        };
        match out.iter_mut().find(|s| s.key == def.key) {
            Some(s) => *s = def,
            None => out.push(def),
        }
    }
    out
}

/// Physical role of a mesh, from its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Physical {
    Ground(usize),
    Wall,
}

/// `1ROAD_05` → the surface whose key is the longest prefix of `ROAD_05`.
fn classify(mesh_name: &str, surfaces: &[SurfaceDef]) -> Option<Physical> {
    let rest = mesh_name.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == mesh_name.len() {
        return None;
    }
    let rest = rest.to_ascii_uppercase();
    if rest.starts_with("WALL") {
        return Some(Physical::Wall);
    }
    surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| rest.starts_with(&s.key))
        .max_by_key(|(_, s)| s.key.len())
        .map(|(i, _)| Physical::Ground(i))
}

/// What a surface is made of, from its key (`GRAVEL`, `CURB_2`, `GRS-CUT-A`), else its
/// rolling sound, else whether it counts as track and how dirty it makes tyres.
fn surface_kind(s: &SurfaceDef) -> Surface {
    const KINDS: [(&[&str], Surface); 5] = [
        (&["KERB", "CURB", "KURB", "RUMBLE"], Surface::Kerb),
        (&["SAND", "GRAVEL"], Surface::Gravel),
        (&["DIRT", "MUD", "SOIL"], Surface::Dirt),
        (&["CARPET", "TURF", "ASTRO"], Surface::Turf),
        (&["GRASS", "GRS"], Surface::Grass),
    ];
    const SOUNDS: [(&str, Surface); 5] = [
        ("kerb", Surface::Kerb),
        ("kurb", Surface::Kerb),
        ("sand", Surface::Gravel),
        ("gravel", Surface::Gravel),
        ("grass", Surface::Grass),
    ];
    if let Some((_, kind)) = KINDS
        .iter()
        .find(|(keys, _)| keys.iter().any(|k| s.key.contains(k)))
    {
        return *kind;
    }
    if s.wav.contains("extraturf") {
        return Surface::Turf;
    }
    if let Some((_, kind)) = SOUNDS.iter().find(|(wav, _)| s.wav.starts_with(wav)) {
        return *kind;
    }
    match (s.valid_track, s.dirt.unwrap_or(0.0)) {
        (true, _) => Surface::Asphalt,
        (false, dirt) if dirt >= 0.5 => Surface::Grass,
        _ => Surface::Runoff,
    }
}

/// Simulation properties of each surface. Grip is relative to the grippiest
/// valid-track surface.
fn surface_props(surfaces: &[SurfaceDef]) -> Vec<SurfaceProps> {
    let reference = surfaces
        .iter()
        .filter(|s| s.valid_track)
        .map(|s| s.friction)
        .fold(0.0, f64::max);
    let reference = if reference > 0.0 { reference } else { 1.0 };
    surfaces
        .iter()
        .map(|s| SurfaceProps {
            kind: surface_kind(s),
            grip: (s.friction / reference).clamp(0.05, 1.5),
            drag: s.damping.clamp(0.0, 0.3),
            dirt: s.dirt.map(|d| d.clamp(0.0, 1.0)),
        })
        .collect()
}

/// Files making up one layout of a track folder.
#[derive(Clone, Debug)]
struct LayoutFiles {
    models: Vec<(PathBuf, DVec3)>,
    ai_line: PathBuf,
    surfaces: PathBuf,
}

/// Locates the files of `layout` (`None` for single-layout tracks).
fn layout_files(dir: &Path, layout: Option<&str>) -> Result<LayoutFiles, Error> {
    let data_dir = match layout {
        Some(l) => dir.join(l),
        None => dir.to_path_buf(),
    };
    let models_ini = match layout {
        Some(l) => dir.join(format!("models_{l}.ini")),
        None => dir.join("models.ini"),
    };
    let models = match std::fs::read_to_string(&models_ini) {
        Ok(src) => ini::parse(&src)
            .iter()
            .filter(|s| s.name.starts_with("MODEL"))
            .filter_map(|s| {
                let file = s.get("FILE")?;
                let pos: Vec<f64> = s
                    .get("POSITION")
                    .unwrap_or("0,0,0")
                    .split(',')
                    .filter_map(|v| v.trim().parse().ok())
                    .collect();
                let offset = if pos.len() == 3 {
                    DVec3::new(pos[0], pos[1], pos[2])
                } else {
                    DVec3::ZERO
                };
                Some((dir.join(file), offset))
            })
            .collect(),
        Err(_) => {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            vec![(dir.join(format!("{name}.kn5")), DVec3::ZERO)]
        }
    };
    if models.is_empty() {
        return Err(Error::Format(format!(
            "{}: no models listed",
            models_ini.display()
        )));
    }
    Ok(LayoutFiles {
        models,
        ai_line: data_dir.join("ai/fast_lane.ai"),
        surfaces: data_dir.join("data/surfaces.ini"),
    })
}

/// Layouts of a track folder: `[None]` for a single-layout track, otherwise the names
/// from `models_<layout>.ini` that have an AI line.
pub fn layouts(dir: &Path) -> Vec<Option<String>> {
    let mut named: Vec<Option<String>> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let layout = name
                .strip_prefix("models_")?
                .strip_suffix(".ini")?
                .to_string();
            dir.join(&layout)
                .join("ai/fast_lane.ai")
                .exists()
                .then_some(Some(layout))
        })
        .collect();
    named.sort();
    if named.is_empty() && dir.join("ai/fast_lane.ai").exists() {
        vec![None]
    } else {
        named
    }
}

fn read(path: &Path) -> Result<Vec<u8>, Error> {
    std::fs::read(path).map_err(|e| Error::Io(path.to_path_buf(), e))
}

/// Converts one layout of a track folder into a package. `name` becomes the track's name.
pub fn convert(dir: &Path, layout: Option<&str>, name: &str) -> Result<TrackPackage, Error> {
    let files = layout_files(dir, layout)?;
    let surface_src = std::fs::read_to_string(&files.surfaces).ok();
    let surfaces = surfaces(surface_src.as_deref());
    let mut ground = Ground::default();
    let mut visual = VisualBuilder::new();
    let mut markers = Vec::new();
    let mut textures = material::TextureCache::default();

    for (path, offset) in &files.models {
        let kn5 = kn5::parse(&read(path)?)
            .map_err(|e| Error::Format(format!("{}: {e}", path.display())))?;
        let place = |p: DVec3| to_sim(p + *offset).as_vec3().to_array();
        markers.extend(
            kn5.dummies
                .iter()
                .map(|d| (d.name.clone(), to_sim(d.position + *offset))),
        );
        let rendered = |m: &&kn5::Mesh| {
            m.visible && m.renderable && !m.indices.is_empty() && !is_marker(&m.name)
        };
        let mut materials = material::Materials::new(
            &kn5,
            kn5.meshes.iter().filter(rendered).map(|m| m.material),
            &mut textures,
        );

        for mesh in &kn5.meshes {
            if mesh.indices.is_empty() {
                continue;
            }
            let positions: Vec<[f32; 3]> = mesh.positions.iter().map(|&p| place(p)).collect();
            let normals: Vec<[f32; 3]> = mesh
                .normals
                .iter()
                .map(|&n| to_sim(n).as_vec3().to_array())
                .collect();
            let indices = oriented(&positions, &normals, &mesh.indices);
            match classify(&mesh.name, &surfaces) {
                Some(Physical::Ground(s)) => {
                    ground.add(PatchKind::Ground(s as u16), &positions, &normals, &indices)
                }
                Some(Physical::Wall) => ground.add(PatchKind::Wall, &positions, &[], &indices),
                None => {}
            }
            if rendered(&mesh) {
                let material = materials.get(&mut visual, &mut textures, mesh.material);
                visual.add_mesh(
                    material,
                    mesh.cast_shadows,
                    &positions,
                    &normals,
                    &mesh.uvs,
                    &indices,
                );
            }
        }
    }
    if !ground.is_drivable() {
        return Err(Error::Format(format!(
            "{}: no road meshes found",
            dir.display()
        )));
    }

    let ai_points = ai::parse(&read(&files.ai_line)?)
        .map_err(|e| Error::Format(format!("{}: {e}", files.ai_line.display())))?;
    let marker = |n: &str| markers.iter().find(|(m, _)| m == n).map(|(_, p)| *p);
    let start = match (marker("AC_TIME_0_L"), marker("AC_TIME_0_R")) {
        (Some(l), Some(r)) => Some((l + r) * 0.5),
        _ => marker("AC_START_0"),
    };
    let centreline = centreline(name, &ai_points, start)?;
    let layout = race_layout(&centreline, &markers)?;
    Ok(TrackPackage {
        centreline,
        surfaces: surface_props(&surfaces),
        layout,
        environment: None,
        ground,
        visual: Some(visual.build()),
    })
}

/// Speed limit in pit lanes, which the game sets per server: 80 km/h, the usual one.
const PIT_SPEED_LIMIT: f64 = 80.0 / 3.6;

/// The race layout from the game's markers: timing lines `AC_TIME_<n>_L/R` (n ≥ 1 are
/// the sector boundaries; 0 is the start/finish line the centreline starts at), grid
/// slots `AC_START_<n>` and pit boxes `AC_PIT_<n>`.
fn race_layout(centreline: &TrackDef, markers: &[(String, DVec3)]) -> Result<Layout, Error> {
    let track = Track::new(centreline).map_err(|e| Error::Format(e.to_string()))?;
    let marker = |n: &str| markers.iter().find(|(m, _)| m == n).map(|(_, p)| *p);
    let locate = |p: DVec3| track.locate(p, track.nearest_index(p));
    let numbered = |prefix: &str| {
        (0..)
            .map_while(|i| marker(&format!("{prefix}{i}")))
            .collect::<Vec<_>>()
    };
    let mut sectors: Vec<f64> = (1..)
        .map_while(|i| {
            let (l, r) = (
                marker(&format!("AC_TIME_{i}_L"))?,
                marker(&format!("AC_TIME_{i}_R"))?,
            );
            Some(locate((l + r) * 0.5).s)
        })
        .filter(|&s| s > 0.0)
        .collect();
    sectors.sort_by(f64::total_cmp);
    let grid = numbered("AC_START_")
        .into_iter()
        .map(|p| {
            let c = locate(p);
            GridSlot { s: c.s, d: c.d }
        })
        .collect();
    let boxes: Vec<Pose> = numbered("AC_PIT_")
        .into_iter()
        .map(|p| Pose {
            pos: p.into(),
            heading: heading(locate(p).sample.tangent),
        })
        .collect();
    Ok(Layout {
        sectors,
        grid,
        pit: (!boxes.is_empty()).then_some(PitLane {
            speed_limit: PIT_SPEED_LIMIT,
            entry: None,
            exit: None,
            boxes,
        }),
    })
}

/// Winds each triangle counter-clockwise as seen from the side its vertex normals point
/// to: the side the game draws it from. Two-sided surfaces are modelled as a pair of
/// triangles facing opposite ways, so each triangle is oriented on its own.
fn oriented(positions: &[[f32; 3]], normals: &[[f32; 3]], indices: &[u32]) -> Vec<u32> {
    let mut indices = indices.to_vec();
    if normals.len() != positions.len() {
        return indices;
    }
    let v = |i: u32| Vec3::from(positions[i as usize]);
    let n = |i: u32| Vec3::from(normals[i as usize]);
    for t in indices.as_chunks_mut::<3>().0 {
        let [a, b, c] = *t;
        if (v(b) - v(a)).cross(v(c) - v(a)).dot(n(a) + n(b) + n(c)) < 0.0 {
            t.swap(1, 2);
        }
    }
    indices
}

/// Builds the centreline from the AI line: thinned to control points, starting at the
/// point nearest `start` (the timing line).
fn centreline(name: &str, points: &[ai::AiPoint], start: Option<DVec3>) -> Result<TrackDef, Error> {
    let pos: Vec<DVec3> = points.iter().map(|p| to_sim(p.pos)).collect();
    let gap = (pos[pos.len() - 1] - pos[0]).length();
    if gap > MAX_LOOP_GAP {
        return Err(Error::Format(format!(
            "the AI line does not close ({gap:.0} m gap); point-to-point tracks are not supported"
        )));
    }
    let first = start
        .map(|s| {
            (0..pos.len())
                .min_by(|&a, &b| {
                    (pos[a] - s)
                        .length_squared()
                        .total_cmp(&(pos[b] - s).length_squared())
                })
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let half = |w: Option<f64>| {
        w.filter(|w| w.is_finite() && *w > 0.0)
            .unwrap_or(DEFAULT_HALF_WIDTH)
            .clamp(1.0, 40.0)
    };

    let mut out: Vec<TrackPoint> = Vec::new();
    let mut last: Option<DVec3> = None;
    for k in 0..pos.len() {
        let i = (first + k) % pos.len();
        if last.is_some_and(|l| (pos[i] - l).length() < CONTROL_SPACING) {
            continue;
        }
        last = Some(pos[i]);
        out.push(TrackPoint {
            pos: pos[i].into(),
            width_left: half(points[i].side_left),
            width_right: half(points[i].side_right),
            bank: 0.0,
        });
    }
    // Drop trailing points that crowd the start.
    while out.len() > 4
        && (DVec3::from(out[out.len() - 1].pos) - DVec3::from(out[0].pos)).length()
            < CONTROL_SPACING * 0.5
    {
        out.pop();
    }
    Ok(TrackDef {
        name: name.into(),
        points: out,
        kerb_width: 1.0,
        kerb_height: 0.0,
        runoff_width: 60.0,
        spacing: 1.0,
    })
}

#[cfg(test)]
mod tests {
    use open_racing_sim::Track;

    use super::*;

    #[test]
    fn classifies_mesh_names() {
        let s = surfaces(Some(
            "[SURFACE_0]\nKEY=ROAD_B\nFRICTION=0.9\nIS_VALID_TRACK=1\n",
        ));
        let key = |p: Option<Physical>| match p {
            Some(Physical::Ground(i)) => Some(s[i].key.as_str()),
            _ => None,
        };
        assert_eq!(key(classify("1ROAD_B_03", &s)), Some("ROAD_B"));
        assert_eq!(key(classify("1ROAD", &s)), Some("ROAD"));
        assert_eq!(key(classify("12grass_x", &s)), Some("GRASS"));
        assert_eq!(classify("1WALL_pit", &s), Some(Physical::Wall));
        assert_eq!(classify("ROAD", &s), None);
        assert_eq!(classify("1TREE", &s), None);
    }

    #[test]
    fn surface_grip_is_relative_to_the_track() {
        let s = surfaces(Some(
            "[SURFACE_0]\nKEY=ROAD\nFRICTION=0.98\nIS_VALID_TRACK=1\n[SURFACE_1]\nKEY=OUT\nFRICTION=0.49\n",
        ));
        let p = surface_props(&s);
        let road = s.iter().position(|s| s.key == "ROAD").unwrap();
        let out = s.iter().position(|s| s.key == "OUT").unwrap();
        let kerb = s.iter().position(|s| s.key == "KERB").unwrap();
        assert_eq!(p[road].kind, Surface::Asphalt);
        assert!((p[road].grip - 1.0).abs() < 1e-12);
        assert_eq!(p[out].kind, Surface::Runoff);
        assert!((p[out].grip - 0.5).abs() < 1e-12);
        assert_eq!(p[kerb].kind, Surface::Kerb);
    }

    #[test]
    fn surface_kinds_follow_key_then_sound() {
        let s = surfaces(Some(concat!(
            "[SURFACE_0]
KEY=GRS-CUT-A
FRICTION=0.6
DIRT_ADDITIVE=1
",
            "[SURFACE_1]
KEY=CARPET
FRICTION=0.7
DIRT_ADDITIVE=0.1
IS_VALID_TRACK=1
WAV=extraturf.wav
",
            "[SURFACE_2]
KEY=TRAP
FRICTION=0.8
DAMPING=0.1
WAV=sand.wav
",
            "[SURFACE_3]
KEY=CURB_2
FRICTION=0.9
IS_VALID_TRACK=1
",
            "[SURFACE_4]
KEY=GRAVEL_B
WAV=kerb.wav
IS_VALID_TRACK=1
",
            "[SURFACE_5]
KEY=VERGE
DIRT_ADDITIVE=1
",
            "[SURFACE_6]
KEY=TARMAC
IS_VALID_TRACK=1
",
        )));
        let p = surface_props(&s);
        let kind = |key: &str| p[s.iter().position(|s| s.key == key).unwrap()].kind;
        assert_eq!(kind("GRS-CUT-A"), Surface::Grass);
        assert_eq!(kind("CARPET"), Surface::Turf);
        assert_eq!(kind("TRAP"), Surface::Gravel);
        assert_eq!(kind("CURB_2"), Surface::Kerb);
        assert_eq!(kind("GRAVEL_B"), Surface::Gravel);
        assert_eq!(kind("VERGE"), Surface::Grass);
        assert_eq!(kind("TARMAC"), Surface::Asphalt);
        assert_eq!(kind("DIRT"), Surface::Dirt);
        let carpet = s.iter().position(|s| s.key == "CARPET").unwrap();
        assert_eq!(p[carpet].dirt(), 0.1);
    }

    #[test]
    fn centreline_from_ai_line() {
        let pts = ai::parse(&ai::tests::sample_square(100.0, 50, true)).unwrap();
        // Start near the second corner: file (100, 0, -100) is sim (100, 100, 0).
        let def = centreline("square", &pts, Some(DVec3::new(100.0, 100.0, 0.0))).unwrap();
        assert_eq!(def.points[0].pos, (100.0, 100.0, 0.0));
        assert_eq!(def.points[0].width_left, 5.0);
        assert_eq!(def.points[0].width_right, 3.0);
        assert!(
            def.points.len() >= 80 && def.points.len() <= 100,
            "{}",
            def.points.len()
        );
        let track = Track::new(&def).unwrap();
        assert!((track.length - 400.0).abs() < 10.0, "{}", track.length);
    }

    #[test]
    fn open_ai_line_is_rejected() {
        let mut pts = ai::parse(&ai::tests::sample_square(100.0, 50, true)).unwrap();
        pts.truncate(120);
        assert!(centreline("open", &pts, None).is_err());
    }

    #[test]
    fn markers_are_not_rendered() {
        assert!(is_marker("AC_PIT_12"));
        assert!(is_marker("AC_TIME_0_L"));
        assert!(is_marker("AC_AUDIO_0"));
        assert!(is_marker("AC_REVERB_BRIDGE"));
        assert!(!is_marker("AC_SEMAPHORE_RED_1"));
        assert!(!is_marker("1ROAD_AC_START"));
    }

    #[test]
    fn triangles_face_their_normals() {
        let p = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(oriented(&p, &[[0.0, 0.0, 1.0]; 3], &[0, 1, 2]), [0, 1, 2]);
        assert_eq!(oriented(&p, &[[0.0, 0.0, -1.0]; 3], &[0, 1, 2]), [0, 2, 1]);
        assert_eq!(oriented(&p, &[], &[0, 1, 2]), [0, 1, 2]);
        // A two-sided pair: each triangle faces its own normals.
        let quad = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let normals = [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, -1.0],
        ];
        assert_eq!(
            oriented(&quad, &normals, &[0, 1, 2, 3, 4, 5]),
            [0, 1, 2, 3, 5, 4]
        );
    }

    #[test]
    fn converts_a_synthetic_folder() {
        let dir = std::env::temp_dir()
            .join(format!("open-racing-ac-{}", std::process::id()))
            .join("sample_track");
        std::fs::create_dir_all(dir.join("ai")).unwrap();
        std::fs::write(dir.join("sample_track.kn5"), kn5::tests::sample()).unwrap();
        std::fs::write(
            dir.join("ai/fast_lane.ai"),
            ai::tests::sample_square(100.0, 50, true),
        )
        .unwrap();

        let pkg = convert(&dir, None, "sample").unwrap();
        assert_eq!(pkg.centreline.name, "sample");
        assert_eq!(pkg.ground.patches.len(), 1);
        assert_eq!(pkg.ground.patches[0].kind, PatchKind::Ground(0));
        assert_eq!(pkg.surfaces[0].kind, Surface::Asphalt);
        let v = pkg.visual.as_ref().unwrap();
        // The texture in the sample is not a valid image, so the material has none.
        assert!(v.textures.is_empty());
        assert_eq!(v.materials[0].base_color_texture, None);
        assert_eq!(
            v.materials[0].alpha_mode,
            open_racing_track::AlphaMode::Mask(0.5)
        );
        // File (10, 0, 1) -> sim (10, -1, 0).
        assert_eq!(v.meshes[0].positions[2], [10.0, -1.0, 0.0]);
        // The start marker at file (11, 2, 3) -> sim (11, -3, 2) picks the AI point at (10, 0, 0).
        assert_eq!(pkg.centreline.points[0].pos, (10.0, 0.0, 0.0));
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
