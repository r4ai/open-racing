//! Properties of the gas in the engine: a mixture of fresh charge (air, with the fuel
//! vapour of a port-injected engine counted as air) and burned gas, described by the burned
//! gas's mass fraction `y`.
//!
//! Specific heats rise with temperature (the vibrational modes of N₂, O₂, CO₂ and H₂O),
//! which is what keeps the peak cylinder temperature near 2500 K rather than 3500 K. Each
//! species' `cp` is tabulated against temperature and interpolated linearly, so its
//! sensible internal energy `u(T)` is piecewise quadratic: the energy and its inverse, the
//! temperature of a given energy, are then exact and need no iteration. Energies are
//! sensible, measured from 298.15 K; the fuel's chemical energy enters as heat released
//! (see `combustion`). Dissociation is left out.

/// Universal gas constant over the molar mass of air, J/(kg·K).
pub const R_AIR: f64 = 287.05;
/// Of the products of a stoichiometric gasoline flame (molar mass ≈ 28.6 g/mol), J/(kg·K).
pub const R_BURNED: f64 = 290.7;
/// Sea-level standard pressure, Pa.
pub const P_STANDARD: f64 = 101_325.0;
/// 25 °C, K.
pub const T_STANDARD: f64 = 298.15;
/// Reference temperature of the sensible energies, K.
const T_REF: f64 = 298.15;

/// Temperatures of the tables, K.
const T: [f64; 18] = [
    200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 1000.0, 1200.0, 1400.0, 1600.0, 1800.0,
    2000.0, 2200.0, 2500.0, 3000.0, 3500.0,
];
/// cp of air, J/(kg·K) (JANAF data).
const CP_AIR: [f64; 18] = [
    1002.0, 1005.0, 1013.0, 1029.0, 1051.0, 1075.0, 1099.0, 1121.0, 1141.0, 1175.0, 1201.0, 1219.0,
    1234.0, 1248.0, 1260.0, 1275.0, 1297.0, 1315.0,
];
/// cp of the products of a stoichiometric hydrocarbon flame, frozen, J/(kg·K).
const CP_BURNED: [f64; 18] = [
    1060.0, 1100.0, 1130.0, 1160.0, 1190.0, 1215.0, 1240.0, 1262.0, 1283.0, 1318.0, 1348.0, 1372.0,
    1392.0, 1410.0, 1425.0, 1443.0, 1465.0, 1480.0,
];

/// Anchor data of one species, interpolated linearly in `cp`.
fn cp_anchor(cp: &[f64; 18], t: f64) -> f64 {
    let mut i = 0;
    while i < T.len() - 2 && t > T[i + 1] {
        i += 1;
    }
    let f = (t - T[i]) / (T[i + 1] - T[i]);
    cp[i] + f * (cp[i + 1] - cp[i])
}

/// Lowest and highest temperatures of the uniform tables, and their spacing, K.
const T_LO: f64 = 100.0;
const T_STEP: f64 = 5.0;
const N: usize = 1001;

/// A species' `cp` and sensible enthalpy on a uniform temperature grid (linear `cp`
/// between grid points, so the enthalpy is exact and quadratic within each).
fn species(anchor: [f64; 18]) -> (Vec<f64>, Vec<f64>) {
    let cp: Vec<f64> = (0..N)
        .map(|i| cp_anchor(&anchor, T_LO + i as f64 * T_STEP))
        .collect();
    let mut h = vec![0.0; N];
    for i in 1..N {
        h[i] = h[i - 1] + 0.5 * (cp[i] + cp[i - 1]) * T_STEP;
    }
    let (i, d) = cell(T_REF);
    let slope = (cp[i + 1] - cp[i]) / T_STEP;
    let h_ref = h[i] + cp[i] * d + 0.5 * slope * d * d;
    for v in &mut h {
        *v -= h_ref;
    }
    (cp, h)
}

/// Grid interval of `t` and the distance into it (the end intervals extend outwards).
#[inline]
fn cell(t: f64) -> (usize, f64) {
    let x = (t - T_LO) / T_STEP;
    let i = (x.max(0.0) as usize).min(N - 2);
    (i, t - (T_LO + i as f64 * T_STEP))
}

/// One grid point of both species: cp and its slope to the next point, and the enthalpy.
#[derive(Clone, Copy, Debug, Default)]
struct Point {
    cp: [f64; 2],
    slope: [f64; 2],
    h: [f64; 2],
}

/// The gas model: fresh charge and burned gas.
#[derive(Clone, Debug)]
pub struct Gas {
    /// Both species together, so a state's properties take one lookup.
    grid: Vec<Point>,
    /// Sensible energy of each species at 150 K, the floor of the pipes' temperatures.
    floor: [f64; 2],
}

impl Default for Gas {
    fn default() -> Self {
        Self::new()
    }
}

impl Gas {
    pub fn new() -> Self {
        let (ca, ha) = species(CP_AIR);
        let (cb, hb) = species(CP_BURNED);
        let grid = (0..N)
            .map(|i| {
                let j = (i + 1).min(N - 1);
                Point {
                    cp: [ca[i], cb[i]],
                    slope: [(ca[j] - ca[i]) / T_STEP, (cb[j] - cb[i]) / T_STEP],
                    h: [ha[i], hb[i]],
                }
            })
            .collect();
        let mut g = Self {
            grid,
            floor: [0.0; 2],
        };
        g.floor = [g.energy(150.0, 0.0), g.energy(150.0, 1.0)];
        g
    }

    /// Gas constant of a mixture with burned fraction `y`, J/(kg·K).
    #[inline]
    pub fn r(&self, y: f64) -> f64 {
        R_AIR + y * (R_BURNED - R_AIR)
    }

