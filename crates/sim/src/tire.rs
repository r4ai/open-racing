//! Tyre force model: Pacejka "Magic Formula" with combined slip via normalised slip,
//! plus inflation pressure, a tread (three zones) and carcass temperature model, wear
//! and dirt picked up off the road, which together scale the grip.

use serde::{Deserialize, Serialize};

use crate::AMBIENT_TEMPERATURE;
use crate::track::Surface;

/// Shape of one Magic Formula curve, specified by physically meaningful numbers.
///
/// `stiffness` (B) is derived at load time so that the curve peaks at `peak_slip`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CurveParams {
    /// Slip at which the force peaks (slip ratio, or slip angle in radians).
    pub peak_slip: f64,
    /// Shape factor C (1.3..1.9 typical). Controls the drop after the peak.
    pub shape: f64,
    /// Curvature factor E (<1). Controls how sharp the peak is.
    pub curvature: f64,
}

/// Magic Formula curve with precomputed stiffness factor.
///
/// The curve is tabulated over `B·x` with its exact slopes and evaluated by cubic
/// Hermite interpolation, which is several times cheaper than the chain of `atan`,
/// `atan` and `sin` and matches it to about 1e-9.
#[derive(Clone, Debug)]
pub struct Curve {
    pub b: f64,
    pub c: f64,
    pub e: f64,
    pub peak_slip: f64,
    /// Value and slope (per table step) at `B·x = k / TABLE_STEPS_PER_UNIT`.
    table: Box<[(f64, f64)]>,
}

/// Table resolution in steps per unit of `B·x`, and its extent; beyond it the curve is
/// evaluated directly.
const TABLE_STEPS_PER_UNIT: f64 = 64.0;
const TABLE_END: f64 = 64.0;

impl Curve {
    pub fn new(p: &CurveParams) -> Self {
        // Peak of sin(C·atan(φ)) is at φ = tan(π / 2C), where φ = Bx − E(Bx − atan Bx).
        // Solve for B·x_peak by bisection (φ is monotonic in Bx for E < 1).
        let target = (std::f64::consts::PI / (2.0 * p.shape)).tan();
        let phi = |bx: f64| bx - p.curvature * (bx - bx.atan());
        let (mut lo, mut hi) = (0.0_f64, 1.0e3_f64);
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if phi(mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let mut curve = Self {
            b: 0.5 * (lo + hi) / p.peak_slip,
            c: p.shape,
            e: p.curvature,
            peak_slip: p.peak_slip,
            table: Box::default(),
        };
        let h = 1.0 / TABLE_STEPS_PER_UNIT;
        curve.table = (0..=(TABLE_END * TABLE_STEPS_PER_UNIT) as usize + 1)
            .map(|k| {
                let bx = k as f64 * h;
                let phi = bx - curve.e * (bx - bx.atan());
                let dphi = 1.0 - curve.e + curve.e / (1.0 + bx * bx);
                let slope = (curve.c * phi.atan()).cos() * curve.c / (1.0 + phi * phi) * dphi;
                (curve.exact(bx), slope * h)
            })
            .collect();
        curve
    }

    /// The Magic Formula at `B·x`.
    #[inline]
    fn exact(&self, bx: f64) -> f64 {
        (self.c * (bx - self.e * (bx - bx.atan())).atan()).sin()
    }

    /// Normalised force (−1..1) at the given slip.
    #[inline]
    pub fn eval(&self, x: f64) -> f64 {
        let bx = self.b * x;
        let t = bx.abs() * TABLE_STEPS_PER_UNIT;
        // Also false for NaN, which the formula passes on.
        if t < TABLE_END * TABLE_STEPS_PER_UNIT {
            let i = t as usize;
            let u = t - i as f64;
            let ((y0, m0), (y1, m1)) = (self.table[i], self.table[i + 1]);
            // Cubic Hermite basis.
            let u2 = u * u;
            let u3 = u2 * u;
            let y = (2.0 * u3 - 3.0 * u2 + 1.0) * y0
                + (u3 - 2.0 * u2 + u) * m0
                + (3.0 * u2 - 2.0 * u3) * y1
                + (u3 - u2) * m1;
            y.copysign(bx)
        } else {
            self.exact(bx)
        }
    }
}

/// One tyre: size, carcass and compound. Loaded from `assets/tires/*.ron`; a car names
/// the tyre for each axle.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TireParams {
    /// Human readable name, e.g. make, compound and size.
    pub name: String,
    /// Unloaded radius in metres.
    pub radius: f64,
    /// Section width in metres.
    pub width: f64,
    /// Vertical stiffness at the optimal pressure in N/m.
    pub vertical_stiffness: f64,
    /// Vertical damping in N·s/m.
    pub vertical_damping: f64,
    /// Friction coefficient at `nominal_load`, longitudinal.
    pub mu_x: f64,
    /// Friction coefficient at `nominal_load`, lateral.
    pub mu_y: f64,
    /// Nominal load in N used for load sensitivity.
    pub nominal_load: f64,
    /// Relative change of μ per unit relative load change (negative: μ drops with load).
    pub load_sensitivity: f64,
    /// Force curves at the optimal pressure.
    pub longitudinal: CurveParams,
    pub lateral: CurveParams,
    /// Longitudinal relaxation length in metres.
    pub relaxation_x: f64,
    /// Lateral relaxation length in metres.
    pub relaxation_y: f64,
    /// Slip angle equivalent per radian of inclination (camber thrust).
    pub camber_thrust: f64,
    /// Optimal camber relative to the road in radians (negative = top leaning inwards).
    pub optimal_camber: f64,
    /// Grip loss per rad² of deviation from the optimal camber.
    pub camber_grip_loss: f64,
    /// Pneumatic trail at zero slip in metres (drives aligning torque / FFB).
    pub pneumatic_trail: f64,
    /// Rolling resistance coefficient at the optimal pressure.
    pub rolling_resistance: f64,
    pub pressure: PressureParams,
    pub thermal: ThermalParams,
}

