//! One-dimensional unsteady flow in a duct of varying cross-section: the quasi-1D Euler
//! equations with wall friction and heat transfer, and a burned-gas fraction carried with
//! the flow, and the unburned fuel it carries, which burns where the gas is hot and has
//! oxygen for it (see [`crate::chem`]).
//!
//! ```text
//! ∂(ρA)/∂t   + ∂(ρuA)/∂x          = 0
//! ∂(ρuA)/∂t  + ∂((ρu² + p)A)/∂x   = p dA/dx − τ_w πD
//! ∂(EA)/∂t   + ∂(u(E + p)A)/∂x    = q̇_w πD
//! ∂(ρyA)/∂t  + ∂(ρuyA)/∂x         = (1 + AFR)·ω̇
//! ∂(ρfA)/∂t  + ∂(ρufA)/∂x         = −ω̇
//! ```
//!
//! with ω̇ the rate the fuel burns per length, whose heat goes into E.
//!
//! Finite volumes, second order in space and time: MUSCL–Hancock (limited linear
//! reconstruction of the primitive variables, half-step predictor) with the HLLC
//! approximate Riemann solver (Toro, *Riemann Solvers and Numerical Methods for Fluid
//! Dynamics*, 3rd ed., §14.4 and §10.4). Conservative, so the mass and energy that an
//! engine pumps through its manifolds balance exactly; shock capturing, for exhaust
//! blowdown; and of low dispersion, for the pressure waves that tune the manifolds and
//! make the sound. The gas is the thermally perfect mixture of [`crate::gas`].
//!
//! Wall shear uses the Fanning friction factor (laminar 16/Re, Haaland's turbulent fit)
//! and the wall heat flux the Reynolds–Colburn analogy.

use crate::chem::Chemistry;
use crate::gas::Gas;

/// Primitive state of a cell or face.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Prim {
    pub rho: f64,
    pub u: f64,
    pub p: f64,
    /// Burned-gas mass fraction.
    pub y: f64,
    /// Unburned fuel's mass fraction (a part of the fresh charge, 1 − y).
    pub f: f64,
}

/// Primitive state with its derived thermodynamic quantities.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub w: Prim,
    pub t: f64,
    pub c: f64,
    pub gamma: f64,
    /// Sensible internal energy, J/kg.
    pub e: f64,
}

impl State {
    pub fn of(gas: &Gas, w: Prim) -> Self {
        let r = gas.r(w.y);
        let t = w.p / (w.rho * r);
        let (cp, h) = gas.cp_h(t, w.y);
        let gamma = cp / (cp - r);
        Self {
            w,
            t,
            c: (gamma * r * t).sqrt(),
            gamma,
            e: h - r * t,
        }
    }

    /// Total energy per volume.
    #[inline]
    pub fn total_energy(&self) -> f64 {
        self.w.rho * (self.e + 0.5 * self.w.u * self.w.u)
    }

    /// Stagnation enthalpy per mass.
    #[inline]
    pub fn total_enthalpy(&self) -> f64 {
        self.e + self.w.p / self.w.rho + 0.5 * self.w.u * self.w.u
    }
}

/// Fluxes per unit area: mass, momentum, energy, burned mass, unburned fuel.
pub type Flux = [f64; 5];

