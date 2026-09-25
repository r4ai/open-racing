//! Reduced moist convection, in simulation coordinates (Z up).
//!
//! Prescribed horizontal wind, buoyancy with entrainment/drag, semi-Lagrangian
//! transport, and warm-water saturation adjustment. This is not a pressure-solving
//! fluid model. Potential temperature handles adiabatic cooling during transport;
//! phase changes conserve total water and moist enthalpy locally. Cirrus remains a
//! separate parameterised ice layer. All grids are independent of graphics quality.

use std::sync::Arc;

use glam::{DVec2, DVec3, Vec4};
use rayon::prelude::*;

use super::{CloudLayer, CloudMap};

pub const VOLUME_X: usize = 64;
pub const VOLUME_Z: usize = 32;
pub const VOLUME_PERIOD: f64 = 16_384.0;
pub const VOLUME_HEIGHT: f64 = 6000.0;
pub const VOLUME_TICK: f64 = 2.0;
const DX: f32 = VOLUME_PERIOD as f32 / VOLUME_X as f32;
const DZ: f32 = VOLUME_HEIGHT as f32 / VOLUME_Z as f32;
const COUNT: usize = VOLUME_X * VOLUME_X * VOLUME_Z;
const LATENT_CP: f32 = 2_490.0; // Lv / cp, K per kg/kg
/// Extinction / liquid water content, m²/kg, for a 12 µm effective droplet radius.
const MASS_EXTINCTION: f32 = 125.0;

#[derive(Clone, Debug)]
pub struct CloudFrame {
    pub time: f64,
    /// Potential temperature (K), vapour (kg/kg), liquid (kg/kg), vertical wind (m/s).
    pub cells: Arc<Vec<Vec4>>,
    /// Extinction, m^-1, X fastest, then Y, then height. Ready for a 3D texture.
    pub extinction: Arc<Vec<f32>>,
    pub winds: [DVec2; VOLUME_Z],
}

#[derive(Clone, Debug)]
pub struct CloudVolume {
    pub previous: CloudFrame,
    pub current: CloudFrame,
    source_map: Arc<CloudMap>,
}

/// A coherent pair; render at `time`, interpolating after backtracing both frames.
#[derive(Clone, Copy)]
pub struct CloudSnapshot<'a> {
    pub previous: &'a CloudFrame,
    pub current: &'a CloudFrame,
    pub time: f64,
    pub generation: u64,
}

#[derive(Clone, Copy)]
pub(super) struct VolumeForcing {
    pub temperature: f32,
    pub dew_point: f32,
    pub pressure: f32,
    pub sunlight: f32,
    pub winds: [DVec2; 4],
    pub layers: [CloudLayer; 4],
}

fn pressure_at(height: f32, surface: f32) -> f32 {
    surface * (-height / 8200.0).exp()
}

fn exner(pressure: f32) -> f32 {
    (pressure / 1000.0).powf(0.2854)
}

fn saturation(temperature: f32, pressure: f32) -> f32 {
    saturation_with_slope(temperature, pressure).0
}

fn saturation_with_slope(temperature: f32, pressure: f32) -> (f32, f32) {
    let c = (temperature - 273.15).clamp(-80.0, 50.0);
    let e = 6.112 * (17.67 * c / (c + 243.5)).exp();
    let qs = 0.622 * e / (pressure - e).max(100.0);
    (
        qs,
        qs * (1.0 + qs / 0.622) * (17.67 * 243.5 / (c + 243.5).powi(2)),
    )
}

/// Solve the saturation equilibrium along T + Lv/cp*qv = constant. A bracketed
/// Newton solve converges in a few exponential evaluations without oscillation.
fn equilibrate(cell: &mut Vec4, pressure: f32) {
    let pi = exner(pressure);
    let t = cell.x * pi;
    let (qs, slope) = saturation_with_slope(t, pressure);
    if (cell.y - qs).abs() < 1e-7 || (cell.y < qs && cell.z <= 0.0) {
        return;
    }
    let mut lo = -cell.z;
    let mut hi = cell.y;
    let mut condensed = ((cell.y - qs) / (1.0 + LATENT_CP * slope)).clamp(lo, hi);
    for _ in 0..6 {
        let (qs, slope) = saturation_with_slope(t + LATENT_CP * condensed, pressure);
        let residual = cell.y - condensed - qs;
        if residual.abs() < 5e-8 || (condensed == -cell.z && residual <= 0.0) {
            break;
        }
        if residual > 0.0 {
            lo = condensed;
        } else {
            hi = condensed;
        }
        let next = condensed + residual / (1.0 + LATENT_CP * slope);
        condensed = if next > lo && next < hi {
            next
        } else {
            (lo + hi) * 0.5
        };
    }
    cell.x += LATENT_CP * condensed / pi;
    cell.y -= condensed;
    cell.z += condensed;
}

