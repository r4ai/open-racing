//! Edits to a library as data. The editor, `open-racing-machinectl apply` and agents all
//! change parts and machines through these, so the same checks apply whoever edits.
//!
//! Targets are `kind/name` for parts (`engine/i4_2l_na`) and `machine/name` for machines.
//! `Set` changes any field by its path in the target's JSON form (`machinectl get` prints
//! it): keys and list indices separated by dots, and `[name]` for the element of a list
//! whose `name` is that — `design.Engine.spec.bore`, `physical.shapes[block].mass`,
//! `design.Exhaust.network.pipes[primary.1].length`, `setup.front.spring_rate`.
//!
//! A list of operations applies atomically: if one fails, or the result does not check,
//! the library is left as it was.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::library::{Library, Target};
use crate::machine::{Machine, Placed};
use crate::part::{Part, PartRef};

// Parts and machines are carried whole; a list of operations is short.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Op {
    /// Adds a part or replaces it whole.
    PutPart {
        part: String,
        value: Part,
    },
    RemovePart {
        part: String,
    },
    /// Renames a part (same kind) and the references to it.
    RenamePart {
        part: String,
        to: String,
    },
    /// Copies a part under a new name (same kind).
    CopyPart {
        part: String,
        to: String,
    },
    PutMachine {
        machine: String,
        value: Machine,
    },
    RemoveMachine {
        machine: String,
    },
    RenameMachine {
        machine: String,
        to: String,
    },
    CopyMachine {
        machine: String,
        to: String,
    },
    /// Adds a part to a machine, or replaces the one of that instance name.
    Place {
        machine: String,
        placed: Placed,
    },
    Unplace {
        machine: String,
        name: String,
    },
    /// Joins two gas terminals (`instance:terminal`).
    Connect {
        machine: String,
        a: String,
        b: String,
    },
    /// Removes the joins of a terminal.
    Disconnect {
        machine: String,
        terminal: String,
    },
    /// Sets any field of a part or machine by its path.
    Set {
        target: String,
        path: String,
        value: Value,
    },
}

