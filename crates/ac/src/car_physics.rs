//! A car's physics, from what a car folder offers, on top of a base car.
//!
//! Three sources refine the base car in turn:
//! 1. the 3D model: wheelbase, tracks and tyre sizes;
//! 2. without physics files, the UI description (`ui/ui_car.json`): mass, the engine's
//!    torque curve and the top speed, which sets the final drive;
//! 3. the physics files (`car.ini`, `engine.ini`, …) of an unpacked data folder, which
//!    replace every value they hold.
//!
//! Whatever no source gives stays as in the base car, and the report says which groups
//! of values those are.

use std::f64::consts::PI;
use std::path::Path;

use open_racing_sim::params::{DAMAGE_ZONES, DifferentialParams};
use open_racing_sim::suspension::{Link, Linkage, Wishbone};
use open_racing_sim::tire::TireParams;
use open_racing_sim::{
    AeroElement, AntiStall, CarParams, Drive, ElectronicsParams, GearboxKind, TurboParams,
};

use crate::ini::{self, Section};
use crate::json::Value;
use crate::{Error, lut};

/// Synchroniser torque given to H-pattern gearboxes, which the game's data does not
/// describe, N·m.
const SYNCHRO_TORQUE: f64 = 25.0;
/// Hand travel between gates of an H-pattern lever with sequential requests, s.
const LEVER_TIME: f64 = 0.25;
const PSI: f64 = 0.0689476;
/// How far behind the lower arm a toe link is placed on an axle whose steering points
/// the files leave out, m.
const TOE_LINK_OFFSET: f64 = 0.12;
/// Density of fuel, kg/l.
const FUEL_DENSITY: f64 = 0.745;
/// All-wheel drive where the files do not say more: the torque split, a moderately
/// locking centre differential and a light front one.
const AWD_FRONT_SHARE: f64 = 0.4;
const AWD_CENTRE_DIFFERENTIAL: DifferentialParams = DifferentialParams {
    preload: 50.0,
    power_ramp: 0.2,
    coast_ramp: 0.1,
};
const AWD_FRONT_DIFFERENTIAL: DifferentialParams = DifferentialParams {
    preload: 20.0,
    power_ramp: 0.1,
    coast_ramp: 0.05,
};
/// Margin of the limiter-limited top speed in top gear over the car's stated top speed.
const TOP_SPEED_MARGIN: f64 = 1.03;
/// The game's physics rate, Hz: its turbo lag is a filter applied at each of its steps.
const GAME_RATE: f64 = 333.0;
/// Pressure offset at which a tyre's pressure-grip window is fitted to the game's linear
/// grip loss, bar.
const PRESSURE_FIT: f64 = 0.25;

/// Wheel centres (body axes, the model's origin), tyre radii and widths from the 3D model,
/// in the simulation's wheel order.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub centres: [glam::DVec3; 4],
    pub radius: [f64; 4],
    pub width: [f64; 4],
}

impl Geometry {
    pub fn front_x(&self) -> f64 {
        0.5 * (self.centres[0].x + self.centres[1].x)
    }

    pub fn wheelbase(&self) -> f64 {
        self.front_x() - 0.5 * (self.centres[2].x + self.centres[3].x)
    }

    pub fn track(&self, front: bool) -> f64 {
        let [l, r] = if front { [0, 1] } else { [2, 3] };
        self.centres[l].y - self.centres[r].y
    }

    /// Height of the ground below the model's origin: the tyres' contact points.
    pub fn ground(&self) -> f64 {
        (0..4)
            .map(|i| self.centres[i].z - self.radius[i])
            .sum::<f64>()
            / 4.0
    }

    pub fn check(&self) -> Result<(), Error> {
        let ok = (1.5..6.0).contains(&self.wheelbase())
            && [true, false]
                .iter()
                .all(|&f| (0.8..3.0).contains(&self.track(f)))
            && self.radius.iter().all(|r| (0.15..1.0).contains(r))
            && self.width.iter().all(|w| (0.05..1.0).contains(w));
        if ok {
            Ok(())
        } else {
            Err(Error::Format(format!(
                "implausible wheel layout in the model: wheelbase {:.2} m, tracks {:.2} / {:.2} m, \
                 tyre radii {:.2?} m",
                self.wheelbase(),
                self.track(true),
                self.track(false),
                self.radius
            )))
        }
    }
}

/// The car being assembled, with a note per group of values saying where it came from.
#[derive(Clone, Debug)]
pub struct Physics {
    pub params: CarParams,
    pub front: TireParams,
    pub rear: TireParams,
    pub notes: Vec<String>,
}

fn avg(a: f64, b: f64) -> f64 {
    0.5 * (a + b)
}

impl Physics {
    pub fn new(params: CarParams, front: TireParams, rear: TireParams) -> Self {
        Self {
            params,
            front,
            rear,
            notes: Vec::new(),
        }
    }

    fn note(&mut self, s: impl Into<String>) {
        self.notes.push(s.into());
    }

    /// Takes the wheel layout and tyre sizes from the model, and scales the inertia with
    /// the size of the car.
    pub fn apply_geometry(&mut self, g: &Geometry) {
        let p = &mut self.params;
        let (wheelbase, track) = (g.wheelbase(), avg(g.track(true), g.track(false)));
        let length = (wheelbase / p.wheelbase).powi(2);
        let width = (track / avg(p.track_front, p.track_rear)).powi(2);
        p.inertia[0] *= width;
        p.inertia[1] *= length;
        p.inertia[2] *= avg(length, width);
        p.wheelbase = wheelbase;
        p.track_front = g.track(true);
        p.track_rear = g.track(false);
        self.front.radius = avg(g.radius[0], g.radius[1]);
        self.front.width = avg(g.width[0], g.width[1]);
        self.rear.radius = avg(g.radius[2], g.radius[3]);
        self.rear.width = avg(g.width[2], g.width[3]);
        self.note("wheelbase, tracks and tyre sizes: from the 3D model");
    }

    fn set_mass(&mut self, mass: f64) {
        let k = mass / self.params.mass;
        self.params.mass = mass;
        self.params.inertia = self.params.inertia.map(|i| i * k);
    }