fn height(z: usize) -> f32 {
    (z as f32 + 0.5) * DZ
}

fn wind_at(f: &VolumeForcing, z: usize) -> DVec2 {
    let h = height(z) as f64;
    let a = ((h - 1000.0) / 3000.0).clamp(0.0, 1.0);
    f.winds[0].lerp(f.winds[2], a)
}

fn environmental_temperature(f: &VolumeForcing, z: usize) -> f32 {
    f.temperature + 273.15 - height(z) * 0.0065
}

fn region(map: &CloudMap, l: &CloudLayer, x: f64, y: f64) -> f32 {
    if l.cover <= 0.001 {
        return 0.0;
    }
    // Match the periodic solver domain, including the moisture source at its seam.
    let p = DVec2::new(x, y) - l.offset;
    ((map.potential(p.x, p.y, l.weights) - super::cover_threshold(l.cover)) / 0.9).clamp(0.0, 1.0)
        as f32
}

fn profile(l: &CloudLayer, region: f32, k: usize, h: f32) -> f32 {
    let reach = if k == 0 { 0.45 + 0.55 * region } else { 1.0 };
    let hn = (h - l.base as f32) / ((l.top - l.base) as f32 * reach).max(1.0);
    region * (hn / 0.12).clamp(0.0, 1.0) * ((1.0 - hn) / 0.35).clamp(0.0, 1.0)
}

fn sample<T: Copy>(cells: &[T], p: DVec3, lerp: impl Fn(T, T, f32) -> T) -> T {
    let x = (p.x / DX as f64 - 0.5).rem_euclid(VOLUME_X as f64);
    let y = (p.y / DX as f64 - 0.5).rem_euclid(VOLUME_X as f64);
    let z = (p.z / DZ as f64 - 0.5).clamp(0.0, (VOLUME_Z - 1) as f64);
    let (ix, iy, iz) = (x as usize, y as usize, z as usize);
    let (fx, fy, fz) = (
        (x - ix as f64) as f32,
        (y - iy as f64) as f32,
        (z - iz as f64) as f32,
    );
    let row = |zz: usize, yy: usize| {
        let base = (zz * VOLUME_X + yy % VOLUME_X) * VOLUME_X;
        lerp(cells[base + ix], cells[base + (ix + 1) % VOLUME_X], fx)
    };
    let plane = |zz: usize| lerp(row(zz, iy), row(zz, iy + 1), fy);
    lerp(plane(iz), plane((iz + 1).min(VOLUME_Z - 1)), fz)
}

impl CloudFrame {
    pub fn sample_extinction(&self, p: DVec3) -> f32 {
        if !(0.0..VOLUME_HEIGHT).contains(&p.z) {
            return 0.0;
        }
        sample(&self.extinction, p, |a, b, t| a + (b - a) * t)
    }
}

impl CloudSnapshot<'_> {
    pub fn blend(&self) -> f32 {
        ((self.time - self.previous.time) / (self.current.time - self.previous.time).max(1e-9))
            .clamp(0.0, 1.0) as f32
    }

    pub fn sample_extinction(&self, p: DVec3) -> f32 {
        let z = (p.z / DZ as f64).clamp(0.0, (VOLUME_Z - 1) as f64) as usize;
        let at = |f: &CloudFrame| p - (f.winds[z] * (self.time - f.time)).extend(0.0);
        let a = self.previous.sample_extinction(at(self.previous));
        let b = self.current.sample_extinction(at(self.current));
        a + (b - a) * self.blend()
    }

    /// Coarse optical-depth integration, shared by road forcing and shadow baking.
    pub fn sun_transmittance(&self, p: DVec3, sun: DVec3) -> f64 {
        if sun.z <= 0.01 {
            return 0.0;
        }
        let mut tau = 0.0;
        let slant = sun.z.max(0.125);
        for z in 0..VOLUME_Z {
            let h = height(z) as f64;
            if h > p.z {
                let at = (p + sun * ((h - p.z) / slant)).with_z(h);
                tau += self.sample_extinction(at) as f64 * DZ as f64 / slant;
                if tau > 9.2 {
                    return 0.0001;
                }
            }
        }
        (-tau).exp()
    }
}

