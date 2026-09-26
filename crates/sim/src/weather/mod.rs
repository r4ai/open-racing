//! Weather at the track: the sun's course, cloud, the air and the wind, and the road
//! temperature they give. Rain is not modelled.
//!
//! What the car feels:
//! - the air's temperature and density: density scales drag, downforce and engine
//!   power; the air, warmed over hot road, cools the tyres;
//! - the wind, with gusts, which adds to or takes from the airspeed;
//! - the road temperature under each tyre, which heats or cools the tread in contact.
//!
//! The model, from large to small:
//! - The sun follows its course for the latitude, day of year and time of day.
//! - The sky is in one of five states from clear to overcast ([`Sky`]). With changes
//!   on, it wanders between neighbouring states every hour or so; cloud cover, humidity,
//!   pressure and wind follow the state with delays of minutes to hours.
//! - Cloud comes in four layers, as a meteorologist would report it:
//!   - cumulus, from convection: none at night, growing through the day as the ground
//!     warms, deeper in unstable air; their flat bases sit at the lifting condensation
//!     level, about 125 m per kelvin of dew-point depression;
//!   - stratus and stratocumulus, a low deck that closes into overcast;
//!   - altostratus and altocumulus, a mid-level sheet at about 3 km;
//!   - cirrus, thin ice cloud at about 9 km.
//!
//!   A 64×64×32 moist grid transports vapour, liquid water and potential temperature
//!   with height-dependent wind and buoyancy. The cloud map seeds the broad moisture
//!   supply; condensation and evaporation evolve the low/mid-level clouds. Cirrus
//!   remains a separate thin ice layer. Shadows integrate the same liquid-water field.
//! - Clear-sky sunlight follows the air mass (Meinel, Haurwitz); cloud cover turns
//!   direct into diffuse light and takes some away (Kasten–Czeplak).
//! - The air temperature follows a daily cycle about the month's mean, damped by cloud,
//!   and falls with height.
//! - The road's temperature balances sun, sky, air and ground ([`RoadTemperature`]).
//!
//! Everything advances with the physics steps, so a drive replays exactly. Moist
//! convection and the slow forecast update every two weather seconds, independent
//! of time acceleration; the renderer interpolates timestamped density snapshots.

mod clouds;
mod road;
mod shade;
mod volume;

use std::sync::Arc;

use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

pub use clouds::{
    CELL_SOFTNESS, CLOUD_MAP_PERIOD, CLOUD_MAP_SIZE, CLOUD_SOFTNESS, CloudMap, cell_density,
    cell_threshold, cloud_density, cover_threshold,
};
pub use road::{Forcing, LANES, LAPSE_RATE, RoadExposure, RoadTemperature};
pub use shade::{OccluderBuilder, Occluders};
pub use volume::{
    CloudFrame, CloudSnapshot, VOLUME_HEIGHT, VOLUME_PERIOD, VOLUME_TICK, VOLUME_X, VOLUME_Z,
};
use volume::{CloudVolume, VolumeForcing};

use crate::{AIR_DENSITY, AMBIENT_TEMPERATURE};

/// Simulated time between updates of the slow parts of the weather, s.
const TICK: f64 = VOLUME_TICK;
/// Ticks between updates of the road's temperature, whose time constants are tens of
/// minutes.
const ROAD_TICKS: u32 = 5;
/// Solar constant, W/m².
const SOLAR_CONSTANT: f64 = 1361.0;
/// Diffuse daylight from a clear sky with the sun on the horizon, W/m².
const TWILIGHT_SKY: f64 = 4.0;
/// How long the sky stays in a state before it may change, on average, s.
const SKY_DWELL: f64 = 50.0 * 60.0;
/// Time constants with which the air temperature, humidity and pressure follow the
/// sky's state, s.
const AIR_LAG: f64 = 40.0 * 60.0;
const HUMIDITY_LAG: f64 = 2.0 * 3600.0;
const PRESSURE_LAG: f64 = 3.0 * 3600.0;
const WIND_LAG: f64 = 15.0 * 60.0;
/// Wander of the wind direction with changes on, rad per √s.
const WIND_VEER: f64 = 0.004;
/// Wind near the ground (about 1 m, over short grass) relative to the 10 m wind.
const NEAR_GROUND_WIND: f64 = 0.6;
/// Gusts: relative strength, how long one lasts, s, and the veer they bring, rad.
const GUSTINESS: f64 = 0.3;
const GUST_TIME: f64 = 6.0;
const GUST_VEER: f64 = 0.2;
/// Time over which the clouds' shapes go through a full cycle of change, s.
const CLOUD_MORPH_PERIOD: f64 = 3.0 * 3600.0;
/// Per layer ([`CloudKind`] order): the wind relative to the 10 m wind, the least it
/// blows, m/s, and how far it veers from the surface wind, rad.
const LAYER_WIND: [f64; 4] = [1.8, 1.6, 3.0, 5.0];
const LAYER_WIND_MIN: [f64; 4] = [3.0, 3.0, 7.0, 15.0];
const LAYER_VEER: [f64; 4] = [0.2, 0.15, 0.4, 0.7];
/// Per layer: scale of its pattern on the cloud map, and a turn of the map's pattern
/// blend so that the layers do not line up.
const LAYER_SCALE: [f64; 4] = [1.0, 1.7, 2.3, 3.1];
const LAYER_PHASE: [f64; 4] = [0.0, 1.7, 3.4, 5.1];
/// Per layer: extinction of sunlight per m of cloud at full density.
const LAYER_EXTINCTION: [f64; 4] = [0.006, 0.008, 0.0025, 0.0004];
/// Per layer: how much wider than its cover the regions where it forms are; within
/// them it gathers into convective cells. The mid-level sheet and cirrus do not.
const LAYER_SPREAD: [f64; 4] = [1.8, 1.3, 1.0, 1.0];
/// Per layer: how much its cover counts towards the sky's cover (thin cirrus hardly).
const LAYER_OPACITY: [f64; 4] = [1.0, 1.0, 0.85, 0.3];
/// Per layer: time constant with which its cover follows the weather, s.
const LAYER_LAG: [f64; 4] = [15.0 * 60.0, 30.0 * 60.0, 45.0 * 60.0, 60.0 * 60.0];
/// Height of the cloud base per kelvin of dew-point depression (lifting condensation
/// level), m, and the range it is kept to.
const LCL_PER_KELVIN: f64 = 125.0;
const LCL_RANGE: (f64, f64) = (300.0, 3000.0);
/// Clear-sky daylight over which convection starts and is fully going, W/m², reached
/// an hour after the sun has given it: the ground takes that long to warm the air.
const CONVECTION_LIGHT: (f64, f64) = (150.0, 650.0);
const CONVECTION_DELAY: f64 = 1.0;
/// Longest slant path through the cloud layer, in layer thicknesses (a low sun).
const MAX_SLANT: f64 = 8.0;
/// Standard sea-level pressure, hPa.
const STANDARD_PRESSURE: f64 = 1013.25;
/// Gas constants of dry air and water vapour, J/(kg·K).
const R_DRY: f64 = 287.05;
const R_VAPOUR: f64 = 461.5;
const KELVIN: f64 = 273.15;
/// Mean air temperature of each month at a temperate European circuit, °C.
const MONTH_TEMPERATURE: [f64; 12] = [
    2.0, 3.0, 7.0, 11.0, 15.5, 19.0, 21.0, 20.5, 16.5, 12.0, 6.5, 3.0,
];
/// Difference between the warmest and coldest hour of a clear day, per month, K.
const MONTH_DAILY_RANGE: [f64; 12] = [
    7.0, 8.0, 10.0, 12.0, 13.0, 13.0, 13.0, 12.5, 11.5, 9.5, 7.5, 6.5,
];