    /// Uses the UI description's mass, torque curve and top speed.
    pub fn apply_ui(&mut self, ui: &Value) {
        let spec = |key: &str| ui.get("specs")?.get(key)?.as_str();
        let mut used = Vec::new();
        if let Some(kg) = spec("weight").and_then(|w| quantity(w, &[("lb", 0.4536)]))
            && (300.0..5000.0).contains(&kg)
        {
            self.set_mass(kg);
            used.push("mass");
        }
        let curve: Vec<(f64, f64)> = ui
            .get("torqueCurve")
            .map(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|p| {
                Some((
                    p.as_array().first()?.as_f64()?,
                    p.as_array().get(1)?.as_f64()?,
                ))
            })
            .filter(|&(rpm, nm)| rpm > 0.0 && nm > 0.0)
            .collect();
        if curve.len() >= 3 {
            self.set_torque_curve(curve);
            used.push("engine torque curve and rev limit");
        }
        // Before the gearing, which follows the driven wheels' size.
        let tags: Vec<String> = ui
            .get("tags")
            .map(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|t| Some(t.as_str()?.to_ascii_lowercase()))
            .collect();
        let tagged = |names: &[&str]| tags.iter().any(|t| names.contains(&t.as_str()));
        if tagged(&["fwd"]) {
            self.params.drive = Drive::Front;
            used.push("front-wheel drive");
        } else if tagged(&["awd", "4wd"]) {
            if !matches!(self.params.drive, Drive::All { .. }) {
                self.params.drive = Drive::All {
                    front_share: AWD_FRONT_SHARE,
                    centre_differential: AWD_CENTRE_DIFFERENTIAL,
                    front_differential: AWD_FRONT_DIFFERENTIAL,
                };
                self.note(format!(
                    "drive: all-wheel from ui/ui_car.json; {:.0} % of the torque to the front and \
                     the differentials assumed",
                    100.0 * AWD_FRONT_SHARE
                ));
            }
        } else if tagged(&["rwd"]) {
            self.params.drive = Drive::Rear;
        }
        if let Some(kmh) = spec("topspeed").and_then(|s| quantity(s, &[("mph", 1.609)]))
            && (60.0..500.0).contains(&kmh)
        {
            self.gear_for_top_speed(kmh / 3.6);
            used.push("final drive (for the top speed)");
        }
        if !used.is_empty() {
            self.note(format!("{}: from ui/ui_car.json", used.join(", ")));
        }
    }

    /// Radius of the driven wheels (the rear ones of an all-wheel-drive car).
    fn driven_radius(&self) -> f64 {
        match self.params.drive {
            Drive::Front => self.front.radius,
            Drive::Rear | Drive::All { .. } => self.rear.radius,
        }
    }

    /// Replaces the torque curve, with the rev limit at its end and the engine's friction
    /// scaled along.
    fn set_torque_curve(&mut self, curve: Vec<(f64, f64)>) {
        let e = &mut self.params.engine;
        let peak = |c: &[(f64, f64)]| c.iter().map(|p| p.1).fold(0.0, f64::max);
        let limiter = curve[curve.len() - 1].0;
        let (rpm, torque) = (
            limiter / e.limiter_rpm,
            peak(&curve) / peak(&e.torque_curve),
        );
        for d in &mut e.drag_curve {
            *d = (d.0 * rpm, d.1 * torque);
        }
        e.limiter_rpm = limiter;
        e.idle_rpm = e.idle_rpm.min(0.4 * limiter);
        self.params.clutch.max_torque = self.params.clutch.max_torque.max(1.5 * peak(&curve));
        e.torque_curve = curve;
    }

    /// Sets the final drive so that the car reaches `speed` (m/s) at the rev limit in top
    /// gear, with a small margin.
    fn gear_for_top_speed(&mut self, speed: f64) {
        let radius = self.driven_radius();
        let p = &mut self.params;
        let omega = p.engine.limiter_rpm * PI / 30.0;
        let top = p.gearbox.ratios[p.gearbox.ratios.len() - 1];
        p.gearbox.final_drive = (omega * radius / (top * speed * TOP_SPEED_MARGIN)).clamp(1.5, 8.0);
    }

    /// Reads the physics files in `dir`.
    pub fn apply_data(&mut self, dir: &Path) -> Result<(), Error> {
        let file = |name: &str| -> Result<Vec<Section>, Error> {
            let path = dir.join(name);
            let src = std::fs::read(&path).map_err(|e| Error::Io(path.clone(), e))?;
            Ok(ini::parse(&String::from_utf8_lossy(&src)))
        };
        let table = |name: &str| -> Vec<(f64, f64)> {
            // Tables in the files are either inline or file names in the same folder.
            if name.trim_start().starts_with('(') {
                return lut::parse(name);
            }
            std::fs::read(dir.join(name))
                .map(|b| lut::parse(&String::from_utf8_lossy(&b)))
                .unwrap_or_default()
        };
        let car = file("car.ini")?;
        let mut used = vec!["car.ini"];
        self.apply_car(&car);
        type Apply = fn(&mut Physics, &[Section], &dyn Fn(&str) -> Vec<(f64, f64)>);
        // Tyres first: the centre of gravity's height follows from their radii.
        for (name, apply) in [
            ("tyres.ini", Self::apply_tyres as Apply),
            ("suspensions.ini", Self::apply_suspensions),
            ("engine.ini", Self::apply_engine),
            ("drivetrain.ini", Self::apply_drivetrain),
            ("brakes.ini", Self::apply_brakes),
            ("aero.ini", Self::apply_aero),
        ] {
            if dir.join(name).is_file() {
                apply(self, &file(name)?, &table);
                used.push(name);
            }
        }
        self.note(format!(
            "physics: from {} in {}",
            used.join(", "),
            dir.display()
        ));
        Ok(())
    }

    fn apply_car(&mut self, s: &[Section]) {
        let get = |sec: &str, key: &str| ini::section(s, sec)?.get_f64(key);
        if let Some(m) = get("BASIC", "TOTALMASS").filter(|m| *m > 0.0) {
            // The game's total mass holds the driver; the car starts with its fuel load.
            let fuel = get("FUEL", "FUEL").unwrap_or(0.0).max(0.0) * FUEL_DENSITY;
            self.params.mass = m + fuel;
        }
        if let Some([w, h, l]) = ini::section(s, "BASIC")
            .and_then(|b| b.get_f64s("INERTIA"))
            .and_then(|v| <[f64; 3]>::try_from(v).ok())
        {
            // The game's inertia is that of a uniform box of these dimensions.
            let m = self.params.mass / 12.0;
            self.params.inertia = [
                m * (w * w + h * h),
                m * (l * l + h * h),
                m * (w * w + l * l),
            ];
        }
        // Ride heights at the front and rear pickup points, which the aero maps read.
        if let (Some(front), Some(rear)) = (
            get("RIDE", "PICKUP_FRONT_HEIGHT"),
            get("RIDE", "PICKUP_REAR_HEIGHT"),
        ) && [front, rear].iter().all(|h| (0.01..0.4).contains(h))
        {
            self.params.aero.ride_height = [front, rear];
        }
        let st = &mut self.params.steering;
        if let Some(lock) = get("CONTROLS", "STEER_LOCK").filter(|l| *l > 0.0) {
            st.lock = lock.to_radians();
        }
        if let Some(ratio) = get("CONTROLS", "STEER_RATIO").filter(|r| *r > 0.0) {
            st.ratio = ratio;
        }
    }