impl Op {
    pub fn kind(&self) -> &'static str {
        match self {
            Op::PutPart { .. } => "PutPart",
            Op::RemovePart { .. } => "RemovePart",
            Op::RenamePart { .. } => "RenamePart",
            Op::CopyPart { .. } => "CopyPart",
            Op::PutMachine { .. } => "PutMachine",
            Op::RemoveMachine { .. } => "RemoveMachine",
            Op::RenameMachine { .. } => "RenameMachine",
            Op::CopyMachine { .. } => "CopyMachine",
            Op::Place { .. } => "Place",
            Op::Unplace { .. } => "Unplace",
            Op::Connect { .. } => "Connect",
            Op::Disconnect { .. } => "Disconnect",
            Op::Set { .. } => "Set",
        }
    }

    /// What the operation changes.
    pub fn targets(&self) -> Vec<String> {
        match self {
            Op::PutPart { part, .. } | Op::RemovePart { part } => vec![part.clone()],
            Op::RenamePart { part, to } | Op::CopyPart { part, to } => {
                vec![part.clone(), to.clone()]
            }
            Op::PutMachine { machine, .. }
            | Op::RemoveMachine { machine }
            | Op::Place { machine, .. }
            | Op::Unplace { machine, .. }
            | Op::Connect { machine, .. }
            | Op::Disconnect { machine, .. } => vec![format!("machine/{machine}")],
            Op::RenameMachine { machine, to } | Op::CopyMachine { machine, to } => {
                vec![format!("machine/{machine}"), format!("machine/{to}")]
            }
            Op::Set { target, .. } => vec![target.clone()],
        }
    }

    /// Applies one operation without checking the whole library.
    pub fn apply(&self, lib: &mut Library) -> Result<(), String> {
        match self {
            Op::PutPart { part, value } => {
                let r = PartRef::parse(part)?;
                if value.kind() != r.kind {
                    return Err(format!(
                        "{part}: the value is a {} part",
                        value.kind().dir()
                    ));
                }
                lib.parts.insert(r, value.clone());
            }
            Op::RemovePart { part } => {
                let r = PartRef::parse(part)?;
                if lib.parts.remove(&r).is_none() {
                    return Err(format!("no part {r}"));
                }
                let used: Vec<String> = lib
                    .machines
                    .iter()
                    .filter(|(_, m)| m.parts.iter().any(|p| p.part == *part))
                    .map(|(n, _)| n.clone())
                    .collect();
                if !used.is_empty() {
                    return Err(format!("{r} is used by {}", used.join(", ")));
                }
            }
            Op::RenamePart { part, to } | Op::CopyPart { part, to } => {
                let r = PartRef::parse(part)?;
                let to_r = if to.contains('/') {
                    PartRef::parse(to)?
                } else {
                    PartRef::parse(&format!("{}/{to}", r.kind.dir()))?
                };
                if to_r.kind != r.kind {
                    return Err(format!("{r} cannot become a {} part", to_r.kind.dir()));
                }
                if lib.parts.contains_key(&to_r) {
                    return Err(format!("{to_r} already exists"));
                }
                let p = lib
                    .parts
                    .get(&r)
                    .cloned()
                    .ok_or_else(|| format!("no part {r}"))?;
                lib.parts.insert(to_r.clone(), p);
                if matches!(self, Op::RenamePart { .. }) {
                    lib.parts.remove(&r);
                    let (old, new) = (r.to_string(), to_r.to_string());
                    for m in lib.machines.values_mut() {
                        for p in &mut m.parts {
                            if p.part == old {
                                p.part = new.clone();
                            }
                        }
                    }
                    for p in lib.parts.values_mut() {
                        if let crate::part::Design::Engine(e) = &mut p.design {
                            for b in [&mut e.bench.intake, &mut e.bench.exhaust] {
                                if b.as_deref() == Some(old.as_str()) {
                                    *b = Some(new.clone());
                                }
                            }
                        }
                    }
                }
            }
            Op::PutMachine { machine, value } => {
                crate::check_name(machine)?;
                lib.machines.insert(machine.clone(), value.clone());
            }
            Op::RemoveMachine { machine } => {
                lib.machines
                    .remove(machine)
                    .ok_or_else(|| format!("no machine \"{machine}\""))?;
            }
            Op::RenameMachine { machine, to } | Op::CopyMachine { machine, to } => {
                crate::check_name(to)?;
                if lib.machines.contains_key(to) {
                    return Err(format!("machine \"{to}\" already exists"));
                }
                let m = lib.machine(machine)?.clone();
                lib.machines.insert(to.clone(), m);
                if matches!(self, Op::RenameMachine { .. }) {
                    lib.machines.remove(machine);
                }
            }
            Op::Place { machine, placed } => {
                let m = machine_mut(lib, machine)?;
                match m.parts.iter_mut().find(|p| p.name == placed.name) {
                    Some(p) => *p = placed.clone(),
                    None => m.parts.push(placed.clone()),
                }
            }
            Op::Unplace { machine, name } => {
                let m = machine_mut(lib, machine)?;
                let n = m.parts.len();
                m.parts.retain(|p| p.name != *name);
                if m.parts.len() == n {
                    return Err(format!("machine/{machine} has no part \"{name}\""));
                }
                if let Some(p) = m
                    .parts
                    .iter()
                    .find(|p| p.attach.as_ref().is_some_and(|a| a.to == *name))
                {
                    return Err(format!("\"{}\" is attached to \"{name}\"", p.name));
                }
                m.gas.retain(|(a, b)| {
                    !a.starts_with(&format!("{name}:")) && !b.starts_with(&format!("{name}:"))
                });
            }
            Op::Connect { machine, a, b } => {
                let m = machine_mut(lib, machine)?;
                m.gas.retain(|(x, y)| x != a && y != a && x != b && y != b);
                m.gas.push((a.clone(), b.clone()));
            }
            Op::Disconnect { machine, terminal } => {
                let m = machine_mut(lib, machine)?;
                m.gas.retain(|(x, y)| x != terminal && y != terminal);
            }
            Op::Set {
                target,
                path,
                value,
            } => match Target::parse(target)? {
                Target::Part(r) => {
                    let p = lib.parts.get(&r).ok_or_else(|| format!("no part {r}"))?;
                    let v = set_path(
                        serde_json::to_value(p).map_err(|e| e.to_string())?,
                        path,
                        value,
                    )?;
                    let new: Part =
                        serde_json::from_value(v).map_err(|e| format!("{target} {path}: {e}"))?;
                    if new.kind() != r.kind {
                        return Err(format!("{target}: a part cannot change its kind"));
                    }
                    lib.parts.insert(r, new);
                }
                Target::Machine(n) => {
                    let m = lib.machine(&n)?;
                    let v = set_path(
                        serde_json::to_value(m).map_err(|e| e.to_string())?,
                        path,
                        value,
                    )?;
                    let new: Machine =
                        serde_json::from_value(v).map_err(|e| format!("{target} {path}: {e}"))?;
                    lib.machines.insert(n, new);
                }
            },
        }
        Ok(())
    }
}