/// State of the sky, from clear to overcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sky {
    Clear,
    /// A few fair-weather cumulus.
    Fair,
    PartlyCloudy,
    Cloudy,
    Overcast,
}

impl Sky {
    pub const ALL: [Self; 5] = [
        Self::Clear,
        Self::Fair,
        Self::PartlyCloudy,
        Self::Cloudy,
        Self::Overcast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Fair => "fair",
            Self::PartlyCloudy => "partly cloudy",
            Self::Cloudy => "cloudy",
            Self::Overcast => "overcast",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.to_ascii_lowercase().replace(['-', '_'], " ");
        Self::ALL.into_iter().find(|k| k.name() == s)
    }

    fn index(self) -> usize {
        self as usize
    }

    /// Cover of each cloud layer ([`CloudKind`] order); for cumulus, the cover with
    /// convection fully going.
    fn layers(self) -> [f64; 4] {
        [
            [0.04, 0.0, 0.0, 0.08],
            [0.3, 0.0, 0.05, 0.2],
            [0.5, 0.12, 0.25, 0.35],
            [0.35, 0.55, 0.55, 0.3],
            [0.0, 0.97, 0.7, 0.2],
        ][self.index()]
    }

    /// Instability of the air, 0..1: how deep cumulus grow.
    fn instability(self) -> f64 {
        [0.15, 0.3, 0.55, 0.45, 0.1][self.index()]
    }

    /// Relative humidity at the day's mean temperature.
    fn humidity(self) -> f64 {
        [0.5, 0.55, 0.62, 0.7, 0.82][self.index()]
    }

    /// Mean temperature relative to the month's, K: clear days are warmer.
    fn warmth(self) -> f64 {
        [1.5, 1.0, 0.0, -1.0, -2.5][self.index()]
    }

    /// Sea-level pressure, hPa: high pressure brings fair weather.
    fn pressure(self) -> f64 {
        [1024.0, 1019.0, 1014.0, 1009.0, 1004.0][self.index()]
    }

    /// Mean wind 10 m above ground, m/s.
    pub fn wind(self) -> f64 {
        [2.0, 3.0, 4.0, 5.5, 6.5][self.index()]
    }
}

/// How the weather starts and changes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherSettings {
    pub sky: Sky,
    /// Whether the sky changes by itself.
    pub dynamic: bool,
    /// Time of day at the start, h (local solar time).
    pub hour: f64,
    /// Month, 1..=12.
    pub month: u32,
    /// Weather seconds per simulated second.
    pub time_scale: f64,
    /// Added to the air temperature the month and sky give, K.
    pub temperature_offset: f64,
    /// Latitude of the circuit, degrees north.
    pub latitude: f64,
    /// Seed of the cloud pattern and the sky's changes.
    #[serde(skip)]
    pub seed: u64,
}

impl Default for WeatherSettings {
    fn default() -> Self {
        Self {
            sky: Sky::Fair,
            dynamic: true,
            hour: 14.0,
            month: 6,
            time_scale: 1.0,
            temperature_offset: 0.0,
            latitude: 48.0,
            seed: 1,
        }
    }
}

impl WeatherSettings {
    /// Day of the year in the middle of the month.
    pub fn day_of_year(&self) -> u32 {
        let m = self.month.clamp(1, 12) as usize;
        [15, 46, 74, 105, 135, 166, 196, 227, 258, 288, 319, 349][m - 1]
    }
}

/// The air where the car is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Air {
    /// °C.
    pub temperature: f64,
    /// kg/m³.
    pub density: f64,
    /// Velocity of the wind at the car's height, m/s (Z up).
    pub wind: DVec3,
    /// Full-throttle engine torque relative to standard conditions (SAE J1349).
    pub engine: f64,
    /// Pressure, hPa.
    pub pressure: f64,
}

/// Sunlight at the track, W/m².
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sunlight {
    /// Unit vector towards the sun (Z up).
    pub direction: DVec3,
    /// Direct sunlight on a surface facing the sun, where no cloud is in the way.
    pub direct: f64,
    /// Diffuse daylight from the whole sky on level ground, with the clouds' share.
    pub diffuse: f64,
    /// Mean daylight on level ground, direct and diffuse, with the cloud cover.
    pub global: f64,
    /// Diffuse and global daylight under a clear sky.
    pub clear_diffuse: f64,
}

/// The cloud layers, from low to high.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudKind {
    Cumulus,
    /// Stratus, or stratocumulus while the deck is broken.
    Stratus,
    /// Altostratus, or altocumulus while the sheet is broken.
    Middle,
    Cirrus,
}

impl CloudKind {
    pub const ALL: [Self; 4] = [Self::Cumulus, Self::Stratus, Self::Middle, Self::Cirrus];
}

/// One layer of cloud: where it is and how it looks, for the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudLayer {
    pub kind: CloudKind,
    /// Share of the sky under this layer's cloud.
    pub cover: f64,
    /// Potential threshold (see [`cover_threshold`]) of the regions where the layer
    /// forms, and the cell threshold (see [`cell_threshold`]) within them.
    pub threshold: f64,
    pub cell_threshold: f64,
    /// Height of the cloud base and of the highest tops above the track's lowest
    /// point, m.
    pub base: f64,
    pub top: f64,
    /// Shift of the layer's pattern with the wind, m (world X, Y).
    pub offset: DVec2,
    /// Size of the layer's pattern relative to the cloud map.
    pub scale: f64,
    /// Weights of the map's two patterns.
    pub weights: [f64; 2],
    /// Extinction per m of cloud at full density.
    pub extinction: f64,
}

impl CloudLayer {
    /// Cloud density (0..1) of the layer's pattern over the world point (x, y), m.
    pub fn density(&self, map: &CloudMap, x: f64, y: f64) -> f64 {
        let at = (DVec2::new(x, y) - self.offset) / self.scale;
        let (potential, cell) = map.sample(at.x, at.y, self.weights);
        cloud_density(potential, self.threshold) * cell_density(cell, self.cell_threshold)
    }
}