    fn apply_suspensions(&mut self, s: &[Section], _: &dyn Fn(&str) -> Vec<(f64, f64)>) {
        let get = |sec: &str, key: &str| ini::section(s, sec)?.get_f64(key);
        let p = &mut self.params;
        if let Some(wb) = get("BASIC", "WHEELBASE").filter(|w| *w > 0.0) {
            p.wheelbase = wb;
        }
        if let Some(cg) = get("BASIC", "CG_LOCATION").filter(|c| (0.0..=1.0).contains(c)) {
            p.front_weight = cg;
        }
        let mut heights = [None, None];
        let mut unsupported = Vec::new();
        for (k, sec) in ["FRONT", "REAR"].into_iter().enumerate() {
            let front = k == 0;
            let axle = if front { &mut p.front } else { &mut p.rear };
            let get = |key: &str| get(sec, key);
            if let Some(v) = get("SPRING_RATE").filter(|v| *v > 0.0) {
                axle.spring_rate = v;
            }
            if let Some(v) = get("DAMP_BUMP").filter(|v| *v > 0.0) {
                axle.bump_damping = v;
            }
            if let Some(v) = get("DAMP_REBOUND").filter(|v| *v > 0.0) {
                axle.rebound_damping = v;
            }
            if let Some(v) = get("BUMP_STOP_RATE").filter(|v| *v > 0.0) {
                axle.bump_stop_rate = v;
            }
            if let Some(v) = get("BUMPSTOP_UP").filter(|v| (0.005..0.3).contains(v)) {
                axle.bump_travel = v;
            }
            if let Some(v) = get("BUMPSTOP_DN").filter(|v| (0.005..0.3).contains(v)) {
                axle.droop_travel = v;
            }
            if let Some(v) = get("STATIC_CAMBER") {
                axle.static_camber = v.to_radians();
            }
            // A suspension the simulation has no linkage for keeps the base car's.
            if let Some(section) = ini::section(s, sec) {
                match linkage(section) {
                    Some(l) => axle.linkage = l,
                    None => {
                        if let Some(kind) = section.get("TYPE") {
                            unsupported.push(format!("{} ({kind})", sec.to_ascii_lowercase()));
                        }
                    }
                }
            }
            if let Some(v) = get("HUB_MASS").filter(|v| *v > 0.0) {
                axle.unsprung_mass = v;
            }
            if let Some(v) = get("TRACK").filter(|v| *v > 0.0) {
                if front {
                    p.track_front = v;
                } else {
                    p.track_rear = v;
                }
            }
            // BASEY is the wheel centre's height relative to the centre of gravity.
            heights[k] = get("BASEY");
        }
        if let Some(v) = get("ARB", "FRONT").filter(|v| *v >= 0.0) {
            p.front.anti_roll_rate = v;
        }
        if let Some(v) = get("ARB", "REAR").filter(|v| *v >= 0.0) {
            p.rear.anti_roll_rate = v;
        }
        if let [Some(f), Some(r)] = heights {
            let fw = p.front_weight;
            let h = fw * (self.front.radius - f) + (1.0 - fw) * (self.rear.radius - r);
            if (0.1..1.5).contains(&h) {
                p.cg_height = h;
            }
        }
        if !unsupported.is_empty() {
            self.note(format!(
                "suspension: {} not simulated; the base car's linkage is kept",
                unsupported.join(", ")
            ));
        }
    }

    fn apply_tyres(&mut self, s: &[Section], table: &dyn Fn(&str) -> Vec<(f64, f64)>) {
        for (sec, thermal) in [("FRONT", "THERMAL_FRONT"), ("REAR", "THERMAL_REAR")] {
            let Some(t) = ini::section(s, sec) else {
                continue;
            };
            let front = sec == "FRONT";
            let tire = if front {
                &mut self.front
            } else {
                &mut self.rear
            };
            let get = |key: &str| t.get_f64(key);
            if let Some(v) = get("RADIUS").filter(|v| *v > 0.1) {
                tire.radius = v;
            }
            if let Some(v) = get("WIDTH").filter(|v| *v > 0.05) {
                tire.width = v;
            }
            if let Some(v) = get("RATE").filter(|v| *v > 0.0) {
                tire.vertical_stiffness = v;
            }
            if let Some(v) = get("DAMP").filter(|v| *v > 0.0) {
                tire.vertical_damping = v;
            }
            if let Some(v) = get("DY_REF").or_else(|| get("DY0")).filter(|v| *v > 0.1) {
                tire.mu_y = v;
            }
            if let Some(v) = get("DX_REF").or_else(|| get("DX0")).filter(|v| *v > 0.1) {
                tire.mu_x = v;
            }
            if let Some(v) = get("FZ0").filter(|v| *v > 0.0) {
                tire.nominal_load = v;
            }
            // μ ∝ (load / FZ0)^(LS_EXP − 1).
            if let Some(v) = get("LS_EXPY").filter(|v| (0.0..=1.2).contains(v)) {
                tire.load_exponent_y = v;
            }
            if let Some(v) = get("LS_EXPX").filter(|v| (0.0..=1.2).contains(v)) {
                tire.load_exponent_x = v;
            }
            // Grip left far past the peak: the Magic Formula tends to sin(C·π/2).
            if let Some(level) = get("FALLOFF_LEVEL").filter(|v| (0.3..1.0).contains(v)) {
                let shape = (2.0 - 2.0 * level.asin() / PI).clamp(1.05, 1.95);
                tire.lateral.shape = shape;
                tire.longitudinal.shape = shape;
            }
            if let Some(v) = get("SPEED_SENSITIVITY").filter(|v| (0.0..0.1).contains(v)) {
                tire.speed_sensitivity = v;
            }
            // Camber grip 1 + D0·γ + D1·γ², a parabola peaking at −D0 / 2·D1.
            if let (Some(d0), Some(d1)) = (get("DCAMBER_0"), get("DCAMBER_1"))
                && d1 < 0.0
            {
                let best = (-d0 / (2.0 * d1)).abs().min(0.1);
                tire.optimal_camber = -best;
                tire.camber_grip_loss = -d1 / (1.0 + d0.abs() * best + d1 * best * best);
            }
            if let Some(v) = get("PRESSURE_SPRING_GAIN").filter(|v| *v > 0.0) {
                tire.pressure.stiffness_per_bar = v / PSI;
            }
            // Grip lost per psi off the ideal pressure: the window is fitted to lose as
            // much a quarter of a bar off.
            if let Some(v) = get("PRESSURE_D_GAIN").filter(|v| (0.0..0.2).contains(v))
                && v > 0.0
            {
                let pp = &mut tire.pressure;
                let loss = (v * PRESSURE_FIT / PSI).min(0.5);
                pp.grip_loss = pp.grip_loss.max(1.5 * loss).min(0.75);
                pp.window = PRESSURE_FIT / (-(1.0 - loss / pp.grip_loss).ln()).sqrt();
            }
            if let Some(v) = get("FRICTION_LIMIT_ANGLE").filter(|v| (1.0..20.0).contains(v)) {
                tire.lateral.peak_slip = v.to_radians();
            }
            if let Some(v) = get("ROLLING_RESISTANCE_0").filter(|v| (0.0..0.1).contains(v)) {
                tire.rolling_resistance = v;
            }
            if let Some(v) = get("PRESSURE_IDEAL").filter(|v| *v > 0.0) {
                tire.pressure.optimal = v * PSI;
            }
            let axle = if front {
                &mut self.params.front
            } else {
                &mut self.params.rear
            };
            if let Some(v) = get("ANGULAR_INERTIA").filter(|v| *v > 0.0) {
                axle.wheel_inertia = v;
            }
            if let Some(v) = get("PRESSURE_STATIC").filter(|v| *v > 0.0) {
                axle.pressure = v * PSI;
            }
            let curve = ini::section(s, thermal)
                .and_then(|t| t.get("PERFORMANCE_CURVE"))
                .map(table)
                .unwrap_or_default();
            if curve.len() >= 2 && curve.iter().all(|g| g.1 > 0.0) {
                tire.thermal.grip_curve = curve;
            }
        }
    }