/// HLLC flux between left and right states (velocities along the common axis).
pub fn hllc(l: &State, r: &State) -> Flux {
    let (rl, ul, pl) = (l.w.rho, l.w.u, l.w.p);
    let (rr, ur, pr) = (r.w.rho, r.w.u, r.w.p);
    let (el, er) = (l.total_energy(), r.total_energy());
    // Einfeldt/Davis wave speed estimates.
    let sl = (ul - l.c).min(ur - r.c);
    let sr = (ul + l.c).max(ur + r.c);
    let fl = [
        rl * ul,
        rl * ul * ul + pl,
        ul * (el + pl),
        rl * ul * l.w.y,
        rl * ul * l.w.f,
    ];
    let fr = [
        rr * ur,
        rr * ur * ur + pr,
        ur * (er + pr),
        rr * ur * r.w.y,
        rr * ur * r.w.f,
    ];
    if sl >= 0.0 {
        return fl;
    }
    if sr <= 0.0 {
        return fr;
    }
    let s_star =
        (pr - pl + rl * ul * (sl - ul) - rr * ur * (sr - ur)) / (rl * (sl - ul) - rr * (sr - ur));
    let star = |rho: f64, u: f64, p: f64, e: f64, w: &Prim, s: f64| {
        let k = rho * (s - u) / (s - s_star);
        [
            k,
            k * s_star,
            k * (e / rho + (s_star - u) * (s_star + p / (rho * (s - u)))),
            k * w.y,
            k * w.f,
        ]
    };
    if s_star >= 0.0 {
        let q = [rl, rl * ul, el, rl * l.w.y, rl * l.w.f];
        let qs = star(rl, ul, pl, el, &l.w, sl);
        std::array::from_fn(|k| fl[k] + sl * (qs[k] - q[k]))
    } else {
        let q = [rr, rr * ur, er, rr * r.w.y, rr * r.w.f];
        let qs = star(rr, ur, pr, er, &r.w, sr);
        std::array::from_fn(|k| fr[k] + sr * (qs[k] - q[k]))
    }
}

/// Dynamic viscosity of the gas (Sutherland), Pa·s.
#[inline]
pub fn viscosity(t: f64) -> f64 {
    1.458e-6 * t * t.sqrt() / (t + 110.4)
}

/// Fanning friction factor of a fully rough pipe of relative roughness `rel` (the high
/// Reynolds number limit of Colebrook's equation, in Haaland's form).
pub fn fanning_rough(rel: f64) -> f64 {
    let x = -1.8 * (rel.max(1e-7) / 3.7).powf(1.11).log10();
    0.25 / (x * x)
}

/// Fanning friction factor at Reynolds number `re`: laminar 16/Re, Blasius' smooth-pipe
/// law 0.0791·Re^−¼ when turbulent, and no less than the pipe's fully rough value `rough`.
#[inline]
pub fn fanning(re: f64, rough: f64) -> f64 {
    if re < 2300.0 {
        return 16.0 / re.max(1.0);
    }
    let blasius = 0.0791 / re.sqrt().sqrt();
    let turb = blasius.max(rough);
    if re < 4000.0 {
        let lam = 16.0 / re;
        lam + (re - 2300.0) / 1700.0 * (turb - lam)
    } else {
        turb
    }
}

/// Which end of a pipe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// x = 0, face 0.
    Start,
    /// x = length, face n.
    End,
}

impl End {
    /// +1 where the outward direction is +x.
    #[inline]
    pub fn sign(self) -> f64 {
        match self {
            End::Start => -1.0,
            End::End => 1.0,
        }
    }
}

/// A pipe discretised into cells.
#[derive(Clone, Debug)]
pub struct Pipe {
    pub name: String,
    pub length: f64,
    pub dx: f64,
    /// Cross-section at cell centres and faces, m².
    pub area: Vec<f64>,
    pub face_area: Vec<f64>,
    /// Hydraulic diameter at cell centres, m.
    pub diameter: Vec<f64>,
    /// Wall temperature, K.
    pub wall_temperature: f64,
    /// Relative roughness of the wall.
    pub roughness: f64,
    /// Its fully rough friction factor.
    rough_friction: f64,
    /// Multiplier on the wall friction (catalyst bricks, filters, bends).
    pub friction_scale: f64,
    /// Multiplier on the wall heat transfer.
    pub heat_scale: f64,
    /// Conserved quantities per unit length: ρA, ρuA, EA, ρyA, ρfA.
    pub q: Vec<[f64; 5]>,
    /// Cell states, kept consistent with `q`.
    pub s: Vec<State>,
    /// Face fluxes (already multiplied by the face area), `n + 1` of them. The network
    /// writes the two end faces.
    pub flux: Vec<Flux>,
    /// Predicted states at the pipe's two end faces, for the boundaries.
    pub end_state: [State; 2],
    /// Heat released by fuel burning inside since it was built, J.
    pub heat_released: f64,
    /// Temperature hint for each cell's energy inversion.
    scratch_l: Vec<State>,
    scratch_r: Vec<State>,
}