impl CloudVolume {
    pub(super) fn new(map: &Arc<CloudMap>, f: &VolumeForcing, time: f64) -> Self {
        let cells: Vec<Vec4> = (0..COUNT)
            .into_par_iter()
            .map(|i| {
                let z = i / (VOLUME_X * VOLUME_X);
                let h = height(z);
                let x = (i % VOLUME_X) as f64 * DX as f64 + 0.5 * DX as f64;
                let y = (i / VOLUME_X % VOLUME_X) as f64 * DX as f64 + 0.5 * DX as f64;
                let pressure = pressure_at(h, f.pressure);
                let t = environmental_temperature(f, z);
                let qs = saturation(t, pressure);
                let mut liquid = 0.0f32;
                for (k, l) in f.layers.iter().take(3).enumerate() {
                    // The old broad region included a second Worley-cell mask. The
                    // physical field directly uses the requested cover, without that
                    // expansion (which otherwise turns fair cumulus into a deck).
                    liquid += profile(l, region(map, l, x, y), k, h) * 0.00022;
                }
                // A cloudy column is saturated; dry gaps retain evaporative capacity.
                let rh = (saturation(f.dew_point + 273.15, f.pressure)
                    / saturation(f.temperature + 273.15, f.pressure))
                .clamp(0.25, 0.95);
                Vec4::new(
                    t / exner(pressure),
                    qs * if liquid > 1e-7 { 1.0 } else { rh },
                    liquid,
                    0.0,
                )
            })
            .collect();
        let current = Self::frame(cells, f, time);
        Self {
            previous: current.clone(),
            current,
            source_map: Arc::clone(map),
        }
    }