#[derive(Clone, Debug)]
pub struct Weather {
    pub settings: WeatherSettings,
    /// Weather time since midnight of the first day, s.
    time: f64,
    /// Simulated time, s, for gusts that do not speed up with the weather's clock.
    clock: f64,
    /// Sky state the weather is heading for.
    regime: Sky,
    /// Cover of each cloud layer ([`CloudKind`] order).
    cover: [f64; 4],
    /// Depth of the cumulus where they are densest, m.
    cumulus_depth: f64,
    /// Air temperature at the track's lowest point, °C.
    air: f64,
    dew_point: f64,
    /// Sea-level pressure, hPa.
    pressure: f64,
    /// 10 m wind speed, m/s, and the direction it blows towards, rad from +X.
    wind_speed: f64,
    wind_heading: f64,
    /// Shift of each cloud layer with its wind, m.
    offsets: [DVec2; 4],
    rng: u64,
    /// Simulated time since the last tick, s.
    since_tick: f64,
    /// Ticks since the road's temperature was updated, and the weather time they
    /// covered, s.
    road_ticks: u32,
    road_time: f64,
    /// Set at each tick: each cloud layer's wind, m/s, and the surface wind's direction.
    winds: [DVec2; 4],
    wind_direction: DVec2,
    // Set at each tick.
    density: f64,
    engine: f64,
    clouds: Option<Arc<CloudMap>>,
    volume: Option<CloudVolume>,
    generation: u64,
    road: Option<RoadTemperature>,
    /// Road temperature without a modelled road ([`Self::steady`]), °C.
    road_uniform: f64,
    scenery: Option<Arc<Occluders>>,
}

impl Weather {
    /// Fixed standard conditions: 25 °C air and road, 1.225 kg/m³, no wind, no clouds.
    pub const STANDARD: Self = Self {
        settings: WeatherSettings {
            sky: Sky::Clear,
            dynamic: false,
            hour: 12.0,
            month: 6,
            time_scale: 0.0,
            temperature_offset: 0.0,
            latitude: 48.0,
            seed: 0,
        },
        time: 12.0 * 3600.0,
        clock: 0.0,
        regime: Sky::Clear,
        cover: [0.0; 4],
        cumulus_depth: 0.0,
        air: AMBIENT_TEMPERATURE,
        dew_point: 10.0,
        pressure: STANDARD_PRESSURE,
        wind_speed: 0.0,
        wind_heading: 0.0,
        offsets: [DVec2::ZERO; 4],
        rng: 0,
        since_tick: 0.0,
        road_ticks: 0,
        road_time: 0.0,
        winds: [DVec2::ZERO; 4],
        wind_direction: DVec2::X,
        density: AIR_DENSITY,
        engine: 1.0,
        clouds: None,
        volume: None,
        generation: 0,
        road: None,
        road_uniform: AMBIENT_TEMPERATURE,
        scenery: None,
    };

    /// No sky and no modelled road: air at `air` °C and sea-level `pressure` hPa, the
    /// whole road at `road` °C, and a 10 m wind of `wind_speed` m/s blowing towards
    /// `wind_heading` (rad from +X) with gusts drawn from `seed`. For training over a
    /// range of conditions at little cost.
    pub fn steady(
        air: f64,
        road: f64,
        pressure: f64,
        wind_speed: f64,
        wind_heading: f64,
        seed: u64,
    ) -> Self {
        let mut weather = Self {
            settings: WeatherSettings {
                seed,
                ..Self::STANDARD.settings
            },
            air,
            dew_point: air.min(Self::STANDARD.dew_point),
            pressure,
            wind_speed,
            wind_heading,
            wind_direction: DVec2::from_angle(wind_heading),
            road_uniform: road,
            ..Self::STANDARD
        };
        weather.update_air();
        weather
    }

    /// Weather over `track`, whose road `scenery` may shade, started from `settings`.
    pub fn new(
        track: &crate::Track,
        scenery: Option<Arc<Occluders>>,
        settings: WeatherSettings,
    ) -> Self {
        let road = RoadTemperature::new(
            track,
            scenery.as_deref(),
            settings.day_of_year(),
            settings.latitude,
            AMBIENT_TEMPERATURE,
        );
        let mut weather = Self {
            settings,
            clouds: Some(Arc::new(CloudMap::new(settings.seed))),
            road: Some(road),
            scenery,
            ..Self::STANDARD
        };
        weather.restart(settings);
        weather
    }

    /// Starts over from `settings`: the sky in its chosen state, the air and the road
    /// as a day of that weather leaves them.
    pub fn restart(&mut self, settings: WeatherSettings) {
        if self.road.is_none() {
            return;
        }
        if self
            .clouds
            .as_ref()
            .is_none_or(|map| map.seed != settings.seed)
        {
            self.clouds = Some(Arc::new(CloudMap::new(settings.seed)));
        }
        if let Some(road) = &self.road {
            let e = road.exposure();
            if e.day_of_year != settings.day_of_year() || e.latitude != settings.latitude {
                self.road = Some(road.with_sun_path(
                    self.scenery.as_deref(),
                    settings.day_of_year(),
                    settings.latitude,
                ));
            }
        }
        self.settings = settings;
        let sky = settings.sky;
        self.rng = clouds::hash(settings.seed ^ 0xA076_1D64_78BD_642F);
        self.regime = sky;
        self.cover = sky.layers();
        self.pressure = sky.pressure();
        self.wind_speed = sky.wind();
        self.wind_heading = self.uniform() * std::f64::consts::TAU;
        for i in 0..4 {
            self.offsets[i] = DVec2::new(self.uniform(), self.uniform()) * CLOUD_MAP_PERIOD;
        }
        self.dew_point = dew_point(self.mean_temperature(), sky.humidity());
        self.since_tick = 0.0;
        self.road_ticks = 0;
        self.road_time = 0.0;
        self.clock = 0.0;
        // A day and a half of this weather before the start, in 2-minute steps, sets
        // the road's temperature, which lags the sun by hours.
        let start = settings.hour.rem_euclid(24.0) * 3600.0;
        let spin = 36.0 * 3600.0;
        let step = 120.0;
        self.time = start - spin;
        self.air = self.air_target();
        let ground = self.ground_temperature();
        if let Some(road) = &mut self.road {
            road.fill(self.air + 3.0, ground);
        }
        while self.time < start {
            self.time += step;
            self.cover = self.cover_target();
            self.cumulus_depth = self.cumulus_depth_target();
            self.air = self.air_target();
            let clear = 1.0 - self.total_cover();
            let forcing = self.forcing();
            if let Some(road) = &mut self.road {
                road.step(&forcing, |_| clear, step);
            }
        }
        self.time = start;
        self.cover = self.cover_target();
        self.cumulus_depth = self.cumulus_depth_target();
        self.air = self.air_target();
        self.update_air();
        self.update_winds();
        self.generation = self.generation.wrapping_add(1);
        self.volume = self
            .clouds
            .as_ref()
            .map(|map| CloudVolume::new(map, &self.volume_forcing(), self.time));
    }

    /// Caches what the physics steps need between ticks.
    fn update_winds(&mut self) {
        self.winds = std::array::from_fn(|i| self.layer_wind(i));
        self.wind_direction = DVec2::from_angle(self.wind_heading);
    }

