//! Where every part of a machine is: parts attached to mounts of other parts, the frame
//! at the root. A suspension and a wheel fitted to an axle appear twice, at the left
//! wheel centre and mirrored at the right one.

use std::collections::HashMap;

use glam::{DAffine3, DMat3, DQuat, DVec3, EulerRot};

use crate::library::Library;
use crate::machine::{Axle, Machine};
use crate::part::{Design, Kind, Mount, Part, PartRef};

/// A rigid transform from Euler angles (degrees, x then y then z) and a translation.
pub fn transform(at: [f64; 3], rotation_deg: [f64; 3]) -> DAffine3 {
    let [a, b, c] = rotation_deg.map(f64::to_radians);
    DAffine3::from_rotation_translation(DQuat::from_euler(EulerRot::XYZ, a, b, c), DVec3::from(at))
}

pub fn mount_transform(m: &Mount) -> DAffine3 {
    transform(m.at, m.rotation_deg)
}

/// Reflection in the car's centre plane (y → −y).
pub fn mirror_y() -> DAffine3 {
    DAffine3::from_mat3(DMat3::from_diagonal(DVec3::new(1.0, -1.0, 1.0)))
}

/// Which side of the car a mirrored corner is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// One appearance of a part in the machine.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Index in `Machine::parts`.
    pub placed: usize,
    pub name: String,
    pub part: PartRef,
    /// Part frame to machine frame (a reflection for right-hand corners).
    pub transform: DAffine3,
    pub side: Option<Side>,
    pub axle: Option<Axle>,
    /// The game's wheel index (front-left, front-right, rear-left, rear-right) of a corner.
    pub wheel: Option<u8>,
}

impl Instance {
    pub fn mirrored(&self) -> bool {
        self.transform.matrix3.determinant() < 0.0
    }
}

/// A resolved machine.
#[derive(Clone, Debug)]
pub struct Assembly {
    pub instances: Vec<Instance>,
    /// Where each placed part's own frame is (for a corner part: its axle's centre).
    pub placed: Vec<DAffine3>,
}

impl Assembly {
    pub fn of_kind(&self, k: Kind) -> impl Iterator<Item = &Instance> {
        self.instances.iter().filter(move |i| i.part.kind == k)
    }

    /// Wheel centres in the game's order.
    pub fn wheel_centres(&self) -> [Option<DVec3>; 4] {
        let mut out = [None; 4];
        for i in self.of_kind(Kind::Wheel) {
            if let Some(w) = i.wheel {
                out[w as usize] = Some(i.transform.translation);
            }
        }
        out
    }
}

fn part<'a>(lib: &'a Library, r: &str) -> Result<(PartRef, &'a Part), String> {
    let pr = PartRef::parse(r)?;
    let p = lib.parts.get(&pr).ok_or_else(|| format!("no part {pr}"))?;
    Ok((pr, p))
}

/// Places every part of a machine.
pub fn assemble(lib: &Library, m: &Machine) -> Result<Assembly, String> {
    let index: HashMap<&str, usize> = m
        .parts
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.as_str(), i))
        .collect();
    let mut world: Vec<Option<DAffine3>> = vec![None; m.parts.len()];
    fn resolve(
        i: usize,
        lib: &Library,
        m: &Machine,
        index: &HashMap<&str, usize>,
        world: &mut Vec<Option<DAffine3>>,
        stack: &mut Vec<usize>,
    ) -> Result<DAffine3, String> {
        if let Some(t) = world[i] {
            return Ok(t);
        }
        if stack.contains(&i) {
            return Err(format!(
                "\"{}\" is attached to itself through other parts",
                m.parts[i].name
            ));
        }
        stack.push(i);
        let p = &m.parts[i];
        let local = transform(p.at, p.rotation_deg);
        let t = match &p.attach {
            None => local,
            Some(a) => {
                let &j = index.get(a.to.as_str()).ok_or_else(|| {
                    format!(
                        "\"{}\" attaches to \"{}\", which is not in the machine",
                        p.name, a.to
                    )
                })?;
                let parent = resolve(j, lib, m, index, world, stack)?;
                let (pr, pp) = part(lib, &m.parts[j].part)?;
                let pm = pp
                    .physical
                    .mounts
                    .iter()
                    .find(|x| x.name == a.at)
                    .ok_or_else(|| format!("{pr} has no mount \"{}\" for \"{}\"", a.at, p.name))?;
                let own = match &a.mount {
                    None => DAffine3::IDENTITY,
                    Some(n) => {
                        let (sr, sp) = part(lib, &p.part)?;
                        mount_transform(
                            sp.physical
                                .mounts
                                .iter()
                                .find(|x| &x.name == n)
                                .ok_or_else(|| format!("{sr} has no mount \"{n}\""))?,
                        )
                    }
                };
                parent * mount_transform(pm) * local * own.inverse()
            }
        };
        stack.pop();
        world[i] = Some(t);
        Ok(t)
    }
    let mut stack = Vec::new();
    for i in 0..m.parts.len() {
        resolve(i, lib, m, &index, &mut world, &mut stack)?;
    }
    let placed: Vec<DAffine3> = world.into_iter().map(|t| t.expect("resolved")).collect();
    // Axles: the suspension fitted to each gives its centre and track.
    let mut axles: HashMap<Axle, (DAffine3, f64)> = HashMap::new();
    for (i, p) in m.parts.iter().enumerate() {
        let (_, pp) = part(lib, &p.part)?;
        if let Design::Suspension(s) = &pp.design {
            let axle = p
                .axle
                .ok_or_else(|| format!("suspension \"{}\" needs an axle", p.name))?;
            if axles.insert(axle, (placed[i], s.track)).is_some() {
                return Err(format!("two suspensions on the {axle:?} axle"));
            }
        }
    }
    let mut instances = Vec::new();
    for (i, p) in m.parts.iter().enumerate() {
        let (pr, _) = part(lib, &p.part)?;
        if matches!(pr.kind, Kind::Suspension | Kind::Wheel) {
            let axle = p
                .axle
                .ok_or_else(|| format!("\"{}\" needs an axle", p.name))?;
            let &(centre, track) = axles.get(&axle).ok_or_else(|| {
                format!(
                    "wheel \"{}\" is on the {axle:?} axle, which has no suspension",
                    p.name
                )
            })?;
            let base = if pr.kind == Kind::Wheel {
                centre * transform(p.at, p.rotation_deg)
            } else {
                centre
            };
            for side in [Side::Left, Side::Right] {
                let half = DAffine3::from_translation(DVec3::new(0.0, 0.5 * track, 0.0));
                let t = match side {
                    Side::Left => base * half,
                    Side::Right => mirror_y() * base * half,
                };
                let wheel = match (axle, side) {
                    (Axle::Front, Side::Left) => 0,
                    (Axle::Front, Side::Right) => 1,
                    (Axle::Rear, Side::Left) => 2,
                    (Axle::Rear, Side::Right) => 3,
                };
                instances.push(Instance {
                    placed: i,
                    name: format!(
                        "{} ({})",
                        p.name,
                        if side == Side::Left { "left" } else { "right" }
                    ),
                    part: pr.clone(),
                    transform: t,
                    side: Some(side),
                    axle: Some(axle),
                    wheel: Some(wheel),
                });
            }
        } else {
            instances.push(Instance {
                placed: i,
                name: p.name.clone(),
                part: pr,
                transform: placed[i],
                side: None,
                axle: p.axle,
                wheel: None,
            });
        }
    }
    Ok(Assembly { instances, placed })
}