/// How the inflation pressure changes the tyre. Pressures are gauge, in bar.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PressureParams {
    /// Hot pressure at which grip peaks.
    pub optimal: f64,
    /// Pressure distance from the optimum over which grip falls away.
    pub window: f64,
    /// Fraction of grip lost far outside the window.
    pub grip_loss: f64,
    /// Change of vertical stiffness per bar, N/m.
    pub stiffness_per_bar: f64,
    /// Relative change of the peak slips per bar (negative: a harder carcass peaks earlier).
    pub peak_slip_per_bar: f64,
    /// Relative change of the rolling resistance per bar.
    pub rolling_resistance_per_bar: f64,
    /// Load share moved from each shoulder to the centre of the tread per bar over the optimum.
    pub crown_load_per_bar: f64,
}

/// Heating, cooling and wear of the tread.
///
/// The tread surface is split into inner, middle and outer zones. Each is heated by its
/// load share of the sliding power and of the tread's part of rolling hysteresis, is
/// cooled by the air and the road, and exchanges heat with its neighbours and the
/// carcass. The carcass takes the rest of the rolling hysteresis; the air inside follows
/// its temperature. Grip follows the load-weighted surface temperature.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThermalParams {
    /// Surface temperature at which grip peaks, °C.
    pub optimal_temperature: f64,
    /// Temperature distance below / above the optimum over which grip falls away, K.
    pub cold_window: f64,
    pub hot_window: f64,
    /// Fraction of grip lost far outside the window.
    pub window_grip_loss: f64,
    /// Temperature of tread and carcass after a reset (out of the tyre blankets), °C.
    pub start_temperature: f64,
    /// Heat capacity of the whole tread surface layer / the carcass, J/K.
    pub surface_capacity: f64,
    pub core_capacity: f64,
    /// Conductance between the whole tread surface and the carcass, W/K.
    pub core_conductance: f64,
    /// Conductance between neighbouring tread zones, W/K.
    pub zone_conductance: f64,
    /// Whole-tread to air conductance at rest, and its increase per m/s of rolling speed, W/K.
    pub air_cooling: f64,
    pub air_cooling_per_speed: f64,
    /// Contact patch to road conductance, W/K.
    pub road_cooling: f64,
    /// Share of the sliding power at the contact patch that heats the tread (the rest heats the road).
    pub slide_heat_share: f64,
    /// Share of the rolling hysteresis generated in the tread, by zone load (the rest heats the carcass).
    pub rolling_heat_share: f64,
    /// Load share moved from the outer to the inner shoulder per rad of inward lean.
    pub camber_load_shift: f64,
    /// Outward lean of the contact patch per unit of inward lateral force / load, rad
    /// (the carcass rolling onto its outer shoulder in a corner).
    pub carcass_roll: f64,
    /// Tread worn away per MJ of sliding energy up to the optimal temperature (1 = worn out).
    /// Wear speeds up by one rate per `hot_window` above the optimum.
    pub wear_rate: f64,
    /// Fraction of grip lost with a worn-out tread.
    pub wear_grip_loss: f64,
}

