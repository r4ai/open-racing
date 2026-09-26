//! Where pipes meet volumes, cylinders, the air and each other.
//!
//! A pipe end opening through an effective area into a reservoir (a volume, a cylinder
//! through its valves, the air) is solved as in Benson's method of characteristics: the
//! wave arriving from inside the pipe is kept (p_f = p + ρc(u − u_f), the acoustic
//! compatibility relation along the outgoing characteristic), and the face velocity is
//! whatever makes the pipe's mass flow equal the quasi-steady flow through the restriction.
//! A shut valve (zero area) reflects the wave from a closed end; a pipe opening into the
//! air reflects it inverted. The one mass, energy and burned-gas flux found this way is
//! given to both the pipe and the reservoir, so both conserve exactly.
//!
//! Two pipes joined end to end exchange the HLLC flux of their end states; a change of
//! section at the joint pushes on the step's wall.

use crate::gas::{Gas, orifice_flow};
use crate::pipe::{Flux, State, hllc};

/// Gas at rest in a reservoir.
#[derive(Clone, Copy, Debug)]
pub struct Reservoir {
    pub p: f64,
    pub t: f64,
    pub y: f64,
    pub gamma: f64,
    pub r: f64,
    /// Specific enthalpy (at rest: also the stagnation enthalpy), J/kg.
    pub h: f64,
}

impl Reservoir {
    pub fn new(gas: &Gas, p: f64, t: f64, y: f64) -> Self {
        Self {
            p,
            t,
            y,
            gamma: gas.gamma(t, y),
            r: gas.r(y),
            h: gas.enthalpy(t, y),
        }
    }

    pub fn density(&self) -> f64 {
        self.p / (self.r * self.t)
    }
}

/// Flux out of a pipe end into a reservoir through the effective area `cda` (capped at
/// the pipe's), given the pipe's end state `s`, the outward direction `sign` (±1 along the
/// pipe's x) and the pipe's end area. Returns the outward flux: mass, outward momentum
/// (with the pressure), energy and burned mass. `guess` holds the face velocity found the
/// step before, which starts the search and is updated.
pub fn pipe_to_reservoir(
    gas: &Gas,
    s: &State,
    sign: f64,
    area: f64,
    cda: f64,
    res: &Reservoir,
    guess: &mut f64,
) -> Flux {
    let v = s.w.u * sign;
    let z = s.w.rho * s.c;
    let cda = cda.min(area);
    if cda <= 0.0 {
        let p = (s.w.p + z * v).max(0.0);
        return [0.0, p * area, 0.0, 0.0];
    }
    let (rho, gamma, r) = (s.w.rho, s.gamma, gas.r(s.w.y));
    let cp = gamma * r / (gamma - 1.0);
    let p_min = 0.02 * s.w.p;
    // Signed outward mass flow through the restriction for a face velocity.
    let flow = |vf: f64| -> (f64, f64, f64) {
        let pf = (s.w.p + z * (v - vf)).max(p_min);
        let rf = rho * (pf / s.w.p).powf(1.0 / gamma);
        let tf = pf / (rf * r);
        // Outflow speed, at most sonic: the face cannot hold more.
        let vo = vf.max(0.0).min((gamma * pf / rf).sqrt());
        let t0 = tf + vo * vo / (2.0 * cp);
        let p0 = pf * (t0 / tf).powf(gamma / (gamma - 1.0));
        let q = if p0 > res.p {
            orifice_flow(cda, p0, t0, res.p, gamma, r).0
        } else {
            -orifice_flow(cda, res.p, res.t, pf, res.gamma, res.r).0
        };
        (q, pf, rf)
    };
    let g = |vf: f64| {
        let (q, _, rf) = flow(vf);
        rf * vf * area - q
    };
    // g rises with vf. Its root lies between a strong compression and the face emptied
    // to the floor pressure; a safeguarded secant from last step's root finds it in a few
    // evaluations.
    let (mut a, mut b) = (v - 2.0 * s.c, v + (s.w.p - p_min) / z);
    let tol = 1e-6 * rho * s.c * area;
    let mut x0 = guess.clamp(a, b);
    let mut g0 = g(x0);
    if g0 < 0.0 {
        a = x0
    } else {
        b = x0
    }
    let mut x1 = (x0 - g0.signum() * 1e-3 * s.c).clamp(a, b);
    let mut x = x0;
    if g0.abs() > tol {
        for _ in 0..50 {
            let g1 = g(x1);
            x = x1;
            if g1.abs() <= tol {
                break;
            }
            if g1 < 0.0 {
                a = x1
            } else {
                b = x1
            }
            let mut x2 = if g1 != g0 {
                x1 - g1 * (x1 - x0) / (g1 - g0)
            } else {
                0.5 * (a + b)
            };
            if !(x2 > a && x2 < b) {
                x2 = 0.5 * (a + b);
            }
            if (x2 - x1).abs() < 1e-9 * s.c {
                x = x2;
                break;
            }
            (x0, g0, x1) = (x1, g1, x2);
        }
    }
    *guess = x;
    let (q, pf, rf) = flow(x);
    let vf = q / (rf * area);
    if q >= 0.0 {
        let tf = pf / (rf * r);
        let h0 = gas.enthalpy(tf, s.w.y) + 0.5 * vf * vf;
        [q, pf * area + q * vf, q * h0, q * s.w.y]
    } else {
        [q, pf * area + q * vf, q * res.h, q * res.y]
    }
}

