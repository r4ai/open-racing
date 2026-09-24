//! Converts car folders in the Assetto Corsa format into open-racing car packages.
//!
//! A car folder holds the car's model, `<folder>.kn5`, with its liveries in
//! `skins/<skin>/`, a UI description `ui/ui_car.json`, and its physics files either as a
//! `data/` folder or packed into `data.acd`. The packed form is not read: open-racing
//! does not unpack it. Without physics files the car's physics are estimated from the
//! model and the UI description on top of a base car (see `car_physics`).
//!
//! The model's parts move by the names of their nodes, as in the game: `WHEEL_LF`,
//! `WHEEL_RF`, `WHEEL_LR` and `WHEEL_RR` spin, steer and follow the suspension;
//! `SUSP_<corner>` steer and follow the suspension without spinning, as does whatever
//! shares a parent node with a wheel alone; `STEER_HR` turns with the steering. Low-detail
//! copies (`COCKPIT_LR`, `STEER_LR`), motion-blurred wheels (`…BLUR…`), damaged parts
//! (`DAMAGE…`) and unfastened belts (`CINTURE_OFF`) are left out.

use std::path::{Path, PathBuf};

use glam::DVec3;
use open_racing_car::{CarPackage, CarVisual, Part, SteeringWheel};
use open_racing_sim::CarModel;
use open_racing_track::VisualBuilder;

use crate::car_physics::{Geometry, Physics};
use crate::{Error, json, kn5, material, oriented, read};

/// Wheel nodes in the simulation's order: front-left, front-right, rear-left, rear-right.
const CORNERS: [&str; 4] = ["LF", "RF", "LR", "RR"];
/// The tread, whose width is the tyre's, lies beyond this share of the tyre's radius;
/// the sidewalls bulge wider below it.
const TREAD: f64 = 0.97;
/// Driver's eyes relative to the steering wheel's centre, along the column and up, in m,
/// when the physics files do not give them.
const EYE_BEHIND_WHEEL: f64 = 0.6;
const EYE_ABOVE_WHEEL: f64 = 0.12;

/// Converts the model's Y-up coordinates (x left, z forward) to the body axes: x forward,
/// y left, z up.
fn to_body(v: DVec3) -> DVec3 {
    DVec3::new(v.z, v.x, v.y)
}

/// Whether `dir` looks like a car folder rather than a track folder.
pub fn is_car_folder(dir: &Path) -> bool {
    dir.join("ui/ui_car.json").is_file()
        || dir.join("data.acd").is_file()
        || dir.join("data/car.ini").is_file()
}

/// Skins of a car folder, sorted: the first is the default.
pub fn skins(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join("skins"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect();
    names.sort();
    names
}

#[derive(Clone, Debug, Default)]
pub struct CarOptions {
    /// Livery to paint the car with; defaults to the first skin.
    pub skin: Option<String>,
    /// Folder of physics files, when the car folder has no `data/`.
    pub data: Option<PathBuf>,
    /// Car whose values stand in for those the folder does not give; defaults to the
    /// bundled GT3 car.
    pub base: Option<CarModel>,
    /// Largest texture edge in texels; defaults to `MAX_TEXTURE_SIZE`.
    pub max_texture_size: Option<usize>,
}

/// Largest texture edge kept by default. Car liveries come at up to 8192 texels, a
/// resolution that only shows from a metre away but takes 16 times the memory.
pub const MAX_TEXTURE_SIZE: usize = 4096;

pub struct CarConversion {
    pub package: CarPackage,
    /// Where each group of physics values came from.
    pub notes: Vec<String>,
    pub skin: Option<String>,
}

fn folder_name(dir: &Path) -> String {
    dir.canonicalize()
        .ok()
        .and_then(|d| d.file_name()?.to_str().map(String::from))
        .unwrap_or_default()
}

/// The model file: `<folder name>.kn5`, or else the largest KN5 file in the folder.
fn model_file(dir: &Path) -> Result<PathBuf, Error> {
    let named = dir.join(format!("{}.kn5", folder_name(dir)));
    if named.is_file() {
        return Ok(named);
    }
    std::fs::read_dir(dir)
        .map_err(|e| Error::Io(dir.to_path_buf(), e))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("kn5")))
        .filter(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| !s.eq_ignore_ascii_case("collider"))
        })
        .max_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(0))
        .ok_or_else(|| Error::Format(format!("{}: no KN5 model found", dir.display())))
}