/// Geometry of a pipe to build.
#[derive(Clone, Debug)]
pub struct PipeGeometry {
    pub name: String,
    pub length: f64,
    /// Diameter along the pipe: (fraction of the length, m), ascending; linear between.
    pub diameter: Vec<(f64, f64)>,
    pub cell_length: f64,
    pub wall_temperature: f64,
    pub roughness: f64,
    pub friction_scale: f64,
    pub heat_scale: f64,
}

impl Pipe {
    /// A pipe full of gas at rest.
    pub fn new(g: &PipeGeometry, gas: &Gas, fill: Prim) -> Self {
        let n = ((g.length / g.cell_length).round() as usize).max(1);
        let dx = g.length / n as f64;
        let d_at = |x: f64| crate::table::lookup(&g.diameter, x / g.length);
        let area_of = |d: f64| std::f64::consts::PI * 0.25 * d * d;
        let diameter: Vec<f64> = (0..n).map(|i| d_at((i as f64 + 0.5) * dx)).collect();
        let area = diameter.iter().map(|&d| area_of(d)).collect::<Vec<_>>();
        let face_area = (0..=n).map(|i| area_of(d_at(i as f64 * dx))).collect();
        let st = State::of(gas, fill);
        let mut pipe = Self {
            name: g.name.clone(),
            length: g.length,
            dx,
            area,
            face_area,
            diameter,
            wall_temperature: g.wall_temperature,
            roughness: g.roughness,
            rough_friction: fanning_rough(g.roughness),
            friction_scale: g.friction_scale,
            heat_scale: g.heat_scale,
            q: vec![[0.0; 5]; n],
            s: vec![st; n],
            flux: vec![[0.0; 5]; n + 1],
            end_state: [st; 2],
            heat_released: 0.0,
            scratch_l: vec![st; n],
            scratch_r: vec![st; n],
        };
        for i in 0..n {
            pipe.q[i] = conserved(&st, pipe.area[i]);
        }
        pipe
    }

    pub fn cells(&self) -> usize {
        self.q.len()
    }

    /// Volume, m³.
    pub fn volume(&self) -> f64 {
        self.area.iter().sum::<f64>() * self.dx
    }

    /// Mass of gas inside, kg.
    pub fn mass(&self) -> f64 {
        self.q.iter().map(|q| q[0]).sum::<f64>() * self.dx
    }

    /// Total energy inside, J.
    pub fn energy(&self) -> f64 {
        self.q.iter().map(|q| q[2]).sum::<f64>() * self.dx
    }

    /// Area of an end, m².
    pub fn end_area(&self, end: End) -> f64 {
        match end {
            End::Start => self.face_area[0],
            End::End => self.face_area[self.cells()],
        }
    }

    /// The predicted state at an end face.
    pub fn end(&self, end: End) -> &State {
        match end {
            End::Start => &self.end_state[0],
            End::End => &self.end_state[1],
        }
    }

    /// Sets the flux through an end face from the outward flux `out` (mass, outward
    /// momentum incl. pressure, energy, burned mass, fuel) — the form the boundaries give.
    pub fn set_end_flux(&mut self, end: End, out: Flux) {
        let s = end.sign();
        let f = [s * out[0], out[1], s * out[2], s * out[3], s * out[4]];
        match end {
            End::Start => self.flux[0] = f,
            End::End => {
                let n = self.cells();
                self.flux[n] = f;
            }
        }
    }

    /// Largest stable time step, s.
    pub fn max_dt(&self, cfl: f64) -> f64 {
        let mut m: f64 = 0.0;
        for s in &self.s {
            m = m.max(s.w.u.abs() + s.c);
        }
        cfl * self.dx / m
    }

