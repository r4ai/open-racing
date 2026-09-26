//! Mass properties: of each shape, of each part, and of a whole machine with its driver,
//! fuel and ballast.

use glam::{DAffine3, DMat3, DVec3};
use serde::Serialize;

use crate::assembly::Assembly;
use crate::library::Library;
use crate::machine::{Axle, Machine};
use crate::part::{Axis, Design, MassSpec, Part, Shape, ShapeKind};

/// Mass, centre of mass and inertia tensor about it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassProps {
    pub mass: f64,
    pub centre: DVec3,
    pub inertia: DMat3,
}

impl Default for MassProps {
    /// Nothing (glam's default matrix is the identity, not zero).
    fn default() -> Self {
        Self {
            mass: 0.0,
            centre: DVec3::ZERO,
            inertia: DMat3::ZERO,
        }
    }
}

impl MassProps {
    pub fn point(mass: f64, at: DVec3) -> Self {
        Self {
            mass,
            centre: at,
            inertia: DMat3::ZERO,
        }
    }

    /// Adds another body.
    pub fn add(&mut self, o: &MassProps) {
        let m = self.mass + o.mass;
        if m <= 0.0 {
            return;
        }
        let c = (self.centre * self.mass + o.centre * o.mass) / m;
        let shift = |p: &MassProps| {
            let d = p.centre - c;
            p.inertia + (DMat3::IDENTITY * d.length_squared() - outer(d, d)) * p.mass
        };
        self.inertia = shift(self) + shift(o);
        self.centre = c;
        self.mass = m;
    }

    /// In another frame.
    pub fn transformed(&self, t: &DAffine3) -> Self {
        let r = t.matrix3;
        Self {
            mass: self.mass,
            centre: t.transform_point3(self.centre),
            inertia: r * self.inertia * r.transpose(),
        }
    }

    /// Principal moments about the body axes (the diagonal) and the largest off-diagonal
    /// term relative to them.
    pub fn diagonal(&self) -> ([f64; 3], f64) {
        let i = self.inertia;
        let d = [i.x_axis.x, i.y_axis.y, i.z_axis.z];
        let off = i.y_axis.x.abs().max(i.z_axis.x.abs()).max(i.z_axis.y.abs());
        (
            d,
            off / d.iter().cloned().fold(f64::MIN, f64::max).max(1e-9),
        )
    }
}

fn outer(a: DVec3, b: DVec3) -> DMat3 {
    DMat3::from_cols(a * b.x, a * b.y, a * b.z)
}

/// A uniform solid shape's mass properties in its part's frame.
pub fn shape_props(s: &Shape) -> MassProps {
    let m = s.mass;
    let local = match s.kind {
        ShapeKind::Box { size: [a, b, c] } => DMat3::from_diagonal(
            DVec3::new(b * b + c * c, a * a + c * c, a * a + b * b) * (m / 12.0),
        ),
        ShapeKind::Sphere { radius } => DMat3::IDENTITY * (0.4 * m * radius * radius),
        ShapeKind::Cylinder {
            radius,
            length,
            axis,
        } => {
            let along = 0.5 * m * radius * radius;
            let across = m * (3.0 * radius * radius + length * length) / 12.0;
            let d = match axis {
                Axis::X => DVec3::new(along, across, across),
                Axis::Y => DVec3::new(across, along, across),
                Axis::Z => DVec3::new(across, across, along),
            };
            DMat3::from_diagonal(d)
        }
    };
    MassProps {
        mass: m,
        centre: DVec3::ZERO,
        inertia: local,
    }
    .transformed(&crate::assembly::transform(s.at, s.rotation_deg))
}

/// A part's mass properties in its own frame.
pub fn part_props(p: &Part) -> MassProps {
    match &p.physical.mass {
        MassSpec::Given {
            mass,
            centre,
            inertia,
        } => MassProps {
            mass: *mass,
            centre: DVec3::from(*centre),
            inertia: DMat3::from_diagonal(DVec3::from(*inertia)),
        },
        MassSpec::Shapes => {
            let mut acc = MassProps::default();
            for s in &p.physical.shapes {
                acc.add(&shape_props(s));
            }
            acc
        }
    }
}

/// Mass of a part's shapes that moves with the wheel.
pub fn unsprung(p: &Part) -> f64 {
    p.physical
        .shapes
        .iter()
        .map(|s| s.mass * s.role.unsprung())
        .sum()
}