/// Flux out of a pipe end opening without restriction into a reservoir (a plenum, a
/// collector, the air), in closed form. Outflow leaves at the reservoir's pressure, the
/// face velocity following from the outgoing characteristic (choked at the local speed of
/// sound); inflow accelerates from the reservoir at rest by Bernoulli's equation, with the
/// face pressure again on the characteristic. The iterative `pipe_to_reservoir` gives the
/// same for an unrestricted end within a fraction of a percent, at a tenth of the cost.
pub fn pipe_open_to_reservoir(gas: &Gas, s: &State, sign: f64, area: f64, res: &Reservoir) -> Flux {
    let v = s.w.u * sign;
    let z = s.w.rho * s.c;
    let vf_out = v + (s.w.p - res.p) / z;
    if vf_out >= 0.0 {
        let mut pf = res.p;
        let mut rf = s.w.rho * (pf / s.w.p).powf(1.0 / s.gamma);
        let cf = (s.gamma * pf / rf).sqrt();
        let mut vf = vf_out;
        if vf > cf {
            // Choked: the face stays sonic and its pressure rises above the reservoir's.
            vf = cf;
            pf = s.w.p + z * (v - vf);
            rf = s.w.rho * (pf / s.w.p).powf(1.0 / s.gamma);
        }
        let q = rf * vf * area;
        let tf = pf / (rf * gas.r(s.w.y));
        let h0 = gas.enthalpy(tf, s.w.y) + 0.5 * vf * vf;
        [q, pf * area + q * vf, q * h0, q * s.w.y]
    } else {
        let rho_r = res.density();
        let k = res.p - s.w.p - z * v;
        let b = z * (2.0 / rho_r).sqrt();
        let root = 0.5 * (-b + (b * b + 4.0 * k).sqrt());
        let drop = root * root;
        let pf = res.p - drop;
        let vf = -(2.0 * drop / rho_r).sqrt();
        let rf = rho_r * (pf / res.p).powf(1.0 / res.gamma);
        let q = rf * vf * area;
        [q, pf * area + q * vf, q * res.h, q * res.y]
    }
}

/// Fluxes out of two pipe ends joined to each other: `a` and `b` are the end states with
/// their outward signs and end areas. Returns the outward flux of each.
pub fn pipe_to_pipe(
    a: &State,
    sign_a: f64,
    area_a: f64,
    b: &State,
    sign_b: f64,
    area_b: f64,
) -> (Flux, Flux) {
    // The common axis points out of `a`, into `b`.
    let mut la = *a;
    la.w.u = a.w.u * sign_a;
    let mut rb = *b;
    rb.w.u = -b.w.u * sign_b;
    let f = hllc(&la, &rb);
    let af = area_a.min(area_b);
    let out_a = [
        f[0] * af,
        f[1] * af + a.w.p * (area_a - af),
        f[2] * af,
        f[3] * af,
    ];
    let out_b = [
        -f[0] * af,
        f[1] * af + b.w.p * (area_b - af),
        -f[2] * af,
        -f[3] * af,
    ];
    (out_a, out_b)
}

