//! Converts a track or car folder in the Assetto Corsa format into an open-racing package,
//! then prints checks of the result.
//!
//! `cargo run --release -p open-racing-ac -- <track folder> [--layout <layout>] [--name <name>]`
//! `cargo run --release -p open-racing-ac -- <car folder> [--skin <skin>] [--data <dir>] [--name <name>]`

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use open_racing_ac::car::{self, CarOptions};
use open_racing_sim::{AutoShift, Car, CarModel, Controls, Drive, Track, TrackDef, TrackPoint};

#[derive(Parser)]
#[command(
    about = "Convert an Assetto Corsa track or car folder into an open-racing package",
    long_about = "Convert an Assetto Corsa track or car folder into an open-racing package.\n\n\
                  Car folders are recognised by ui/ui_car.json, data.acd or data/car.ini."
)]
struct Args {
    /// Track or car folder in the Assetto Corsa format.
    folder: PathBuf,
    /// Track: layout to convert, for folders with several (the `<layout>` of `models_<layout>.ini`).
    #[arg(long)]
    layout: Option<String>,
    /// Package name, used with `--track` or `--car`. Defaults to the folder name, plus
    /// `-<layout>` for a track layout.
    #[arg(long)]
    name: Option<String>,
    /// Output directory. Defaults to `<content>/tracks/<name>` or `<content>/cars/<name>`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Car: livery (a folder in `skins/`). Defaults to the first.
    #[arg(long)]
    skin: Option<String>,
    /// Car: folder of unpacked physics files (`car.ini`, `engine.ini`, …) when the car
    /// folder has no `data/` folder.
    #[arg(long)]
    data: Option<PathBuf>,
    /// Car: car file whose values stand in for those the folder does not give. Defaults
    /// to the bundled GT3 car.
    #[arg(long)]
    base: Option<PathBuf>,
    /// Car: largest texture edge in texels; larger textures keep their smaller mip levels.
    #[arg(long, default_value_t = car::MAX_TEXTURE_SIZE)]
    max_texture: usize,
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn folder_name(folder: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(folder
        .canonicalize()?
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("the folder has no usable name")?
        .to_string())
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if car::is_car_folder(&args.folder) {
        run_car(args)
    } else {
        run_track(args)
    }
}

fn run_car(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let name = match &args.name {
        Some(n) => n.clone(),
        None => folder_name(&args.folder)?,
    };
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| open_racing_car::cars_dir().join(&name));
    let base = args
        .base
        .as_ref()
        .map(CarModel::load)
        .transpose()
        .map_err(|e| format!("--base: {e}"))?;
    let t0 = std::time::Instant::now();
    let mut conversion = car::convert(
        &args.folder,
        CarOptions {
            skin: args.skin.clone(),
            data: args.data.clone(),
            base,
            max_texture_size: Some(args.max_texture),
        },
    )?;
    conversion.package.params.name = name.clone();
    let pkg = &conversion.package;
    if let Some(v) = &pkg.visual {
        let triangles: usize = v.visual.meshes.iter().map(|m| m.indices.len() / 3).sum();
        println!(
            "model: {} textures, {} materials, {} batches, {triangles} triangles, skin {}",
            v.visual.textures.len(),
            v.visual.materials.len(),
            v.visual.meshes.len(),
            conversion.skin.as_deref().unwrap_or("(none)")
        );
    }
    println!("sources:");
    for note in &conversion.notes {
        println!("  {note}");
    }
    let model = CarModel::new(
        pkg.params.clone(),
        pkg.front_tire.clone(),
        pkg.rear_tire.clone(),
    )?;
    report_car(&model);
    pkg.save(&out)?;
    println!("wrote {} in {:.1?}", out.display(), t0.elapsed());
    if args.out.is_none() {
        println!("drive it with --car {name}");
    }
    Ok(())
}