/// A machine's mass properties.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MachineMass {
    /// Everything: parts, driver, fuel, ballast. kg.
    pub mass: f64,
    /// Centre of mass, m (machine frame).
    pub centre: [f64; 3],
    /// Principal moments about the centre, body axes, kg·m².
    pub inertia: [f64; 3],
    /// Largest cross term relative to the principal moments.
    pub cross_inertia: f64,
    /// Unsprung mass of one wheel on the front and rear axle, kg.
    pub unsprung: [f64; 2],
    /// Share of the weight on the front axle.
    pub front_weight: f64,
    pub wheelbase: f64,
    /// Front and rear.
    pub track: [f64; 2],
    /// Each placed part's mass, kg (corner parts both sides).
    pub parts: Vec<(String, f64)>,
    #[serde(skip)]
    pub props: MassProps,
}

pub fn machine_mass(lib: &Library, m: &Machine, asm: &Assembly) -> Result<MachineMass, String> {
    let mut total = MassProps::default();
    let mut per_part: Vec<f64> = vec![0.0; m.parts.len()];
    let mut unsprung_axle = [0.0f64; 2];
    let mut seat = None;
    let mut tank = None;
    for inst in &asm.instances {
        let p = lib
            .parts
            .get(&inst.part)
            .ok_or_else(|| format!("no part {}", inst.part))?;
        let props = part_props(p).transformed(&inst.transform);
        total.add(&props);
        per_part[inst.placed] += props.mass;
        if let Some(axle) = inst.axle {
            let k = if axle == Axle::Front { 0 } else { 1 };
            unsprung_axle[k] += unsprung(p);
        }
        match &p.design {
            Design::Interior(i) => {
                seat = Some(inst.transform.transform_point3(DVec3::from(i.seat)))
            }
            Design::FuelTank(f) => tank = Some((inst.transform.translation, f.density)),
            _ => {}
        }
    }
    if let Some(s) = seat {
        total.add(&MassProps::point(
            m.driver.mass,
            s + DVec3::new(0.1, 0.0, 0.25),
        ));
    }
    if let Some((at, density)) = tank {
        total.add(&MassProps::point(m.setup.fuel * density, at));
    }
    for (at, kg) in &m.setup.ballast {
        total.add(&MassProps::point(*kg, DVec3::from(*at)));
    }
    let wheels = asm.wheel_centres();
    let [Some(fl), Some(fr), Some(rl), Some(rr)] = wheels else {
        return Err("the machine needs a suspension and a wheel on each axle".into());
    };
    let (xf, xr) = (0.5 * (fl.x + fr.x), 0.5 * (rl.x + rr.x));
    let wheelbase = xf - xr;
    if wheelbase <= 0.0 {
        return Err("the front axle must be ahead of the rear".into());
    }
    let (inertia, cross) = total.diagonal();
    Ok(MachineMass {
        mass: total.mass,
        centre: total.centre.to_array(),
        inertia,
        cross_inertia: cross,
        unsprung: [unsprung_axle[0] / 2.0, unsprung_axle[1] / 2.0],
        front_weight: (total.centre.x - xr) / wheelbase,
        wheelbase,
        track: [(fl.y - fr.y).abs(), (rl.y - rr.y).abs()],
        parts: m
            .parts
            .iter()
            .map(|p| p.name.clone())
            .zip(per_part)
            .collect(),
        props: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part::Role;

    fn shape(kind: ShapeKind, at: [f64; 3], mass: f64) -> Shape {
        Shape {
            name: "s".into(),
            kind,
            at,
            rotation_deg: [0.0; 3],
            mass,
            colour: [0.5; 3],
            role: Role::Body,
        }
    }

    #[test]
    fn two_boxes_combine_by_the_parallel_axis_theorem() {
        let b = ShapeKind::Box {
            size: [1.0, 1.0, 1.0],
        };
        let mut acc = MassProps::default();
        acc.add(&shape_props(&shape(b, [1.0, 0.0, 0.0], 6.0)));
        acc.add(&shape_props(&shape(b, [-1.0, 0.0, 0.0], 6.0)));
        assert_eq!(acc.mass, 12.0);
        assert!(acc.centre.length() < 1e-12);
        // Each box: 6/12·2 = 1 about any axis; about y and z the offset adds 6·1² each.
        let (d, cross) = acc.diagonal();
        assert!((d[0] - 2.0).abs() < 1e-12, "{d:?} {:?}", acc.inertia);
        assert!((d[1] - 14.0).abs() < 1e-12 && (d[2] - 14.0).abs() < 1e-12);
        assert!(cross < 1e-12);
    }

    #[test]
    fn rotated_cylinder() {
        let c = ShapeKind::Cylinder {
            radius: 0.3,
            length: 0.2,
            axis: crate::part::Axis::Y,
        };
        let mut s = shape(c, [0.0; 3], 10.0);
        let a = shape_props(&s).diagonal().0;
        s.rotation_deg = [0.0, 0.0, 90.0];
        let b = shape_props(&s).diagonal().0;
        assert!((a[1] - 0.45).abs() < 1e-12);
        assert!((b[0] - a[1]).abs() < 1e-9 && (b[1] - a[0]).abs() < 1e-9);
    }
}
