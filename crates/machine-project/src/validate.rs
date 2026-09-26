//! Checks: errors refuse an edit, warnings are reported.

use std::collections::HashSet;

use open_racing_engine_sim::build::network_terminals;
use open_racing_engine_sim::{Build, Network};

use crate::library::Library;
use crate::machine::{Axle, Machine};
use crate::part::{Count, Design, Kind, Part, PartRef, ShapeKind};

/// Checks a part on its own.
pub fn part(r: &PartRef, p: &Part, lib: &Library) -> Result<(), String> {
    let e = |m: String| format!("{r}: {m}");
    let mut names = HashSet::new();
    for s in &p.physical.shapes {
        if !names.insert(&s.name) {
            return Err(e(format!("two shapes named \"{}\"", s.name)));
        }
        if s.mass < 0.0 {
            return Err(e(format!("shape \"{}\" has a negative mass", s.name)));
        }
        let ok = match s.kind {
            ShapeKind::Box { size } => size.iter().all(|&v| v > 0.0),
            ShapeKind::Cylinder { radius, length, .. } => radius > 0.0 && length > 0.0,
            ShapeKind::Sphere { radius } => radius > 0.0,
        };
        if !ok {
            return Err(e(format!("shape \"{}\" needs positive sizes", s.name)));
        }
    }
    let mut names = HashSet::new();
    for m in &p.physical.mounts {
        if !names.insert(&m.name) {
            return Err(e(format!("two mounts named \"{}\"", m.name)));
        }
    }
    match &p.design {
        Design::Engine(en) => {
            open_racing_engine_sim::build::validate_engine(&en.spec).map_err(e)?;
            for (k, name) in [
                (Kind::Intake, &en.bench.intake),
                (Kind::Exhaust, &en.bench.exhaust),
            ] {
                if let Some(n) = name {
                    let pr = PartRef::parse(n).map_err(e)?;
                    if pr.kind != k {
                        return Err(e(format!(
                            "the bench's {} must be a {} part, not {pr}",
                            k.dir(),
                            k.dir()
                        )));
                    }
                    if !lib.parts.contains_key(&pr) {
                        return Err(e(format!(
                            "the bench uses {pr}, which is not in the library"
                        )));
                    }
                }
            }
        }
        Design::Intake(i) => network(&i.network).map_err(e)?,
        Design::Exhaust(x) => network(&x.network).map_err(e)?,
        Design::Suspension(s) => {
            if !(s.track > 0.5 && s.track < 3.0) {
                return Err(e("the track must be between 0.5 and 3 m".into()));
            }
        }
        Design::Wheel(w) => {
            if !(w.tire.radius > 0.1 && w.inertia > 0.0) {
                return Err(e("the tyre needs a radius and the wheel an inertia".into()));
            }
        }
        Design::FuelTank(f) if f.capacity <= 0.0 => {
            return Err(e("the tank needs a capacity".into()));
        }
        _ => {}
    }
    Ok(())
}

/// Checks an intake or exhaust network by building it onto a stand-in engine.
pub fn network(n: &Network) -> Result<(), String> {
    let mut names = HashSet::new();
    for v in &n.volumes {
        crate::check_name(&v.name)?;
        if !names.insert(v.name.as_str()) {
            return Err(format!("two volumes named \"{}\"", v.name));
        }
    }
    for p in &n.pipes {
        crate::check_name(&p.name)?;
        if !names.insert(p.name.as_str()) {
            return Err(format!("\"{}\" names two elements", p.name));
        }
    }
    let t = network_terminals(n);
    let mut seen = HashSet::new();
    for x in &t {
        if !seen.insert(x) {
            return Err(format!("two pipe ends are terminal \"{x}\""));
        }
    }
    let engine = open_racing_engine_sim::samples::i4();
    Build::new(&engine).system("check", n).build().map(|_| ())
}

/// Checks a machine against the library.
pub fn machine(name: &str, m: &Machine, lib: &Library) -> Result<(), String> {
    let e = |s: String| format!("machine/{name}: {s}");
    let mut names = HashSet::new();
    let mut kinds: Vec<(Kind, Option<Axle>)> = Vec::new();
    for p in &m.parts {
        crate::check_name(&p.name).map_err(e)?;
        if p.name.contains(':') {
            return Err(e(format!("\"{}\": instance names cannot hold ':'", p.name)));
        }
        if !names.insert(p.name.as_str()) {
            return Err(e(format!("two parts named \"{}\"", p.name)));
        }
        let r = PartRef::parse(&p.part).map_err(e)?;
        if !lib.parts.contains_key(&r) {
            return Err(e(format!(
                "\"{}\" uses {r}, which is not in the library",
                p.name
            )));
        }
        if matches!(r.kind, Kind::Suspension | Kind::Wheel) && p.axle.is_none() {
            return Err(e(format!("\"{}\" ({r}) needs an axle", p.name)));
        }
        kinds.push((r.kind, p.axle));
    }
    for k in Kind::ALL {
        match k.count() {
            Count::One => {
                if kinds.iter().filter(|x| x.0 == k).count() > 1 {
                    return Err(e(format!("more than one {} part", k.dir())));
                }
            }
            Count::PerAxle => {
                for a in [Axle::Front, Axle::Rear] {
                    if kinds.iter().filter(|x| x.0 == k && x.1 == Some(a)).count() > 1 {
                        return Err(e(format!("more than one {} on the {a:?} axle", k.dir())));
                    }
                }
            }
            Count::Any => {}
        }
    }
    crate::assembly::assemble(lib, m).map_err(e)?;
    if let Some(b) =
        crate::engines::for_machine(lib, m, open_racing_engine_sim::Quality::Draft).map_err(e)?
    {
        b.build().map_err(e)?;
    }
    Ok(())
}

/// Everything in the library.
pub fn library(lib: &Library) -> Result<(), String> {
    for (r, p) in &lib.parts {
        crate::check_name(&r.name)?;
        part(r, p, lib)?;
    }
    for (n, m) in &lib.machines {
        crate::check_name(n)?;
        machine(n, m, lib)?;
    }
    Ok(())
}

/// Things that work but may not be meant, for a machine.
pub fn machine_warnings(m: &Machine, lib: &Library) -> Vec<String> {
    let mut w = Vec::new();
    let has = |k: Kind| {
        m.parts
            .iter()
            .any(|p| PartRef::parse(&p.part).is_ok_and(|r| r.kind == k))
    };
    for k in [
        Kind::Frame,
        Kind::Engine,
        Kind::Transmission,
        Kind::Driveline,
        Kind::Brakes,
        Kind::Steering,
    ] {
        if !has(k) {
            w.push(format!("no {} part: the bake needs one", k.dir()));
        }
    }
    if let Ok(Some(b)) = crate::engines::for_machine(lib, m, open_racing_engine_sim::Quality::Draft)
        && let Ok((_, warnings)) = b.build()
    {
        w.extend(warnings);
    }
    w
}