    fn apply_engine(&mut self, s: &[Section], table: &dyn Fn(&str) -> Vec<(f64, f64)>) {
        let get = |sec: &str, key: &str| ini::section(s, sec)?.get_f64(key);
        // The game's curve is the engine without boost, as open-racing's is.
        let curve: Vec<(f64, f64)> = ini::section(s, "HEADER")
            .and_then(|h| h.get("POWER_CURVE"))
            .map(table)
            .unwrap_or_default()
            .into_iter()
            .filter(|&(rpm, nm)| rpm > 0.0 && nm > 0.0)
            .collect();
        if curve.len() >= 2 {
            self.set_torque_curve(curve);
        }
        self.apply_turbos(s);
        let e = &mut self.params.engine;
        if let Some(v) = get("ENGINE_DATA", "LIMITER").filter(|v| *v > 1000.0) {
            e.limiter_rpm = v;
        }
        if let Some(v) = get("ENGINE_DATA", "MINIMUM").filter(|v| *v > e.stall_rpm) {
            e.idle_rpm = v;
        }
        if let Some(v) = get("ENGINE_DATA", "INERTIA").filter(|v| *v > 0.0) {
            e.inertia = v;
        }
        // The speed above which the game's engine takes damage wears the valvetrain.
        if let Some(v) = get("DAMAGE", "RPM_THRESHOLD").filter(|v| *v > e.idle_rpm) {
            e.over_rev_rpm = Some(v);
        }
        // Coast torque grows about linearly with rpm up to the reference point.
        if let (Some(rpm), Some(nm)) = (get("COAST_REF", "RPM"), get("COAST_REF", "TORQUE"))
            && rpm > 0.0
            && nm > 0.0
        {
            let top = e.limiter_rpm.max(rpm) * 1.05;
            e.drag_curve = vec![(0.0, 0.15 * nm), (rpm, nm), (top, nm * top / rpm)];
        }
    }

    /// The game's turbochargers (`[TURBO_n]`) as one: their boosts, capped by their
    /// wastegates, add up; the reference speed, response and lag are averaged, weighted
    /// by boost. The game's lag is a filter per step of its physics.
    fn apply_turbos(&mut self, s: &[Section]) {
        // Each turbocharger's section and its boost, capped by its wastegate.
        let turbos: Vec<(&Section, f64)> = s
            .iter()
            .filter(|sec| sec.name.to_ascii_uppercase().starts_with("TURBO_"))
            .filter_map(|t| {
                let max = t.get_f64("MAX_BOOST").filter(|b| *b > 0.0)?;
                let gate = t.get_f64("WASTEGATE").filter(|w| *w > 0.0).unwrap_or(max);
                Some((t, max.min(gate)))
            })
            .collect();
        let boost: f64 = turbos.iter().map(|t| t.1).sum();
        if boost <= 0.0 {
            return;
        }
        // Boost-weighted mean of `key` over the turbochargers that give a valid one.
        let mean = |key: &str, valid: fn(f64) -> bool| {
            let (sum, weight) = turbos
                .iter()
                .filter_map(|(t, b)| Some((t.get_f64(key).filter(|v| valid(*v))?, *b)))
                .fold((0.0, 0.0), |(sum, weight), (v, b)| {
                    (sum + v * b, weight + b)
                });
            (weight > 0.0).then(|| sum / weight)
        };
        let limiter = self.params.engine.limiter_rpm;
        self.params.engine.turbo = Some(TurboParams {
            max_boost: boost,
            reference_rpm: mean("REFERENCE_RPM", |r| r > 0.0)
                .unwrap_or(0.5 * limiter)
                .clamp(0.1 * limiter, limiter),
            spool_time: mean("LAG_UP", |l| (0.0..1.0).contains(&l))
                .map_or(0.4, |l| 1.0 / (GAME_RATE * (1.0 - l)))
                .clamp(0.05, 5.0),
            flow_exponent: mean("GAMMA", |g| g > 0.0).unwrap_or(2.0).clamp(0.5, 5.0),
            wastegate_band: (0.1 * boost).clamp(0.02, 0.2),
            blow_off_valve: true,
        });
        self.note(format!(
            "turbo: {} turbocharger(s) simulated as one with a wastegate at {boost:.2} bar, \
             spooling from the exhaust flow",
            turbos.len()
        ));
    }