    /// Reconstructs, predicts half a step, and computes the interior face fluxes and the
    /// end states.
    pub fn predict(&mut self, gas: &Gas, dt: f64) {
        let n = self.cells();
        let h = 0.5 * dt / self.dx;
        for i in 0..n {
            let w = self.s[i].w;
            if i > 0 && i < n - 1 {
                let (a, b) = (self.s[i - 1].w, self.s[i + 1].w);
                let d = Prim {
                    rho: limit(w.rho - a.rho, b.rho - w.rho),
                    u: limit(w.u - a.u, b.u - w.u),
                    p: limit(w.p - a.p, b.p - w.p),
                    y: limit(w.y - a.y, b.y - w.y),
                    f: limit(w.f - a.f, b.f - w.f),
                };
                // Half-step evolution of the primitive equations.
                let g = self.s[i].gamma;
                let dt_w = Prim {
                    rho: -h * (w.u * d.rho + w.rho * d.u),
                    u: -h * (w.u * d.u + d.p / w.rho),
                    p: -h * (g * w.p * d.u + w.u * d.p),
                    y: -h * w.u * d.y,
                    f: -h * w.u * d.f,
                };
                let mut wl = Prim {
                    rho: w.rho - 0.5 * d.rho + dt_w.rho,
                    u: w.u - 0.5 * d.u + dt_w.u,
                    p: w.p - 0.5 * d.p + dt_w.p,
                    y: w.y - 0.5 * d.y + dt_w.y,
                    f: w.f - 0.5 * d.f + dt_w.f,
                };
                let mut wr = Prim {
                    rho: w.rho + 0.5 * d.rho + dt_w.rho,
                    u: w.u + 0.5 * d.u + dt_w.u,
                    p: w.p + 0.5 * d.p + dt_w.p,
                    y: w.y + 0.5 * d.y + dt_w.y,
                    f: w.f + 0.5 * d.f + dt_w.f,
                };
                if wl.rho <= 0.0 || wr.rho <= 0.0 || wl.p <= 0.0 || wr.p <= 0.0 {
                    wl = w;
                    wr = w;
                }
                wl.y = wl.y.clamp(0.0, 1.0);
                wr.y = wr.y.clamp(0.0, 1.0);
                wl.f = wl.f.clamp(0.0, 1.0 - wl.y);
                wr.f = wr.f.clamp(0.0, 1.0 - wr.y);
                self.scratch_l[i] = State::of(gas, wl);
                self.scratch_r[i] = State::of(gas, wr);
            } else {
                self.scratch_l[i] = self.s[i];
                self.scratch_r[i] = self.s[i];
            }
        }
        for i in 1..n {
            let f = hllc(&self.scratch_r[i - 1], &self.scratch_l[i]);
            let a = self.face_area[i];
            self.flux[i] = f.map(|v| v * a);
        }
        self.end_state = [self.scratch_l[0], self.scratch_r[n - 1]];
    }

    /// Advances the cells by `dt` with the fluxes (the ends set by the network) and the
    /// source terms, and updates the cell states.
    pub fn update(&mut self, gas: &Gas, dt: f64) {
        self.advance(gas, None, dt);
    }

    /// As `update`, burning the unburned fuel of the cells hot enough, and with the
    /// oxygen, for it; returns the heat released, J.
    pub fn update_reacting(&mut self, gas: &Gas, chem: &Chemistry, dt: f64) -> f64 {
        let heat = self.advance(gas, Some(chem), dt);
        self.heat_released += heat;
        heat
    }