/// Replaces the model's textures with the skin's files of the same name. The game's
/// liveries are DDS or PNG files; others are skipped.
fn apply_skin(kn5: &mut kn5::Kn5, skin_dir: &Path) {
    for entry in std::fs::read_dir(skin_dir).into_iter().flatten().flatten() {
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        let Some(t) = kn5
            .textures
            .iter_mut()
            .find(|t| t.name.eq_ignore_ascii_case(&name))
        else {
            continue;
        };
        match std::fs::read(entry.path()) {
            Ok(data) if data.starts_with(b"DDS ") || data.starts_with(b"\x89PNG") => t.data = data,
            Ok(_) => eprintln!("warning: skin texture {name}: only DDS and PNG are supported"),
            Err(e) => eprintln!("warning: skin texture {name}: {e}"),
        }
    }
}

/// Nodes that give the model's parts their motion.
struct Rig {
    wheels: [usize; 4],
    /// Nodes that move with each wheel without spinning.
    hubs: [Vec<usize>; 4],
    steering: Option<usize>,
    hidden: Vec<bool>,
}

impl Rig {
    fn new(kn5: &kn5::Kn5) -> Result<Self, Error> {
        let find = |name: &str| {
            kn5.dummies
                .iter()
                .position(|d| d.name.eq_ignore_ascii_case(name))
        };
        let wheels = CORNERS.map(|c| find(&format!("WHEEL_{c}")));
        let missing: Vec<String> = CORNERS
            .iter()
            .zip(&wheels)
            .filter(|(_, w)| w.is_none())
            .map(|(c, _)| format!("WHEEL_{c}"))
            .collect();
        if !missing.is_empty() {
            return Err(Error::Format(format!(
                "the model lacks the wheel nodes {}",
                missing.join(", ")
            )));
        }
        let wheels = wheels.map(|w| w.expect("checked"));
        let is_ancestor = |a: usize, mut n: usize| loop {
            match kn5.dummies[n].parent {
                Some(p) if p == a => return true,
                Some(p) => n = p,
                None => return false,
            }
        };
        let hubs = std::array::from_fn(|i| {
            let mut hub: Vec<usize> = find(&format!("SUSP_{}", CORNERS[i])).into_iter().collect();
            // A parent holding this wheel alone, below the root, carries the wheel's
            // upright and brake.
            if let Some(p) = kn5.dummies[wheels[i]].parent
                && kn5.dummies[p].parent.is_some()
                && (0..4).all(|j| j == i || !is_ancestor(p, wheels[j]))
            {
                hub.push(p);
            }
            hub
        });
        let steer_hr = find("STEER_HR");
        let steering = steer_hr.or_else(|| find("STEER_LR"));
        let hidden = kn5
            .dummies
            .iter()
            .map(|d| is_hidden(&d.name, steer_hr.is_some()))
            .collect();
        Ok(Self {
            wheels,
            hubs,
            steering,
            hidden,
        })
    }

    /// The part a mesh belongs to, or `None` when it is left out.
    fn part(&self, kn5: &kn5::Kn5, mesh: &kn5::Mesh) -> Option<Part> {
        if is_hidden(&mesh.name, true) {
            return None;
        }
        let mut part = None;
        let mut node = mesh.parent;
        while let Some(n) = node {
            if self.hidden[n] {
                return None;
            }
            part = part.or_else(|| {
                let wheel = |nodes: &dyn Fn(usize) -> bool| (0..4u8).find(|&i| nodes(i as usize));
                wheel(&|i| self.wheels[i] == n)
                    .map(Part::Wheel)
                    .or_else(|| wheel(&|i| self.hubs[i].contains(&n)).map(Part::Hub))
                    .or((self.steering == Some(n)).then_some(Part::SteeringWheel))
            });
            node = kn5.dummies[n].parent;
        }
        Some(part.unwrap_or(Part::Body))
    }
}

fn is_hidden(name: &str, has_high_detail_steering: bool) -> bool {
    let n = name.to_ascii_uppercase();
    n.contains("BLUR")
        || n.starts_with("DAMAGE")
        || n == "COCKPIT_LR"
        || n == "CINTURE_OFF"
        || (n == "STEER_LR" && has_high_detail_steering)
}

