//! `open-racing-machinectl`: the machine library from the command line, for scripts and
//! AI agents. `machinectl guide` prints the format and the operations.

use std::io::Read;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use open_racing_engine_sim::{Quality, Script, analysis, dyno, render, trace};
use open_racing_machine_project::library::Target;
use open_racing_machine_project::{
    Library, bake, chart, engines, inspect, library_dir, ops, samples, sound, validate,
};

#[derive(Parser)]
#[command(
    name = "open-racing-machinectl",
    about = "Machines built from parts, from the command line"
)]
struct Cli {
    /// Library directory (default: <content>/machine-src).
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The format, the operations and the workflow.
    Guide,
    /// Writes the sample library (a GT3-class machine, a V8 and an inline four with their
    /// intakes and exhausts) into the library, keeping what is there.
    Init,
    /// Parts by kind and machines.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Facts about a part (`kind/name`) or machine (`machine/name`).
    Info {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Prints a part or machine (or one field of it, by path) as JSON: the paths `Set`
    /// takes.
    Get {
        target: String,
        path: Option<String>,
    },
    /// Applies operations (a RON or JSON file, or - for stdin), all or nothing.
    Apply {
        ops: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Copies a part or machine under a new name.
    Copy { from: String, to: String },
    /// Checks the whole library, or one target.
    Check { target: Option<String> },
    /// Runs an engine (on its bench, or a machine's) on the simulated dyno.
    Dyno {
        target: String,
        /// from:to:step, rpm.
        #[arg(long)]
        rpm: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        throttle: f64,
        #[arg(long, default_value = "normal")]
        quality: String,
        /// Fit another intake or exhaust on the bench.
        #[arg(long)]
        intake: Option<String>,
        #[arg(long)]
        exhaust: Option<String>,
        #[arg(long)]
        json: bool,
        /// Writes the dyno sheet as a PNG.
        #[arg(long)]
        png: Option<PathBuf>,
    },
    /// Records one cycle: p–V, p–θ, valve lifts and flows, and pipe pressures.
    Trace {
        target: String,
        #[arg(long)]
        rpm: f64,
        #[arg(long, default_value_t = 1.0)]
        throttle: f64,
        /// Cylinder number, from 1.
        #[arg(long, default_value_t = 1)]
        cylinder: usize,
        /// Record the pipes whose names contain these (comma separated).
        #[arg(long)]
        pipes: Option<String>,
        #[arg(long, default_value = "normal")]
        quality: String,
        #[arg(long)]
        json: bool,
        /// Writes the cylinder pressure against crank angle as a PNG.
        #[arg(long)]
        png: Option<PathBuf>,
    },
    /// Renders the engine's sound to a WAV file, and describes it (level, peaks, orders).
    Sound {
        target: String,
        /// A built-in script: sweep, idle, blips, rev, overrun.
        #[arg(long)]
        preset: Option<String>,
        /// A script file (RON): duration, load, rpm and throttle keys.
        #[arg(long)]
        script: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        /// Microphones, comma separated: exhaust, intake, exterior, cabin.
        #[arg(long, default_value = "exhaust")]
        mic: String,
        #[arg(long, default_value = "normal")]
        quality: String,
        #[arg(long)]
        intake: Option<String>,
        #[arg(long)]
        exhaust: Option<String>,
        #[arg(long)]
        json: bool,
        /// Writes the order levels against speed as a PNG.
        #[arg(long)]
        png: Option<PathBuf>,
    },
    /// Side and top views of a machine's shapes as a PNG.
    Preview {
        machine: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Bakes a machine into a car package, `<content>/cars/<name>/`.
    Bake {
        machine: String,
        #[arg(long, default_value = "normal")]
        quality: String,
        #[arg(long, default_value_t = 500.0)]
        rpm_step: f64,
        #[arg(long)]
        no_drive: bool,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

fn quality(s: &str) -> Result<Quality, String> {
    Ok(match s {
        "draft" => Quality::Draft,
        "normal" => Quality::Normal,
        "high" => Quality::High,
        "ultra" => Quality::Ultra,
        _ => return Err(format!("quality \"{s}\": draft, normal, high or ultra")),
    })
}

fn machine_name(s: &str) -> &str {
    s.strip_prefix("machine/").unwrap_or(s)
}

fn print_json<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).expect("serialises"));
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let dir = cli.dir.clone().unwrap_or_else(library_dir);
    let open = || -> Result<Library, String> {
        if !dir.exists() {
            return Err(format!(
                "no library at {} (machinectl init writes the samples there)",
                dir.display()
            ));
        }
        Library::open(&dir)
    };
    match cli.command {
        Command::Guide => print!("{}", include_str!("../../README.md")),
        Command::Init => {
            let written = samples::init(&dir)?;
            println!("{} files written into {}", written.len(), dir.display());
        }
        Command::List { json } => {
            let lib = open()?;
            let parts: Vec<String> = lib.parts.keys().map(|r| r.to_string()).collect();
            let machines: Vec<String> = lib
                .machines
                .keys()
                .map(|m| format!("machine/{m}"))
                .collect();
            if json {
                print_json(&serde_json::json!({"parts": parts, "machines": machines}));
            } else {
                for p in parts.iter().chain(&machines) {
                    println!("{p}");
                }
            }
        }
        Command::Info { target, json } => {
            let lib = open()?;
            match Target::parse(&target)? {
                Target::Part(r) => {
                    let p = lib.parts.get(&r).ok_or_else(|| format!("no part {r}"))?;
                    let s = inspect::part(&lib, &r, p);
                    if json {
                        print_json(&s);
                    } else {
                        println!(
                            "{}: {:.1} kg, {} shapes, mounts {:?}",
                            s.part, s.mass, s.shapes, s.mounts
                        );
                        if !p.description.is_empty() {
                            println!("  {}", p.description);
                        }
                        if let Some(e) = &s.engine {
                            println!(
                                "  {} cylinders, {:.3} l, {:.1} × {:.1} mm, {:.1}:1, firing {:?} at {:?}°",
                                e.cylinders,
                                e.displacement,
                                e.bore_stroke.0 * 1e3,
                                e.bore_stroke.1 * 1e3,
                                e.compression_ratio,
                                e.firing_order,
                                e.firing_intervals
                            );
                            println!(
                                "  valves: IVO {:.0}° BTDC, IVC {:.0}° ABDC, EVO {:.0}° BBDC, EVC {:.0}° ATDC, overlap {:.0}°",
                                e.intake_opens_btdc,
                                e.intake_closes_abdc,
                                e.exhaust_opens_bbdc,
                                e.exhaust_closes_atdc,
                                e.overlap
                            );
                            println!(
                                "  idle {:.0} rpm, limiter {:.0} rpm ({:.1} m/s piston speed)",
                                e.idle_rpm, e.limiter_rpm, e.piston_speed_at_limiter
                            );
                        }
                        if let Some(n) = &s.network {
                            println!(
                                "  {} pipes ({:.2} m), {:.2} l, terminals {:?}, mouths {:?}",
                                n.pipes, n.pipe_length, n.volume, n.terminals, n.mouths
                            );
                        }
                        if !s.used_by.is_empty() {
                            println!("  used by {}", s.used_by.join(", "));
                        }
                    }
                }
                Target::Machine(n) => {
                    let s = inspect::machine(&lib, &n)?;
                    if json {
                        print_json(&s);
                    } else {
                        let m = &s.mass;
                        println!(
                            "machine/{n}: {:.0} kg, {:.1} % front, CG {:.3} m high, wheelbase {:.3} m, tracks {:.3} / {:.3} m",
                            m.mass,
                            m.front_weight * 100.0,
                            s.cg_height,
                            m.wheelbase,
                            m.track[0],
                            m.track[1]
                        );
                        println!(
                            "  inertia {:.0} / {:.0} / {:.0} kg·m², unsprung {:.1} / {:.1} kg a wheel",
                            m.inertia[0], m.inertia[1], m.inertia[2], m.unsprung[0], m.unsprung[1]
                        );
                        for (name, kg) in &m.parts {
                            let part = &s
                                .parts
                                .iter()
                                .find(|p| &p.0 == name)
                                .map(|p| p.1.clone())
                                .unwrap_or_default();
                            println!("  {name:<18} {part:<28} {kg:7.1} kg");
                        }
                        println!("  gas joins: {}", s.gas.len());
                        for w in &s.warnings {
                            println!("  warning: {w}");
                        }
                    }
                }
            }
        }
        Command::Get { target, path } => {
            let lib = open()?;
            let v = match Target::parse(&target)? {
                Target::Part(r) => {
                    serde_json::to_value(lib.parts.get(&r).ok_or_else(|| format!("no part {r}"))?)
                }
                Target::Machine(n) => serde_json::to_value(lib.machine(&n)?),
            }
            .map_err(|e| e.to_string())?;
            match path {
                Some(p) => print_json(ops::get_path(&v, &p)?),
                None => print_json(&v),
            }
        }
        Command::Apply { ops: file, dry_run } => {
            let mut lib = open()?;
            let src = if file == "-" {
                let mut s = String::new();
                std::io::stdin()
                    .read_to_string(&mut s)
                    .map_err(|e| e.to_string())?;
                s
            } else {
                std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?
            };
            let list = ops::parse(&src)?;
            ops::apply_all(&mut lib, &list)?;
            if dry_run {
                println!(
                    "{} operations check; {} files would change",
                    list.len(),
                    lib.changed().len()
                );
            } else {
                let written = lib.save()?;
                println!(
                    "{} operations applied; {} files written",
                    list.len(),
                    written.len()
                );
            }
        }
        Command::Copy { from, to } => {
            let mut lib = open()?;
            let op = match Target::parse(&from)? {
                Target::Part(r) => ops::Op::CopyPart {
                    part: r.to_string(),
                    to,
                },
                Target::Machine(m) => ops::Op::CopyMachine {
                    machine: m,
                    to: machine_name(&to).into(),
                },
            };
            ops::apply_all(&mut lib, &[op])?;
            lib.save()?;
        }
        Command::Check { target } => {
            let lib = open()?;
            match target {
                None => {
                    validate::library(&lib)?;
                    for (n, m) in &lib.machines {
                        for w in validate::machine_warnings(m, &lib) {
                            println!("machine/{n}: warning: {w}");
                        }
                    }
                    println!(
                        "{} parts and {} machines check",
                        lib.parts.len(),
                        lib.machines.len()
                    );
                }
                Some(t) => {
                    match Target::parse(&t)? {
                        Target::Part(r) => validate::part(
                            &r,
                            lib.parts.get(&r).ok_or_else(|| format!("no part {r}"))?,
                            &lib,
                        )?,
                        Target::Machine(n) => {
                            validate::machine(&n, lib.machine(&n)?, &lib)?;
                            for w in validate::machine_warnings(lib.machine(&n)?, &lib) {
                                println!("warning: {w}");
                            }
                        }
                    }
                    println!("{t} checks");
                }
            }
        }
        Command::Dyno {
            target,
            rpm,
            throttle,
            quality: q,
            intake,
            exhaust,
            json,
            png,
        } => {
            let lib = open()?;
            let b = build_for(
                &lib,
                &target,
                intake.as_deref(),
                exhaust.as_deref(),
                quality(&q)?,
            )?;
            let e = &b.engine.ecu;
            let (from, to, step) = match rpm {
                Some(s) => {
                    let v: Vec<f64> = s
                        .split(':')
                        .map(|x| x.parse::<f64>().map_err(|e| format!("--rpm: {e}")))
                        .collect::<Result<_, _>>()?;
                    match v[..] {
                        [a, b, c] => (a, b, c),
                        [a, b] => (a, b, 500.0),
                        _ => return Err("--rpm from:to:step".into()),
                    }
                }
                None => ((e.idle_rpm - 200.0).max(800.0), e.limiter_rpm, 500.0),
            };
            let run = dyno::sweep(&b, &dyno::speeds(from, to, step), throttle)?;
            if let Some(p) = png {
                std::fs::write(&p, chart::dyno_png(&target.to_uppercase(), &run))
                    .map_err(|e| e.to_string())?;
            }
            if json {
                print_json(&run);
            } else {
                println!(
                    "  rpm    N·m     kW  bmep bar  pmep  fmep    VE   bsfc  pmax bar @°  knock"
                );
                for p in &run.points {
                    println!(
                        "{:5.0} {:6.1} {:6.1} {:9.2} {:5.2} {:5.2} {:5.3} {:6.0} {:9.1} {:3.0} {:6.2}{}",
                        p.rpm,
                        p.brake_torque,
                        p.power / 1e3,
                        p.bmep / 1e5,
                        p.pmep / 1e5,
                        p.fmep / 1e5,
                        p.volumetric_efficiency,
                        p.bsfc,
                        p.peak_pressure / 1e5,
                        p.peak_deg,
                        p.knock,
                        if p.converged { "" } else { "  (not settled)" }
                    );
                }
                if let (Some(t), Some(w)) = (run.peak_torque(), run.peak_power()) {
                    println!(
                        "peak {:.0} N·m at {:.0} rpm, {:.0} kW at {:.0} rpm",
                        t.brake_torque,
                        t.rpm,
                        w.power / 1e3,
                        w.rpm
                    );
                }
            }
        }
        Command::Trace {
            target,
            rpm,
            throttle,
            cylinder,
            pipes,
            quality: q,
            json,
            png,
        } => {
            let lib = open()?;
            let b = build_for(&lib, &target, None, None, quality(&q)?)?;
            let pipes: Vec<String> = pipes
                .map(|p| p.split(',').map(String::from).collect())
                .unwrap_or_default();
            let t = trace::cycle(&b, rpm, throttle, cylinder.saturating_sub(1), &pipes)?;
            if let Some(p) = png {
                let s = vec![
                    chart::Series {
                        label: format!("CYLINDER {cylinder} BAR"),
                        colour: [200, 60, 40],
                        points: t
                            .deg
                            .iter()
                            .zip(&t.pressure)
                            .map(|(d, p)| (*d, p / 1e5))
                            .collect(),
                        right: false,
                        dashed: false,
                    },
                    chart::Series {
                        label: "INTAKE LIFT MM".into(),
                        colour: [40, 90, 200],
                        points: t
                            .deg
                            .iter()
                            .zip(&t.intake_lift)
                            .map(|(d, l)| (*d, l * 1e3))
                            .collect(),
                        right: true,
                        dashed: false,
                    },
                    chart::Series {
                        label: "EXHAUST LIFT MM".into(),
                        colour: [40, 160, 80],
                        points: t
                            .deg
                            .iter()
                            .zip(&t.exhaust_lift)
                            .map(|(d, l)| (*d, l * 1e3))
                            .collect(),
                        right: true,
                        dashed: false,
                    },
                ];
                let title = format!("{} AT {rpm:.0} RPM", target.to_uppercase());
                std::fs::write(
                    &p,
                    chart::line_chart(&title, "CRANK DEG FROM FIRING TDC", "BAR", Some("MM"), &s),
                )
                .map_err(|e| e.to_string())?;
            }
            if json {
                print_json(&t);
            } else {
                let (i, pmax) = t
                    .pressure
                    .iter()
                    .enumerate()
                    .fold((0, 0.0), |a, (i, &p)| if p > a.1 { (i, p) } else { a });
                println!(
                    "cylinder {cylinder} at {rpm:.0} rpm: {} degrees recorded, peak {:.1} bar at {:.0}°, peak {:.0} K",
                    t.deg.len(),
                    pmax / 1e5,
                    t.deg.get(i).copied().unwrap_or(0.0),
                    t.temperature.iter().cloned().fold(0.0, f64::max)
                );
            }
        }
        Command::Sound {
            target,
            preset,
            script,
            out,
            mic,
            quality: q,
            intake,
            exhaust,
            json,
            png,
        } => {
            let lib = open()?;
            let b = build_for(
                &lib,
                &target,
                intake.as_deref(),
                exhaust.as_deref(),
                quality(&q)?,
            )?;
            let script: Script = match (preset, script) {
                (_, Some(f)) => {
                    ron::from_str(&std::fs::read_to_string(&f).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?
                }
                (Some(p), None) => Script::preset(&p, &b.engine).ok_or_else(|| {
                    format!("no preset \"{p}\" (one of {})", render::PRESETS.join(", "))
                })?,
                (None, None) => Script::preset("sweep", &b.engine).expect("built in"),
            };
            let (mut model, _) = b.build()?;
            let names: Vec<String> = mic.split(',').map(|s| s.trim().to_string()).collect();
            let machine = target.strip_prefix("machine/");
            let settings = sound::mic_settings(&lib, machine, &model, &names)?;
            let rec = render::record(&mut model, &script, settings)?;
            rec.write_wav(&out, -1.0, None).map_err(|e| e.to_string())?;
            let report = analysis::analyse(&rec, 16);
            if let Some(p) = png {
                let m = &report.mics[0];
                // Against speed for a sweep; against time when the speed goes up and down.
                let rising = m.order_track.windows(2).all(|w| w[1].rpm >= w[0].rpm - 1.0);
                let x = |s: &open_racing_engine_sim::analysis::OrderSlice| {
                    if rising { s.rpm } else { s.time }
                };
                let colours = [
                    [200, 60, 40],
                    [40, 90, 200],
                    [40, 160, 80],
                    [160, 60, 180],
                    [200, 140, 20],
                    [60, 60, 60],
                ];
                // The strongest orders.
                let mut order_idx: Vec<usize> = (0..m.orders.len()).collect();
                let level = |k: usize| m.order_track.iter().map(|s| s.orders_db[k]).sum::<f64>();
                order_idx.sort_by(|a, b| level(*b).total_cmp(&level(*a)));
                let s: Vec<chart::Series> = order_idx
                    .iter()
                    .take(5)
                    .enumerate()
                    .map(|(n, &k)| chart::Series {
                        label: format!("ORDER {}", m.orders[k]),
                        colour: colours[n],
                        points: m
                            .order_track
                            .iter()
                            .map(|s| (x(s), s.orders_db[k]))
                            .collect(),
                        right: false,
                        dashed: false,
                    })
                    .chain(std::iter::once(chart::Series {
                        label: "OVERALL".into(),
                        colour: colours[5],
                        points: m.order_track.iter().map(|s| (x(s), s.level_db)).collect(),
                        right: false,
                        dashed: true,
                    }))
                    .collect();
                std::fs::write(
                    &p,
                    chart::line_chart(
                        &format!("{} {}", target.to_uppercase(), m.name.to_uppercase()),
                        if rising { "RPM" } else { "S" },
                        "DB",
                        None,
                        &s,
                    ),
                )
                .map_err(|e| e.to_string())?;
            }
            if json {
                print_json(&report);
            } else {
                println!("{} ({:.1} s)", out.display(), script.duration);
                for m in &report.mics {
                    println!(
                        "{}: {:.1} dB overall; peaks {}",
                        m.name,
                        m.level_db,
                        m.peaks
                            .iter()
                            .take(4)
                            .map(|(f, d)| format!("{f:.0} Hz {d:.0} dB"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
        Command::Preview { machine, out } => {
            let lib = open()?;
            let name = machine_name(&machine);
            let asm = open_racing_machine_project::assembly::assemble(&lib, lib.machine(name)?)?;
            let out = out.unwrap_or_else(|| PathBuf::from(format!("{name}.png")));
            std::fs::write(&out, chart::preview_png(&lib, &asm)).map_err(|e| e.to_string())?;
            println!("{}", out.display());
        }
        Command::Bake {
            machine,
            quality: q,
            rpm_step,
            no_drive,
            out,
            json,
        } => {
            let lib = open()?;
            let name = machine_name(&machine);
            let o = bake::BakeOptions {
                quality: quality(&q)?,
                rpm_step,
                drive: !no_drive,
            };
            let (pkg, report) = bake::bake(&lib, name, &o)?;
            let dir = out.unwrap_or_else(|| bake::package_dir(name));
            pkg.save(&dir).map_err(|e| e.to_string())?;
            if json {
                print_json(&report);
            } else {
                let m = &report.mass;
                println!("{} written", dir.display());
                println!(
                    "car: {:.0} kg, {:.1} % front, CG {:.3} m, wheelbase {:.3} m, inertia {:.0}/{:.0}/{:.0}",
                    m.mass,
                    m.front_weight * 100.0,
                    report.cg_height,
                    m.wheelbase,
                    m.inertia[0],
                    m.inertia[1],
                    m.inertia[2]
                );
                println!(
                    "engine: {:.0} N·m at {:.0} rpm, {:.0} kW at {:.0} rpm",
                    report.peak_torque.1,
                    report.peak_torque.0,
                    report.peak_power.1 / 1e3,
                    report.peak_power.0
                );
                if let Some(d) = &report.drive {
                    let t = |v: Option<f64>| v.map_or("-".into(), |v| format!("{v:.1} s"));
                    println!(
                        "check: settles {:+.0} mm, 0-100 km/h {}, 0-200 km/h {}, {:.0} km/h after 60 s",
                        d.settle * 1e3,
                        t(d.zero_100),
                        t(d.zero_200),
                        d.speed_60s
                    );
                }
                for w in &report.warnings {
                    println!("warning: {w}");
                }
                println!("drive it: cargo dev -- --car {name}");
            }
        }
    }
    Ok(())
}

/// The engine build of a target, with bench swaps.
fn build_for(
    lib: &Library,
    target: &str,
    intake: Option<&str>,
    exhaust: Option<&str>,
    q: Quality,
) -> Result<open_racing_engine_sim::Build, String> {
    if target.starts_with("machine/") {
        if intake.is_some() || exhaust.is_some() {
            return Err("--intake and --exhaust are for an engine on its bench; change a machine's parts with Place".into());
        }
        engines::for_target(lib, target, q)
    } else {
        engines::for_engine(lib, target, intake, exhaust, q)
    }
}