    /// Advances the weather by one physics step of `dt` s.
    pub fn step(&mut self, dt: f64) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        // Without a modelled road only the gusts move on.
        self.clock += dt;
        if self.road.is_none() {
            return;
        }
        let scale = self.settings.time_scale;
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let mut remaining = dt * scale.min(120.0);
        while remaining > 1e-12 {
            let part = remaining.min(TICK - self.since_tick);
            self.time += part;
            for (offset, wind) in self.offsets.iter_mut().zip(self.winds) {
                // Never wrap the shared displacement. Each texture wraps at its own
                // period when converted to GPU coordinates.
                *offset += wind * part;
            }
            self.since_tick += part;
            remaining -= part;
            if self.since_tick >= TICK - 1e-9 {
                self.since_tick = 0.0;
                self.tick(TICK);
                let forcing = self.volume_forcing();
                if let Some(volume) = &mut self.volume {
                    volume.advance(&forcing);
                }
            }
        }
    }

    fn volume_forcing(&self) -> VolumeForcing {
        let mut layers = self.cloud_layers();
        // Synoptic moisture regions translate, but never morph between noise
        // maps. The transported thermodynamic state drives cloud evolution.
        for layer in &mut layers[..3] {
            layer.weights = [1.0, 0.0];
        }
        VolumeForcing {
            temperature: self.air as f32,
            dew_point: self.dew_point as f32,
            pressure: self.pressure as f32,
            sunlight: self.sunlight().global as f32,
            winds: self.winds,
            layers,
        }
    }

    pub fn cloud_snapshot(&self) -> Option<CloudSnapshot<'_>> {
        self.volume.as_ref().map(|v| CloudSnapshot {
            previous: &v.previous,
            current: &v.current,
            time: (self.time - TICK).max(v.previous.time),
            generation: self.generation,
        })
    }

    pub fn weather_time(&self) -> f64 {
        self.time
    }

    pub fn cloud_generation(&self) -> u64 {
        self.generation
    }

    pub fn cloud_winds(&self) -> [DVec2; 4] {
        self.winds
    }

    /// Updates the slow parts over `dt` s of weather time.
    fn tick(&mut self, dt: f64) {
        if dt <= 0.0 {
            return;
        }
        if self.settings.dynamic && self.uniform() < dt / SKY_DWELL {
            // Mostly back towards the chosen sky, sometimes further away.
            let i = self.regime.index() as isize;
            let home = self.settings.sky.index() as isize;
            let toward = if home == i {
                if self.uniform() < 0.5 { 1 } else { -1 }
            } else {
                (home - i).signum()
            };
            let step = if self.uniform() < 0.7 {
                toward
            } else {
                -toward
            };
            self.regime = Sky::ALL[(i + step).clamp(0, 4) as usize];
        }
        if self.settings.dynamic {
            self.wind_heading += WIND_VEER * dt.sqrt() * self.normal();
        }
        let r = self.regime;
        let follow =
            |x: &mut f64, target: f64, lag: f64| *x += (target - *x) * (1.0 - (-dt / lag).exp());
        let cover = self.cover_target();
        for i in 0..4 {
            follow(&mut self.cover[i], cover[i], LAYER_LAG[i]);
        }
        let depth = self.cumulus_depth_target();
        follow(&mut self.cumulus_depth, depth, LAYER_LAG[0]);
        follow(&mut self.pressure, r.pressure(), PRESSURE_LAG);
        follow(&mut self.wind_speed, r.wind(), WIND_LAG);
        let dew = dew_point(self.mean_temperature(), r.humidity());
        follow(&mut self.dew_point, dew, HUMIDITY_LAG);
        let air = self.air_target();
        follow(&mut self.air, air, AIR_LAG);
        self.update_air();
        self.update_winds();
        self.road_ticks += 1;
        self.road_time += dt;
        if self.road_ticks < ROAD_TICKS {
            return;
        }
        let dt = std::mem::take(&mut self.road_time);
        self.road_ticks = 0;
        let forcing = self.forcing();
        let layers = self.cloud_layers();
        let snapshot = self.volume.as_ref().map(|v| CloudSnapshot {
            previous: &v.previous,
            current: &v.current,
            time: (self.time - TICK).max(v.previous.time),
            generation: self.generation,
        });
        if let Some(road) = &mut self.road {
            let sampler = CloudShadowSampler {
                map: self.clouds.as_deref(),
                snapshot,
                layers,
                sun: forcing.sun,
                base: road.base_height(),
            };
            road.step(&forcing, |p| sampler.transmittance(p), dt);
        }
    }

    fn update_air(&mut self) {
        let t = self.air + KELVIN;
        let vapour = vapour_pressure(self.dew_point.min(self.air));
        let dry = self.pressure - vapour;
        self.density = (dry * 100.0 / (R_DRY * t)) + (vapour * 100.0 / (R_VAPOUR * t));
        self.engine = dry / STANDARD_PRESSURE * ((AMBIENT_TEMPERATURE + KELVIN) / t).sqrt();
    }

    /// Uniform in [0, 1).
    fn uniform(&mut self) -> f64 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        (clouds::hash(self.rng) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box–Muller).
    fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform().max(1e-12), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }

    /// The day's mean temperature for the month, the sky and the offset, °C.
    fn mean_temperature(&self) -> f64 {
        let m = self.settings.month.clamp(1, 12) as usize - 1;
        MONTH_TEMPERATURE[m] + self.regime.warmth() + self.settings.temperature_offset
    }

    /// Air temperature the time of day and the cloud cover call for, °C: coldest before
    /// dawn, warmest mid-afternoon; cloud damps the swing.
    fn air_target(&self) -> f64 {
        let m = self.settings.month.clamp(1, 12) as usize - 1;
        let amplitude = 0.5 * MONTH_DAILY_RANGE[m] * (1.0 - 0.65 * self.total_cover());
        let hour = self.hour();
        let phase = (hour - 15.0) / 24.0 * std::f64::consts::TAU;
        // Sharper warming in the morning than cooling in the evening.
        let wave = phase.cos() + 0.15 * (2.0 * phase).cos();
        self.mean_temperature() + amplitude * wave / 1.15
    }

    /// Temperature of the ground half a metre down: the mean of the last days, °C.
    fn ground_temperature(&self) -> f64 {
        self.mean_temperature() + 2.0
    }

    fn forcing(&self) -> Forcing {
        let light = self.sunlight();
        let t = self.air + KELVIN;
        let e = vapour_pressure(self.dew_point.min(self.air));
        // Clear sky (Brutsaert), and cloud radiating close to a black body at the air's
        // temperature.
        let clear = 1.24 * (e / t).powf(1.0 / 7.0);
        // Low cloud radiates most, cirrus hardly: weigh the layers by their heights'
        // temperatures and opacity.
        let [cu, st, mid, ci] = self.cover;
        let cloud =
            1.0 - (1.0 - 0.95 * st) * (1.0 - 0.85 * cu) * (1.0 - 0.6 * mid) * (1.0 - 0.1 * ci);
        let emissivity = clear + (1.0 - clear) * cloud;
        Forcing {
            sun: light.direction,
            hour: self.hour(),
            direct: light.direct,
            diffuse: light.diffuse,
            air: self.air,
            sky_longwave: emissivity * 5.670_374e-8 * t.powi(4),
            wind: self.wind_speed * NEAR_GROUND_WIND,
            ground: self.ground_temperature(),
        }
    }

    /// Time of day, h.
    pub fn hour(&self) -> f64 {
        (self.time / 3600.0).rem_euclid(24.0)
    }

    /// Days passed since the start.
    pub fn day(&self) -> i64 {
        (self.time / 86400.0).floor() as i64
    }

    /// The sky state the weather is heading for.
    pub fn regime(&self) -> Sky {
        self.regime
    }

    /// Share of the sky under cloud, all layers together, counting thin cloud for less.
    pub fn cloud_cover(&self) -> f64 {
        self.total_cover()
    }

    fn total_cover(&self) -> f64 {
        1.0 - (0..4)
            .map(|i| 1.0 - self.cover[i] * LAYER_OPACITY[i])
            .product::<f64>()
    }

    /// Cover each layer is heading for: cumulus with the day's convection, which a low
    /// deck shuts off by shading the ground.
    fn cover_target(&self) -> [f64; 4] {
        let mut cover = self.regime.layers();
        cover[0] *= self.convection() * (1.0 - 0.8 * cover[1]);
        cover
    }

    /// How far convection is going, 0..1: it follows the sunshine an hour late.
    fn convection(&self) -> f64 {
        let hour = self.hour() - CONVECTION_DELAY;
        let sun = sun_direction(self.settings.latitude, self.settings.day_of_year(), hour);
        let (direct, diffuse) = clear_sky(sun.z);
        let light = direct * sun.z.max(0.0) + diffuse;
        let (lo, hi) = CONVECTION_LIGHT;
        let u = ((light - lo) / (hi - lo)).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    }

    /// Depth cumulus grow to where they are densest: shallow fair-weather cumulus in
    /// stable air, towering congestus in unstable air with strong convection, m.
    fn cumulus_depth_target(&self) -> f64 {
        let i = self.regime.instability();
        // Humilis a few hundred metres deep, mediocris about as deep as wide, congestus
        // kilometres.
        (250.0 + 2800.0 * i * i) * (0.4 + 0.6 * self.convection())
    }

    /// Height of the lifting condensation level above the track, m: where rising air
    /// cools to its dew point.
    pub fn condensation_level(&self) -> f64 {
        (LCL_PER_KELVIN * (self.air - self.dew_point).max(0.0)).clamp(LCL_RANGE.0, LCL_RANGE.1)
    }

    /// Cover of each layer ([`CloudKind`] order).
    pub fn layer_cover(&self) -> [f64; 4] {
        self.cover
    }

    /// Air temperature at the track's lowest point, °C.
    pub fn air_temperature(&self) -> f64 {
        self.air
    }

    pub fn relative_humidity(&self) -> f64 {
        (vapour_pressure(self.dew_point.min(self.air)) / vapour_pressure(self.air)).min(1.0)
    }

    /// Sea-level pressure, hPa.
    pub fn pressure(&self) -> f64 {
        self.pressure
    }

    pub fn air_density(&self) -> f64 {
        self.density
    }

    /// 10 m wind speed, m/s, and the compass direction it blows from, degrees (0 = from
    /// north, +Y).
    pub fn wind(&self) -> (f64, f64) {
        let from = self.wind_heading + std::f64::consts::PI;
        let compass = 90.0 - from.to_degrees();
        (self.wind_speed, compass.rem_euclid(360.0))
    }

    /// Wind at the height of cloud layer `i`, m/s (world X, Y).
    fn layer_wind(&self, i: usize) -> DVec2 {
        let speed = (self.wind_speed * LAYER_WIND[i]).max(LAYER_WIND_MIN[i]);
        // Veering with height, as in the northern hemisphere's Ekman spiral.
        DVec2::from_angle(self.wind_heading - LAYER_VEER[i]) * speed
    }

    /// The air at `position`, with the gusts of the moment.
    #[inline]
    pub fn air_at(&self, position: DVec3) -> Air {
        let wind = if self.wind_speed > 0.0 {
            let gust = smooth_noise(self.clock / GUST_TIME, self.settings.seed);
            let veer = smooth_noise(self.clock / GUST_TIME + 17.3, self.settings.seed ^ 1);
            let speed = self.wind_speed * NEAR_GROUND_WIND * (1.0 + GUSTINESS * gust).max(0.0);
            // Veered by a small angle: the direction plus that much of its perpendicular.
            let dir = self.wind_direction + self.wind_direction.perp() * (GUST_VEER * veer);
            (dir * speed).extend(0.0)
        } else {
            DVec3::ZERO
        };
        let temperature = match &self.road {
            Some(road) => self.air - LAPSE_RATE * (position.z - road.base_height()),
            None => self.air,
        };
        Air {
            temperature,
            density: self.density,
            wind,
            engine: self.engine,
            pressure: self.pressure,
        }
    }

    /// Road surface temperature at track coordinates (s, d), °C.
    #[inline]
    pub fn road_temperature(&self, s: f64, d: f64) -> f64 {
        match &self.road {
            Some(road) => road.at(s, d),
            None => self.road_uniform,
        }
    }

    pub fn road(&self) -> Option<&RoadTemperature> {
        self.road.as_ref()
    }

    pub fn cloud_map(&self) -> Option<&CloudMap> {
        self.clouds.as_deref()
    }

    /// Unit vector towards the sun (Z up; +Y is north, +X east).
    pub fn sun_direction(&self) -> DVec3 {
        sun_direction(
            self.settings.latitude,
            self.settings.day_of_year() + self.day().rem_euclid(365) as u32,
            self.hour(),
        )
    }

    /// Sunlight now, with the cloud cover.
    pub fn sunlight(&self) -> Sunlight {
        let direction = self.sun_direction();
        let (direct, clear_diffuse) = clear_sky(direction.z);
        let horizontal = direct * direction.z.max(0.0);
        let clear_global = horizontal + clear_diffuse;
        let c = self.total_cover();
        // Kasten–Czeplak for the mean; what the clouds take from the direct beam mostly
        // comes back scattered.
        let global = clear_global * (1.0 - 0.75 * c.powf(3.4));
        let diffuse = (global - horizontal * (1.0 - c)).max(clear_diffuse);
        Sunlight {
            direction,
            direct,
            diffuse,
            global,
            clear_diffuse,
        }
    }

    /// The cloud layers now, from low to high.
    pub fn cloud_layers(&self) -> [CloudLayer; 4] {
        let angle = self.time / CLOUD_MORPH_PERIOD * std::f64::consts::TAU;
        let lcl = self.condensation_level();
        let [_, st, mid, _] = self.cover;
        // Stratus under a moist layer is low; the deck thickens as it closes.
        let stratus_base = (0.6 * lcl).clamp(250.0, 900.0);
        let heights = [
            (lcl, lcl + self.cumulus_depth),
            (stratus_base, stratus_base + 300.0 + 400.0 * st),
            (3200.0, 3200.0 + 400.0 + 900.0 * mid),
            (8500.0, 9300.0),
        ];
        std::array::from_fn(|i| {
            let a = angle + LAYER_PHASE[i];
            let cover = self.cover[i];
            let regions = (cover * LAYER_SPREAD[i]).min(0.97);
            CloudLayer {
                kind: CloudKind::ALL[i],
                cover,
                threshold: cover_threshold(regions),
                cell_threshold: cell_threshold(cover / regions.max(1e-6)),
                base: heights[i].0,
                top: heights[i].1,
                offset: self.offsets[i],
                scale: LAYER_SCALE[i],
                weights: [a.cos(), a.sin()],
                extinction: LAYER_EXTINCTION[i],
            }
        })
    }

    /// Share of direct sunlight the clouds let through to `point`.
    pub fn sun_transmittance(&self, point: DVec3) -> f64 {
        self.cloud_shadow_sampler().transmittance(point)
    }

    /// Cache the common sun/layer state when sampling many shadow rays.
    pub fn cloud_shadow_sampler(&self) -> CloudShadowSampler<'_> {
        CloudShadowSampler {
            map: self.cloud_map(),
            snapshot: self.cloud_snapshot(),
            layers: self.cloud_layers(),
            sun: self.sun_direction(),
            base: self.road.as_ref().map_or(0.0, RoadTemperature::base_height),
        }
    }
}