fn machine_mut<'a>(lib: &'a mut Library, name: &str) -> Result<&'a mut Machine, String> {
    lib.machines
        .get_mut(name)
        .ok_or_else(|| format!("no machine \"{name}\""))
}

/// Splits a path into keys: `a.b[c].0` → `a`, `b`, `[c]`, `0`.
fn keys(path: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            '[' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let mut name = String::from("[");
                loop {
                    match chars.next() {
                        Some(']') => break,
                        Some(c) => name.push(c),
                        None => return Err(format!("\"{path}\": unclosed [")),
                    }
                }
                out.push(name);
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        return Err("an empty path".into());
    }
    Ok(out)
}

/// The value at a path of a JSON value.
pub fn get_path<'a>(v: &'a Value, path: &str) -> Result<&'a Value, String> {
    let mut cur = v;
    for k in keys(path)? {
        cur = step(cur, &k).ok_or_else(|| format!("\"{path}\": nothing at \"{k}\""))?;
    }
    Ok(cur)
}

fn step<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    if let Some(name) = k.strip_prefix('[') {
        return v
            .as_array()?
            .iter()
            .find(|e| e.get("name").and_then(Value::as_str) == Some(name));
    }
    match v {
        Value::Object(m) => m.get(k),
        Value::Array(a) => a.get(k.parse::<usize>().ok()?),
        _ => None,
    }
}

fn step_mut<'a>(v: &'a mut Value, k: &str) -> Option<&'a mut Value> {
    if let Some(name) = k.strip_prefix('[') {
        return v
            .as_array_mut()?
            .iter_mut()
            .find(|e| e.get("name").and_then(Value::as_str) == Some(name));
    }
    match v {
        Value::Object(m) => m.get_mut(k),
        Value::Array(a) => a.get_mut(k.parse::<usize>().ok()?),
        _ => None,
    }
}

/// Sets the value at a path (an object's missing last key is added: an optional field).
pub fn set_path(mut root: Value, path: &str, value: &Value) -> Result<Value, String> {
    let ks = keys(path)?;
    let (last, head) = ks.split_last().expect("not empty");
    let mut cur = &mut root;
    for k in head {
        cur = step_mut(cur, k).ok_or_else(|| format!("\"{path}\": nothing at \"{k}\""))?;
    }
    match step_mut(cur, last) {
        Some(slot) => *slot = value.clone(),
        None => match cur {
            Value::Object(m) if !last.starts_with('[') => {
                m.insert(last.clone(), value.clone());
            }
            _ => return Err(format!("\"{path}\": nothing at \"{last}\"")),
        },
    }
    Ok(root)
}

/// Applies operations all or nothing, checking every part and machine they touch (and
/// the machines using a touched part).
pub fn apply_all(lib: &mut Library, ops: &[Op]) -> Result<(), String> {
    let mut next = lib.clone();
    for (i, op) in ops.iter().enumerate() {
        op.apply(&mut next)
            .map_err(|e| format!("operation {} ({}): {e}", i + 1, op.kind()))?;
    }
    check_touched(&next, ops)?;
    lib.adopt(next);
    Ok(())
}