    fn advance(&mut self, gas: &Gas, chem: Option<&Chemistry>, dt: f64) -> f64 {
        let mut heat = 0.0;
        let n = self.cells();
        let k = dt / self.dx;
        for i in 0..n {
            let (fl, fr) = (self.flux[i], self.flux[i + 1]);
            let st = self.s[i];
            let q = &mut self.q[i];
            for j in 0..5 {
                q[j] += k * (fl[j] - fr[j]);
            }
            // Pressure on the walls of a changing section.
            q[1] += k * st.w.p * (self.face_area[i + 1] - self.face_area[i]);
            // Wall friction and heat transfer.
            let d = self.diameter[i];
            let (rho, u) = (st.w.rho, st.w.u);
            let re = rho * u.abs() * d / viscosity(st.t);
            if self.friction_scale == 0.0 && self.heat_scale == 0.0 {
                if q[0] <= 0.0 {
                    q[0] = 1e-9 * self.area[i];
                }
                q[3] = q[3].clamp(0.0, q[0]);
                q[4] = q[4].clamp(0.0, q[0] - q[3]);
                if let Some(c) = chem {
                    heat += burn(q, &st, c, dt) * self.dx;
                }
                self.s[i] = primitive_near(gas, q, self.area[i], st.t);
                continue;
            }
            let f = fanning(re, self.rough_friction);
            let perimeter = std::f64::consts::PI * d;
            let tau = self.friction_scale * f * 0.5 * rho * u * u.abs();
            let dmom = (dt * tau * perimeter).min(q[1].abs());
            q[1] -= dmom * u.signum();
            // Colburn: St Pr^(2/3) = f/2; Pr ≈ 0.72. Natural convection floor.
            let cp = st.gamma / (st.gamma - 1.0) * gas.r(st.w.y);
            let h_conv = (0.5 * f * rho * u.abs() * cp * 1.25).max(10.0) * self.heat_scale;
            q[2] += dt * h_conv * perimeter * (self.wall_temperature - st.t);
            if q[0] <= 0.0 {
                q[0] = 1e-9 * self.area[i];
            }
            q[3] = q[3].clamp(0.0, q[0]);
            q[4] = q[4].clamp(0.0, q[0] - q[3]);
            if let Some(c) = chem {
                heat += burn(q, &st, c, dt) * self.dx;
            }
            self.s[i] = primitive_near(gas, q, self.area[i], st.t);
        }
        heat
    }

    /// Unburned fuel inside, kg.
    pub fn fuel(&self) -> f64 {
        self.q.iter().map(|q| q[4]).sum::<f64>() * self.dx
    }
}

/// Burns a cell's unburned fuel over `dt` as the chemistry lets it at the state `st` it
/// had; returns the heat released per length, J/m.
#[inline]
fn burn(q: &mut [f64; 5], st: &State, chem: &Chemistry, dt: f64) -> f64 {
    // Traces (a flame's last 0.02 %) would warm the gas by a kelvin or less.
    if q[4] <= 1e-4 * q[0] || st.t < crate::chem::T_MIN {
        return 0.0;
    }
    let dm = chem.burn(st.w.rho, st.t, st.w.y, st.w.f, dt) * q[0];
    if dm <= 0.0 {
        return 0.0;
    }
    q[4] -= dm;
    q[3] = (q[3] + dm * (1.0 + chem.afr)).min(q[0]);
    q[2] += dm * chem.heat;
    dm * chem.heat
}

/// Minmod-limited slope from backward and forward differences (van Leer's MC limiter).
#[inline]
fn limit(a: f64, b: f64) -> f64 {
    if a * b <= 0.0 {
        0.0
    } else {
        let s = a.signum();
        s * (2.0 * a.abs()).min(2.0 * b.abs()).min(0.5 * (a + b).abs())
    }
}

/// Conserved quantities per length of a state in a section of area `a`.
pub fn conserved(s: &State, a: f64) -> [f64; 5] {
    let rho = s.w.rho;
    [
        rho * a,
        rho * s.w.u * a,
        s.total_energy() * a,
        rho * s.w.y * a,
        rho * s.w.f * a,
    ]
}

/// State of conserved quantities per length `q` in a section of area `a`.
pub fn primitive(gas: &Gas, q: &[f64; 5], a: f64) -> State {
    primitive_near(gas, q, a, 300.0)
}