/// Wheel centres and tyre sizes from the wheels' vertices.
fn geometry(kn5: &kn5::Kn5, rig: &Rig, parts: &[Option<Part>]) -> Result<Geometry, Error> {
    let mut g = Geometry {
        centres: rig.wheels.map(|w| to_body(kn5.dummies[w].position)),
        radius: [0.0; 4],
        width: [0.0; 4],
    };
    for i in 0..4 {
        let centre = g.centres[i];
        let points: Vec<DVec3> = kn5
            .meshes
            .iter()
            .zip(parts)
            .filter(|(_, p)| **p == Some(Part::Wheel(i as u8)))
            .flat_map(|(m, _)| m.positions.iter().map(|&p| to_body(p) - centre))
            .collect();
        let radial = |p: &DVec3| p.x.hypot(p.z);
        let r = points.iter().map(radial).fold(0.0, f64::max);
        let (lo, hi) = points
            .iter()
            .filter(|p| radial(p) > TREAD * r)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p.y), hi.max(p.y))
            });
        g.radius[i] = r;
        g.width[i] = hi - lo;
    }
    g.check()?;
    Ok(g)
}

/// The physics files: `--data`, or the folder's `data/`.
fn data_dir(dir: &Path, options: &CarOptions) -> Option<PathBuf> {
    options
        .data
        .clone()
        .or_else(|| Some(dir.join("data")).filter(|d| d.join("car.ini").is_file()))
}

/// The driver's eyes from `car.ini`, in the model's coordinates.
fn driver_eyes(data: Option<&Path>) -> Option<DVec3> {
    let src = std::fs::read(data?.join("car.ini")).ok()?;
    let sections = crate::ini::parse(&String::from_utf8_lossy(&src));
    let v = crate::ini::section(&sections, "GRAPHICS")?.get_f64s("DRIVEREYES")?;
    (v.len() == 3).then(|| to_body(DVec3::new(v[0], v[1], v[2])))
}

/// Converts a car folder into a package.
pub fn convert(dir: &Path, options: CarOptions) -> Result<CarConversion, Error> {
    let path = model_file(dir)?;
    let mut kn5 =
        kn5::parse(&read(&path)?).map_err(|e| Error::Format(format!("{}: {e}", path.display())))?;
    let skin = options
        .skin
        .clone()
        .or_else(|| skins(dir).into_iter().next());
    if let Some(s) = &skin {
        let skin_dir = dir.join("skins").join(s);
        if !skin_dir.is_dir() {
            return Err(Error::Format(format!(
                "no skin {s}; the folder has: {}",
                skins(dir).join(", ")
            )));
        }
        apply_skin(&mut kn5, &skin_dir);
    }

    let rig = Rig::new(&kn5)?;
    let parts: Vec<Option<Part>> = kn5
        .meshes
        .iter()
        .map(|m| {
            (m.visible && m.renderable && !m.indices.is_empty())
                .then(|| rig.part(&kn5, m))
                .flatten()
        })
        .collect();
    let geometry = geometry(&kn5, &rig, &parts)?;

    let base = options.base.clone().unwrap_or_else(CarModel::gt3);
    let mut physics = Physics::new(
        base.params.clone(),
        base.front_tire.p.clone(),
        base.rear_tire.p.clone(),
    );
    physics.apply_geometry(&geometry);
    let data = data_dir(dir, &options);
    match &data {
        Some(d) => physics.apply_data(d)?,
        None => {
            let ui_path = dir.join("ui/ui_car.json");
            match std::fs::read(&ui_path) {
                Ok(src) => match json::parse(&String::from_utf8_lossy(&src)) {
                    Ok(ui) => physics.apply_ui(&ui),
                    Err(e) => eprintln!("warning: {}: {e}", ui_path.display()),
                },
                Err(_) => eprintln!("warning: {} not found", ui_path.display()),
            }
            if dir.join("data.acd").is_file() {
                physics.notes.push(
                    "physics files: packed in data.acd, which open-racing does not read".into(),
                );
            }
        }
    }
    physics.notes.push(format!(
        "{}: from the base car ({})",
        if data.is_some() {
            "anything the physics files do not set"
        } else {
            "centre of gravity, suspension, steering, brakes, gear ratios, differential, aero and tyre compound"
        },
        base.params.name
    ));
    let Physics {
        mut params,
        front: front_tire,
        rear: rear_tire,
        notes,
    } = physics;
    params.name = folder_name(dir);
    CarModel::new(params.clone(), front_tire.clone(), rear_tire.clone())
        .map_err(|e| Error::Format(format!("the converted physics are invalid: {e}")))?;

    // The body's origin: the centre of gravity at static ride height, in the model.
    let centre_y = geometry.centres.iter().map(|c| c.y).sum::<f64>() / 4.0;
    let cg = DVec3::new(
        geometry.front_x() - geometry.wheelbase() * (1.0 - params.front_weight),
        centre_y,
        geometry.ground() + params.cg_height,
    );
    let steering = rig.steering.map(|s| {
        let node = &kn5.dummies[s];
        let axis = to_body(node.world.transform_vector3(DVec3::Z)).normalize_or(DVec3::NEG_X);
        // Along the column towards the driver.
        let axis = if axis.x > 0.0 { -axis } else { axis };
        (to_body(node.position), axis)
    });
    let origin = |part: Part| match part {
        Part::Body => cg,
        Part::Wheel(i) | Part::Hub(i) => geometry.centres[i as usize],
        Part::SteeringWheel => steering.map_or(cg, |(pivot, _)| pivot),
    };
    let max_texture = options.max_texture_size.unwrap_or(MAX_TEXTURE_SIZE);
    let visual = build_visual(&kn5, &parts, origin, max_texture);
    let eye = driver_eyes(data.as_deref())
        .or_else(|| {
            steering
                .map(|(pivot, axis)| pivot + axis * EYE_BEHIND_WHEEL + DVec3::Z * EYE_ABOVE_WHEEL)
        })
        .map(|e| (e - cg).as_vec3().to_array());
    let package = CarPackage {
        params,
        front_tire,
        rear_tire,
        visual: Some(CarVisual {
            steering_wheel: steering.map(|(pivot, axis)| SteeringWheel {
                pivot: (pivot - cg).as_vec3().to_array(),
                axis: axis.as_vec3().to_array(),
            }),
            driver_eye: eye,
            ..visual
        }),
    };
    Ok(CarConversion {
        package,
        notes,
        skin,
    })
}

