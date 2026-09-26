//! Putting an engine together with its intake and exhaust for the simulator: in a
//! machine, or on its own with the bench systems it names.

use open_racing_engine_sim::{Ambient, Build, Quality, System};

use crate::library::Library;
use crate::machine::Machine;
use crate::part::{Design, EnginePart, Kind, PartRef};

/// Instance name the simulator gives the engine.
const ENGINE: &str = "engine";

/// The engine of a machine with its intakes and exhausts, or `None` without an engine.
pub fn for_machine(lib: &Library, m: &Machine, quality: Quality) -> Result<Option<Build>, String> {
    let asm = crate::assembly::assemble(lib, m)?;
    let mut engine: Option<(&str, &EnginePart, glam::DVec3)> = None;
    let mut systems = Vec::new();
    for (i, p) in m.parts.iter().enumerate() {
        let r = PartRef::parse(&p.part)?;
        let part = lib.parts.get(&r).ok_or_else(|| format!("no part {r}"))?;
        let at = asm.placed[i].translation;
        match &part.design {
            Design::Engine(e) => engine = Some((p.name.as_str(), e, at)),
            Design::Intake(x) => systems.push((p.name.clone(), x.network.clone(), at)),
            Design::Exhaust(x) => systems.push((p.name.clone(), x.network.clone(), at)),
            _ => {}
        }
    }
    let Some((name, e, _)) = engine else {
        return Ok(None);
    };
    if systems.iter().any(|s| s.0 == ENGINE) && name != ENGINE {
        return Err(format!(
            "an intake or exhaust cannot be called \"{ENGINE}\" beside the engine \"{name}\""
        ));
    }
    let rename = |t: &str| -> String {
        match t.split_once(':') {
            Some((inst, rest)) if inst == name => format!("{ENGINE}:{rest}"),
            _ => t.to_string(),
        }
    };
    Ok(Some(Build {
        engine: e.spec.clone(),
        systems: systems
            .into_iter()
            .map(|(n, network, at)| System {
                name: n,
                network,
                offset: at.to_array(),
            })
            .collect(),
        connections: m.gas.iter().map(|(a, b)| (rename(a), rename(b))).collect(),
        quality,
        ambient: Ambient::default(),
    }))
}

/// An engine part on the bench, with the intake and exhaust its bench names (overridden by
/// `intake` / `exhaust` when given).
pub fn for_engine(
    lib: &Library,
    engine: &str,
    intake: Option<&str>,
    exhaust: Option<&str>,
    quality: Quality,
) -> Result<Build, String> {
    let r = PartRef::parse(engine)?;
    let part = lib.parts.get(&r).ok_or_else(|| format!("no part {r}"))?;
    let Design::Engine(e) = &part.design else {
        return Err(format!("{r} is not an engine"));
    };
    let mut b = Build::new(&e.spec).quality(quality);
    for (kind, name) in [
        (
            Kind::Intake,
            intake.map(String::from).or(e.bench.intake.clone()),
        ),
        (
            Kind::Exhaust,
            exhaust.map(String::from).or(e.bench.exhaust.clone()),
        ),
    ] {
        let Some(n) = name else { continue };
        let n = if n.contains('/') {
            n
        } else {
            format!("{}/{n}", kind.dir())
        };
        let pr = PartRef::parse(&n)?;
        let p = lib.parts.get(&pr).ok_or_else(|| format!("no part {pr}"))?;
        let net = match &p.design {
            Design::Intake(x) if kind == Kind::Intake => &x.network,
            Design::Exhaust(x) if kind == Kind::Exhaust => &x.network,
            _ => return Err(format!("{pr} is not an {}", kind.dir())),
        };
        b = b.system(kind.dir(), net);
    }
    Ok(b)
}

/// The engine build of whatever a target names: an engine part (on its bench) or a
/// machine.
pub fn for_target(lib: &Library, target: &str, quality: Quality) -> Result<Build, String> {
    match target.strip_prefix("machine/") {
        Some(m) => for_machine(lib, lib.machine(m)?, quality)?
            .ok_or_else(|| format!("machine/{m} has no engine")),
        None => for_engine(lib, target, None, None, quality),
    }
}