/// Flow from reservoir `a` to reservoir `b` through `cda`: mass, energy and burned mass
/// leaving `a` (negative when the flow is the other way).
pub fn reservoir_to_reservoir(cda: f64, a: &Reservoir, b: &Reservoir) -> [f64; 3] {
    if a.p >= b.p {
        let q = orifice_flow(cda, a.p, a.t, b.p, a.gamma, a.r).0;
        [q, q * a.h, q * a.y]
    } else {
        let q = orifice_flow(cda, b.p, b.t, a.p, b.gamma, b.r).0;
        [-q, -q * b.h, -q * b.y]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe::Prim;

    #[test]
    fn closed_end_doubles_an_arriving_wave() {
        let gas = Gas::new();
        let s = State::of(
            &gas,
            Prim {
                rho: 1.2,
                u: 5.0,
                p: 1e5,
                y: 0.0,
            },
        );
        let res = Reservoir::new(&gas, 1e5, 300.0, 0.0);
        let f = pipe_to_reservoir(&gas, &s, 1.0, 1e-3, 0.0, &res, &mut 0.0);
        assert_eq!(f[0], 0.0);
        let rise = f[1] / 1e-3 - 1e5;
        assert!((rise - 1.2 * s.c * 5.0).abs() < 1e-6);
    }

    #[test]
    fn closed_form_open_end_matches_the_nozzle_solution() {
        let gas = Gas::new();
        let res = Reservoir::new(&gas, 1e5, 300.0, 0.0);
        for (p, u) in [
            (1.02e5, 0.0),
            (0.97e5, -20.0),
            (1.1e5, 40.0),
            (0.9e5, 10.0),
            (1.0e5, -60.0),
        ] {
            let s = State::of(
                &gas,
                Prim {
                    rho: p / (287.05 * 300.0),
                    u,
                    p,
                    y: 0.0,
                },
            );
            let a = pipe_open_to_reservoir(&gas, &s, 1.0, 1e-3, &res);
            let b = pipe_to_reservoir(&gas, &s, 1.0, 1e-3, 1e-3, &res, &mut 0.0);
            let scale = s.w.rho * s.c * 1e-3;
            assert!(
                (a[0] - b[0]).abs() < 0.03 * scale.max(b[0].abs()),
                "{p} {u}: {a:?} {b:?}"
            );
        }
    }

    #[test]
    fn open_end_balances_at_rest_and_flows_out_under_pressure() {
        let gas = Gas::new();
        let res = Reservoir::new(&gas, 1e5, 300.0, 0.0);
        let rest = State::of(
            &gas,
            Prim {
                rho: 1e5 / (287.05 * 300.0),
                u: 0.0,
                p: 1e5,
                y: 0.0,
            },
        );
        let f = pipe_to_reservoir(&gas, &rest, 1.0, 1e-3, 1e-3, &res, &mut 0.0);
        assert!(f[0].abs() < 1e-5 * rest.w.rho * rest.c * 1e-3, "{f:?}");
        let high = State::of(
            &gas,
            Prim {
                rho: 1.3,
                u: 0.0,
                p: 1.1e5,
                y: 0.0,
            },
        );
        let f = pipe_to_reservoir(&gas, &high, 1.0, 1e-3, 1e-3, &res, &mut 0.0);
        assert!(f[0] > 0.0);
        // Out of the start of a pipe (outward is −x) it flows the same way.
        let g = pipe_to_reservoir(&gas, &high, -1.0, 1e-3, 1e-3, &res, &mut 0.0);
        assert!((f[0] - g[0]).abs() < 1e-12);
        let low = State::of(
            &gas,
            Prim {
                rho: 1.0,
                u: 0.0,
                p: 0.9e5,
                y: 0.0,
            },
        );
        assert!(pipe_to_reservoir(&gas, &low, 1.0, 1e-3, 1e-3, &res, &mut 0.0)[0] < 0.0);
    }
}