/// The model's meshes, each in the frame of its part. Every part gets its own materials,
/// so that no batch mixes parts.
fn build_visual(
    kn5: &kn5::Kn5,
    parts: &[Option<Part>],
    origin: impl Fn(Part) -> DVec3,
    max_texture: usize,
) -> CarVisual {
    let mut visual = VisualBuilder::new();
    let mut textures = material::TextureCache::with_max_size(max_texture);
    let used = kn5.meshes.iter().zip(parts).filter(|(_, p)| p.is_some());
    let mut materials = material::Materials::new(kn5, used.map(|(m, _)| m.material), &mut textures);
    let mut part_of_material = Vec::new();
    let mut groups: Vec<Part> = parts.iter().flatten().copied().collect();
    groups.sort_by_key(|p| format!("{p:?}"));
    groups.dedup();
    for part in groups {
        materials.start_group();
        let o = origin(part);
        for (mesh, _) in kn5
            .meshes
            .iter()
            .zip(parts)
            .filter(|(_, p)| **p == Some(part))
        {
            let positions: Vec<[f32; 3]> = mesh
                .positions
                .iter()
                .map(|&p| (to_body(p) - o).as_vec3().to_array())
                .collect();
            let normals: Vec<[f32; 3]> = mesh
                .normals
                .iter()
                .map(|&n| to_body(n).as_vec3().to_array())
                .collect();
            let indices = oriented(&positions, &normals, &mesh.indices);
            let m = materials.get(&mut visual, &mut textures, mesh.material);
            if part_of_material.len() <= m as usize {
                part_of_material.resize(m as usize + 1, Part::Body);
            }
            part_of_material[m as usize] = part;
            visual.add_mesh(
                m,
                mesh.cast_shadows,
                &positions,
                &normals,
                &mesh.uvs,
                &indices,
            );
        }
    }
    let visual = visual.build();
    CarVisual {
        mesh_parts: visual
            .meshes
            .iter()
            .map(|m| part_of_material[m.material as usize])
            .collect(),
        visual,
        steering_wheel: None,
        driver_eye: None,
    }
}

#[cfg(test)]
mod tests {
    use glam::DMat4;

