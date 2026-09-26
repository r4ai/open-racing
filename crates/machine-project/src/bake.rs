//! Baking a machine into the game's car package.
//!
//! The machine's detailed models are reduced to what the game simulates:
//! - mass, centre of gravity and inertia from every part's shapes, the driver, the fuel
//!   and ballast; the wheelbase and tracks from where the suspensions put the wheels;
//! - the engine from a simulated dyno: the full-load torque curve with its intake and
//!   exhaust fitted, the motoring (drag) curve with the throttle shut and the fuel cut,
//!   its displacement, manifold volume, crank inertia and the ECU's speeds;
//! - suspension geometry, tyres, brakes, steering, gearbox, differential and
//!   electronics, which the parts still carry in the game's own terms, with the
//!   machine's setup;
//! - aero elements referred to the centre of gravity;
//! - a model from the stand-in shapes, each on the part of the car that moves it.

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::DVec3;
use open_racing_car::{CarPackage, CarVisual, Part as VisualPart, SteeringWheel};
use open_racing_engine_sim::dyno::{self, DynoRun};
use open_racing_engine_sim::spec::End;
use open_racing_engine_sim::{Build, Quality};
use open_racing_sim::params::{AeroParams, AxleParams, CarParams, EngineParams, EnginePosition};
use open_racing_sim::{AutoShift, Car, CarModel, Controls, Track, TrackDef, TrackPoint};
use open_racing_track::visual::{Material, Mesh, Visual};
use serde::Serialize;

use crate::assembly::{Assembly, assemble};
use crate::library::Library;
use crate::machine::{Axle, AxleSetup, Machine};
use crate::mass::{MachineMass, machine_mass};
use crate::mesh::Tris;
use crate::part::{Design, Kind, Part, PartRef, Role};

/// How to bake.
#[derive(Clone, Debug)]
pub struct BakeOptions {
    pub quality: Quality,
    /// Dyno speed step, rpm.
    pub rpm_step: f64,
    /// Drive the baked car on a straight to check it.
    pub drive: bool,
}

impl Default for BakeOptions {
    fn default() -> Self {
        Self {
            quality: Quality::Normal,
            rpm_step: 500.0,
            drive: true,
        }
    }
}

/// What the bake found.
#[derive(Clone, Debug, Serialize)]
pub struct BakeReport {
    pub mass: MachineMass,
    pub dyno: DynoRun,
    pub cg_height: f64,
    pub peak_torque: (f64, f64),
    pub peak_power: (f64, f64),
    pub drive: Option<DriveCheck>,
    pub warnings: Vec<String>,
}

/// A straight-line run of the baked car.
#[derive(Clone, Debug, Serialize)]
pub struct DriveCheck {
    /// How far the car settles from its ride height, m.
    pub settle: f64,
    pub zero_100: Option<f64>,
    pub zero_200: Option<f64>,
    /// km/h after 60 s.
    pub speed_60s: f64,
}

fn find<'a>(
    lib: &'a Library,
    m: &Machine,
    kind: Kind,
    axle: Option<Axle>,
) -> Result<(&'a Part, usize), String> {
    for (i, p) in m.parts.iter().enumerate() {
        let r = PartRef::parse(&p.part)?;
        if r.kind == kind && (axle.is_none() || p.axle == axle) {
            return Ok((lib.parts.get(&r).ok_or_else(|| format!("no part {r}"))?, i));
        }
    }
    Err(match axle {
        Some(a) => format!("the machine has no {} on the {a:?} axle", kind.dir()),
        None => format!("the machine has no {} part", kind.dir()),
    })
}

