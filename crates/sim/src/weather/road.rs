//! Temperature of the road surface, patch by patch.
//!
//! The road is split into patches along the track and across it (left, middle,
//! right). Each patch has an asphalt surface layer over a thicker base course and
//! balances, per square metre:
//! - sunshine it absorbs, where neither scenery nor cloud shades it, on its own slope;
//! - diffuse daylight from the part of the sky it sees;
//! - long-wave radiation from the sky (more under cloud and humid air) and from the
//!   scenery around it, against what it emits itself;
//! - convection to the air, stronger in wind;
//! - conduction into the base course and on into the ground.
//!
//! What the scenery hides is found once, by casting rays: the share of the sky each
//! patch sees, and whether it sees the sun at each half hour of the day. Sunlit road
//! gets hot in the afternoon while a patch under a grandstand stays near the air
//! temperature; after sunset open road cools below the air by radiating to a clear sky.

use std::sync::Arc;

use glam::DVec3;

use super::shade::{Occluders, RAY_LIFT};
use crate::track::Track;

/// Length of a patch along the track, m.
const PATCH_LENGTH: f64 = 8.0;
/// Patches across the road.
pub const LANES: usize = 3;
/// Times of day at which the sun's visibility is found, per day.
const SUN_SLOTS: usize = 48;

/// Stefan–Boltzmann constant, W/(m²·K⁴).
const SIGMA: f64 = 5.670_374e-8;
const KELVIN: f64 = 273.15;
/// Solar reflectance and thermal emissivity of weathered asphalt.
const ALBEDO: f64 = 0.12;
const EMISSIVITY: f64 = 0.93;
/// Reflectance of the surroundings, for sunlight they reflect onto the road.
const SURROUNDINGS_ALBEDO: f64 = 0.2;
/// Heat capacity of the top 40 mm of asphalt and of the 260 mm base course below, J/(m²·K).
const SURFACE_CAPACITY: f64 = 8.0e4;
const BASE_CAPACITY: f64 = 5.2e5;
/// Conductance from the surface layer to the base course, and from the base course to
/// the ground below it, W/(m²·K).
const BASE_CONDUCTANCE: f64 = 8.0;
const GROUND_CONDUCTANCE: f64 = 1.5;
/// Convective heat transfer to still air, and its increase per m/s of wind, W/(m²·K).
const CONVECTION: f64 = 6.0;
const CONVECTION_PER_WIND: f64 = 4.0;
/// Rays per patch to find the share of the sky it sees.
const SKY_RAYS: [(f64, usize); 3] = [(0.25, 6), (0.6, 8), (0.92, 6)];

/// The weather that drives the road's temperature at one moment.
#[derive(Clone, Copy, Debug)]
pub struct Forcing {
    /// Unit vector towards the sun (Z up).
    pub sun: DVec3,
    /// Time of day, h.
    pub hour: f64,
    /// Direct sunlight on a surface facing the sun, and diffuse daylight on open level
    /// ground, W/m².
    pub direct: f64,
    pub diffuse: f64,
    /// Air temperature at the track's reference height, °C; it falls with height.
    pub air: f64,
    /// Long-wave radiation from the sky on open level ground, W/m².
    pub sky_longwave: f64,
    /// Wind speed near the ground, m/s.
    pub wind: f64,
    /// Temperature of the ground below the base course, °C.
    pub ground: f64,
}

/// What the scenery hides from each patch. Depends on the track, the scenery and the
/// sun's path, not on the weather.
#[derive(Debug)]
pub struct RoadExposure {
    /// Centre and surface normal of each patch.
    points: Vec<DVec3>,
    normals: Vec<DVec3>,
    /// Share of the sky each patch sees, weighted by its angle of incidence.
    sky_view: Vec<f32>,
    /// Share of the sun each patch sees at each slot, 0..255, slot-major.
    sun: Vec<u8>,
    /// The sun's path the table is for.
    pub day_of_year: u32,
    pub latitude: f64,
}

/// Patches of a track: lateral bounds of the road at each row.
#[derive(Debug)]
struct Rows {
    count: usize,
    spacing: f64,
    /// Left and right road edge at each row, m from the centreline.
    edges: Vec<(f64, f64)>,
    /// Height of the track's lowest point, the reference for the air temperature.
    base: f64,
}

#[derive(Clone, Debug)]
pub struct RoadTemperature {
    rows: Arc<Rows>,
    exposure: Arc<RoadExposure>,
    /// Temperature of each patch's surface layer and base course, °C.
    surface: Vec<f32>,
    base: Vec<f32>,
}

/// Air temperature falls this much per m of height, K.
pub const LAPSE_RATE: f64 = 0.0065;