/// Atmospheric pressure, bar.
const ATMOSPHERE: f64 = 1.01325;
/// 0 °C in K.
const KELVIN: f64 = 273.15;

/// Grip lost by a tread fully coated, per kind of coat ([`crate::Coat`] order): wet grass
/// clippings are slickest, grit rolls under the tread like ball bearings.
const COAT_GRIP_LOSS: [f64; 3] = [0.3, 0.25, 0.2];
/// Distance off the road over which the tread picks up most of a coat, m.
const DIRT_PICKUP_LENGTH: f64 = 8.0;
/// Distance rolled on paved surfaces over which the tread sheds most of its coat, m,
/// per kind of coat: loose grit flies off quickly, sticky soil takes longest.
const COAT_SHED_LENGTH: [f64; 3] = [80.0, 100.0, 40.0];
/// How much faster sliding scrubs the coat off than rolling, per metre.
const DIRT_SCRUB: f64 = 4.0;

/// Temperature, wear and dirt of one tyre.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TireCondition {
    /// Surface temperature of the inner, middle and outer tread zone, °C.
    pub tread_temperature: [f64; 3],
    /// Carcass temperature, °C.
    pub core_temperature: f64,
    /// Tread worn away, 0 = new, 1 = worn out.
    pub wear: f64,
    /// Loose material on the tread by kind ([`crate::Coat`] order); the sum is 0 when clean and
    /// 1 when fully coated.
    pub coat: [f64; 3],
}

impl TireCondition {
    /// Gauge pressure in bar of a tyre set to `cold` bar at the ambient temperature: the
    /// air inside is at the carcass temperature.
    #[inline]
    pub fn pressure(&self, cold: f64) -> f64 {
        (cold + ATMOSPHERE) * (self.core_temperature + KELVIN) / (AMBIENT_TEMPERATURE + KELVIN)
            - ATMOSPHERE
    }

    /// How much of the tread is coated, 0 = clean, 1 = fully.
    #[inline]
    pub fn dirt(&self) -> f64 {
        self.coat.iter().sum()
    }

    /// Covers `coat` (by kind, as shares of the tread) of the part of the tread still
    /// clean, so a coated tyre picks up less.
    #[inline]
    pub fn add_coat(&mut self, coat: [f64; 3]) {
        let room = (1.0 - self.dirt()).max(0.0);
        let scale = room / coat.iter().sum::<f64>().max(1.0);
        for (c, add) in self.coat.iter_mut().zip(coat) {
            *c += add * scale;
        }
    }

    /// Rolls `distance` m on `surface` (sliding `slide` m of it): picks up its loose
    /// material, `pickup` (0..1) setting how readily, or sheds the coat on a paved
    /// surface, faster while sliding. Returns what was shed, by kind of coat.
    #[inline]
    pub fn roll_dirt(
        &mut self,
        surface: Surface,
        pickup: f64,
        distance: f64,
        slide: f64,
    ) -> [f64; 3] {
        if let Some(coat) = surface.coat() {
            let mut add = [0.0; 3];
            add[coat as usize] = (pickup * distance / DIRT_PICKUP_LENGTH).min(1.0);
            self.add_coat(add);
            return [0.0; 3];
        }
        if !surface.paved() {
            return [0.0; 3];
        }
        let wipe = distance + DIRT_SCRUB * slide;
        std::array::from_fn(|k| {
            let shed = self.coat[k] * (wipe / COAT_SHED_LENGTH[k]).min(1.0);
            self.coat[k] -= shed;
            shed
        })
    }

    /// Surface temperature weighted by the load on each tread zone, °C.
    #[inline]
    pub fn surface_temperature(&self, load: &[f64; 3]) -> f64 {
        (0..3).map(|k| load[k] * self.tread_temperature[k]).sum()
    }
}

/// Tyre forces in the contact patch frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct TireForce {
    pub fx: f64,
    pub fy: f64,
    /// Self-aligning torque about the contact normal.
    pub mz: f64,
}

