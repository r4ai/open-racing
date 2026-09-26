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

/// Pressure at a pipe's end face where the gas arriving from inside (state `s`, outward
/// velocity `v`) is slowed by `dv` = v − v_face: the exact relation along the outgoing
/// wave (Toro, *Riemann Solvers*, §4.2), a shock when the gas is stopped (dv > 0) and an
/// isentropic rarefaction when it is drawn out (dv < 0). The acoustic p + ρc·dv is its
/// small-amplitude limit, which a valve shutting on gas leaving at hundreds of m/s would
/// take below zero; the rarefaction instead tends to a vacuum, floored at `p_min`.
#[inline]
pub fn face_pressure(s: &State, dv: f64, p_min: f64) -> f64 {
    let (p, rho, c, g) = (s.w.p, s.w.rho, s.c, s.gamma);
    if dv <= 0.0 {
        let base = 1.0 + 0.5 * (g - 1.0) * dv / c;
        if base <= 0.0 {
            return p_min;
        }
        (p * base.powf(2.0 * g / (g - 1.0))).max(p_min)
    } else {
        let a = 2.0 / ((g + 1.0) * rho);
        let b = (g - 1.0) / (g + 1.0) * p;
        let v2 = dv * dv;
        p + (v2 + (v2 * v2 + 4.0 * a * v2 * (p + b)).sqrt()) / (2.0 * a)
    }
}

/// Density at the end face behind the wave of `face_pressure`: isentropic through a
/// rarefaction, Rankine–Hugoniot through a shock.
#[inline]
fn face_density(s: &State, pf: f64) -> f64 {
    let (p, rho, g) = (s.w.p, s.w.rho, s.gamma);
    let r = pf / p;
    if r <= 1.0 {
        rho * r.powf(1.0 / g)
    } else {
        let k = (g - 1.0) / (g + 1.0);
        rho * (r + k) / (k * r + 1.0)
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
    let cda = cda.min(area);
    let p_min = 0.02 * s.w.p;
    if cda <= 0.0 {
        return [0.0, face_pressure(s, v, p_min) * area, 0.0, 0.0];
    }
    if v + s.c < 0.0 {
        return supersonic_inflow(s, area, cda, res);
    }
    let (rho, gamma, r) = (s.w.rho, s.gamma, gas.r(s.w.y));
    let cp = gamma * r / (gamma - 1.0);
    // Signed outward mass flow through the restriction for a face velocity.
    let flow = |vf: f64| -> (f64, f64, f64) {
        let pf = face_pressure(s, v - vf, p_min);
        let rf = face_density(s, pf);
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
    // g rises with vf (bar choking). Its root lies between a strong compression and the face emptied
    // to the floor pressure; a safeguarded secant from last step's root finds it in a few
    // evaluations.
    let vacuum = 2.0 * s.c / (gamma - 1.0)
        * (1.0 - (p_min / s.w.p).powf((gamma - 1.0) / (2.0 * gamma)));
    // Gas rushing at the restriction may have to be stopped by a strong shock, and gas
    // can come in from the reservoir at up to its speed of sound: the bracket spans both.
    let c_res = (res.gamma * res.r * res.t).sqrt();
    let (mut a, mut b) = ((v - 2.0 * s.c).min(-2.0 * (s.c + c_res)), v + vacuum);
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

/// Flux into a pipe whose gas at the end moves inward faster than sound: no wave from
/// inside reaches the face, so the reservoir alone sets it. The restriction's jet
/// discharges at the pipe's pressure, its stagnation enthalpy kept, but no lower than the
/// supersonic exit pressure of an isentropic nozzle from the restriction's throat to the
/// pipe's section: gas cannot be drawn out faster than the area ratio lets it expand.
fn supersonic_inflow(s: &State, area: f64, cda: f64, res: &Reservoir) -> Flux {
    let g = res.gamma;
    let mach = supersonic_mach(area / cda, g);
    let exit = res.p * (1.0 + 0.5 * (g - 1.0) * mach * mach).powf(-g / (g - 1.0));
    let pf = s.w.p.max(exit);
    let q = orifice_flow(cda, res.p, res.t, pf, res.gamma, res.r).0;
    if q <= 0.0 {
        return [0.0, pf * area, 0.0, 0.0];
    }
    // cp·(T0 − T) = ½·u², u = q·R·T/(pf·A).
    let cp = res.gamma * res.r / (res.gamma - 1.0);
    let k = (q * res.r / (pf * area)).powi(2);
    let t = (-cp + (cp * cp + 2.0 * k * cp * res.t).sqrt()) / k;
    let vf = -q * res.r * t / (pf * area);
    [-q, pf * area - q * vf, -q * res.h, -q * res.y]
}

/// Mach number (≥ 1) at which an isentropic nozzle's section is `ratio` times its throat's.
fn supersonic_mach(ratio: f64, g: f64) -> f64 {
    let ratio = ratio.max(1.0);
    let e = (g + 1.0) / (2.0 * (g - 1.0));
    let f = |m: f64| (2.0 / (g + 1.0) * (1.0 + 0.5 * (g - 1.0) * m * m)).powf(e) / m;
    let (mut lo, mut hi) = (1.0, 1.0);
    while f(hi) < ratio && hi < 50.0 {
        hi *= 2.0;
    }
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if f(mid) < ratio {
            lo = mid
        } else {
            hi = mid
        }
    }
    0.5 * (lo + hi)
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
    let g = s.gamma;
    // The velocity change that brings the arriving gas to the reservoir's pressure.
    let dv = if res.p <= s.w.p {
        2.0 * s.c / (g - 1.0) * ((res.p / s.w.p).powf((g - 1.0) / (2.0 * g)) - 1.0)
    } else {
        let a = 2.0 / ((g + 1.0) * s.w.rho);
        let b = (g - 1.0) / (g + 1.0) * s.w.p;
        (res.p - s.w.p) * (a / (res.p + b)).sqrt()
    };
    let vf_out = v - dv;
    if vf_out >= 0.0 {
        let mut pf = res.p;
        let mut rf = face_density(s, pf);
        let cf = (g * pf / rf).sqrt();
        let mut vf = vf_out;
        if vf > cf {
            // Choked: the face stays sonic and its pressure rises above the reservoir's.
            // Along the outgoing wave v + 2c/(γ−1) is kept, so the sonic face has
            // c* = (γ−1)/(γ+1)·(v + 2c/(γ−1)).
            let cs = (g - 1.0) / (g + 1.0) * (v + 2.0 * s.c / (g - 1.0));
            vf = cs;
            pf = s.w.p * (cs / s.c).powf(2.0 * g / (g - 1.0));
            rf = face_density(s, pf);
        }
        let q = rf * vf * area;
        let tf = pf / (rf * gas.r(s.w.y));
        let h0 = gas.enthalpy(tf, s.w.y) + 0.5 * vf * vf;
        [q, pf * area + q * vf, q * h0, q * s.w.y]
    } else {
        let rho_r = res.density();
        let k = res.p - s.w.p - z * v;
        let b = z * (2.0 / rho_r).sqrt();
        let root = 0.5 * (-b + (b * b + 4.0 * k).max(0.0).sqrt());
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
        // A weak shock: the acoustic ρc·u, a little more.
        let rise = f[1] / 1e-3 - 1e5;
        let acoustic = 1.2 * s.c * 5.0;
        assert!(rise > acoustic && rise < 1.01 * acoustic, "{rise} {acoustic}");
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
