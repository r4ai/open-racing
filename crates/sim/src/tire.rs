//! Tyre force model: Pacejka "Magic Formula" with combined slip via normalised slip.

use serde::{Deserialize, Serialize};

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
}
