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

use open_racing_sim::params::DifferentialParams;
use open_racing_sim::tire::TireParams;
use open_racing_sim::{CarParams, Drive, ElectronicsParams, GearboxKind};

use crate::ini::{self, Section};
use crate::json::Value;
use crate::{Error, lut};

/// Synchroniser torque given to H-pattern gearboxes, which the game's data does not
/// describe, N·m.
const SYNCHRO_TORQUE: f64 = 25.0;
/// Hand travel between gates of an H-pattern lever with sequential requests, s.
const LEVER_TIME: f64 = 0.25;
const PSI: f64 = 0.0689476;
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
            // μ ∝ (load / FZ0)^(LS_EXPY − 1).
            if let Some(v) = get("LS_EXPY").filter(|v| (0.0..=1.0).contains(v)) {
                tire.load_sensitivity = v - 1.0;
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
            if let Some(&(temp, _)) = curve.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
                tire.thermal.optimal_temperature = temp;
            }
        }
    }

    fn apply_engine(&mut self, s: &[Section], table: &dyn Fn(&str) -> Vec<(f64, f64)>) {
        let get = |sec: &str, key: &str| ini::section(s, sec)?.get_f64(key);
        let boost: f64 = s
            .iter()
            .filter(|sec| sec.name.to_ascii_uppercase().starts_with("TURBO_"))
            .filter_map(|sec| sec.get_f64("MAX_BOOST"))
            .sum();
        let curve: Vec<(f64, f64)> = ini::section(s, "HEADER")
            .and_then(|h| h.get("POWER_CURVE"))
            .map(table)
            .unwrap_or_default()
            .into_iter()
            .filter(|&(rpm, nm)| rpm > 0.0 && nm > 0.0)
            // Turbochargers add their boost; the curve is taken at full boost.
            .map(|(rpm, nm)| (rpm, nm * (1.0 + boost.max(0.0))))
            .collect();
        if curve.len() >= 2 {
            self.set_torque_curve(curve);
        }
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
        // Coast torque grows about linearly with rpm up to the reference point.
        if let (Some(rpm), Some(nm)) = (get("COAST_REF", "RPM"), get("COAST_REF", "TORQUE"))
            && rpm > 0.0
            && nm > 0.0
        {
            let top = e.limiter_rpm.max(rpm) * 1.05;
            e.drag_curve = vec![(0.0, 0.15 * nm), (rpm, nm), (top, nm * top / rpm)];
        }
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
            GearboxKind::HPattern {
                synchro_torque: SYNCHRO_TORQUE,
                lever_time: LEVER_TIME,
            }
        } else {
            GearboxKind::Sequential {
                shift_time: get("GEARBOX", "CHANGE_UP_TIME")
                    .filter(|v| *v >= 0.0)
                    .map_or(base_time, |ms| ms / 1000.0),
                dog_release_torque,
            }
        };
        let limiter = self.params.engine.limiter_rpm;
        self.params.electronics = ElectronicsParams {
            anti_stall: None,
            auto_blip: get("AUTOBLIP", "ELECTRONIC").is_some_and(|v| v != 0.0),
            ignition_cut: !shifter,
            downshift_protection_rpm: get("DOWNSHIFT_PROTECTION", "ACTIVE")
                .filter(|v| *v != 0.0)
                .map(|_| limiter + get("DOWNSHIFT_PROTECTION", "OVERREV").unwrap_or(0.0)),
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

    /// Wings and the body, each an area with lift and drag coefficients at its angle,
    /// become drag and downforce areas, the downforce split between the axles by where
    /// each wing sits.
    fn apply_aero(&mut self, s: &[Section], table: &dyn Fn(&str) -> Vec<(f64, f64)>) {
        let p = &mut self.params;
        let front_x = p.wheelbase * (1.0 - p.front_weight);
        let (mut drag, mut front, mut rear, mut wings) = (0.0, 0.0, 0.0, 0);
        for w in s
            .iter()
            .filter(|w| w.name.to_ascii_uppercase().starts_with("WING_"))
        {
            let (Some(chord), Some(span)) = (w.get_f64("CHORD"), w.get_f64("SPAN")) else {
                continue;
            };
            let angle = w.get_f64("ANGLE").unwrap_or(0.0);
            let coefficient = |key: &str, gain: &str| {
                let t = w.get(key).map(table).unwrap_or_default();
                lut::lookup(&t, angle).unwrap_or(0.0) * w.get_f64(gain).unwrap_or(1.0)
            };
            let area = chord * span;
            let x = w
                .get_f64s("POSITION")
                .and_then(|v| v.get(2).copied())
                .unwrap_or(0.0);
            let share = ((x - (front_x - p.wheelbase)) / p.wheelbase).clamp(0.0, 1.0);
            let lift = area * coefficient("LUT_AOA_CL", "CL_GAIN");
            drag += area * coefficient("LUT_AOA_CD", "CD_GAIN");
            front += lift * share;
            rear += lift * (1.0 - share);
            wings += 1;
        }
        if wings > 0 && drag > 0.0 {
            let a = &mut p.aero;
            a.drag_area = drag;
            // The game's lift is positive downwards; the simulation has no lift.
            a.downforce_area_front = front.max(0.0);
            a.downforce_area_rear = rear.max(0.0);
        }
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
                "[BASIC]\nTOTALMASS=1200\nINERTIA=1.9,1.1,4.5\n[FUEL]\nFUEL=40\n[CONTROLS]\nSTEER_LOCK=360\nSTEER_RATIO=12\n",
            ),
            (
                "suspensions.ini",
                "[BASIC]\nWHEELBASE=2.5\nCG_LOCATION=0.48\n[FRONT]\nTRACK=1.6\nSPRING_RATE=90000\nDAMP_BUMP=4000\nDAMP_REBOUND=7000\nSTATIC_CAMBER=-3\nHUB_MASS=40\nBASEY=-0.08\n[REAR]\nTRACK=1.58\nSPRING_RATE=100000\nBASEY=-0.07\n[ARB]\nFRONT=50000\nREAR=30000\n",
            ),
            (
                "tyres.ini",
                "[FRONT]\nRADIUS=0.33\nWIDTH=0.28\nDY_REF=1.5\nDX_REF=1.55\nLS_EXPY=0.85\nFRICTION_LIMIT_ANGLE=7.5\nPRESSURE_STATIC=22\nPRESSURE_IDEAL=27\nANGULAR_INERTIA=1.3\n[THERMAL_FRONT]\nPERFORMANCE_CURVE=tcurve.lut\n[REAR]\nRADIUS=0.34\nWIDTH=0.3\n",
            ),
            ("tcurve.lut", "0|0.8\n85|1.0\n150|0.9\n"),
            (
                "engine.ini",
                "[HEADER]\nPOWER_CURVE=power.lut\n[ENGINE_DATA]\nLIMITER=7500\nMINIMUM=1200\nINERTIA=0.15\n[COAST_REF]\nRPM=7000\nTORQUE=70\n[TURBO_0]\nMAX_BOOST=0.5\n",
            ),
            ("power.lut", "0|0\n1000|200\n4000|300\n7500|250\n"),
            (
                "drivetrain.ini",
                "[TRACTION]\nTYPE=RWD\n[GEARS]\nCOUNT=5\nGEAR_R=-3.1\nGEAR_1=3.0\nGEAR_2=2.2\nGEAR_3=1.7\nGEAR_4=1.35\nGEAR_5=1.1\nFINAL=3.9\n[GEARBOX]\nCHANGE_UP_TIME=80\n[CLUTCH]\nMAX_TORQUE=800\n[DIFFERENTIAL]\nPOWER=0.4\nCOAST=0.2\nPRELOAD=60\n",
            ),
            ("brakes.ini", "[DATA]\nMAX_TORQUE=3000\nFRONT_SHARE=0.62\n"),
            (
                "aero.ini",
                "[WING_0]\nCHORD=1\nSPAN=2\nPOSITION=0,0,0\nLUT_AOA_CD=(|0=0.35|10=0.5|)\nLUT_AOA_CL=(|0=0.2|)\n[WING_1]\nCHORD=0.3\nSPAN=1.6\nPOSITION=0,0.9,-2.0\nANGLE=5\nLUT_AOA_CL=wing.lut\nLUT_AOA_CD=(|0=0.1|)\nCL_GAIN=1\n",
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
        assert!((p.front.load_sensitivity + 0.15).abs() < 1e-12);
        assert!((c.front.pressure - 22.0 * PSI).abs() < 1e-12);
        assert_eq!(p.front.thermal.optimal_temperature, 85.0);
        assert_eq!(c.engine.torque_curve[1], (4000.0, 450.0));
        assert_eq!((c.engine.limiter_rpm, c.engine.idle_rpm), (7500.0, 1200.0));
        assert_eq!(c.gearbox.ratios.len(), 5);
        assert_eq!((c.gearbox.reverse, c.gearbox.final_drive), (3.1, 3.9));
        assert_eq!(c.differential.preload, 60.0);
        assert_eq!((c.brakes.max_torque, c.brakes.front_bias), (6000.0, 0.62));
        assert!((c.aero.drag_area - (2.0 * 0.35 + 0.48 * 0.1)).abs() < 1e-9);
        // The rear wing sits behind the rear axle: all its 1.5 · 0.48 m² goes to the rear.
        let body = 2.0 * 0.2;
        let front_x: f64 = 2.5 * 0.52;
        let share = ((0.0 - (front_x - 2.5)) / 2.5).clamp(0.0, 1.0);
        assert!((c.aero.downforce_area_front - body * share).abs() < 1e-9);
        assert!((c.aero.downforce_area_rear - (body * (1.0 - share) + 0.72)).abs() < 1e-9);
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