    fn apply_drivetrain(&mut self, s: &[Section], _: &dyn Fn(&str) -> Vec<(f64, f64)>) {
        let get = |sec: &str, key: &str| ini::section(s, sec)?.get_f64(key);
        let g = &mut self.params.gearbox;
        if let Some(n) = get("GEARS", "COUNT").filter(|n| (1.0..=12.0).contains(n)) {
            let ratios: Option<Vec<f64>> = (1..=n as usize)
                .map(|i| get("GEARS", &format!("GEAR_{i}")).filter(|r| *r > 0.0))
                .collect();
            if let Some(r) = ratios {
                g.ratios = r;
            }
        }
        if let Some(v) = get("GEARS", "GEAR_R") {
            g.reverse = v.abs();
        }
        if let Some(v) = get("GEARS", "FINAL").filter(|v| *v > 0.0) {
            g.final_drive = v;
        }
        // A car that takes an H-pattern shifter has a manual gearbox; the others shift by
        // paddles through a sequential one, cutting the ignition on upshifts.
        let shifter = get("GEARBOX", "SUPPORTS_SHIFTER").is_some_and(|v| v != 0.0);
        let (base_time, dog_release_torque) = match g.kind {
            GearboxKind::Sequential {
                shift_time,
                dog_release_torque,
            } => (shift_time, dog_release_torque),
            _ => (0.1, 100.0),
        };
        g.kind = if shifter {
            GearboxKind::h_pattern(SYNCHRO_TORQUE, LEVER_TIME)
        } else {
            GearboxKind::Sequential {
                shift_time: get("GEARBOX", "CHANGE_UP_TIME")
                    .filter(|v| *v >= 0.0)
                    .map_or(base_time, |ms| ms / 1000.0),
                dog_release_torque,
            }
        };
        let e = &self.params.engine;
        // An automatic clutch the car always has works like anti-stall: open below its
        // minimum engine speed, closed above its maximum.
        let anti_stall = get("AUTOCLUTCH", "FORCED_ON")
            .filter(|v| *v != 0.0)
            .and_then(|_| Some((get("AUTOCLUTCH", "MIN_RPM")?, get("AUTOCLUTCH", "MAX_RPM")?)))
            .filter(|&(min, max)| min > e.stall_rpm && max > min)
            .map(|(min, max)| AntiStall {
                rpm: min,
                band_rpm: max - min,
            });
        self.params.electronics = ElectronicsParams {
            anti_stall,
            auto_blip: get("AUTOBLIP", "ELECTRONIC").is_some_and(|v| v != 0.0),
            ignition_cut: !shifter,
            downshift_protection_rpm: get("DOWNSHIFT_PROTECTION", "ACTIVE")
                .filter(|v| *v != 0.0)
                .map(|_| e.limiter_rpm + get("DOWNSHIFT_PROTECTION", "OVERREV").unwrap_or(0.0)),
            ..Default::default()
        };
        if let Some(v) = get("CLUTCH", "MAX_TORQUE").filter(|v| *v > 0.0) {
            self.params.clutch.max_torque = v;
        }
        // A differential's values from `<prefix>POWER`, `<prefix>COAST` and `<prefix>PRELOAD`
        // in `section`, over `d`.
        let read_diff = |d: &mut DifferentialParams, section: &str, prefix: &str| {
            let get = |key: &str| get(section, &format!("{prefix}{key}")).filter(|v| *v >= 0.0);
            if let Some(v) = get("POWER") {
                d.power_ramp = v;
            }
            if let Some(v) = get("COAST") {
                d.coast_ramp = v;
            }
            if let Some(v) = get("PRELOAD") {
                d.preload = v;
            }
        };
        read_diff(&mut self.params.differential, "DIFFERENTIAL", "");
        let drive = ini::section(s, "TRACTION")
            .and_then(|t| t.get("TYPE"))
            .map(str::to_ascii_uppercase);
        match drive.as_deref() {
            Some("RWD") => self.params.drive = Drive::Rear,
            Some("FWD") => self.params.drive = Drive::Front,
            Some(awd) if awd.starts_with("AWD") => {
                // The game's all-wheel drive: the torque split and three differentials in
                // `[AWD]`; the rear one replaces `[DIFFERENTIAL]`.
                let (mut front_share, mut centre_differential, mut front_differential) = (
                    AWD_FRONT_SHARE,
                    AWD_CENTRE_DIFFERENTIAL,
                    AWD_FRONT_DIFFERENTIAL,
                );
                let share = get("AWD", "FRONT_SHARE").filter(|v| (0.0..=100.0).contains(v));
                if let Some(percent) = share {
                    front_share = percent / 100.0;
                }
                read_diff(&mut centre_differential, "AWD", "CENTRE_DIFF_");
                read_diff(&mut front_differential, "AWD", "FRONT_DIFF_");
                read_diff(&mut self.params.differential, "AWD", "REAR_DIFF_");
                self.params.drive = Drive::All {
                    front_share,
                    centre_differential,
                    front_differential,
                };
                if share.is_none() || awd != "AWD" {
                    self.note(format!(
                        "drive: {awd} is simulated as all-wheel drive through a limited-slip \
                         centre differential, {:.0} % of the torque to the front",
                        100.0 * front_share
                    ));
                }
            }
            Some(other) => self.note(format!(
                "drive: unknown type {other}; the base car's drive is kept"
            )),
            None => {}
        }
    }

    fn apply_brakes(&mut self, s: &[Section], _: &dyn Fn(&str) -> Vec<(f64, f64)>) {
        let Some(d) = ini::section(s, "DATA") else {
            return;
        };
        let b = &mut self.params.brakes;
        // The game's maximum torque applies at each end of the car, split by the share.
        if let Some(v) = d.get_f64("MAX_TORQUE").filter(|v| *v > 0.0) {
            b.max_torque = 2.0 * v;
        }
        if let Some(v) = d.get_f64("FRONT_SHARE").filter(|v| (0.0..=1.0).contains(v)) {
            b.front_bias = v;
        }
    }

    /// Wings and the body (`[WING_n]`) become aero elements: an area with lift and drag
    /// coefficients against the angle of attack and multipliers against the ride height
    /// under it, set at its angle, losing downforce with sideslip and damage.
    fn apply_aero(&mut self, s: &[Section], table: &dyn Fn(&str) -> Vec<(f64, f64)>) {
        let mut elements = Vec::new();
        for w in s
            .iter()
            .filter(|w| w.name.to_ascii_uppercase().starts_with("WING_"))
        {
            let (Some(chord), Some(span)) = (w.get_f64("CHORD"), w.get_f64("SPAN")) else {
                continue;
            };
            // Tables against the angle of attack in degrees, scaled by the gain.
            let by_angle = |key: &str, gain: &str| -> Vec<(f64, f64)> {
                let gain = w.get_f64(gain).unwrap_or(1.0);
                let t: Vec<(f64, f64)> = w
                    .get(key)
                    .map(table)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(deg, c)| (deg.to_radians(), c * gain))
                    .collect();
                if t.is_empty() { vec![(0.0, 0.0)] } else { t }
            };
            let by_height = |key: &str| w.get(key).map(table).unwrap_or_default();
            // The game's damage is the impact speed in km/h; its zones' factors cost
            // downforce and drag per unit of it.
            let zones = |kind: &str| -> [f64; DAMAGE_ZONES] {
                ["FRONT", "REAR", "LEFT", "RIGHT"].map(|zone| {
                    w.get_f64(&format!("ZONE_{zone}_{kind}"))
                        .map_or(0.0, |v| (v.abs() * 3.6).min(1.0))
                })
            };
            let position = w.get_f64s("POSITION").unwrap_or_default();
            let at = |k: usize| position.get(k).copied().unwrap_or(0.0);
            elements.push(AeroElement {
                name: w.get("NAME").unwrap_or(&w.name).to_string(),
                // The game's axes: x left-right, y up, z forward, from the CG.
                position: [at(2), at(1)],
                area: chord * span,
                angle: w.get_f64("ANGLE").unwrap_or(0.0).to_radians(),
                lift: by_angle("LUT_AOA_CL", "CL_GAIN"),
                drag: by_angle("LUT_AOA_CD", "CD_GAIN"),
                height_lift: by_height("LUT_GH_CL"),
                height_drag: by_height("LUT_GH_CD"),
                yaw_lift: w.get_f64("YAW_CL_GAIN").unwrap_or(0.0),
                damage_lift: zones("CL"),
                damage_drag: zones("CD"),
            });
        }
        if elements.is_empty() {
            return;
        }
        self.params.aero.elements = elements;
        let skipped: Vec<&str> = ["FIN_", "DYNAMIC_CONTROLLER_"]
            .into_iter()
            .filter(|prefix| {
                s.iter()
                    .any(|sec| sec.name.to_ascii_uppercase().starts_with(prefix))
            })
            .collect();
        if !skipped.is_empty() {
            self.note(format!(
                "aero: {} sections (fins, active aero) are not simulated",
                skipped.join(", ")
            ));
        }
    }
}

