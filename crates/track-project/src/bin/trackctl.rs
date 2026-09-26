//! Command-line access to track projects, for scripts and AI agents: create, inspect,
//! edit with operations, preview, check and bake. `trackctl guide` prints the format.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use open_racing_track_project::{
    Cache, Error, Project, Side, assets, bake, centreline, corners, curve, dem, inspect, ops,
    pitlane, preview, validate,
};

#[derive(Parser)]
#[command(
    name = "open-racing-trackctl",
    about = "Create, edit, check and bake open-racing track projects"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Prints the project format and the operations, for people and agents.
    Guide,
    /// Creates a project with a small circuit to start from.
    New {
        /// Project directory, or a name under <content>/track-src/.
        project: String,
        /// Track name; defaults to the directory's name.
        #[arg(long)]
        name: Option<String>,
        /// Overwrite an existing project.
        #[arg(long)]
        force: bool,
    },
    /// Summarises a project: roads, node distances, radii, grades, markers, warnings.
    Info {
        project: String,
        /// As JSON.
        #[arg(long)]
        json: bool,
    },
    /// Applies a list of operations (RON or JSON; `-` reads standard input), all or
    /// nothing, and saves the project.
    Apply {
        project: String,
        ops: String,
        /// Check the operations without saving.
        #[arg(long)]
        dry_run: bool,
    },
    /// Draws a plan view as PNG.
    Preview {
        project: String,
        /// Defaults to preview.png in the project's directory.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Pixels along the longer side.
        #[arg(long, default_value_t = 1600)]
        size: usize,
    },
    /// Lists the project's textures and models, what uses each, and those missing.
    Assets {
        project: String,
        /// As JSON.
        #[arg(long)]
        json: bool,
    },
    /// Copies textures (.png, .dds) and models (.glb, .gltf) into the project's assets/
    /// and prints the path to refer to each by.
    Import {
        project: String,
        files: Vec<PathBuf>,
    },
    /// Lays a road along a real circuit's centreline: a GPS track (.gpx), a KML line, a
    /// GeoJSON line (as OpenStreetMap exports give) or a CSV of x, y[, z] metres or
    /// lon, lat[, ele] with a header. Replaces the road's nodes if it exists.
    Centreline {
        project: String,
        file: PathBuf,
        /// The road to lay or replace.
        #[arg(long, default_value = "imported")]
        road: String,
        /// How far the road may stray from the line, m; smaller keeps more nodes.
        #[arg(long, default_value_t = 1.0)]
        tolerance: f64,
        /// Make it the main road (it must be a closed loop).
        #[arg(long)]
        main: bool,
    },
    /// Lays a pit lane beside a stretch of the main road, joining it at both ends, and
    /// makes it the pit lane with its boxes. Defaults to a stretch round the start line.
    Pitlane {
        project: String,
        /// The lane's road.
        #[arg(long, default_value = "pit")]
        road: String,
        /// Where it leaves and rejoins the main road, as spline parameters (node index
        /// plus fraction).
        #[arg(long)]
        from: Option<f64>,
        #[arg(long)]
        to: Option<f64>,
        /// Run it on the left of the main road rather than the right.
        #[arg(long)]
        left: bool,
        /// Gap between the track's edge and the lane's, m.
        #[arg(long, default_value_t = 8.0)]
        gap: f64,
        /// Lane width, m.
        #[arg(long, default_value_t = 10.0)]
        width: f64,
        #[arg(long, default_value_t = 12)]
        boxes: usize,
    },
    /// Lays kerbs round a road's corners (as `info` numbers them): outside at entry and
    /// exit, inside at the apex, and optionally gravel or run-off and a wall outside.
    /// What these corners had is replaced; it stays with its corner as the road changes.
    Kerbs {
        project: String,
        /// Defaults to the main road.
        #[arg(long)]
        road: Option<String>,
        /// Only these corners (by number); all by default.
        #[arg(long, value_delimiter = ',')]
        corners: Vec<usize>,
        /// The strip type of the kerbs ("kerb" by default).
        #[arg(long)]
        style: Option<String>,
        #[arg(long, default_value_t = 1.5)]
        width: f64,
        /// Leave out the entry, apex or exit kerbs.
        #[arg(long)]
        no_entry: bool,
        #[arg(long)]
        no_apex: bool,
        #[arg(long)]
        no_exit: bool,
        /// A strip type to lay outside each corner, beyond its kerbs ("gravel").
        #[arg(long)]
        outside: Option<String>,
        #[arg(long, default_value_t = 12.0)]
        outside_width: f64,
        /// A wall type to put up outside each corner ("tyre wall").
        #[arg(long)]
        wall: Option<String>,
        /// Its distance from the road's edge, m.
        #[arg(long, default_value_t = 20.0)]
        wall_offset: f64,
    },
    /// Puts roads' and splines' nodes on the ground of elevation data: a GeoTIFF
    /// (.tif), an ESRI ASCII grid (.asc) or x y z points (.xyz, .csv, .txt), in metres
    /// or, once a GPS centreline has placed the project, longitudes and latitudes or
    /// UTM metres. With --terrain the ground round the roads follows it too.
    Dem {
        project: String,
        file: PathBuf,
        /// Only these roads or splines; all by default.
        #[arg(long, value_delimiter = ',')]
        lines: Vec<String>,
        /// Height above the ground, m.
        #[arg(long, default_value_t = 0.0)]
        offset: f64,
        /// Also make the terrain follow it away from the roads: the file is copied into
        /// the project's assets/terrain.
        #[arg(long)]
        terrain: bool,
    },
    /// Paints the start/finish line across the main road and a line at the front of
    /// each grid slot, in place of those painted before.
    Paint { project: String },
    /// Bakes the project and checks the package, without saving it.
    Check {
        project: String,
        /// Also drive a test lap.
        #[arg(long)]
        lap: bool,
    },
    /// Bakes the project into a track package, checks it and drives a test lap.
    Bake {
        project: String,
        /// Package directory; defaults to <content>/tracks/<project name>.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Skip the test lap.
        #[arg(long)]
        no_lap: bool,
    },
}