/// Main figures, and a run on a straight: acceleration, top speed and whether the car
/// settles at its ride height.
fn report_car(model: &CarModel) {
    let p = &model.params;
    // Full load in standard air, boost included, over the rev range.
    let peak = (1..=40)
        .map(|k| k as f64 / 40.0 * p.engine.limiter_rpm)
        .map(|rpm| (rpm, model.engine.full_load(rpm)))
        .fold((0.0f64, 0.0f64, 0.0f64), |(t, w, b), (rpm, (nm, boost))| {
            (
                t.max(nm),
                w.max(nm * rpm * std::f64::consts::PI / 30.0),
                b.max(boost),
            )
        });
    let drive = match p.drive {
        Drive::Rear => "rear-wheel drive".to_string(),
        Drive::Front => "front-wheel drive".to_string(),
        Drive::All { front_share, .. } => {
            format!("all-wheel drive ({:.0} % front)", 100.0 * front_share)
        }
    };
    println!(
        "car: {:.0} kg, {:.0} % front, {drive}, wheelbase {:.2} m, tracks {:.2} / {:.2} m, CG {:.2} m high",
        p.mass,
        100.0 * p.front_weight,
        p.wheelbase,
        p.track_front,
        p.track_rear,
        p.cg_height
    );
    println!(
        "tyres: {:.0}/{:.0} mm front, {:.0}/{:.0} mm rear (width / radius)",
        model.front_tire.p.width * 1e3,
        model.front_tire.p.radius * 1e3,
        model.rear_tire.p.width * 1e3,
        model.rear_tire.p.radius * 1e3
    );
    println!(
        "engine: {:.0} N·m, {:.0} kW{}, {:.1} l, limiter {:.0} rpm, {} gears",
        peak.0,
        peak.1 / 1e3,
        if model.engine.turbocharged() {
            format!(" at {:.2} bar boost", peak.2)
        } else {
            String::new()
        },
        model.engine.displacement * 1e3,
        p.engine.limiter_rpm,
        p.gearbox.ratios.len()
    );

    // A near-straight: a circle of 5 km radius.
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
    .expect("valid test track");
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
    let sag = car.state.position.z - z0;
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
    let time = |t: Option<f64>| t.map_or("-".into(), |t| format!("{t:.1} s"));
    let tel = &car.telemetry;
    let q = 0.5 * open_racing_sim::AIR_DENSITY * car.speed().powi(2);
    println!(
        "aero at {:.0} km/h: drag area {:.2} m², downforce areas {:.2} / {:.2} m², ride heights {:.0} / {:.0} mm",
        car.speed() * 3.6,
        tel.drag / q.max(1.0),
        tel.downforce[0] / q.max(1.0),
        tel.downforce[1] / q.max(1.0),
        tel.ride_height[0] * 1e3,
        tel.ride_height[1] * 1e3
    );
    println!(
        "check: settles {:+.0} mm from its ride height, 0-100 km/h {}, 0-200 km/h {}, {:.0} km/h after 60 s",
        sag * 1e3,
        time(t100),
        time(t200),
        car.speed() * 3.6
    );
}

fn run_track(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let layouts = open_racing_ac::layouts(&args.folder);
    let named: Vec<&str> = layouts.iter().flatten().map(String::as_str).collect();
    if args.layout.is_none() && !named.is_empty() {
        return Err(format!(
            "{} has several layouts; choose one with --layout: {}",
            args.folder.display(),
            named.join(", ")
        )
        .into());
    }
    let folder_name = folder_name(&args.folder)?;
    let name = match (&args.name, &args.layout) {
        (Some(n), _) => n.clone(),
        (None, Some(l)) => format!("{folder_name}-{l}"),
        (None, None) => folder_name,
    };
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| open_racing_track::tracks_dir().join(&name));

    let t0 = std::time::Instant::now();
    let package = open_racing_ac::convert(&args.folder, args.layout.as_deref(), &name)?;
    if let Some(v) = &package.visual {
        let triangles: usize = v.meshes.iter().map(|m| m.indices.len() / 3).sum();
        println!(
            "render data: {} textures, {} materials, {} batches, {triangles} triangles",
            v.textures.len(),
            v.materials.len(),
            v.meshes.len()
        );
    }
    let track = package.build_track()?;
    report(&track);
    package.save(&out)?;
    println!("wrote {} in {:.1?}", out.display(), t0.elapsed());
    if args.out.is_none() {
        println!("drive it with --track {name}");
    }
    Ok(())
}

/// Checks that the centreline fits the road meshes.
fn report(track: &Track) {
    println!(
        "centreline: {:.0} m, {} samples",
        track.length,
        track.samples.len()
    );
    let ground = track.ground.as_ref().expect("converted tracks have ground");
    let (mut over, mut edge_l, mut edge_r, mut n, mut dz) = (0, 0, 0, 0, 0.0f64);
    for smp in track.samples.iter().step_by(5) {
        n += 1;
        if let Some(h) = ground.raycast_down(smp.pos, 3.0) {
            over += 1;
            dz = dz.max((h.point.z - smp.pos.z).abs());
        }
        // Just inside each edge the surface should count as track, if the widths are
        // on the right sides.
        let on_track = |off: f64| {
            !track
                .query(smp.pos + smp.lateral * off, 0)
                .surface
                .off_track()
        };
        edge_l += on_track(smp.width_left - 0.5) as usize;
        edge_r += on_track(-(smp.width_right - 0.5)) as usize;
    }
    println!("centreline over the road meshes: {over}/{n}, max height difference {dz:.2} m");
    println!("track surface just inside the left edge: {edge_l}/{n}, right edge: {edge_r}/{n}");
    let turn: f64 = track
        .samples
        .iter()
        .map(|s| s.curvature * track.spacing)
        .sum();
    println!(
        "total turning {:.0}° ({})",
        turn.to_degrees(),
        if turn > 0.0 {
            "anticlockwise"
        } else {
            "clockwise"
        }
    );
}