/// The linkage of a double-wishbone (`DWB`) or strut (`STRUT`) suspension from its
/// hardpoints, or none for the other kinds. The game's points are relative to the hub
/// of a wheel: x across, y up, z forward; they become the left wheel's, x forward, y left
/// (outboard), z up. An axle without steering points gets a toe link beside the lower
/// arm, behind the axle.
fn linkage(s: &Section) -> Option<Linkage> {
    let raw = |key: &str| -> Option<[f64; 3]> {
        let v = s.get_f64s(key)?;
        Some([*v.first()?, *v.get(1)?, *v.get(2)?])
    };
    let kind = s.get("TYPE")?.to_ascii_uppercase();
    let (bottom_front, bottom_rear) = (raw("WBCAR_BOTTOM_FRONT")?, raw("WBCAR_BOTTOM_REAR")?);
    let bottom = raw("WBTYRE_BOTTOM")?;
    // Inboard is the way from the upright's joints to the car's.
    let inboard = if bottom_front[0] + bottom_rear[0] > 2.0 * bottom[0] {
        1.0
    } else {
        -1.0
    };
    let point = |p: [f64; 3]| [p[2], -inboard * p[0], p[1]];
    let get = |key: &str| raw(key).map(point);
    let lower = Wishbone {
        front: point(bottom_front),
        rear: point(bottom_rear),
        outer: point(bottom),
    };
    let tie_rod = match (get("WBCAR_STEER"), get("WBTYRE_STEER")) {
        (Some(inner), Some(outer)) => Link { inner, outer },
        _ => {
            let behind = |p: [f64; 3], x: f64| [x, p[1], p[2]];
            let x = lower.rear[0].min(lower.outer[0]) - TOE_LINK_OFFSET;
            let middle = |a: [f64; 3], b: [f64; 3]| [0.0, 0.5 * (a[1] + b[1]), 0.5 * (a[2] + b[2])];
            Link {
                inner: behind(middle(lower.front, lower.rear), x),
                outer: behind(lower.outer, x),
            }
        }
    };
    match kind.as_str() {
        "DWB" => Some(Linkage::DoubleWishbone {
            upper: Wishbone {
                front: get("WBCAR_TOP_FRONT")?,
                rear: get("WBCAR_TOP_REAR")?,
                outer: get("WBTYRE_TOP")?,
            },
            lower,
            tie_rod,
        }),
        "STRUT" => Some(Linkage::MacPherson {
            lower,
            strut_top: get("STRUT_CAR")?,
            strut_bottom: get("STRUT_TYRE")?,
            tie_rod,
        }),
        _ => None,
    }
}