    /// cp (J/(kg·K)) and sensible enthalpy (J/kg) together.
    #[inline]
    pub fn cp_h(&self, t: f64, y: f64) -> (f64, f64) {
        let (i, d) = cell(t);
        let p = &self.grid[i];
        let cp_a = p.cp[0] + p.slope[0] * d;
        let cp_b = p.cp[1] + p.slope[1] * d;
        let h_a = p.h[0] + (p.cp[0] + 0.5 * p.slope[0] * d) * d;
        let h_b = p.h[1] + (p.cp[1] + 0.5 * p.slope[1] * d) * d;
        (cp_a + y * (cp_b - cp_a), h_a + y * (h_b - h_a))
    }

    /// cp, J/(kg·K).
    #[inline]
    pub fn cp(&self, t: f64, y: f64) -> f64 {
        self.cp_h(t, y).0
    }

    /// Ratio of specific heats.
    #[inline]
    pub fn gamma(&self, t: f64, y: f64) -> f64 {
        let cp = self.cp(t, y);
        cp / (cp - self.r(y))
    }

    /// Sensible enthalpy, J/kg.
    #[inline]
    pub fn enthalpy(&self, t: f64, y: f64) -> f64 {
        self.cp_h(t, y).1
    }

    /// Sensible internal energy, J/kg.
    #[inline]
    pub fn energy(&self, t: f64, y: f64) -> f64 {
        self.enthalpy(t, y) - self.r(y) * t
    }

    /// Sensible internal energy at 150 K, J/kg.
    #[inline]
    pub fn floor_energy(&self, y: f64) -> f64 {
        self.floor[0] + y * (self.floor[1] - self.floor[0])
    }

    /// Temperature of the mixture whose internal energy is `u`, K.
    pub fn temperature(&self, u: f64, y: f64) -> f64 {
        self.temperature_near(u, y, 300.0 + u / 800.0)
    }

    /// Temperature of internal energy `u`, by Newton's method from a nearby guess (such as
    /// the temperature a step before): `u(T)` is nearly linear, so one or two iterations do.
    #[inline]
    pub fn temperature_near(&self, u: f64, y: f64, guess: f64) -> f64 {
        self.temperature_cp_near(u, y, guess).0
    }

    /// As `temperature_near`, with cp there (J/(kg·K), from the last iteration).
    #[inline]
    pub fn temperature_cp_near(&self, u: f64, y: f64, guess: f64) -> (f64, f64) {
        let r = self.r(y);
        let mut t = guess.clamp(T_LO, 5000.0);
        let mut cp = 0.0;
        for _ in 0..8 {
            let (c, h) = self.cp_h(t, y);
            cp = c;
            let dt = (h - r * t - u) / (c - r);
            t -= dt;
            if dt.abs() < 1e-4 {
                break;
            }
        }
        (t, cp)
    }

    /// Speed of sound, m/s.
    pub fn sound_speed(&self, t: f64, y: f64) -> f64 {
        (self.gamma(t, y) * self.r(y) * t).sqrt()
    }
}

/// Mass flow through a restriction of effective area `cda` (discharge coefficient ×
/// geometric area) from a reservoir at stagnation `p0`, `t0` into a static pressure `p`,
/// quasi-steady and isentropic up to the throat (choked below the critical ratio), kg/s.
/// Also returns the velocity at the throat. Heywood (1988), appendix C.
#[inline]
pub fn orifice_flow(cda: f64, p0: f64, t0: f64, p: f64, gamma: f64, r: f64) -> (f64, f64) {
    if cda <= 0.0 || p0 <= p {
        return (0.0, 0.0);
    }
    let g = gamma;
    let critical = (2.0 / (g + 1.0)).powf(g / (g - 1.0));
    let ratio = (p / p0).max(critical);
    let rho0 = p0 / (r * t0);
    let pr = ratio.powf(1.0 / g);
    let rho_t = rho0 * pr;
    let t_t = t0 * ratio.powf((g - 1.0) / g);
    let v = (2.0 * g / (g - 1.0) * r * (t0 - t_t)).max(0.0).sqrt();
    (cda * rho_t * v, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_and_temperature_are_inverse() {
        let g = Gas::new();
        for y in [0.0, 0.3, 1.0] {
            for t in [150.0, 250.0, 298.15, 555.0, 1234.0, 2600.0, 3400.0, 4000.0] {
                let u = g.energy(t, y);
                let back = g.temperature(u, y);
                assert!((back - t).abs() < 1e-6, "y {y} t {t} back {back}");
            }
        }
    }

    #[test]
    fn air_is_diatomic_when_cold_and_softer_when_hot() {
        let g = Gas::new();
        assert!((g.gamma(300.0, 0.0) - 1.40).abs() < 0.005);
        assert!(g.gamma(1000.0, 0.0) < 1.34);
        assert!(g.gamma(2500.0, 1.0) < 1.26);
        assert!((g.sound_speed(293.15, 0.0) - 343.0).abs() < 1.5);
    }

    #[test]
    fn orifice_chokes() {
        let (a, b) = (
            orifice_flow(1e-4, 3e5, 300.0, 1e5, 1.4, R_AIR).0,
            orifice_flow(1e-4, 3e5, 300.0, 0.5e5, 1.4, R_AIR).0,
        );
        assert!(
            (a - b).abs() < 1e-12,
            "choked flow is independent of the back pressure"
        );
        // Choked air: 0.0404 p0 A / sqrt(T0).
        assert!((a - 0.0404 * 3e5 * 1e-4 / 300f64.sqrt()).abs() / a < 0.01);
    }
}