    use super::*;
    use crate::reader::write::Writer;

    /// A node's transform relative to its parent, as the file stores it.
    fn translation(x: f32, y: f32, z: f32) -> [f32; 16] {
        DMat4::from_translation(DVec3::new(x.into(), y.into(), z.into()))
            .to_cols_array()
            .map(|v| v as f32)
    }

    /// A car model: a body box, four wheels as a ring of tread vertices under `HUB_<c>`
    /// nodes with a calliper next to each, a blurred rim, and a steering wheel.
    fn sample_kn5() -> Vec<u8> {
        let mut w = Writer::default();
        w.raw(b"sc6969").i32(6).i32(0);
        w.i32(0);
        w.i32(1)
            .string("paint")
            .string("ksPerPixel")
            .u8(0)
            .u8(0)
            .i32(0);
        w.i32(0).i32(0);
        let mesh = |w: &mut Writer, name: &str, points: &[[f32; 3]]| {
            w.i32(2).string(name).i32(0).u8(1);
            w.u8(1).u8(1).u8(0);
            w.i32(points.len() as i32);
            for p in points {
                w.f32s(p)
                    .f32s(&[0.0, 1.0, 0.0])
                    .f32s(&[0.0, 0.0])
                    .f32s(&[1.0, 0.0, 0.0]);
            }
            let n = points.len() as u16 / 3 * 3;
            w.i32(n as i32);
            for i in 0..n {
                w.u16(i);
            }
            w.i32(0).i32(0).f32s(&[0.0, 1000.0]).f32s(&[0.0; 4]).u8(1);
        };
        let dummy = |w: &mut Writer, name: &str, children: i32, m: [f32; 16]| {
            w.i32(1).string(name).i32(children).u8(1).f32s(&m);
        };
        // Root with a body, four hubs, the steering wheel and a blurred copy of it.
        dummy(&mut w, "root", 7, translation(0.0, 0.0, 0.0));
        mesh(
            &mut w,
            "BODY",
            &[[-0.9, 0.2, -2.0], [0.9, 0.2, -2.0], [0.0, 1.2, 2.2]],
        );
        // Model axes: x left, y up, z forward; the wheel centres sit 0.33 m up.
        for (c, x, z) in [
            ("LF", 0.8, 1.3),
            ("RF", -0.8, 1.3),
            ("LR", 0.8, -1.2),
            ("RR", -0.8, -1.2),
        ] {
            dummy(&mut w, &format!("HUB_{c}"), 2, translation(x, 0.33, z));
            dummy(&mut w, &format!("WHEEL_{c}"), 1, translation(0.0, 0.0, 0.0));
            // Tread ring of radius 0.33, 0.3 wide (along model x).
            let ring: Vec<[f32; 3]> = (0..12)
                .map(|k| {
                    let a = k as f32 * std::f32::consts::TAU / 12.0;
                    [
                        if k % 2 == 0 { 0.15 } else { -0.15 },
                        0.33 * a.sin(),
                        0.33 * a.cos(),
                    ]
                })
                .collect();
            mesh(&mut w, &format!("TYRE_{c}"), &ring);
            mesh(
                &mut w,
                &format!("CALLIPER_{c}"),
                &[[0.0, 0.1, 0.1], [0.0, 0.2, 0.1], [0.0, 0.1, 0.2]],
            );
        }
        // Steering wheel at the left seat, 0.3 m ahead of the centre, 0.7 m up, its column
        // (local z) pointing forward and down.
        let tilt =
            DMat4::from_translation(DVec3::new(0.35, 0.7, 0.3)) * DMat4::from_rotation_x(0.3);
        dummy(
            &mut w,
            "STEER_HR",
            1,
            tilt.to_cols_array().map(|v| v as f32),
        );
        mesh(
            &mut w,
            "RIM",
            &[[-0.15, 0.0, 0.0], [0.15, 0.0, 0.0], [0.0, 0.15, 0.0]],
        );
        dummy(&mut w, "RIM_BLUR_LF", 1, translation(0.0, 0.0, 0.0));
        mesh(
            &mut w,
            "BLURRED",
            &[[0.0; 3], [0.1, 0.0, 0.0], [0.0, 0.1, 0.0]],
        );
        w.0
    }