/// Bakes a machine into a car package.
pub fn bake(
    lib: &Library,
    name: &str,
    o: &BakeOptions,
) -> Result<(CarPackage, BakeReport), String> {
    let m = lib.machine(name)?;
    crate::validate::machine(name, m, lib)?;
    let asm = assemble(lib, m)?;
    let mass = machine_mass(lib, m, &asm)?;
    let cg = DVec3::from(mass.centre);
    let mut warnings = crate::validate::machine_warnings(m, lib);
    if mass.cross_inertia > 0.05 {
        warnings.push(format!(
            "the inertia's cross terms are {:.0} % of its principal moments; the game takes the diagonal",
            mass.cross_inertia * 100.0
        ));
    }
    // Axles.
    let axle = |a: Axle,
                setup: &AxleSetup|
     -> Result<(AxleParams, open_racing_sim::tire::TireParams), String> {
        let (sp, _) = find(lib, m, Kind::Suspension, Some(a))?;
        let (wp, _) = find(lib, m, Kind::Wheel, Some(a))?;
        let (Design::Suspension(s), Design::Wheel(w)) = (&sp.design, &wp.design) else {
            unreachable!("found by kind")
        };
        let k = if a == Axle::Front { 0 } else { 1 };
        Ok((
            AxleParams {
                tire: if k == 0 {
                    "front_tire.ron".into()
                } else {
                    "rear_tire.ron".into()
                },
                pressure: setup.pressure,
                unsprung_mass: mass.unsprung[k],
                wheel_inertia: w.inertia,
                linkage: s.linkage.clone(),
                actuation: s.actuation.clone(),
                spring_rate: setup.spring_rate,
                bump_damping: setup.bump_damping,
                rebound_damping: setup.rebound_damping,
                fast_bump_damping: setup.fast_bump_damping,
                fast_rebound_damping: setup.fast_rebound_damping,
                damper_knee: setup.damper_knee,
                anti_roll_rate: setup.anti_roll_rate,
                heave: setup.heave.clone(),
                bump_travel: s.bump_travel,
                droop_travel: s.droop_travel,
                bump_stop_rate: s.bump_stop_rate,
                static_camber: setup.camber,
                static_toe: setup.toe,
            },
            w.tire.clone(),
        ))
    };
    let (front, front_tire) = axle(Axle::Front, &m.setup.front)?;
    let (rear, rear_tire) = axle(Axle::Rear, &m.setup.rear)?;
    // Engine.
    let (ep, ei) = find(lib, m, Kind::Engine, None)?;
    let Design::Engine(engine) = &ep.design else {
        unreachable!()
    };
    let build = crate::engines::for_machine(lib, m, o.quality)?.expect("has an engine");
    let ecu = &engine.spec.ecu;
    let lo = (ecu.idle_rpm - 200.0).max(600.0);
    let mut rpms = dyno::speeds(lo, ecu.limiter_rpm, o.rpm_step);
    if rpms.last().is_some_and(|&r| r < ecu.limiter_rpm - 1.0) {
        rpms.push(ecu.limiter_rpm - 1.0);
    }
    let run = dyno::sweep(&build, &rpms, 1.0)?;
    for p in &run.points {
        if !p.converged {
            warnings.push(format!("the dyno did not settle at {:.0} rpm", p.rpm));
        }
    }
    let (model, _) = build.build()?;
    let throttle = m
        .parts
        .iter()
        .filter_map(|p| lib.part(&p.part).ok())
        .find_map(|p| match &p.design {
            Design::Intake(i) => Some(i.throttle.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let (_, seat_i) = find(lib, m, Kind::Interior, None).unwrap_or((ep, ei));
    let engine_x = asm.placed[ei]
        .transform_point3(crate::mass::part_props(ep).centre)
        .x;
    let seat_x = asm.placed[seat_i].translation.x;
    let wheels = asm.wheel_centres();
    let rear_x = 0.5 * (wheels[2].unwrap().x + wheels[3].unwrap().x);
    let position = if engine_x > seat_x {
        EnginePosition::Front
    } else if engine_x > rear_x {
        EnginePosition::Mid
    } else {
        EnginePosition::Rear
    };
    let first_drag = run.drag.first().map(|d| d.1).unwrap_or(10.0);
    let engine_params = EngineParams {
        torque_curve: run
            .points
            .iter()
            .map(|p| (p.rpm, p.brake_torque.max(0.0)))
            .collect(),
        drag_curve: std::iter::once((0.0, 0.7 * first_drag))
            .chain(run.drag.iter().map(|&(r, d)| (r, d.max(0.0))))
            .collect(),
        inertia: model.inertia,
        idle_rpm: ecu.idle_rpm,
        limiter_rpm: ecu.limiter_rpm,
        stall_rpm: ecu.stall_rpm,
        idle_authority: 0.35,
        idle_band_rpm: 300.0,
        displacement: Some(model.displacement() * 1e3),
        manifold_volume: Some(manifold_volume(&build) * 1e3),
        throttle,
        turbo: None,
        cooling: engine.cooling.clone(),
        position,
        over_rev_rpm: None,
    };
    // Drivetrain and the rest.
    let (tp, _) = find(lib, m, Kind::Transmission, None)?;
    let (dp, _) = find(lib, m, Kind::Driveline, None)?;
    let (bp, _) = find(lib, m, Kind::Brakes, None)?;
    let (stp, _) = find(lib, m, Kind::Steering, None)?;
    let (Design::Transmission(t), Design::Driveline(d), Design::Brakes(b), Design::Steering(st)) =
        (&tp.design, &dp.design, &bp.design, &stp.design)
    else {
        unreachable!()
    };
    let mut gearbox = t.gearbox.clone();
    if let Some(g) = &m.setup.gearing {
        gearbox.ratios = g.ratios.clone();
        gearbox.final_drive = g.final_drive;
    }
    let mut brakes = b.brakes.clone();
    brakes.front_bias = m.setup.brake_bias;
    let electronics = find(lib, m, Kind::Electronics, None)
        .ok()
        .and_then(|(p, _)| match &p.design {
            Design::Electronics(e) => Some(e.electronics.clone()),
            _ => None,
        })
        .unwrap_or_default();
    // Aero.
    let mut elements = Vec::new();
    for inst in asm.of_kind(Kind::Aero) {
        let Design::Aero(a) = &lib.parts[&inst.part].design else {
            unreachable!()
        };
        for e in &a.elements {
            let mut e = e.clone();
            let p = inst
                .transform
                .transform_point3(DVec3::new(e.position[0], 0.0, e.position[1]));
            e.position = [p.x - cg.x, p.z - cg.z];
            if let Some((_, angle)) = m.setup.wings.iter().find(|w| w.0 == e.name) {
                e.angle = *angle;
            }
            elements.push(e);
        }
    }
    let params = CarParams {
        name: name.to_string(),
        mass: mass.mass,
        inertia: mass.inertia,
        cg_height: cg.z,
        front_weight: mass.front_weight,
        wheelbase: mass.wheelbase,
        track_front: mass.track[0],
        track_rear: mass.track[1],
        front,
        rear,
        steering: st.steering.clone(),
        brakes,
        engine: engine_params,
        clutch: t.clutch.clone(),
        gearbox,
        electronics,
        drive: d.drive.clone(),
        differential: m
            .setup
            .differential
            .clone()
            .unwrap_or_else(|| d.differential.clone()),
        aero: AeroParams {
            ride_height: [m.setup.front.ride_height, m.setup.rear.ride_height],
            elements,
        },
    };
    let visual = visual(lib, m, &asm, cg)?;
    let car = CarModel::new(params.clone(), front_tire.clone(), rear_tire.clone())
        .map_err(|e| format!("the baked car does not check: {e}"))?;
    let drive = o.drive.then(|| drive_check(&car));
    let peak_torque = run
        .peak_torque()
        .map(|p| (p.rpm, p.brake_torque))
        .unwrap_or_default();
    let peak_power = run
        .peak_power()
        .map(|p| (p.rpm, p.power))
        .unwrap_or_default();
    Ok((
        CarPackage {
            params,
            front_tire,
            rear_tire,
            visual: Some(visual),
        },
        BakeReport {
            cg_height: cg.z,
            mass,
            dyno: run,
            peak_torque,
            peak_power,
            drive,
            warnings,
        },
    ))
}

/// Volume between the throttles and the valves: every intake pipe ending at a terminal,
/// and the volume it starts from when no throttle stands between, m³.
fn manifold_volume(b: &Build) -> f64 {
    let mut v = 0.0;
    let mut counted = std::collections::HashSet::new();
    for s in &b.systems {
        for p in &s.network.pipes {
            let term = matches!(p.b, End::Terminal(_)) || matches!(p.a, End::Terminal(_));
            let intake = matches!(&p.b, End::Terminal(t) if t.starts_with("intake"))
                || matches!(&p.a, End::Terminal(t) if t.starts_with("intake"));
            if !(term && intake) {
                continue;
            }
            let d: f64 = p.diameter.iter().map(|d| d.1).sum::<f64>() / p.diameter.len() as f64;
            v += std::f64::consts::PI * 0.25 * d * d * p.length;
            for end in [&p.a, &p.b] {
                if let End::Volume {
                    name,
                    restriction: None,
                } = end
                    && counted.insert((s.name.clone(), name.clone()))
                {
                    v += s
                        .network
                        .volumes
                        .iter()
                        .find(|x| &x.name == name)
                        .map_or(0.0, |x| x.volume);
                }
            }
        }
    }
    v.max(1e-4)
}

/// sRGB (0..1) to linear.
fn linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The stand-in model: shapes grouped by the part of the car that moves them and by
/// colour.
fn visual(lib: &Library, m: &Machine, asm: &Assembly, cg: DVec3) -> Result<CarVisual, String> {
    let wheels = asm.wheel_centres();
    let mut groups: BTreeMap<(u8, u8, [u32; 3]), Tris> = BTreeMap::new();
    let mut interior = None;
    for inst in &asm.instances {
        let p = &lib.parts[&inst.part];
        if let Design::Interior(i) = &p.design {
            let t = inst.transform;
            interior = Some((
                t.transform_point3(DVec3::from(i.steering_wheel)),
                t.transform_vector3(DVec3::from(i.steering_axis))
                    .normalize_or_zero(),
                t.transform_point3(DVec3::from(i.eye)),
            ));
        }
    }
    for inst in &asm.instances {
        let p = &lib.parts[&inst.part];
        for s in &p.physical.shapes {
            let (kind, index, origin) = match (s.role, inst.wheel) {
                (Role::Wheel, Some(w)) => (1u8, w, wheels[w as usize].unwrap_or(cg)),
                (Role::Hub, Some(w)) => (2, w, wheels[w as usize].unwrap_or(cg)),
                (Role::SteeringWheel, _) if interior.is_some() => (3, 0, interior.unwrap().0),
                _ => (0, 0, cg),
            };
            let colour = s.colour.map(|c| (c.clamp(0.0, 1.0) * 1000.0) as u32);
            let t = glam::DAffine3::from_translation(-origin) * inst.transform;
            groups
                .entry((kind, index, colour))
                .or_default()
                .append(&crate::mesh::shape(s), &t);
        }
    }
    let mut materials: Vec<Material> = Vec::new();
    let mut colours: Vec<[u32; 3]> = Vec::new();
    let mut meshes = Vec::new();
    let mut parts = Vec::new();
    for ((kind, index, colour), tris) in groups {
        let mi = match colours.iter().position(|c| *c == colour) {
            Some(i) => i,
            None => {
                colours.push(colour);
                let c = colour.map(|v| linear(v as f32 / 1000.0));
                materials.push(Material {
                    base_color: [c[0], c[1], c[2], 1.0],
                    roughness: 0.5,
                    ..Default::default()
                });
                materials.len() - 1
            }
        };
        let n = tris.positions.len();
        meshes.push(Mesh {
            material: mi as u32,
            cast_shadows: true,
            positions: tris.positions,
            normals: tris.normals,
            uvs: vec![[0.0, 0.0]; n],
            indices: tris.indices,
            lod: None,
        });
        parts.push(match kind {
            1 => VisualPart::Wheel(index),
            2 => VisualPart::Hub(index),
            3 => VisualPart::SteeringWheel,
            _ => VisualPart::Body,
        });
    }
    let _ = m;
    Ok(CarVisual {
        visual: Visual {
            textures: Vec::new(),
            materials,
            meshes,
        },
        mesh_parts: parts,
        steering_wheel: interior.map(|(pivot, axis, _)| SteeringWheel {
            pivot: (pivot - cg).as_vec3().to_array(),
            axis: axis.as_vec3().to_array(),
        }),
        driver_eye: interior.map(|(_, _, eye)| (eye - cg).as_vec3().to_array()),
        wheel_axles: None,
    })
}

/// Drives the car straight: how it settles, and how fast it accelerates.
pub fn drive_check(model: &CarModel) -> DriveCheck {
    let points = (0..32)
        .map(|i| {
            let a = i as f64 / 32.0 * std::f64::consts::TAU;
            TrackPoint {
                pos: (5000.0 * a.cos(), 5000.0 * a.sin(), 0.0),
                width_left: 30.0,
                width_right: 30.0,
                bank: 0.0,
            }
        })
        .collect();
    let track = Track::new(&TrackDef {
        name: "straight".into(),
        points,
        kerb_width: 1.0,
        kerb_height: 0.0,
        runoff_width: f64::INFINITY,
        spacing: 1.0,
    })
    .expect("a valid test track");
    let mut car = Car::new(Arc::new(model.clone()), &track, 0.0, 0.0, 0.0, 1);
    let z0 = car.state.position.z;
    for _ in 0..2000 {
        car.step(
            &track,
            &Controls {
                brake: 0.3,
                ..Default::default()
            },
        );
    }
    let settle = car.state.position.z - z0;
    let (mut t100, mut t200) = (None, None);
    for _ in 0..60_000 {
        let shift = AutoShift.shift(&car);
        car.step(
            &track,
            &Controls {
                throttle: 1.0,
                shift,
                ..Default::default()
            },
        );
        let t = car.state.time - 2.0;
        let v = car.speed() * 3.6;
        if t100.is_none() && v >= 100.0 {
            t100 = Some(t);
        }
        if t200.is_none() && v >= 200.0 {
            t200 = Some(t);
        }
    }
    DriveCheck {
        settle,
        zero_100: t100,
        zero_200: t200,
        speed_60s: car.speed() * 3.6,
    }
}

/// Where a baked machine's package goes: `<content>/cars/<name>/`.
pub fn package_dir(name: &str) -> std::path::PathBuf {
    open_racing_car::cars_dir().join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sample machine bakes into a package the game loads, matching the GT3 it was
    /// derived from where it should, and drives.
    #[test]
    fn the_sample_machine_bakes_into_a_car_the_game_loads() {
        let lib = crate::samples::library("t");
        let o = BakeOptions {
            quality: Quality::Draft,
            rpm_step: 1500.0,
            drive: true,
        };
        let (pkg, report) = bake(&lib, "gt3_v8", &o).unwrap();
        let dir = std::env::temp_dir().join(format!("open-racing-bake-{}", std::process::id()));
        pkg.save(&dir).unwrap();
        let back = CarPackage::load(&dir, true).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(back.visual.is_some_and(|v| !v.visual.meshes.is_empty()));
        let p = &back.params;
        assert!((p.wheelbase - 2.65).abs() < 1e-6);
        assert!((p.front_weight - 0.45).abs() < 0.02, "{}", p.front_weight);
        assert!((p.cg_height - 0.42).abs() < 0.03, "{}", p.cg_height);
        assert!(
            p.engine.torque_curve.iter().any(|t| t.1 > 350.0),
            "{:?}",
            p.engine.torque_curve
        );
        CarModel::new(back.params, back.front_tire, back.rear_tire).unwrap();
        let d = report.drive.unwrap();
        assert!(d.settle.abs() < 0.02, "settles {}", d.settle);
        assert!(d.zero_100.is_some_and(|t| t < 6.0), "{:?}", d.zero_100);
    }
}