#[derive(Clone, Copy)]
pub struct CloudShadowSampler<'a> {
    map: Option<&'a CloudMap>,
    snapshot: Option<CloudSnapshot<'a>>,
    layers: [CloudLayer; 4],
    sun: DVec3,
    base: f64,
}

impl<'a> CloudShadowSampler<'a> {
    pub fn with_snapshot(mut self, snapshot: CloudSnapshot<'a>) -> Self {
        self.snapshot = Some(snapshot);
        self
    }

    pub fn transmittance(&self, point: DVec3) -> f64 {
        self.map.map_or(1.0, |map| {
            sun_through_clouds(
                map,
                &self.layers,
                self.sun,
                point,
                self.base,
                self.snapshot.is_some(),
            )
        }) * self.snapshot.map_or(1.0, |s| {
            s.sun_transmittance(point - DVec3::Z * self.base, self.sun)
        })
    }
}

/// Share of sunlight along `sun` that the cloud layers let through to `point`; the
/// layers' heights are above `base`.
fn sun_through_clouds(
    map: &CloudMap,
    layers: &[CloudLayer; 4],
    sun: DVec3,
    point: DVec3,
    base: f64,
    cirrus_only: bool,
) -> f64 {
    if sun.z <= 0.01 {
        return 0.0;
    }
    let slant = (1.0 / sun.z).min(MAX_SLANT);
    let mut depth = 0.0;
    for layer in layers
        .iter()
        .filter(|l| l.cover > 1e-3 && (!cirrus_only || l.kind == CloudKind::Cirrus))
    {
        let middle = base + 0.5 * (layer.base + layer.top);
        let rise = (middle - point.z).max(0.0);
        let at = point.truncate() + sun.truncate() * (rise / sun.z);
        let density = layer.density(map, at.x, at.y);
        // Heaps are as deep as the layer only where they are densest; sheets are as
        // thick as the layer wherever they are.
        let thickness = (layer.top - layer.base)
            * if layer.kind == CloudKind::Cumulus {
                density
            } else {
                1.0
            };
        depth += layer.extinction * density * thickness;
    }
    (-depth * slant).exp()
}