    fn sample_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("open-racing-ac-car-{name}-{}", std::process::id()))
            .join("sample_car");
        std::fs::create_dir_all(dir.join("ui")).unwrap();
        std::fs::write(dir.join("sample_car.kn5"), sample_kn5()).unwrap();
        std::fs::write(
            dir.join("ui/ui_car.json"),
            r#"{"specs": {"weight": "1250kg", "topspeed": "260km/h"},
                "torqueCurve": [[1000, 300], [5000, 500], [8000, 420]],}"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn converts_a_synthetic_folder() {
        let dir = sample_folder("ui");
        assert!(is_car_folder(&dir));
        let c = convert(&dir, CarOptions::default()).unwrap();
        let p = &c.package.params;
        assert_eq!(p.name, "sample_car");
        assert!((p.wheelbase - 2.5).abs() < 1e-6 && (p.track_front - 1.6).abs() < 1e-6);
        assert!((c.package.front_tire.radius - 0.33).abs() < 1e-6);
        assert!((c.package.rear_tire.width - 0.3).abs() < 1e-6);
        assert_eq!(p.mass, 1250.0);
        assert_eq!(p.engine.limiter_rpm, 8000.0);

        let v = c.package.visual.as_ref().unwrap();
        let count = |part: Part| v.mesh_parts.iter().filter(|&&p| p == part).count();
        assert_eq!(count(Part::Body), 1);
        assert_eq!(count(Part::Wheel(3)), 1);
        assert_eq!(count(Part::Hub(3)), 1);
        assert_eq!(count(Part::SteeringWheel), 1);
        // The blurred wheel is left out.
        assert_eq!(v.mesh_parts.len(), 10);

        // The body is relative to the centre of gravity: the front axle is
        // wheelbase · (1 − front weight) ahead of it, and the ground cg_height below.
        let body = &v.visual.meshes[v.mesh_parts.iter().position(|&p| p == Part::Body).unwrap()];
        let nose = body.positions[2];
        let front_x = p.wheelbase * (1.0 - p.front_weight);
        assert!(
            (f64::from(nose[0]) - (2.2 - 1.3 + front_x)).abs() < 1e-5,
            "{nose:?}"
        );
        assert!(
            (f64::from(nose[2]) - (1.2 - p.cg_height)).abs() < 1e-5,
            "{nose:?}"
        );
        // Wheels are relative to their centres: the right rear tread is 0.33 from its axle.
        let tread = &v.visual.meshes[v
            .mesh_parts
            .iter()
            .position(|&p| p == Part::Wheel(3))
            .unwrap()];
        assert!(
            tread
                .positions
                .iter()
                .all(|q| (q[0].hypot(q[2]) - 0.33).abs() < 1e-5)
        );

        // The column points back to the driver, and the eyes sit behind and above the wheel.
        let s = v.steering_wheel.unwrap();
        assert!(s.axis[0] < -0.9 && s.axis[2] > 0.2, "{:?}", s.axis);
        assert!((s.pivot[1] - 0.35).abs() < 1e-5);
        let eye = v.driver_eye.unwrap();
        assert!(
            eye[0] < s.pivot[0] - 0.4 && eye[2] > s.pivot[2] + 0.2,
            "{eye:?}"
        );

        // The package loads back as a car.
        let out = dir.parent().unwrap().join("package");
        c.package.save(&out).unwrap();
        CarModel::load(open_racing_car::physics_path(&out)).unwrap();
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn physics_files_take_precedence() {
        let dir = sample_folder("data");
        crate::car_physics::tests::write_data(&dir.join("data"));
        let c = convert(&dir, CarOptions::default()).unwrap();
        let p = &c.package.params;
        assert_eq!(p.front_weight, 0.48);
        assert_eq!(p.engine.limiter_rpm, 7500.0);
        assert!(
            c.notes
                .iter()
                .any(|n| n.starts_with("physics: from car.ini")),
            "{:?}",
            c.notes
        );
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn models_without_wheels_are_rejected() {
        let mut w = Writer::default();
        w.raw(b"sc6969").i32(6).i32(0).i32(0).i32(0);
        w.i32(1)
            .string("root")
            .i32(0)
            .u8(1)
            .f32s(&translation(0.0, 0.0, 0.0));
        let kn5 = kn5::parse(&w.0).unwrap();
        assert!(Rig::new(&kn5).is_err());
    }
}
