//! Tyre force model: Pacejka "Magic Formula" with combined slip via normalised slip,
//! plus a two-layer tread temperature model with wear that scales the grip.

use serde::{Deserialize, Serialize};

use crate::AMBIENT_TEMPERATURE;

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
#[derive(Clone, Copy, Debug)]
pub struct Curve {
    pub b: f64,
    pub c: f64,
    pub e: f64,
    pub peak_slip: f64,
}

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
        Self {
            b: 0.5 * (lo + hi) / p.peak_slip,
            c: p.shape,
            e: p.curvature,
            peak_slip: p.peak_slip,
        }
    }

    /// Normalised force (−1..1) at the given slip.
    #[inline]
    pub fn eval(&self, x: f64) -> f64 {
        let bx = self.b * x;
        (self.c * (bx - self.e * (bx - bx.atan())).atan()).sin()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TireParams {
    /// Unloaded radius in metres.
    pub radius: f64,
    /// Vertical stiffness in N/m.
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
    /// Rolling resistance coefficient.
    pub rolling_resistance: f64,
    pub thermal: ThermalParams,
}

/// Heating, cooling and wear of the tread.
///
/// The tread surface is heated by the sliding power at the contact patch and cooled by
/// the air and the road; the carcass is heated by rolling hysteresis and exchanges heat
/// with the surface. Grip follows the surface temperature.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThermalParams {
    /// Surface temperature at which grip peaks, °C.
    pub optimal_temperature: f64,
    /// Temperature distance below / above the optimum over which grip falls away, K.
    pub cold_window: f64,
    pub hot_window: f64,
    /// Fraction of grip lost far outside the window.
    pub window_grip_loss: f64,
    /// Temperature of surface and carcass after a reset (out of the tyre blankets), °C.
    pub start_temperature: f64,
    /// Heat capacity of the tread surface layer / the carcass, J/K.
    pub surface_capacity: f64,
    pub core_capacity: f64,
    /// Conductance between surface and carcass, W/K.
    pub core_conductance: f64,
    /// Surface to air conductance at rest, and its increase per m/s of rolling speed, W/K.
    pub air_cooling: f64,
    pub air_cooling_per_speed: f64,
    /// Surface to road conductance while in contact, W/K.
    pub road_cooling: f64,
    /// Share of the sliding power at the contact patch that heats the tread (the rest heats the road).
    pub slide_heat_share: f64,
    /// Tread worn away per MJ of sliding energy up to the optimal temperature (1 = worn out).
    /// Wear speeds up by one rate per `hot_window` above the optimum.
    pub wear_rate: f64,
    /// Fraction of grip lost with a worn-out tread.
    pub wear_grip_loss: f64,
}

/// Temperature and wear of one tyre.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TireCondition {
    /// Tread surface temperature, °C.
    pub surface_temperature: f64,
    /// Carcass temperature, °C.
    pub core_temperature: f64,
    /// Tread worn away, 0 = new, 1 = worn out.
    pub wear: f64,
}

/// Tyre forces in the contact patch frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct TireForce {
    pub fx: f64,
    pub fy: f64,
    /// Self-aligning torque about the contact normal.
    pub mz: f64,
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
        TireCondition { surface_temperature: t, core_temperature: t, wear: 0.0 }
    }

    /// Grip multiplier for the tread temperature and wear.
    #[inline]
    pub fn condition_grip(&self, c: &TireCondition) -> f64 {
        let t = &self.p.thermal;
        let dt = c.surface_temperature - t.optimal_temperature;
        let window = if dt < 0.0 { t.cold_window } else { t.hot_window };
        let temperature = 1.0 - t.window_grip_loss * (1.0 - (-(dt / window).powi(2)).exp());
        temperature * (1.0 - t.wear_grip_loss * c.wear)
    }

    /// Advances temperatures and wear by `dt`.
    ///
    /// * `slide_power` – power dissipated by sliding at the contact patch, W
    /// * `rolling_power` – power lost to rolling resistance, W
    /// * `speed` – rolling speed, m/s
    /// * `on_road` – whether the tyre touches the road
    #[inline]
    pub fn update_condition(&self, c: &mut TireCondition, slide_power: f64, rolling_power: f64, speed: f64, on_road: bool, dt: f64) {
        let t = &self.p.thermal;
        let to_core = t.core_conductance * (c.surface_temperature - c.core_temperature);
        let cooling = t.air_cooling + t.air_cooling_per_speed * speed + if on_road { t.road_cooling } else { 0.0 };
        let to_ambient = cooling * (c.surface_temperature - AMBIENT_TEMPERATURE);
        let overheat = (c.surface_temperature - t.optimal_temperature).max(0.0) / t.hot_window;
        c.wear = (c.wear + t.wear_rate * 1e-6 * slide_power * (1.0 + overheat) * dt).min(1.0);
        c.surface_temperature += dt * (t.slide_heat_share * slide_power - to_core - to_ambient) / t.surface_capacity;
        c.core_temperature += dt * (rolling_power + to_core) / t.core_capacity;
    }

    /// Steady-state forces for given transient slips.
    ///
    /// * `kappa` – longitudinal slip ratio
    /// * `alpha` – slip angle (tan α) with camber thrust already folded in
    /// * `fz` – normal load in N
    /// * `mu_scale` – surface and camber grip multiplier
    #[inline]
    pub fn forces(&self, kappa: f64, alpha: f64, fz: f64, mu_scale: f64) -> TireForce {
        if fz <= 0.0 {
            return TireForce::default();
        }
        let dfz = (fz - self.p.nominal_load) / self.p.nominal_load;
        let load_mu = (1.0 + self.p.load_sensitivity * dfz).max(0.2) * mu_scale;

        // Normalised combined slip: each direction is scaled by its own peak slip, then
        // the pure-slip curve is evaluated at the combined magnitude and split back.
        let sx = kappa / self.long.peak_slip;
        let sy = alpha / self.lat.peak_slip;
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
    fn grip_peaks_at_optimal_temperature_and_drops_with_wear() {
        let tire = CarModel::gt3().front_tire;
        let at = |temperature: f64, wear: f64| tire.condition_grip(&TireCondition { surface_temperature: temperature, core_temperature: temperature, wear });
        let optimal = tire.p.thermal.optimal_temperature;
        assert!((at(optimal, 0.0) - 1.0).abs() < 1e-12);
        assert!(at(optimal - 40.0, 0.0) < at(optimal - 10.0, 0.0));
        assert!(at(optimal + 40.0, 0.0) < at(optimal + 10.0, 0.0));
        assert!(at(optimal, 1.0) < at(optimal, 0.5));
    }

    #[test]
    fn sliding_heats_and_wears_rolling_cools() {
        let tire = CarModel::gt3().front_tire;
        let mut c = tire.fresh();
        for _ in 0..3000 {
            tire.update_condition(&mut c, 50_000.0, 0.0, 30.0, true, 1e-3);
        }
        assert!(c.surface_temperature > tire.p.thermal.start_temperature + 20.0, "{c:?}");
        assert!(c.core_temperature > tire.p.thermal.start_temperature);
        assert!(c.wear > 0.0);
        let (hot, wear) = (c.surface_temperature, c.wear);
        for _ in 0..20_000 {
            tire.update_condition(&mut c, 0.0, 0.0, 30.0, true, 1e-3);
        }
        assert!(c.surface_temperature < hot - 20.0, "{c:?}");
        assert_eq!(c.wear, wear);
    }
}