/// Unit vector towards the sun (Z up, +Y north, +X east) at `latitude` degrees on
/// `day_of_year` at local solar time `hour`.
pub fn sun_direction(latitude: f64, day_of_year: u32, hour: f64) -> DVec3 {
    let declination = 23.44f64.to_radians()
        * ((284.0 + day_of_year as f64) / 365.0 * std::f64::consts::TAU).sin();
    let hour_angle = (hour - 12.0) / 24.0 * std::f64::consts::TAU;
    let phi = latitude.to_radians();
    let (sd, cd) = declination.sin_cos();
    let (sp, cp) = phi.sin_cos();
    let (sh, ch) = hour_angle.sin_cos();
    DVec3::new(-cd * sh, cp * sd - sp * cd * ch, sp * sd + cp * cd * ch).normalize()
}

/// Clear-sky direct normal and diffuse horizontal sunlight for the sun at elevation
/// sine `sin_elevation`, W/m². A little diffuse light remains in the twilight.
pub fn clear_sky(sin_elevation: f64) -> (f64, f64) {
    let elevation = sin_elevation.clamp(-1.0, 1.0).asin().to_degrees();
    if elevation <= -6.0 {
        return (0.0, 0.0);
    }
    if elevation <= 0.5 {
        // Civil twilight: fades from the light at sunrise to none at −6°.
        let (_, at_horizon) = clear_sky((0.5f64).to_radians().sin() + 1e-9);
        let u = (elevation + 6.0) / 6.5;
        return (0.0, at_horizon * u * u);
    }
    let s = sin_elevation;
    // Kasten–Young air mass, Meinel beam, Haurwitz global. Haurwitz fades too early near
    // the horizon, where the sky still gives about 4 W/m² (some 500 lux) with the sun on
    // it, and more as it rises.
    let air_mass = 1.0 / (s + 0.50572 * (elevation + 6.07995).powf(-1.6364));
    let direct = SOLAR_CONSTANT * 0.7f64.powf(air_mass.powf(0.678));
    let global = 1098.0 * s * (-0.057 / s).exp();
    let low_sky = TWILIGHT_SKY * (elevation.min(6.0) / 3.0).exp();
    (direct, (global - direct * s).max(0.1 * global).max(low_sky))
}

/// Saturation vapour pressure over water at `t` °C, hPa (Magnus).
fn vapour_pressure(t: f64) -> f64 {
    6.1094 * (17.625 * t / (t + 243.04)).exp()
}

/// Dew point of air at `t` °C and relative humidity `rh`, °C.
fn dew_point(t: f64, rh: f64) -> f64 {
    let g = (rh.max(0.01)).ln() + 17.625 * t / (t + 243.04);
    243.04 * g / (17.625 - g)
}