impl RoadTemperature {
    /// Patches over `track`, with the sun's visibility for `day_of_year` at `latitude`
    /// found against `scenery`. All at `start` °C.
    pub fn new(
        track: &Track,
        scenery: Option<&Occluders>,
        day_of_year: u32,
        latitude: f64,
        start: f64,
    ) -> Self {
        let count = (track.length / PATCH_LENGTH).round().max(1.0) as usize;
        let spacing = track.length / count as f64;
        let mut edges = Vec::with_capacity(count);
        let mut points = Vec::with_capacity(count * LANES);
        let mut normals = Vec::with_capacity(count * LANES);
        for i in 0..count {
            let s = (i as f64 + 0.5) * spacing;
            let smp = track.sample_at(s);
            let (left, right) = (smp.width_left, -smp.width_right);
            edges.push((left, right));
            for lane in 0..LANES {
                let d = left + (right - left) * (lane as f64 + 0.5) / LANES as f64;
                let (p, _, n) = track.pose_at(s, d);
                points.push(p);
                normals.push(n);
            }
        }
        let base = track
            .samples
            .iter()
            .map(|s| s.pos.z)
            .fold(f64::INFINITY, f64::min);
        let rows = Rows {
            count,
            spacing,
            edges,
            base,
        };
        let exposure = RoadExposure::new(points, normals, scenery, day_of_year, latitude);
        let n = count * LANES;
        Self {
            rows: Arc::new(rows),
            exposure: Arc::new(exposure),
            surface: vec![start as f32; n],
            base: vec![start as f32; n],
        }
    }

    pub fn exposure(&self) -> &RoadExposure {
        &self.exposure
    }

    /// Finds the sun's visibility again for another day of the year or latitude.
    pub fn with_sun_path(
        &self,
        scenery: Option<&Occluders>,
        day_of_year: u32,
        latitude: f64,
    ) -> Self {
        let e = &self.exposure;
        let exposure = RoadExposure::new(
            e.points.clone(),
            e.normals.clone(),
            scenery,
            day_of_year,
            latitude,
        );
        Self {
            exposure: Arc::new(exposure),
            ..self.clone()
        }
    }

    /// Height of the lowest point of the track, m, where the air has the reference
    /// temperature.
    pub fn base_height(&self) -> f64 {
        self.rows.base
    }

    fn patch(&self, s: f64, d: f64) -> usize {
        let rows = &self.rows;
        let i = ((s / rows.spacing).floor() as isize).rem_euclid(rows.count as isize) as usize;
        let (left, right) = rows.edges[i];
        let lane = ((left - d) / (left - right) * LANES as f64).floor();
        i * LANES + lane.clamp(0.0, (LANES - 1) as f64) as usize
    }

    /// Surface temperature of the road at track coordinates (s, d), °C. Kerbs and
    /// run-off take the temperature of the nearest road patch.
    #[inline]
    pub fn at(&self, s: f64, d: f64) -> f64 {
        self.surface[self.patch(s, d)] as f64
    }

    /// Mean and range of the surface temperature, °C.
    pub fn stats(&self) -> (f64, f64, f64) {
        let (mut lo, mut hi, mut sum) = (f64::INFINITY, f64::NEG_INFINITY, 0.0);
        for &t in &self.surface {
            let t = t as f64;
            lo = lo.min(t);
            hi = hi.max(t);
            sum += t;
        }
        (sum / self.surface.len() as f64, lo, hi)
    }

    /// Sets every patch to `surface` and `base` °C.
    pub fn fill(&mut self, surface: f64, base: f64) {
        self.surface.fill(surface as f32);
        self.base.fill(base as f32);
    }

