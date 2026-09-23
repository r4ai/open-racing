//! Converts a track folder in the Assetto Corsa format into an open-racing track package,
//! then prints checks of the result.
//!
//! `cargo run --release -p open-racing-ac -- <track folder> [--layout <layout>] [--name <name>]`

use std::path::PathBuf;

use clap::Parser;
use open_racing_sim::{Surface, Track};

#[derive(Parser)]
#[command(about = "Convert an Assetto Corsa track folder into an open-racing track package")]
struct Args {
    /// Track folder in the Assetto Corsa format.
    folder: PathBuf,
    /// Layout to convert, for folders with several (the `<layout>` of `models_<layout>.ini`).
    #[arg(long)]
    layout: Option<String>,
    /// Package name, used with `--track`. Defaults to the folder name, plus `-<layout>`.
    #[arg(long)]
    name: Option<String>,
    /// Output directory. Defaults to `<content>/tracks/<name>`.
    #[arg(long)]
    out: Option<PathBuf>,
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let layouts = open_racing_ac::layouts(&args.folder);
    let named: Vec<&str> = layouts.iter().flatten().map(String::as_str).collect();
    if args.layout.is_none() && !named.is_empty() {
        return Err(format!("{} has several layouts; choose one with --layout: {}", args.folder.display(), named.join(", ")).into());
    }
    let absolute = args.folder.canonicalize()?;
    let folder_name = absolute.file_name().and_then(|n| n.to_str()).ok_or("the folder has no usable name")?;
    let name = match (&args.name, &args.layout) {
        (Some(n), _) => n.clone(),
        (None, Some(l)) => format!("{folder_name}-{l}"),
        (None, None) => folder_name.to_string(),
    };
    let out = args.out.clone().unwrap_or_else(|| open_racing_track::tracks_dir().join(&name));

    let t0 = std::time::Instant::now();
    let package = open_racing_ac::convert(&args.folder, args.layout.as_deref(), &name)?;
    if let Some(v) = &package.visual {
        let triangles: usize = v.meshes.iter().map(|m| m.indices.len() / 3).sum();
        println!("render data: {} textures, {} materials, {} batches, {triangles} triangles", v.textures.len(), v.materials.len(), v.meshes.len());
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
    println!("centreline: {:.0} m, {} samples", track.length, track.samples.len());
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
        let on_track = |off: f64| track.query(smp.pos + smp.lateral * off, 0).surface != Surface::Grass;
        edge_l += on_track(smp.width_left - 0.5) as usize;
        edge_r += on_track(-(smp.width_right - 0.5)) as usize;
    }
    println!("centreline over the road meshes: {over}/{n}, max height difference {dz:.2} m");
    println!("track surface just inside the left edge: {edge_l}/{n}, right edge: {edge_r}/{n}");
    let turn: f64 = track.samples.iter().map(|s| s.curvature * track.spacing).sum();
    println!("total turning {:.0}° ({})", turn.to_degrees(), if turn > 0.0 { "anticlockwise" } else { "clockwise" });
}
