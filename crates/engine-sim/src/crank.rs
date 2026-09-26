//! The crank train: where each cylinder's piston is at a crank angle, and when each
//! cylinder fires.
//!
//! A piston is at top dead centre when its crankpin points along its bank's axis, so a
//! cylinder on a bank at β with its throw at α reaches TDC at crank angle β − α (mod 360°).
//! Over a four-stroke cycle (720°) it passes TDC twice; the firing order says which of the
//! two is its firing TDC.

use std::f64::consts::PI;

use crate::spec::{EngineSpec, Layout};

/// Crank angle of each cylinder's firing TDC within the 720° cycle, rad, with cylinder
/// `firing_order[0]` firing at the angle of its own TDC (mod 360°). In cylinder order.
pub fn firing_angles(layout: &Layout) -> Result<Vec<f64>, String> {
    let n = layout.cylinders.len();
    if n == 0 {
        return Err("the engine has no cylinders".into());
    }
    let mut tdc = Vec::with_capacity(n);
    for (i, c) in layout.cylinders.iter().enumerate() {
        let bank = layout
            .banks
            .iter()
            .find(|b| b.name == c.bank)
            .ok_or_else(|| {
                format!(
                    "cylinder {} is on bank \"{}\", which is not in the layout",
                    i + 1,
                    c.bank
                )
            })?;
        let throw = *layout.throws_deg.get(c.throw).ok_or_else(|| {
            format!(
                "cylinder {} is on throw {}, which is not in the layout",
                i + 1,
                c.throw
            )
        })?;
        tdc.push((bank.angle_deg - throw).rem_euclid(360.0));
    }
    let order = &layout.firing_order;
    let mut sorted = order.clone();
    sorted.sort_unstable();
    if sorted != (1..=n).collect::<Vec<_>>() {
        return Err(format!(
            "the firing order must name each of the {n} cylinders once"
        ));
    }
    let mut angle = vec![0.0; n];
    let mut prev = f64::NEG_INFINITY;
    let mut first = 0.0;
    for (k, &cyl) in order.iter().enumerate() {
        let t = tdc[cyl - 1];
        let a = if k == 0 {
            first = t;
            t
        } else {
            // The earliest of its two TDCs after the previous firing.
            let mut a = t;
            while a <= prev + 1e-6 {
                a += 360.0;
            }
            a
        };
        if a >= first + 720.0 - 1e-6 {
            return Err(format!(
                "firing order {order:?}: cylinder {cyl} cannot fire after cylinder {} within one cycle",
                order[k - 1]
            ));
        }
        angle[cyl - 1] = a;
        prev = a;
    }
    Ok(angle
        .iter()
        .map(|a| a.to_radians().rem_euclid(4.0 * PI))
        .collect())
}

/// Firing intervals in firing order, degrees.
pub fn firing_intervals(layout: &Layout) -> Result<Vec<f64>, String> {
    let a = firing_angles(layout)?;
    let o = &layout.firing_order;
    let n = o.len();
    Ok((0..n)
        .map(|k| {
            let (x, y) = (a[o[k] - 1], a[o[(k + 1) % n] - 1]);
            (y - x).rem_euclid(4.0 * PI).to_degrees()
        })
        .collect())
}

/// Slider-crank geometry of one cylinder.
#[derive(Clone, Copy, Debug)]
pub struct SliderCrank {
    /// Crank radius, m.
    pub r: f64,
    /// Rod length, m.
    pub l: f64,
    /// Pin offset, m.
    pub e: f64,
    /// Piston area, m².
    pub area: f64,
    /// Clearance volume, m³.
    pub clearance: f64,
    x_max: f64,
}

/// Where the piston is at a crank angle.
#[derive(Clone, Copy, Debug)]
pub struct Piston {
    /// Cylinder volume, m³.
    pub volume: f64,
    /// dV/dψ, m³/rad.
    pub dv: f64,
    /// Piston distance from the crank axis, m, and its first two derivatives by angle.
    pub x: f64,
    pub dx: f64,
    pub ddx: f64,
}

impl SliderCrank {
    pub fn new(e: &EngineSpec) -> Self {
        let r = 0.5 * e.stroke;
        let area = PI * 0.25 * e.bore * e.bore;
        let x_max = ((e.rod + r).powi(2) - e.pin_offset.powi(2)).sqrt();
        let x_min = ((e.rod - r).powi(2) - e.pin_offset.powi(2)).sqrt();
        let swept = area * (x_max - x_min);
        Self {
            r,
            l: e.rod,
            e: e.pin_offset,
            area,
            clearance: swept / (e.compression_ratio - 1.0),
            x_max,
        }
    }

    /// Swept volume, m³.
    pub fn swept(&self) -> f64 {
        let x_min = ((self.l - self.r).powi(2) - self.e.powi(2)).sqrt();
        self.area * (self.x_max - x_min)
    }