/// Grip multiplier falling from 1 at `delta = 0` towards `1 - loss` far outside `window`.
#[inline]
fn window_grip(delta: f64, window: f64, loss: f64) -> f64 {
    1.0 - loss * (1.0 - (-(delta / window).powi(2)).exp())
}

#[derive(Clone, Debug)]
pub struct TireModel {
    pub p: TireParams,
    pub long: Curve,
    pub lat: Curve,
}

impl TireModel {
    pub fn new(p: TireParams) -> Self {
        Self {
            long: Curve::new(&p.longitudinal),
            lat: Curve::new(&p.lateral),
            p,
        }
    }

    /// A new tyre at its start temperature.
    pub fn fresh(&self) -> TireCondition {
        let t = self.p.thermal.start_temperature;
        TireCondition {
            tread_temperature: [t; 3],
            core_temperature: t,
            wear: 0.0,
            coat: [0.0; 3],
        }
    }

    #[inline]
    pub fn vertical_stiffness(&self, pressure: f64) -> f64 {
        self.p.vertical_stiffness
            + self.p.pressure.stiffness_per_bar * (pressure - self.p.pressure.optimal)
    }

    #[inline]
    pub fn rolling_resistance(&self, pressure: f64) -> f64 {
        self.p.rolling_resistance
            * (1.0
                + self.p.pressure.rolling_resistance_per_bar * (pressure - self.p.pressure.optimal))
    }

    /// Share of the load on the inner, middle and outer tread zone.
    ///
    /// * `lean` – inclination towards the car's centreline, rad
    /// * `inward_force` – lateral force towards the car's centreline per unit load
    #[inline]
    pub fn tread_load(&self, lean: f64, inward_force: f64, pressure: f64) -> [f64; 3] {
        let t = &self.p.thermal;
        let shift = t.camber_load_shift * (lean - t.carcass_roll * inward_force);
        let crown = self.p.pressure.crown_load_per_bar * (pressure - self.p.pressure.optimal);
        let w = [1.0 + shift - crown, 1.0 + 2.0 * crown, 1.0 - shift - crown].map(|x| x.max(0.05));
        let sum: f64 = w.iter().sum();
        w.map(|x| x / sum)
    }

    /// Grip multiplier for tread temperature, wear, pressure and dirt.
    #[inline]
    pub fn condition_grip(&self, c: &TireCondition, load: &[f64; 3], pressure: f64) -> f64 {
        let t = &self.p.thermal;
        let dt = c.surface_temperature(load) - t.optimal_temperature;
        let temperature = window_grip(
            dt,
            if dt < 0.0 {
                t.cold_window
            } else {
                t.hot_window
            },
            t.window_grip_loss,
        );
        let pp = &self.p.pressure;
        temperature
            * window_grip(pressure - pp.optimal, pp.window, pp.grip_loss)
            * (1.0 - t.wear_grip_loss * c.wear)
            * (1.0 - (0..3).map(|k| COAT_GRIP_LOSS[k] * c.coat[k]).sum::<f64>())
    }

    /// Advances temperatures and wear by `dt`.
    ///
    /// * `load` – share of the load on each tread zone, see [`Self::tread_load`]
    /// * `slide_power` – power dissipated by sliding at the contact patch, W
    /// * `rolling_power` – power lost to rolling resistance, W
    /// * `speed` – rolling speed, m/s
    /// * `on_road` – whether the tyre touches the road
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn update_condition(
        &self,
        c: &mut TireCondition,
        load: &[f64; 3],
        slide_power: f64,
        rolling_power: f64,
        speed: f64,
        on_road: bool,
        dt: f64,
    ) {
        let t = &self.p.thermal;
        let overheat =
            (c.surface_temperature(load) - t.optimal_temperature).max(0.0) / t.hot_window;
        c.wear = (c.wear + t.wear_rate * 1e-6 * slide_power * (1.0 + overheat) * dt).min(1.0);

        let old = c.tread_temperature;
        let cooling = (t.air_cooling
            + t.air_cooling_per_speed * speed
            + if on_road { t.road_cooling } else { 0.0 })
            / 3.0;
        let mut to_core = 0.0;
        for k in 0..3 {
            let core = t.core_conductance / 3.0 * (old[k] - c.core_temperature);
            let neighbours: f64 = [k.wrapping_sub(1), k + 1]
                .iter()
                .filter_map(|&j| old.get(j))
                .map(|&tj| t.zone_conductance * (old[k] - tj))
                .sum();
            let cooling = cooling * (old[k] - AMBIENT_TEMPERATURE);
            let heat =
                (t.slide_heat_share * slide_power + t.rolling_heat_share * rolling_power) * load[k];
            c.tread_temperature[k] +=
                dt * (heat - core - neighbours - cooling) / (t.surface_capacity / 3.0);
            to_core += core;
        }
        c.core_temperature +=
            dt * ((1.0 - t.rolling_heat_share) * rolling_power + to_core) / t.core_capacity;
    }