/// As `primitive`, with a guess of the temperature.
#[inline]
pub fn primitive_near(gas: &Gas, q: &[f64; 5], a: f64, t_guess: f64) -> State {
    let rho = q[0] / a;
    let u = q[1] / q[0];
    let y = (q[3] / q[0]).clamp(0.0, 1.0);
    let f = (q[4] / q[0]).clamp(0.0, 1.0 - y);
    let e = (q[2] / q[0] - 0.5 * u * u).max(gas.floor_energy(y));
    let (t, cp) = gas.temperature_cp_near(e, y, t_guess);
    let r = gas.r(y);
    let gamma = cp / (cp - r);
    State {
        w: Prim {
            rho,
            u,
            p: rho * r * t,
            y,
            f,
        },
        t,
        c: (gamma * r * t).sqrt(),
        gamma,
        e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tube(n_len: f64, cell: f64) -> (Gas, Pipe) {
        let gas = Gas::new();
        let g = PipeGeometry {
            name: "tube".into(),
            length: n_len,
            diameter: vec![(0.0, 0.05)],
            cell_length: cell,
            wall_temperature: 300.0,
            roughness: 0.0,
            friction_scale: 0.0,
            heat_scale: 0.0,
        };
        let pipe = Pipe::new(
            &g,
            &gas,
            Prim {
                rho: 1.2,
                u: 0.0,
                p: 1e5,
                y: 0.0,
                f: 0.0,
            },
        );
        (gas, pipe)
    }

    fn closed_ends(pipe: &mut Pipe) {
        let n = pipe.cells();
        for end in [End::Start, End::End] {
            let s = *pipe.end(end);
            let v = s.w.u * end.sign();
            let p = s.w.p + s.w.rho * s.c * v;
            pipe.set_end_flux(end, [0.0, p * pipe.end_area(end), 0.0, 0.0, 0.0]);
        }
        let _ = n;
    }

    /// Sod's shock tube (in air at room temperature scale): the density plateau between
    /// the contact and the shock matches the exact solution for γ = 1.4.
    #[test]
    fn sod_shock_tube() {
        let (gas, mut pipe) = tube(1.0, 0.0025);
        let n = pipe.cells();
        for i in 0..n {
            let w = if i < n / 2 {
                Prim {
                    rho: 1.0,
                    u: 0.0,
                    p: 1e5,
                    y: 0.0,
                    f: 0.0,
                }
            } else {
                Prim {
                    rho: 0.125,
                    u: 0.0,
                    p: 1e4,
                    y: 0.0,
                    f: 0.0,
                }
            };
            let st = State::of(&gas, w);
            pipe.q[i] = conserved(&st, pipe.area[i]);
            pipe.s[i] = st;
        }
        let mut t = 0.0;
        let t_end = 0.0006;
        while t < t_end {
            let dt = pipe.max_dt(0.8).min(t_end - t);
            pipe.predict(&gas, dt);
            closed_ends(&mut pipe);
            pipe.update(&gas, dt);
            t += dt;
        }
        // Exact solution for γ=1.4, (1, 0, 1e5) | (0.125, 0, 1e4): p* = 30313 Pa,
        // u* = 293 m/s (scaled from the classic (1,0,1)|(0.125,0,0.1) case: velocities
        // × sqrt(1e5) = 316.2).
        let x_contact = 0.5 + 0.92745 * 316.23 * t_end;
        let x_shock = 0.5 + 1.75216 * 316.23 * t_end;
        let probe = ((x_contact + x_shock) * 0.5 / pipe.dx) as usize;
        let s = pipe.s[probe];
        assert!((s.w.p - 30313.0).abs() / 30313.0 < 0.03, "p* {}", s.w.p);
        assert!((s.w.u - 0.92745 * 316.23).abs() < 10.0, "u* {}", s.w.u);
        // The gas is not quite calorically perfect (γ(300 K) ≈ 1.4, dropping when hot).
        assert!(
            (s.w.rho - 0.26557).abs() / 0.26557 < 0.04,
            "rho* {}",
            s.w.rho
        );
    }

    #[test]
    fn closed_tube_conserves_mass_and_energy() {
        let (gas, mut pipe) = tube(0.5, 0.01);
        // A pressure pulse.
        for i in 10..15 {
            let st = State::of(
                &gas,
                Prim {
                    rho: 1.5,
                    u: 0.0,
                    p: 1.4e5,
                    y: 0.5,
                    f: 0.0,
                },
            );
            pipe.q[i] = conserved(&st, pipe.area[i]);
            pipe.s[i] = st;
        }
        let (m0, e0) = (pipe.mass(), pipe.energy());
        for _ in 0..2000 {
            let dt = pipe.max_dt(0.8);
            pipe.predict(&gas, dt);
            closed_ends(&mut pipe);
            pipe.update(&gas, dt);
        }
        assert!((pipe.mass() - m0).abs() / m0 < 1e-12);
        assert!((pipe.energy() - e0).abs() / e0 < 1e-12);
    }
}