    fn frame(cells: Vec<Vec4>, f: &VolumeForcing, time: f64) -> CloudFrame {
        let coefficients: [f32; VOLUME_Z] = std::array::from_fn(|z| {
            let p = pressure_at(height(z), f.pressure);
            p * 100.0 / (287.05 * exner(p)) * MASS_EXTINCTION
        });
        let extinction = cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let z = i / (VOLUME_X * VOLUME_X);
                c.z / c.x * coefficients[z]
            })
            .collect();
        CloudFrame {
            time,
            cells: Arc::new(cells),
            extinction: Arc::new(extinction),
            winds: std::array::from_fn(|z| wind_at(f, z)),
        }
    }

    pub(super) fn advance(&mut self, f: &VolumeForcing) {
        self.previous = self.current.clone();
        let max_wind = f.winds.iter().map(|w| w.length()).fold(0.0f64, f64::max);
        let substeps = (VOLUME_TICK * (max_wind / DX as f64).max(8.0 / DZ as f64))
            .ceil()
            .max(1.0) as usize;
        let dt = (VOLUME_TICK / substeps as f64) as f32;
        let rh = (saturation(f.dew_point + 273.15, f.pressure)
            / saturation(f.temperature + 273.15, f.pressure))
        .clamp(0.1, 1.0);
        // Large-scale moisture supply is an external forcing from the forecast,
        // analogous to open-boundary humid air entering a small weather domain.
        // Without this, a periodic box exhausts its cloud water through entrainment.
        // Cache horizontal profiles once per column, not once per volume sample.
        let regions: Vec<[f32; 3]> = (0..VOLUME_X * VOLUME_X)
            .map(|i| {
                let x = (i % VOLUME_X) as f64 * DX as f64 + 0.5 * DX as f64;
                let y = (i / VOLUME_X) as f64 * DX as f64 + 0.5 * DX as f64;
                std::array::from_fn(|k| region(&self.source_map, &f.layers[k], x, y))
            })
            .collect();
        let environment: [(f32, f32, f32, f32, DVec2); VOLUME_Z] = std::array::from_fn(|z| {
            let pressure = pressure_at(height(z), f.pressure);
            let t = environmental_temperature(f, z);
            (
                pressure,
                exner(pressure),
                t,
                saturation(t, pressure) * rh,
                wind_at(f, z),
            )
        });
        for _ in 0..substeps {
            let cells = (0..COUNT)
                .into_par_iter()
                .map(|i| {
                    let z = i / (VOLUME_X * VOLUME_X);
                    let h = height(z);
                    let p = DVec3::new(
                        (i % VOLUME_X) as f64 * DX as f64 + 0.5 * DX as f64,
                        (i / VOLUME_X % VOLUME_X) as f64 * DX as f64 + 0.5 * DX as f64,
                        h as f64,
                    );
                    let old = self.current.cells[i];
                    let (pressure, pi, environment, dry_humidity, horizontal_wind) = environment[z];
                    let r = regions[i % (VOLUME_X * VOLUME_X)];
                    let moist = (0..3)
                        .map(|k| profile(&f.layers[k], r[k], k, h))
                        .fold(0.0f32, f32::max);
                    let cloudy = (moist / 0.03).clamp(0.0, 1.0);
                    let local_rh = rh + (1.0 - rh) * cloudy * cloudy * (3.0 - 2.0 * cloudy);
                    let humidity = dry_humidity / rh * local_rh;
                    let velocity = horizontal_wind.extend(old.w as f64);
                    let mut c = sample(&self.current.cells, p - velocity * dt as f64, |a, b, t| {
                        a.lerp(b, t)
                    });
                    let buoyancy = 9.81
                        * ((c.x * pi - environment) / environment + 0.61 * (c.y - humidity) - c.z);
                    let thermal = (f.sunlight / 650.0).clamp(0.0, 1.0)
                        * (1.0 - h / 1800.0).max(0.0)
                        * (0.1 + 0.9 * r[0]);
                    c.w = ((c.w + buoyancy * dt) * (-dt / 90.0).exp()).clamp(-8.0, 8.0);
                    // Surface heat/moisture and free-atmosphere entrainment are explicit
                    // external sources, not silently part of the phase-change budget.
                    c.x += (environment / pi - c.x) * (1.0 - (-dt / 1800.0).exp());
                    c.x += thermal * dt * 0.0015;
                    let supplied_water = saturation(c.x * pi, pressure) * local_rh + 0.001 * moist;
                    c.y += (supplied_water - c.y - c.z) * (1.0 - (-dt / 90.0).exp());
                    c.y = c.y.max(0.0);
                    c.z = c.z.max(0.0);
                    equilibrate(&mut c, pressure);
                    c
                })
                .collect();
            self.current = Self::frame(cells, f, self.current.time + dt as f64);
        }
        self.current.time = self.previous.time + VOLUME_TICK;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_change_conserves_water_and_enthalpy() {
        for (vapour, liquid) in [(0.025, 0.0), (0.001, 0.002)] {
            let mut c = Vec4::new(290.0, vapour, liquid, 0.0);
            let water = c.y + c.z;
            let enthalpy = c.x + LATENT_CP * c.y;
            equilibrate(&mut c, 1000.0);
            assert!((c.y + c.z - water).abs() < 1e-7);
            assert!((c.x + LATENT_CP * c.y - enthalpy).abs() < 1e-3);
            assert!(c.y >= 0.0 && c.z >= 0.0);
            assert!(c.y <= saturation(c.x, 1000.0) + 2e-6);
        }
    }

    #[test]
    fn moist_ascent_condenses_and_dry_air_evaporates() {
        let mut c = Vec4::new(290.0, saturation(290.0, 1000.0), 0.0, 0.0);
        equilibrate(&mut c, 850.0);
        assert!(c.z > 0.0001);
        c.y = 0.0;
        equilibrate(&mut c, 850.0);
        assert!(c.z < 1e-6);
    }

    #[test]
    fn sampler_wraps_horizontally_without_a_seam() {
        let cells: Vec<f32> = (0..COUNT).map(|i| (i % 97) as f32).collect();
        let at = |p| sample(&cells, p, |a, b, t| a + (b - a) * t);
        let p = DVec3::new(-7.0, 29.0, 750.0);
        assert!((at(p) - at(p + DVec3::new(VOLUME_PERIOD, -VOLUME_PERIOD, 0.0))).abs() < 1e-4);
    }
}