fn check_touched(lib: &Library, ops: &[Op]) -> Result<(), String> {
    let mut parts = Vec::new();
    let mut machines = std::collections::BTreeSet::new();
    for op in ops {
        for t in op.targets() {
            match Target::parse(&t) {
                Ok(Target::Part(r)) => parts.push(r),
                Ok(Target::Machine(m)) => {
                    machines.insert(m);
                }
                Err(_) => {}
            }
        }
    }
    for r in &parts {
        if let Some(p) = lib.parts.get(r) {
            crate::validate::part(r, p, lib)?;
        }
        let s = r.to_string();
        for (n, m) in &lib.machines {
            if m.parts.iter().any(|p| p.part == s) {
                machines.insert(n.clone());
            }
        }
    }
    for n in machines {
        if let Some(m) = lib.machines.get(&n) {
            crate::validate::machine(&n, m, lib)?;
        }
    }
    Ok(())
}

/// Operations from text: JSON (`{…}` or `[…]`) or RON, one or a list.
pub fn parse(src: &str) -> Result<Vec<Op>, String> {
    #[allow(clippy::large_enum_variant)]
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<Op>),
        One(Op),
    }
    let t = src.trim_start();
    let v: OneOrMany = if t.starts_with('{') {
        serde_json::from_str(src).map_err(|e| format!("JSON: {e}"))?
    } else {
        match serde_json::from_str(src) {
            Ok(v) => v,
            Err(json) => ron::from_str(src).map_err(|e| {
                if t.starts_with("[{") {
                    format!("JSON: {json}")
                } else {
                    format!("RON: {e}")
                }
            })?,
        }
    };
    Ok(match v {
        OneOrMany::Many(v) => v,
        OneOrMany::One(o) => vec![o],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_find_named_elements() {
        let v: Value =
            serde_json::json!({"a": {"list": [{"name": "x", "v": 1}, {"name": "y", "v": 2}]}});
        assert_eq!(get_path(&v, "a.list[y].v").unwrap(), &serde_json::json!(2));
        assert_eq!(get_path(&v, "a.list.0.v").unwrap(), &serde_json::json!(1));
        let w = set_path(v, "a.list[x].v", &serde_json::json!(5)).unwrap();
        assert_eq!(get_path(&w, "a.list[x].v").unwrap(), &serde_json::json!(5));
        assert!(set_path(w, "a.nope.v", &serde_json::json!(1)).is_err());
    }

    #[test]
    fn a_failing_list_changes_nothing() {
        let mut lib = crate::samples::library("t");
        let before = crate::library::part_text(lib.part("engine/i4_2l_na").unwrap());
        let ops = parse(
            r#"[
                Set(target: "engine/i4_2l_na", path: "design.Engine.spec.compression_ratio", value: 12.0),
                Set(target: "engine/i4_2l_na", path: "design.Engine.spec.bore", value: -1.0),
            ]"#,
        )
        .unwrap();
        assert!(apply_all(&mut lib, &ops).is_err());
        assert_eq!(
            crate::library::part_text(lib.part("engine/i4_2l_na").unwrap()),
            before
        );
    }

    #[test]
    fn renaming_a_part_follows_its_uses() {
        let mut lib = crate::samples::library("t");
        let ops = parse(r#"{"RenamePart": {"part": "exhaust/v8_race", "to": "v8_open"}}"#).unwrap();
        apply_all(&mut lib, &ops).unwrap();
        let m = lib.machine("gt3_v8").unwrap();
        assert!(m.parts.iter().any(|p| p.part == "exhaust/v8_open"));
        let crate::part::Design::Engine(e) = &lib.part("engine/v8_4l_flatplane").unwrap().design
        else {
            panic!()
        };
        assert_eq!(e.bench.exhaust.as_deref(), Some("exhaust/v8_open"));
        // A part in use cannot go.
        assert!(
            apply_all(
                &mut lib,
                &parse(r#"RemovePart(part: "exhaust/v8_open")"#).unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn swapping_an_exhaust_by_path() {
        let mut lib = crate::samples::library("t");
        let op = parse(r#"Set(target: "machine/gt3_v8", path: "parts[exhaust].part", value: "exhaust/v8_merged")"#).unwrap();
        apply_all(&mut lib, &op).unwrap();
        let b = crate::engines::for_machine(
            &lib,
            lib.machine("gt3_v8").unwrap(),
            open_racing_engine_sim::Quality::Draft,
        )
        .unwrap()
        .unwrap();
        let (m, warnings) = b.build().unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            m.mouths
                .iter()
                .filter(|m| m.name.starts_with("exhaust:"))
                .count(),
            1
        );
    }
}