/// A directory, or the name of one under the projects directory.
fn resolve(project: &str) -> PathBuf {
    let path = Path::new(project);
    if path.components().count() > 1 || path.exists() {
        path.to_path_buf()
    } else {
        open_racing_track_project::projects_dir().join(project)
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    match cli.command {
        Command::Guide => print!("{}", include_str!("../../README.md")),
        Command::New {
            project,
            name,
            force,
        } => {
            let dir = resolve(&project);
            if dir.join(open_racing_track_project::PROJECT_FILE).exists() && !force {
                return Err(Error::Invalid(format!(
                    "{} already holds a project (--force overwrites it)",
                    dir.display()
                )));
            }
            let name = name.unwrap_or_else(|| {
                dir.file_name()
                    .map_or("track".into(), |n| n.to_string_lossy().into_owned())
            });
            Project::new(&name).save(&dir)?;
            println!("created {}", dir.display());
        }
        Command::Info { project, json } => {
            let dir = resolve(&project);
            let p = Project::load(&dir)?;
            let scene = bake::build_in(&p, &dir);
            for e in &scene.failed {
                eprintln!("warning: {e}");
            }
            let summary = inspect::summarize(&p, &scene);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&summary).expect("summary serialises")
                );
            } else {
                print_summary(&summary);
            }
        }
        Command::Apply {
            project,
            ops: source,
            dry_run,
        } => {
            let dir = resolve(&project);
            let mut p = Project::load(&dir)?;
            let src = if source == "-" {
                let mut s = String::new();
                std::io::stdin()
                    .read_to_string(&mut s)
                    .map_err(|e| Error::Io("<stdin>".into(), e))?;
                s
            } else {
                std::fs::read_to_string(&source).map_err(|e| Error::Io(source.clone().into(), e))?
            };
            let list = ops::parse(&src)?;
            ops::apply_all(&mut p, &list)?;
            if !dry_run {
                p.save(&dir)?;
            }
            println!(
                "{} {} operations",
                if dry_run { "checked" } else { "applied" },
                list.len()
            );
            let summary = inspect::summarize(&p, &bake::build_in(&p, &dir));
            for w in &summary.warnings {
                println!("warning: {w}");
            }
        }
        Command::Preview { project, out, size } => {
            let dir = resolve(&project);
            let p = Project::load(&dir)?;
            let picture = preview::render(&p, &bake::build_in(&p, &dir), size.clamp(64, 8192));
            let out = out.unwrap_or_else(|| dir.join("preview.png"));
            std::fs::write(&out, picture.to_png()).map_err(|e| Error::Io(out.clone(), e))?;
            println!(
                "wrote {} ({} × {})",
                out.display(),
                picture.width,
                picture.height
            );
        }
        Command::Assets { project, json } => {
            let dir = resolve(&project);
            let p = Project::load(&dir)?;
            let list = assets::list(&p, &dir);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&list).expect("assets serialise")
                );
            } else {
                for a in &list {
                    let size = a.bytes.map_or("MISSING".to_string(), |b| {
                        format!("{} KiB", b.div_ceil(1024))
                    });
                    let used = if a.used_by.is_empty() {
                        "unused".to_string()
                    } else {
                        a.used_by.join(", ")
                    };
                    println!("{:?} {} ({size}): {used}", a.kind, a.path.display());
                }
            }
        }
        Command::Import { project, files } => {
            let dir = resolve(&project);
            for f in &files {
                println!("{}", assets::import(&dir, f)?.display());
            }
        }
        Command::Centreline {
            project,
            file,
            road,
            tolerance,
            main,
        } => {
            let dir = resolve(&project);
            let mut p = Project::load(&dir)?;
            let src = std::fs::read_to_string(&file).map_err(|e| Error::Io(file.clone(), e))?;
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            let line = centreline::read(&name, &src)?;
            let mut list = centreline::road_ops(&p, &line, &road, tolerance);
            if main {
                list.push(ops::Op::SetMainRoad { road: road.clone() });
            }
            ops::apply_all(&mut p, &list)?;
            p.save(&dir)?;
            let r = p.road(&road).expect("just laid");
            println!(
                "laid \"{road}\" along {} points with {} nodes, {}",
                line.points.len(),
                r.nodes.len(),
                if line.closed { "closed" } else { "open" }
            );
            if let Some((lon, lat)) = line.origin {
                println!("(0, 0) is at longitude {lon:.6}, latitude {lat:.6}");
            }
        }
        Command::Pitlane {
            project,
            road,
            from,
            to,
            left,
            gap,
            width,
            boxes,
        } => {
            let dir = resolve(&project);
            let mut p = Project::load(&dir)?;
            let around = pitlane::Plan::around_start(&p);
            let plan = pitlane::Plan {
                from: from.unwrap_or(around.from),
                to: to.unwrap_or(around.to),
                side: if left { Side::Left } else { Side::Right },
                gap,
                width,
                boxes,
                ..around
            };
            let list = pitlane::ops(&p, &road, &plan)?;
            ops::apply_all(&mut p, &list)?;
            p.save(&dir)?;
            println!(
                "laid pit lane \"{road}\" from u = {:.2} to {:.2} with {boxes} boxes",
                plan.from, plan.to
            );
        }
        Command::Kerbs {
            project,
            road,
            corners: only,
            style,
            width,
            no_entry,
            no_apex,
            no_exit,
            outside,
            outside_width,
            wall,
            wall_offset,
        } => {
            let dir = resolve(&project);
            let mut p = Project::load(&dir)?;
            let name = road.unwrap_or_else(|| p.main_road.clone());
            let r = p
                .road(&name)
                .ok_or_else(|| Error::Invalid(format!("no road named \"{name}\"")))?;
            let smp = curve::Sampled::new(r, r.resolution);
            let start = if name == p.main_road {
                smp.s_at(p.markers.start)
            } else {
                0.0
            };
            let kerbs = corners::Kit::kerbs(&p, style.as_deref(), width);
            let kit = corners::Kit {
                entry: kerbs.entry.filter(|_| !no_entry),
                apex: kerbs.apex.filter(|_| !no_apex),
                exit: kerbs.exit.filter(|_| !no_exit),
                outside: outside.map(|s| (s, outside_width)),
                wall: wall.map(|s| (s, wall_offset)),
            };
            for (what, style) in [
                ("strip", kit.outside.as_ref().map(|s| &s.0)),
                ("wall", kit.wall.as_ref().map(|s| &s.0)),
            ] {
                if let Some(s) = style
                    && p.strip_style(s).is_none()
                    && p.wall_style(s).is_none()
                {
                    return Err(Error::Invalid(format!("no {what} type named \"{s}\"")));
                }
            }
            let found = corners::find(&smp, start);
            let mut list = Vec::new();
            for c in found
                .iter()
                .filter(|c| only.is_empty() || only.contains(&c.number))
            {
                list.extend(corners::kit_ops(&p, &name, &smp, &found, c, &kit));
            }
            ops::apply_all(&mut p, &list)?;
            p.save(&dir)?;
            println!("kerbed {} corners of \"{name}\"", found.len());
        }
        Command::Dem {
            project,
            file,
            lines,
            offset,
            terrain,
        } => {
            let dir = resolve(&project);
            let mut p = Project::load(&dir)?;
            let heights = dem::read_file(&file, p.geo)?;
            let (mut list, moved) = dem::node_ops(&p, &heights, &lines, offset);
            if terrain {
                let name = file.file_name().unwrap_or_default();
                let rel = PathBuf::from("assets").join("terrain").join(name);
                let target = dir.join(&rel);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| Error::Io(parent.to_path_buf(), e))?;
                }
                std::fs::copy(&file, &target).map_err(|e| Error::Io(target.clone(), e))?;
                list.push(ops::Op::SetTerrain {
                    terrain: open_racing_track_project::project::Terrain {
                        heights: Some(rel),
                        heights_offset: offset,
                        ..p.terrain.clone()
                    },
                });
            }
            ops::apply_all(&mut p, &list)?;
            p.save(&dir)?;
            println!("put {moved} nodes on the ground");
            if terrain {
                println!("the terrain follows it away from the roads");
            }
        }
        Command::Paint { project } => {
            let dir = resolve(&project);
            let mut p = Project::load(&dir)?;
            let list = pitlane::start_and_grid(&p);
            ops::apply_all(&mut p, &list)?;
            p.save(&dir)?;
            println!(
                "painted the start line and {} grid slots",
                p.markers.grid.count
            );
        }
        Command::Check { project, lap } => {
            let dir = resolve(&project);
            let p = Project::load(&dir)?;
            let package = bake::bake(&p, &dir, &mut Cache::default())?;
            let report = validate::check(&package, lap);
            print!("{report}");
            if !report.ok() {
                return Err(Error::Invalid("the track does not pass its checks".into()));
            }
        }
        Command::Bake {
            project,
            out,
            no_lap,
        } => {
            let dir = resolve(&project);
            let p = Project::load(&dir)?;
            let package = bake::bake(&p, &dir, &mut Cache::default())?;
            let report = validate::check(&package, !no_lap);
            print!("{report}");
            let out = out.unwrap_or_else(|| open_racing_track::tracks_dir().join(&p.name));
            package.save(&out)?;
            println!("baked into {}", out.display());
            if !report.ok() {
                return Err(Error::Invalid(
                    "baked, but the track does not pass its checks".into(),
                ));
            }
        }
    }
    Ok(())
}