    /// Steady-state forces for given transient slips.
    ///
    /// * `kappa` – longitudinal slip ratio
    /// * `alpha` – slip angle (tan α) with camber thrust already folded in
    /// * `fz` – normal load in N
    /// * `mu_scale` – surface, camber and condition grip multiplier
    /// * `pressure` – inflation pressure, bar
    #[inline]
    pub fn forces(
        &self,
        kappa: f64,
        alpha: f64,
        fz: f64,
        mu_scale: f64,
        pressure: f64,
    ) -> TireForce {
        if fz <= 0.0 {
            return TireForce::default();
        }
        let dfz = (fz - self.p.nominal_load) / self.p.nominal_load;
        let load_mu = (1.0 + self.p.load_sensitivity * dfz).max(0.2) * mu_scale;

        // Normalised combined slip: each direction is scaled by its own peak slip, then
        // the pure-slip curve is evaluated at the combined magnitude and split back.
        // Pressure stretches or squeezes both curves along the slip axis.
        let slip_scale =
            1.0 + self.p.pressure.peak_slip_per_bar * (pressure - self.p.pressure.optimal);
        let sx = kappa / (self.long.peak_slip * slip_scale);
        let sy = alpha / (self.lat.peak_slip * slip_scale);
        let rho = (sx * sx + sy * sy).sqrt();
        if rho < 1e-9 {
            return TireForce::default();
        }
        let fx = self.p.mu_x * load_mu * fz * self.long.eval(rho * self.long.peak_slip) * sx / rho;
        let fy = self.p.mu_y * load_mu * fz * self.lat.eval(rho * self.lat.peak_slip) * sy / rho;

        // Pneumatic trail collapses as the tyre saturates, so aligning torque drops
        // past the peak – the classic "light steering" cue.
        let trail = self.p.pneumatic_trail * (1.0 - (sy.abs() / 1.5).min(1.0)).powi(2);
        TireForce {
            fx,
            fy,
            mz: -trail * fy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::CarModel;

    #[test]
    fn curve_peaks_at_requested_slip() {
        let c = Curve::new(&CurveParams {
            peak_slip: 0.1,
            shape: 1.6,
            curvature: 0.5,
        });
        let peak = c.eval(0.1);
        assert!((peak - 1.0).abs() < 1e-6);
        assert!(c.eval(0.09) < peak && c.eval(0.11) < peak);
        assert!((c.eval(-0.1) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn tabulated_curve_matches_the_formula() {
        for (shape, curvature) in [(1.3, -1.0), (1.6, 0.5), (1.9, 0.95), (1.45, 0.0)] {
            let c = Curve::new(&CurveParams {
                peak_slip: 0.08,
                shape,
                curvature,
            });
            let mut worst: f64 = 0.0;
            for k in -20_000..=20_000 {
                let x = k as f64 * 1e-4 * 1.37;
                worst = worst.max((c.eval(x) - c.exact(c.b * x)).abs());
            }
            assert!(worst < 1e-8, "C {shape} E {curvature}: error {worst}");
        }
    }

    fn front() -> TireModel {
        CarModel::gt3().front_tire
    }

    #[test]
    fn grass_coats_the_tread_and_the_road_wipes_it_off() {
        use crate::track::Coat;
        let mut c = front().fresh();
        for _ in 0..20 {
            c.roll_dirt(Surface::Grass, 1.0, 1.0, 0.0);
        }
        assert!(c.coat[Coat::Grass as usize] > 0.9, "coat {:?}", c.coat);
        // The coat is full: gravel adds nothing.
        c.roll_dirt(Surface::Gravel, 1.0, 1.0, 0.0);
        assert!(c.dirt() <= 1.0 + 1e-12);
        let coated = c.dirt();
        let shed: f64 = (0..100)
            .map(|_| {
                c.roll_dirt(Surface::Asphalt, 0.0, 1.0, 0.0)
                    .iter()
                    .sum::<f64>()
            })
            .sum();
        assert!((shed + c.dirt() - coated).abs() < 1e-12);
        assert!(
            (0.15..0.45).contains(&c.dirt()),
            "dirt after 100 m {}",
            c.dirt()
        );
        let rolled = c.dirt();
        c.roll_dirt(Surface::Asphalt, 0.0, 10.0, 10.0);
        assert!(rolled - c.dirt() > 5.0 * rolled * 10.0 / COAT_SHED_LENGTH[0] * 0.9);
        // Turf barely coats.
        let mut t = front().fresh();
        t.roll_dirt(Surface::Turf, Surface::Turf.dirt(), 1.0, 0.0);
        assert!(t.dirt() < 0.02);
    }

    #[test]
    fn grip_peaks_at_optimal_temperature_and_pressure_and_drops_with_wear() {
        let tire = front();
        let (t, p) = (tire.p.thermal.optimal_temperature, tire.p.pressure.optimal);
        let even = [1.0 / 3.0; 3];
        let at = |temperature: f64, pressure: f64, wear: f64| {
            tire.condition_grip(
                &TireCondition {
                    tread_temperature: [temperature; 3],
                    core_temperature: temperature,
                    wear,
                    coat: [0.0; 3],
                },
                &even,
                pressure,
            )
        };
        assert!((at(t, p, 0.0) - 1.0).abs() < 1e-12);
        assert!(at(t - 40.0, p, 0.0) < at(t - 10.0, p, 0.0));
        assert!(at(t + 40.0, p, 0.0) < at(t + 10.0, p, 0.0));
        assert!(at(t, p - 0.3, 0.0) < at(t, p - 0.1, 0.0));
        assert!(at(t, p + 0.3, 0.0) < at(t, p + 0.1, 0.0));
        assert!(at(t, p, 1.0) < at(t, p, 0.5));
    }

    #[test]
    fn pressure_follows_carcass_temperature() {
        let mut c = front().fresh();
        c.core_temperature = AMBIENT_TEMPERATURE;
        assert!((c.pressure(1.4) - 1.4).abs() < 1e-12);
        c.core_temperature = 85.0;
        // Gas law: about 0.5 bar rise from 25 °C to 85 °C, as seen on GT3 slicks.
        assert!(
            (1.85..1.95).contains(&c.pressure(1.4)),
            "{}",
            c.pressure(1.4)
        );
    }

    #[test]
    fn tread_load_follows_camber_cornering_and_pressure() {
        let tire = front();
        let p = tire.p.pressure.optimal;
        let [inner, middle, outer] = tire.tread_load(0.0, 0.0, p);
        assert!((inner - middle).abs() < 1e-12 && (outer - middle).abs() < 1e-12);
        let [inner, _, outer] = tire.tread_load(0.06, 0.0, p);
        assert!(inner > outer, "negative camber loads the inner shoulder");
        let [inner, _, outer] = tire.tread_load(0.0, 1.5, p);
        assert!(
            outer > inner,
            "cornering rolls the outside tyre onto its outer shoulder"
        );
        let [inner, middle, _] = tire.tread_load(0.0, 0.0, p + 0.3);
        assert!(middle > inner, "over-inflation loads the crown");
    }

    #[test]
    fn sliding_heats_the_loaded_zone_and_wears_rolling_cools() {
        let tire = front();
        let mut c = tire.fresh();
        let load = [0.5, 0.3, 0.2];
        for _ in 0..3000 {
            tire.update_condition(&mut c, &load, 50_000.0, 0.0, 30.0, true, 1e-3);
        }
        let [inner, middle, outer] = c.tread_temperature;
        assert!(inner > middle && middle > outer, "{c:?}");
        assert!(outer > tire.p.thermal.start_temperature + 20.0, "{c:?}");
        assert!(c.core_temperature > tire.p.thermal.start_temperature);
        assert!(c.wear > 0.0);
        let (hot, wear) = (c.surface_temperature(&load), c.wear);
        for _ in 0..20_000 {
            tire.update_condition(&mut c, &load, 0.0, 0.0, 30.0, true, 1e-3);
        }
        assert!(c.surface_temperature(&load) < hot - 20.0, "{c:?}");
        assert_eq!(c.wear, wear);
    }
}