/// Smooth noise in time, about −1..1, from a seed.
fn smooth_noise(t: f64, seed: u64) -> f64 {
    let (i, f) = (t.floor(), t - t.floor());
    let value = |i: f64| {
        let h = clouds::hash(seed ^ (i as i64 as u64).wrapping_mul(0x2545_F491_4F6C_DD1D));
        (h >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    };
    let s = f * f * (3.0 - 2.0 * f);
    value(i) * (1.0 - s) + value(i + 1.0) * s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Track;

    fn track() -> Track {
        Track::from_ron(include_str!("../../../../assets/tracks/lakeside.ron")).unwrap()
    }

    #[test]
    fn steady_weather_holds_its_air_and_road_and_gusts() {
        let mut w = Weather::steady(35.0, 55.0, 1000.0, 6.0, 0.0, 3);
        let at = DVec3::new(100.0, 50.0, 30.0);
        assert_eq!(w.air_at(at).temperature, 35.0);
        assert_eq!(w.road_temperature(10.0, 2.0), 55.0);
        // Hot, low-pressure air is thinner and weakens the engine.
        assert!(w.air_density() < Weather::STANDARD.air_density());
        assert!(w.air_at(at).engine < 1.0);
        let mut speeds = Vec::new();
        for _ in 0..30_000 {
            w.step(0.001);
            speeds.push(w.air_at(at).wind.length());
        }
        let (lo, hi) = speeds
            .iter()
            .fold((f64::MAX, 0.0_f64), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(lo > 0.0 && hi - lo > 0.1, "gusts {lo}..{hi}");
        assert_eq!(w.air_at(at).temperature, 35.0);
        assert_eq!(Weather::STANDARD.air_at(at).wind, DVec3::ZERO);
    }

    fn weather(sky: Sky, hour: f64) -> Weather {
        Weather::new(
            &track(),
            None,
            WeatherSettings {
                sky,
                hour,
                dynamic: false,
                ..Default::default()
            },
        )
    }

    #[test]
    fn the_sun_rises_in_the_east_and_culminates_in_the_south() {
        let morning = sun_direction(48.0, 172, 8.0);
        assert!(morning.x > 0.5 && morning.z > 0.0);
        let noon = sun_direction(48.0, 172, 12.0);
        assert!(noon.y < 0.0 && noon.x.abs() < 1e-9);
        // 90° − 48° + 23.4° at the summer solstice.
        assert!((noon.z.asin().to_degrees() - 65.4).abs() < 0.5);
        assert!(sun_direction(48.0, 172, 0.0).z < 0.0);
    }

    #[test]
    fn clear_sky_sunlight_is_realistic() {
        let (direct, diffuse) = clear_sky(1.0);
        assert!((850.0..1000.0).contains(&direct), "{direct}");
        assert!((50.0..150.0).contains(&diffuse), "{diffuse}");
        let (low, _) = clear_sky(5f64.to_radians().sin());
        assert!(low < 0.5 * direct);
    }

    #[test]
    fn sunlit_road_is_hot_in_the_afternoon_and_cools_at_night() {
        let day = weather(Sky::Clear, 14.0);
        let (road, _, _) = day.road().unwrap().stats();
        let air = day.air_temperature();
        assert!(road > air + 12.0, "road {road} air {air}");
        let night = weather(Sky::Clear, 4.0);
        let (road, _, _) = night.road().unwrap().stats();
        let air = night.air_temperature();
        assert!(road < air + 3.0, "road {road} air {air}");
        assert!(night.air_temperature() < day.air_temperature() - 5.0);
    }

    #[test]
    fn overcast_days_keep_the_road_close_to_the_air() {
        let clear = weather(Sky::Clear, 14.0);
        let overcast = weather(Sky::Overcast, 14.0);
        let excess = |w: &Weather| w.road().unwrap().stats().0 - w.air_temperature();
        assert!(excess(&overcast) < 0.5 * excess(&clear));
        assert!(overcast.sunlight().global < 0.4 * clear.sunlight().global);
    }

    #[test]
    fn hot_air_is_thinner_and_weakens_the_engine() {
        let mut cold = weather(Sky::Clear, 14.0);
        cold.settings.temperature_offset = -15.0;
        cold.restart(cold.settings);
        let hot = weather(Sky::Clear, 14.0);
        assert!(cold.air_density() > hot.air_density() * 1.04);
        let at = |w: &Weather| w.air_at(DVec3::ZERO).engine;
        assert!(at(&cold) > at(&hot));
        assert!((1.1..1.3).contains(&hot.air_density()));
    }

    #[test]
    fn the_weather_changes_by_itself_and_replays_exactly() {
        let mut a = Weather::new(
            &track(),
            None,
            WeatherSettings {
                sky: Sky::PartlyCloudy,
                time_scale: 60.0,
                seed: 9,
                ..Default::default()
            },
        );
        // This test exercises the long-term scalar forecast/RNG. The volume has
        // separate, shorter transport and replay tests below.
        a.volume = None;
        let mut b = a.clone();
        let mut regimes = std::collections::HashSet::new();
        for _ in 0..180 {
            a.step(1.0);
            b.step(1.0);
            regimes.insert(a.regime());
        }
        assert!(regimes.len() > 1);
        assert_eq!(a.air_temperature(), b.air_temperature());
        assert_eq!(a.cloud_cover(), b.cloud_cover());
    }

    #[test]
    fn cloud_shadows_block_the_sun() {
        let w = weather(Sky::Cloudy, 13.0);
        let n = 400;
        let shaded = (0..n)
            .map(|i| w.sun_transmittance(DVec3::new(i as f64 * 97.0, i as f64 * 31.0, 0.0)))
            .filter(|&t| t < 0.2)
            .count();
        assert!((n / 4..n).contains(&shaded), "{shaded}");
    }

    #[test]
    fn cumulus_grow_with_the_days_convection_under_the_condensation_level() {
        let cumulus = |hour| {
            let w = weather(Sky::PartlyCloudy, hour);
            let [cu, ..] = w.cloud_layers();
            (cu.cover, cu.base, w.air_temperature() - w.dew_point)
        };
        let (night, _, _) = cumulus(3.0);
        let (morning, _, _) = cumulus(8.0);
        let (afternoon, base, spread) = cumulus(15.0);
        assert!(
            night < 0.02 && morning < afternoon && afternoon > 0.3,
            "{night} {morning} {afternoon}"
        );
        assert!(
            (base - 125.0 * spread).abs() < 1.0,
            "base {base} spread {spread}"
        );
        assert!((600.0..2500.0).contains(&base));
    }

    #[test]
    fn overcast_is_a_low_deck_under_a_mid_level_sheet() {
        let w = weather(Sky::Overcast, 13.0);
        let [cu, st, mid, _] = w.cloud_layers();
        assert!(cu.cover < 0.05 && st.cover > 0.9 && mid.cover > 0.5);
        assert!(st.top < mid.base && st.base < 900.0);
    }

    #[test]
    fn standard_weather_is_fixed() {
        let mut w = Weather::STANDARD;
        w.step(1.0);
        let air = w.air_at(DVec3::new(0.0, 0.0, 100.0));
        assert_eq!(air.temperature, AMBIENT_TEMPERATURE);
        assert_eq!(air.density, AIR_DENSITY);
        assert_eq!(air.wind, DVec3::ZERO);
        assert_eq!(w.road_temperature(10.0, 0.0), AMBIENT_TEMPERATURE);
    }

    #[test]
    fn volume_is_independent_of_frame_rate_and_speed() {
        let initial = weather(Sky::Fair, 14.0);
        let mut reference = initial.clone();
        reference.settings.time_scale = 1.0;
        reference.step(12.0);
        for speed in [1.0, 10.0, 60.0, 120.0] {
            for fps in [30.0, 60.0, 120.0] {
                let mut w = initial.clone();
                w.settings.time_scale = speed;
                for _ in 0..(12.0 / speed * fps) as usize {
                    w.step(1.0 / fps);
                }
                let a = w.cloud_snapshot().unwrap();
                let b = reference.cloud_snapshot().unwrap();
                assert!((a.time - b.time).abs() < 1e-6, "speed={speed}, fps={fps}");
                for (a, b) in a.current.cells.iter().zip(b.current.cells.iter()) {
                    assert!(
                        (*a - *b).abs().max_element() < 2e-5,
                        "speed={speed}, fps={fps}"
                    );
                }
            }
        }
    }

    #[test]
    fn changing_speed_and_restoring_a_snapshot_preserves_the_trajectory() {
        let mut w = weather(Sky::Fair, 14.0);
        w.step(1.3);
        let mut replay = w.clone();
        w.settings.time_scale = 0.0;
        w.step(20.0);
        w.settings.time_scale = 10.0;
        w.step(0.87);
        replay.settings.time_scale = 1.0;
        replay.step(8.7);
        assert_eq!(
            w.cloud_snapshot().unwrap().current.cells,
            replay.cloud_snapshot().unwrap().current.cells
        );
        let mut changed = w.settings;
        changed.seed += 1;
        w.settings = changed; // UI may modify the public settings before restarting.
        w.restart(changed);
        assert_eq!(
            w.cloud_map().unwrap().texels(),
            CloudMap::new(changed.seed).texels()
        );
    }

    #[test]
    fn volume_replay_and_acceleration_have_the_same_weather_clock() {
        let mut slow = weather(Sky::PartlyCloudy, 14.0);
        slow.settings.dynamic = false;
        slow.settings.time_scale = 1.0;
        let mut fast = slow.clone();
        fast.settings.time_scale = 120.0;
        let replay = fast.clone();
        for _ in 0..120 {
            slow.step(0.1);
        }
        fast.step(0.1);
        let a = slow.cloud_snapshot().unwrap();
        let b = fast.cloud_snapshot().unwrap();
        assert!((a.time - b.time).abs() < 1e-6);
        let max_error = a
            .current
            .extinction
            .iter()
            .zip(b.current.extinction.iter())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max);
        assert!(max_error < 1e-6, "{max_error}");
        let mut replay = replay;
        replay.step(0.1);
        assert_eq!(
            replay.cloud_snapshot().unwrap().current.cells,
            b.current.cells
        );
        assert!(
            b.current
                .cells
                .iter()
                .all(|c| c.is_finite() && c.y >= 0.0 && c.z >= 0.0)
        );
    }

    #[test]
    fn pause_restart_and_noise_displacements_are_continuous() {
        let mut w = weather(Sky::Fair, 14.0);
        w.settings.time_scale = 0.0;
        let before = w.weather_time();
        w.step(10.0);
        assert_eq!(w.weather_time(), before);
        w.settings.time_scale = 120.0;
        w.offsets[0] = DVec2::splat(CLOUD_MAP_PERIOD - 0.01);
        let offset = w.offsets[0];
        let wind = w.winds[0];
        w.step(0.01);
        assert!((w.offsets[0] - offset - wind * 1.2).length() < 1e-8);
        let generation = w.cloud_generation();
        let old = w.cloud_snapshot().unwrap().current.cells.clone();
        w.settings.seed += 1;
        // Restart must rebuild the volume even if the caller already changed settings.
        w.restart(w.settings);
        assert_ne!(w.cloud_generation(), generation);
        assert_ne!(w.cloud_snapshot().unwrap().current.cells, old);
    }

    #[test]
    fn a_cloudy_forecast_supplies_moisture_over_twenty_minutes() {
        let mut w = weather(Sky::Cloudy, 14.0);
        w.settings.dynamic = false;
        w.settings.time_scale = 120.0;
        let mass = |w: &Weather| {
            w.cloud_snapshot()
                .unwrap()
                .current
                .cells
                .iter()
                .map(|c| c.z as f64)
                .sum::<f64>()
        };
        let initial = mass(&w);
        for _ in 0..100 {
            w.step(0.1);
        }
        let final_mass = mass(&w);
        assert!(
            final_mass > initial * 0.1 && final_mass < initial * 10.0,
            "initial {initial}, final {final_mass}"
        );
    }

    #[test]
    fn dry_forcing_evaporates_clouds_and_sunlight_drives_updrafts() {
        let w = weather(Sky::Fair, 14.0);
        let mut dry = w.volume.clone().unwrap();
        let mut shaded = dry.clone();
        let mut heated = dry.clone();
        let mut f = w.volume_forcing();
        let original = dry.current.cells.iter().map(|c| c.z as f64).sum::<f64>();
        let mut dry_forcing = f;
        dry_forcing.dew_point = -30.0;
        dry_forcing.sunlight = 0.0;
        for layer in &mut dry_forcing.layers {
            layer.cover = 0.0;
        }
        for _ in 0..300 {
            dry.advance(&dry_forcing);
        }
        assert!(dry.current.cells.iter().map(|c| c.z as f64).sum::<f64>() < original * 0.1);
        for _ in 0..60 {
            f.sunlight = 0.0;
            shaded.advance(&f);
            f.sunlight = 650.0;
            heated.advance(&f);
        }
        let updraft = |v: &CloudVolume| {
            v.current
                .cells
                .iter()
                .map(|c| c.w.max(0.0) as f64)
                .sum::<f64>()
        };
        assert!(updraft(&heated) > updraft(&shaded));
    }

    #[test]
    fn fair_and_overcast_forecasts_remain_stable_for_an_hour() {
        for sky in [Sky::Fair, Sky::Overcast] {
            let mut w = weather(sky, 14.0);
            w.settings.time_scale = 120.0;
            let mass = |w: &Weather| {
                w.cloud_snapshot()
                    .unwrap()
                    .current
                    .cells
                    .iter()
                    .map(|c| c.z as f64)
                    .sum::<f64>()
            };
            let initial = mass(&w);
            for _ in 0..300 {
                w.step(0.1);
            }
            let final_mass = mass(&w);
            assert!(
                final_mass > initial * 0.25 && final_mass < initial * 4.0,
                "{sky:?}: initial={initial}, final={final_mass}"
            );
            for c in w.cloud_snapshot().unwrap().current.cells.iter() {
                assert!(c.is_finite() && c.y >= 0.0 && c.z >= 0.0 && c.w.abs() <= 8.0);
                assert!((220.0..370.0).contains(&c.x));
            }
        }
    }
}