/// The first number in a text such as `1,400kg` or `270+km/h`, times the factor of the
/// first unit in `units` the text mentions.
fn quantity(text: &str, units: &[(&str, f64)]) -> Option<f64> {
    let clean = text.replace(',', "");
    let start = clean.find(|c: char| c.is_ascii_digit())?;
    let digits: String = clean[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value: f64 = digits.parse().ok()?;
    let lower = clean.to_ascii_lowercase();
    let factor = units
        .iter()
        .find(|(u, _)| lower.contains(u))
        .map_or(1.0, |&(_, f)| f);
    Some(value * factor)
}

#[cfg(test)]
pub(crate) mod tests {
    use open_racing_sim::CarModel;

    use super::*;
    use crate::json;

    fn base() -> Physics {
        let gt3 = CarModel::gt3();
        Physics::new(
            gt3.params.clone(),
            gt3.front_tire.p.clone(),
            gt3.rear_tire.p.clone(),
        )
    }

    #[test]
    fn quantities_in_text() {
        assert_eq!(quantity("1,400kg", &[("lb", 0.5)]), Some(1400.0));
        assert_eq!(quantity("270+km/h", &[("mph", 2.0)]), Some(270.0));
        assert_eq!(quantity("100 mph", &[("mph", 2.0)]), Some(200.0));
        assert_eq!(quantity("--s 0-100", &[]), Some(0.0));
        assert_eq!(quantity("n/a", &[]), None);
    }

    #[test]
    fn ui_description_sets_mass_engine_and_gearing() {
        let mut p = base();
        let inertia = p.params.inertia[2];
        let ui = json::parse(
            r#"{"specs": {"weight": "1500kg", "topspeed": "250km/h"},
                "torqueCurve": [[0, 0], [2000, 400], [5000, 600], [7000, 500]],
                "tags": ["awd"]}"#,
        )
        .unwrap();
        p.apply_ui(&ui);
        assert_eq!(p.params.mass, 1500.0);
        assert!(p.params.inertia[2] > inertia);
        assert_eq!(p.params.engine.limiter_rpm, 7000.0);
        assert_eq!(p.params.engine.torque_curve[0], (2000.0, 400.0));
        // At the limiter in top gear the car does 250 km/h plus the margin.
        let g = &p.params.gearbox;
        let v = 7000.0 * PI / 30.0 * p.rear.radius / (g.ratios[5] * g.final_drive);
        assert!((v * 3.6 - 250.0 * TOP_SPEED_MARGIN).abs() < 0.5, "{v}");
        assert_eq!(p.notes.len(), 2, "{:?}", p.notes);
        CarModel::new(p.params, p.front, p.rear).unwrap();
    }

    /// A made-up car in the game's file layout.
    pub fn write_data(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let files = [
            (
                "car.ini",
                "[BASIC]\nTOTALMASS=1200\nINERTIA=1.9,1.1,4.5\n[FUEL]\nFUEL=40\n[CONTROLS]\nSTEER_LOCK=360\nSTEER_RATIO=12\n[RIDE]\nPICKUP_FRONT_HEIGHT=0.06\nPICKUP_REAR_HEIGHT=0.09\n",
            ),
            (
                "suspensions.ini",
                "[BASIC]\nWHEELBASE=2.5\nCG_LOCATION=0.48\n[FRONT]\nTYPE=DWB\nTRACK=1.6\nSPRING_RATE=90000\nDAMP_BUMP=4000\nDAMP_REBOUND=7000\nSTATIC_CAMBER=-3\nHUB_MASS=40\nBASEY=-0.08\nWBCAR_TOP_FRONT=0.3,0.12,0.15\nWBCAR_TOP_REAR=0.3,0.12,-0.15\nWBCAR_BOTTOM_FRONT=0.35,-0.1,0.2\nWBCAR_BOTTOM_REAR=0.35,-0.1,-0.2\nWBTYRE_TOP=0.05,0.15,0\nWBTYRE_BOTTOM=0.05,-0.1,0\n[REAR]\nTRACK=1.58\nSPRING_RATE=100000\nBASEY=-0.07\n[ARB]\nFRONT=50000\nREAR=30000\n",
            ),
            (
                "tyres.ini",
                "[FRONT]\nRADIUS=0.33\nWIDTH=0.28\nDY_REF=1.5\nDX_REF=1.55\nLS_EXPY=0.85\nFRICTION_LIMIT_ANGLE=7.5\nPRESSURE_STATIC=22\nPRESSURE_IDEAL=27\nANGULAR_INERTIA=1.3\nLS_EXPX=0.8\nFALLOFF_LEVEL=0.8\nSPEED_SENSITIVITY=0.003\nDCAMBER_0=1.2\nDCAMBER_1=-13\nPRESSURE_D_GAIN=0.004\nPRESSURE_SPRING_GAIN=5000\n[THERMAL_FRONT]\nPERFORMANCE_CURVE=tcurve.lut\n[REAR]\nRADIUS=0.34\nWIDTH=0.3\n",
            ),
            ("tcurve.lut", "0|0.8\n85|1.0\n150|0.9\n"),
            (
                "engine.ini",
                "[HEADER]\nPOWER_CURVE=power.lut\n[ENGINE_DATA]\nLIMITER=7500\nMINIMUM=1200\nINERTIA=0.15\n[COAST_REF]\nRPM=7000\nTORQUE=70\n[TURBO_0]\nMAX_BOOST=0.5\nWASTEGATE=0.4\nREFERENCE_RPM=4000\nGAMMA=2.5\nLAG_UP=0.99\n[DAMAGE]\nRPM_THRESHOLD=8000\n",
            ),
            ("power.lut", "0|0\n1000|200\n4000|300\n7500|250\n"),
            (
                "drivetrain.ini",
                "[TRACTION]\nTYPE=RWD\n[GEARS]\nCOUNT=5\nGEAR_R=-3.1\nGEAR_1=3.0\nGEAR_2=2.2\nGEAR_3=1.7\nGEAR_4=1.35\nGEAR_5=1.1\nFINAL=3.9\n[GEARBOX]\nCHANGE_UP_TIME=80\n[CLUTCH]\nMAX_TORQUE=800\n[DIFFERENTIAL]\nPOWER=0.4\nCOAST=0.2\nPRELOAD=60\n",
            ),
            ("brakes.ini", "[DATA]\nMAX_TORQUE=3000\nFRONT_SHARE=0.62\n"),
            (
                "aero.ini",
                "[WING_0]\nCHORD=1\nSPAN=2\nPOSITION=0,0,0\nLUT_AOA_CD=(|0=0.35|10=0.5|)\nLUT_AOA_CL=(|0=0.2|)\nLUT_GH_CL=(|0.02=0.8|0.08=1.0|)\nZONE_FRONT_CL=0.01\n[WING_1]\nCHORD=0.3\nSPAN=1.6\nPOSITION=0,0.9,-2.0\nANGLE=5\nLUT_AOA_CL=wing.lut\nLUT_AOA_CD=(|0=0.1|)\nCL_GAIN=1\n",
            ),
            ("wing.lut", "0|1.0\n10|2.0\n"),
        ];
        for (name, src) in files {
            std::fs::write(dir.join(name), src).unwrap();
        }
    }

    #[test]
    fn physics_files_replace_the_base() {
        let dir = std::env::temp_dir().join(format!("open-racing-ac-data-{}", std::process::id()));
        write_data(&dir);
        let mut p = base();
        p.apply_data(&dir).unwrap();
        let c = &p.params;
        assert!((c.mass - (1200.0 + 40.0 * FUEL_DENSITY)).abs() < 1e-9);
        assert!((c.inertia[2] - c.mass / 12.0 * (1.9f64.powi(2) + 4.5f64.powi(2))).abs() < 1e-6);
        assert!((c.steering.lock - 2.0 * PI).abs() < 1e-9);
        assert_eq!(
            (c.wheelbase, c.front_weight, c.track_rear),
            (2.5, 0.48, 1.58)
        );
        assert_eq!(c.front.spring_rate, 90000.0);
        assert!((c.front.static_camber + 3f64.to_radians()).abs() < 1e-12);
        assert_eq!(c.rear.anti_roll_rate, 30000.0);
        let h = 0.48 * (0.33 + 0.08) + 0.52 * (0.34 + 0.07);
        assert!((c.cg_height - h).abs() < 1e-9);
        assert_eq!(
            (p.front.radius, p.front.mu_y, p.rear.width),
            (0.33, 1.5, 0.3)
        );
        assert_eq!(
            (p.front.load_exponent_x, p.front.load_exponent_y),
            (0.8, 0.85)
        );
        let shape = 2.0 - 2.0 * 0.8f64.asin() / PI;
        assert!((p.front.lateral.shape - shape).abs() < 1e-12);
        assert_eq!(p.front.speed_sensitivity, 0.003);
        assert!((p.front.optimal_camber + 1.2 / 26.0).abs() < 1e-12);
        assert!((p.front.pressure.stiffness_per_bar - 5000.0 / PSI).abs() < 1e-6);
        // A quarter of a bar off loses what the game's linear loss does there.
        let pp = &p.front.pressure;
        let lost = pp.grip_loss * (1.0 - (-(PRESSURE_FIT / pp.window).powi(2)).exp());
        assert!((lost - 0.004 * PRESSURE_FIT / PSI).abs() < 1e-9);
        assert_eq!(
            p.front.thermal.grip_curve,
            [(0.0, 0.8), (85.0, 1.0), (150.0, 0.9)]
        );
        // The front's hardpoints become the left wheel's: inboard is −y, forward the
        // game's z. The rear has no geometry and keeps the base car's.
        let Linkage::DoubleWishbone { upper, lower, .. } = &c.front.linkage else {
            panic!("{:?}", c.front.linkage);
        };
        assert_eq!(upper.outer, [0.0, -0.05, 0.15]);
        assert_eq!(lower.front, [0.2, -0.35, -0.1]);
        assert_eq!(c.rear.linkage, base().params.rear.linkage);
        // The arms meet 2.13 m inboard: the wheel gains about 1 / 2.13 rad of negative
        // camber per metre it rises.
        let model = CarModel::new(c.clone(), p.front.clone(), p.rear.clone()).unwrap();
        let gain = model.axle_figures(true).kinematics.camber_gain;
        assert!((gain + 0.3 / 0.64).abs() < 0.05, "{gain}");
        assert!((c.front.pressure - 22.0 * PSI).abs() < 1e-12);
        // The curve stays the engine's without boost; the turbo adds it.
        assert_eq!(c.engine.torque_curve[1], (4000.0, 300.0));
        let turbo = c.engine.turbo.as_ref().unwrap();
        assert_eq!((turbo.max_boost, turbo.reference_rpm), (0.4, 4000.0));
        assert!((turbo.spool_time - 1.0 / 3.33).abs() < 1e-9);
        assert_eq!(turbo.flow_exponent, 2.5);
        assert_eq!((c.engine.limiter_rpm, c.engine.idle_rpm), (7500.0, 1200.0));
        assert_eq!(c.engine.over_rev_rpm, Some(8000.0));
        assert_eq!(c.gearbox.ratios.len(), 5);
        assert_eq!((c.gearbox.reverse, c.gearbox.final_drive), (3.1, 3.9));
        assert_eq!(c.differential.preload, 60.0);
        assert_eq!((c.brakes.max_torque, c.brakes.front_bias), (6000.0, 0.62));
        // Each wing becomes an element.
        let a = &c.aero;
        assert_eq!(a.ride_height, [0.06, 0.09]);
        let [body, wing] = &a.elements[..] else {
            panic!("{:?}", a.elements);
        };
        assert_eq!((body.area, body.lift[..].to_vec()), (2.0, vec![(0.0, 0.2)]));
        assert!((body.drag[1].0 - 10f64.to_radians()).abs() < 1e-12);
        assert_eq!(body.height_lift, [(0.02, 0.8), (0.08, 1.0)]);
        assert!((body.damage_lift[0] - 0.036).abs() < 1e-12);
        assert_eq!(body.damage_lift[1..], [0.0; 3]);
        assert_eq!(wing.position, [-2.0, 0.9]);
        assert!((wing.area - 0.48).abs() < 1e-12);
        assert!((wing.angle - 5f64.to_radians()).abs() < 1e-12);
        assert_eq!(wing.lift[1].1, 2.0);
        CarModel::new(p.params.clone(), p.front.clone(), p.rear.clone()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Converts the made-up car with `drivetrain` as its `drivetrain.ini`.
    fn with_drivetrain(tag: &str, drivetrain: &str) -> Physics {
        let dir =
            std::env::temp_dir().join(format!("open-racing-ac-drive-{tag}-{}", std::process::id()));
        write_data(&dir);
        std::fs::write(dir.join("drivetrain.ini"), drivetrain).unwrap();
        let mut p = base();
        p.apply_data(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        CarModel::new(p.params.clone(), p.front.clone(), p.rear.clone()).unwrap();
        p
    }

    #[test]
    fn gearbox_type_and_electronics_follow_the_drivetrain() {
        let p = with_drivetrain(
            "manual",
            "[GEARBOX]\nSUPPORTS_SHIFTER=1\nCHANGE_UP_TIME=200\n[AUTOBLIP]\nELECTRONIC=0\n",
        );
        assert!(matches!(
            p.params.gearbox.kind,
            GearboxKind::HPattern { .. }
        ));
        assert_eq!(p.params.electronics, ElectronicsParams::default());

        let p = with_drivetrain(
            "paddles",
            "[GEARBOX]\nSUPPORTS_SHIFTER=0\nCHANGE_UP_TIME=60\n[AUTOBLIP]\nELECTRONIC=1\n\
             [DOWNSHIFT_PROTECTION]\nACTIVE=1\nOVERREV=200\n",
        );
        let GearboxKind::Sequential { shift_time, .. } = p.params.gearbox.kind else {
            panic!("{:?}", p.params.gearbox.kind);
        };
        assert!((shift_time - 0.06).abs() < 1e-12);
        let e = &p.params.electronics;
        assert!(e.auto_blip && e.ignition_cut && e.anti_stall.is_none());
        assert_eq!(e.downshift_protection_rpm, Some(7500.0 + 200.0));
    }

    #[test]
    fn an_automatic_clutch_the_car_always_has_becomes_anti_stall() {
        let clutch = |forced| {
            with_drivetrain(
                &format!("autoclutch{forced}"),
                &format!("[AUTOCLUTCH]\nMIN_RPM=2000\nMAX_RPM=3000\nFORCED_ON={forced}\n"),
            )
            .params
            .electronics
            .anti_stall
        };
        assert_eq!(
            clutch(1),
            Some(AntiStall {
                rpm: 2000.0,
                band_rpm: 1000.0
            })
        );
        // A driver aid the player may turn off is not the car's.
        assert_eq!(clutch(0), None);
    }

    #[test]
    fn front_wheel_drive_drives_the_front_through_its_differential() {
        let p = with_drivetrain(
            "fwd",
            "[TRACTION]\nTYPE=FWD\n[DIFFERENTIAL]\nPOWER=0.25\nCOAST=0.05\nPRELOAD=30\n",
        );
        assert_eq!(p.params.drive, Drive::Front);
        assert_eq!(p.params.differential.power_ramp, 0.25);
        assert!(
            p.notes.iter().all(|n| !n.starts_with("drive")),
            "{:?}",
            p.notes
        );
    }

    #[test]
    fn all_wheel_drive_reads_the_split_and_three_differentials() {
        let p = with_drivetrain(
            "awd",
            "[TRACTION]\nTYPE=AWD\n[DIFFERENTIAL]\nPOWER=0.9\n[AWD]\nFRONT_SHARE=35\n\
             FRONT_DIFF_POWER=0.1\nFRONT_DIFF_COAST=0.05\nFRONT_DIFF_PRELOAD=10\n\
             CENTRE_DIFF_POWER=0.6\nCENTRE_DIFF_COAST=0.3\nCENTRE_DIFF_PRELOAD=80\n\
             REAR_DIFF_POWER=0.4\nREAR_DIFF_COAST=0.2\nREAR_DIFF_PRELOAD=50\n",
        );
        let diff = |preload, power_ramp, coast_ramp| DifferentialParams {
            preload,
            power_ramp,
            coast_ramp,
        };
        assert_eq!(
            p.params.drive,
            Drive::All {
                front_share: 0.35,
                centre_differential: diff(80.0, 0.6, 0.3),
                front_differential: diff(10.0, 0.1, 0.05),
            }
        );
        assert_eq!(p.params.differential, diff(50.0, 0.4, 0.2));
        assert!(
            p.notes.iter().all(|n| !n.starts_with("drive")),
            "{:?}",
            p.notes
        );

        // The newer model: all-wheel drive with what the files give, and a note.
        let p = with_drivetrain("awd2", "[TRACTION]\nTYPE=AWD2\n");
        assert!(matches!(p.params.drive, Drive::All { front_share, .. } if front_share == 0.4));
        assert!(
            p.notes.iter().any(|n| n.starts_with("drive: AWD2")),
            "{:?}",
            p.notes
        );
    }

    #[test]
    fn ui_tags_set_the_drive_and_gear_the_driven_wheels() {
        let mut p = base();
        p.front.radius = 0.30;
        let ui = json::parse(r#"{"specs": {"topspeed": "200km/h"}, "tags": ["FWD"]}"#).unwrap();
        p.apply_ui(&ui);
        assert_eq!(p.params.drive, Drive::Front);
        let g = &p.params.gearbox;
        let v = p.params.engine.limiter_rpm * PI / 30.0 * 0.30 / (g.ratios[5] * g.final_drive);
        assert!((v * 3.6 - 200.0 * TOP_SPEED_MARGIN).abs() < 0.5, "{v}");
    }
}