    /// At angle `psi` from TDC, rad.
    pub fn at(&self, psi: f64) -> Piston {
        let (s, c) = psi.sin_cos();
        let (r, l) = (self.r, self.l);
        let k = r * s - self.e;
        let q = (l * l - k * k).sqrt();
        let x = r * c + q;
        let dx = -r * s - k * r * c / q;
        let ddx = -r * c - r * r * c * c / q + k * r * s / q - k * k * r * r * c * c / (q * q * q);
        Piston {
            volume: self.clearance + self.area * (self.x_max - x),
            dv: -self.area * dx,
            x,
            dx,
            ddx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{Bank, CylinderPlace};

    fn layout(
        banks: &[(&str, f64)],
        throws: &[f64],
        cyl: &[(&str, usize)],
        order: &[usize],
    ) -> Layout {
        Layout {
            banks: banks
                .iter()
                .map(|(n, a)| Bank {
                    name: n.to_string(),
                    angle_deg: *a,
                })
                .collect(),
            throws_deg: throws.to_vec(),
            cylinders: cyl
                .iter()
                .map(|(b, t)| CylinderPlace {
                    bank: b.to_string(),
                    throw: *t,
                })
                .collect(),
            firing_order: order.to_vec(),
        }
    }

    #[test]
    fn inline_four_fires_every_180() {
        let l = layout(
            &[("A", 0.0)],
            &[0.0, 180.0, 180.0, 0.0],
            &[("A", 0), ("A", 1), ("A", 2), ("A", 3)],
            &[1, 3, 4, 2],
        );
        assert_eq!(firing_intervals(&l).unwrap(), vec![180.0; 4]);
        // 1-2-3-4 is impossible: 2 and 3 share a throw, so 3 fires 360° after 2, and 4
        // would then have to fire a whole cycle after 1.
        let bad = Layout {
            firing_order: vec![1, 2, 3, 4],
            ..l.clone()
        };
        assert!(firing_intervals(&bad).is_err());
    }

    #[test]
    fn flat_plane_v8_is_even_and_alternates_banks() {
        // Throws 0/180/180/0, cylinders 1-4 on bank A (+45°), 5-8 on bank B (−45°).
        let cyl: Vec<(&str, usize)> = (0..8)
            .map(|i| (if i < 4 { "A" } else { "B" }, i % 4))
            .collect();
        let l = layout(
            &[("A", 45.0), ("B", -45.0)],
            &[0.0, 180.0, 180.0, 0.0],
            &cyl,
            &[1, 6, 2, 5, 4, 7, 3, 8],
        );
        let iv = firing_intervals(&l).unwrap();
        let banks: Vec<&str> = l
            .firing_order
            .iter()
            .map(|&c| l.cylinders[c - 1].bank.as_str())
            .collect();
        assert_eq!(banks, ["A", "B", "A", "B", "A", "B", "A", "B"]);
        for v in &iv {
            assert!((v - 90.0).abs() < 1e-9, "{iv:?}");
        }
    }

    #[test]
    fn cross_plane_v8_is_even_but_not_alternating() {
        let cyl: Vec<(&str, usize)> = (0..8)
            .map(|i| (if i < 4 { "A" } else { "B" }, i % 4))
            .collect();
        let l = layout(
            &[("A", 45.0), ("B", -45.0)],
            &[0.0, 90.0, 270.0, 180.0],
            &cyl,
            &[1, 8, 4, 2, 7, 3, 6, 5],
        );
        let iv = firing_intervals(&l).unwrap();
        assert!(iv.iter().all(|v| (v - 90.0).abs() < 1e-9), "{iv:?}");
    }

    #[test]
    fn odd_fire_orders_are_rejected_when_impossible() {
        let l = layout(&[("A", 0.0)], &[0.0], &[("A", 0), ("A", 0)], &[1, 2, 1]);
        assert!(firing_angles(&l).is_err());
    }

    #[test]
    fn volume_derivative_matches() {
        let spec = crate::samples::i4();
        let k = SliderCrank::new(&spec);
        for psi in [0.3, 1.0, 2.0, 4.0] {
            let h = 1e-6;
            let num = (k.at(psi + h).volume - k.at(psi - h).volume) / (2.0 * h);
            assert!((num - k.at(psi).dv).abs() < 1e-9);
            let num2 = (k.at(psi + h).dx - k.at(psi - h).dx) / (2.0 * h);
            assert!((num2 - k.at(psi).ddx).abs() < 1e-5);
        }
        let v_bdc = k.at(PI).volume;
        let cr = v_bdc / k.at(0.0).volume;
        assert!((cr - spec.compression_ratio).abs() < 1e-9);
    }
}