    /// Advances the temperatures by `dt` s of weather. `cloud` gives the share of
    /// sunlight the clouds let through to a point.
    pub fn step(&mut self, f: &Forcing, cloud: impl Fn(DVec3) -> f64, dt: f64) {
        let e = &*self.exposure;
        let slot = f.hour.rem_euclid(24.0) / 24.0 * SUN_SLOTS as f64;
        let (s0, u) = (slot.floor() as usize % SUN_SLOTS, slot.fract());
        let s1 = (s0 + 1) % SUN_SLOTS;
        let n = self.surface.len();
        let sun_up = f.sun.z > 0.0 && f.direct > 0.0;
        let h = CONVECTION + CONVECTION_PER_WIND * f.wind;
        // Energy balance per m²; explicit, the layers' time constants are far longer than
        // the steps.
        for k in 0..n {
            let p = e.points[k];
            let air = f.air - LAPSE_RATE * (p.z - self.rows.base);
            let t = self.surface[k] as f64;
            let t_base = self.base[k] as f64;
            let sky = e.sky_view[k] as f64;
            let mut shortwave = f.diffuse * sky;
            if sun_up {
                let seen =
                    (e.sun[s0 * n + k] as f64 * (1.0 - u) + e.sun[s1 * n + k] as f64 * u) / 255.0;
                let incidence = e.normals[k].dot(f.sun).max(0.0);
                if seen > 0.0 && incidence > 0.0 {
                    shortwave += f.direct * incidence * seen * cloud(p);
                }
            }
            // Sunlight on the surroundings, part of it reflected onto the road.
            shortwave +=
                (1.0 - sky) * SURROUNDINGS_ALBEDO * (f.direct * f.sun.z.max(0.0) + f.diffuse);
            let surroundings = EMISSIVITY * SIGMA * (air + KELVIN).powi(4);
            let longwave = EMISSIVITY
                * (sky * f.sky_longwave + (1.0 - sky) * surroundings
                    - SIGMA * (t + KELVIN).powi(4));
            let convection = h * (air - t);
            let to_base = BASE_CONDUCTANCE * (t - t_base);
            let to_ground = GROUND_CONDUCTANCE * (t_base - f.ground);
            let net = (1.0 - ALBEDO) * shortwave + longwave + convection - to_base;
            self.surface[k] = (t + dt * net / SURFACE_CAPACITY) as f32;
            self.base[k] = (t_base + dt * (to_base - to_ground) / BASE_CAPACITY) as f32;
        }
    }
}

impl RoadExposure {
    fn new(
        points: Vec<DVec3>,
        normals: Vec<DVec3>,
        scenery: Option<&Occluders>,
        day_of_year: u32,
        latitude: f64,
    ) -> Self {
        let n = points.len();
        let scenery = scenery.filter(|o| !o.is_empty());
        let mut sky_view = vec![1.0f32; n];
        let mut sun = vec![0u8; n * SUN_SLOTS];
        let suns: Vec<DVec3> = (0..SUN_SLOTS)
            .map(|slot| {
                super::sun_direction(latitude, day_of_year, slot as f64 * 24.0 / SUN_SLOTS as f64)
            })
            .collect();
        let origin = |k: usize| points[k] + normals[k] * RAY_LIFT;
        let sky_dirs = sky_directions();
        let patch = |k: usize| -> (f32, [u8; SUN_SLOTS]) {
            let mut seen = [0u8; SUN_SLOTS];
            let Some(o) = scenery else {
                for (slot, sun) in suns.iter().enumerate() {
                    seen[slot] = if sun.z > 0.0 { 255 } else { 0 };
                }
                return (1.0, seen);
            };
            let p = origin(k);
            let (mut open, mut total) = (0.0, 0.0);
            for &(dir, weight) in &sky_dirs {
                total += weight;
                open += weight * o.transmission(p, dir);
            }
            for (slot, sun) in suns.iter().enumerate() {
                if sun.z > 0.0 {
                    seen[slot] = (o.transmission(p, *sun) * 255.0).round() as u8;
                }
            }
            ((open / total) as f32, seen)
        };
        // The rays are independent; spread the patches over the cores.
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(16);
        let chunk = n.div_ceil(threads).max(1);
        let results: Vec<Vec<(f32, [u8; SUN_SLOTS])>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..n)
                .step_by(chunk)
                .map(|start| {
                    let patch = &patch;
                    scope.spawn(move || (start..(start + chunk).min(n)).map(patch).collect())
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (k, (view, seen)) in results.into_iter().flatten().enumerate() {
            sky_view[k] = view;
            for (slot, &v) in seen.iter().enumerate() {
                sun[slot * n + k] = v;
            }
        }
        Self {
            points,
            normals,
            sky_view,
            sun,
            day_of_year,
            latitude,
        }
    }

    /// Mean share of the sky the patches see.
    pub fn mean_sky_view(&self) -> f64 {
        self.sky_view.iter().map(|&v| v as f64).sum::<f64>() / self.sky_view.len().max(1) as f64
    }
}

/// Directions over the sky with weights for the light they bring onto level ground
/// (cosine-weighted), in rings of (sine of elevation, count).
fn sky_directions() -> Vec<(DVec3, f64)> {
    let mut dirs = Vec::new();
    for (ring, &(z, count)) in SKY_RAYS.iter().enumerate() {
        let r = (1.0 - z * z).sqrt();
        for i in 0..count {
            let a = (i as f64 + 0.5 * ring as f64) / count as f64 * std::f64::consts::TAU;
            // Each ring stands for a band of the sky whose cosine-weighted share is about
            // equal, so the weights are the incidence per ray.
            dirs.push((DVec3::new(r * a.cos(), r * a.sin(), z), z / count as f64));
        }
    }
    dirs
}