fn print_summary(s: &inspect::Summary) {
    println!("{} (main road \"{}\")", s.name, s.main_road);
    println!(
        "extent x {:.0}..{:.0} m, y {:.0}..{:.0} m",
        s.bounds[0][0], s.bounds[1][0], s.bounds[0][1], s.bounds[1][1]
    );
    for r in &s.roads {
        println!(
            "road \"{}\": {} {:.0} m, width {:.1}..{:.1} / {:.1}..{:.1} m, height {:.1}..{:.1} m, steepest {:.1} % at s = {:.0}, tightest radius {:.0} m at s = {:.0}",
            r.name,
            if r.closed { "closed" } else { "open" },
            r.length,
            r.width_left[0],
            r.width_left[1],
            r.width_right[0],
            r.width_right[1],
            r.elevation[0],
            r.elevation[1],
            r.max_grade.value,
            r.max_grade.s,
            r.min_radius.value,
            r.min_radius.s
        );
        for n in &r.nodes {
            println!(
                "  node {:>3} at ({:.1}, {:.1}, {:.1}), s = {:.0} m",
                n.index, n.pos[0], n.pos[1], n.pos[2], n.s
            );
        }
        for (side, strips) in [("left", &r.strips_left), ("right", &r.strips_right)] {
            for st in strips {
                let at = if st.stretches.is_empty() {
                    "everywhere".to_string()
                } else {
                    st.stretches
                        .iter()
                        .map(|[a, b]| format!("{a:.0}..{b:.0} m"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                println!(
                    "  {side} strip \"{}\": {:.1} m of {}, {at}",
                    st.name, st.width, st.surface
                );
            }
        }
        if !r.barriers.is_empty() {
            println!("  barriers: {}", r.barriers.join(", "));
        }
        for c in &r.corners {
            println!(
                "  turn {}: {:?}, {:.0}° at radius {:.0} m, s = {:.0}..{:.0} m (apex {:.0})",
                c.number, c.dir, c.angle, c.radius, c.entry, c.exit, c.apex
            );
        }
    }
    for sp in &s.splines {
        let nodes = sp
            .nodes
            .iter()
            .map(|p| format!("({:.1}, {:.1}, {:.1})", p[0], p[1], p[2]))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "spline \"{}\": {} of {}, {:.0} m through {nodes}",
            sp.name, sp.shape, sp.material, sp.length
        );
    }
    let m = &s.markers;
    println!("start at s = {:.0} m", m.start.s);
    for (i, sec) in m.sectors.iter().enumerate() {
        println!("sector {} starts at s = {:.0} m", i + 2, sec.s);
    }
    println!("{} grid slots", m.grid_slots);
    if let Some(p) = &m.pit {
        println!("pit lane \"{}\" with {} boxes", p.road, p.boxes.len());
    }
    for w in &s.warnings {
        println!("warning: {w}");
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
