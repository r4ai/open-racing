//! The weather's settings: the sky, time and season the drive starts in, set on the
//! settings screen (Esc, Tab to "weather") and saved between runs, or given by the
//! track. The weather itself is simulated with the car (see `open_racing_sim::weather`)
//! and drawn by `open_racing_sky`, so the clouds drawn are those that shade the road
//! and warm or cool it.

use bevy::prelude::*;
use open_racing_sim::weather::Weather;
use open_racing_sim::{Sky, WeatherSettings};
use open_racing_sky::{SkySettings, WeatherSource};
use open_racing_track::Environment;
use serde::{Deserialize, Serialize};

use crate::Args;
use crate::bindings;
use crate::driving::Simulation;
use crate::graphics::{GraphicsSettings, Level};

const SETTINGS_FILE: &str = "weather.ron";

/// Weather settings kept between runs (the seed is new each run).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeatherConfig {
    pub settings: WeatherSettings,
    /// Tracks that give their own sky and light start in them.
    pub track: bool,
    /// The sky and light the track gives, if it does.
    #[serde(skip)]
    pub environment: Option<Environment>,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            settings: WeatherSettings::default(),
            track: true,
            environment: None,
        }
    }
}

/// `settings` with the sky, time, season, temperature and latitude a track gives.
pub fn with_track(settings: WeatherSettings, e: &Environment) -> WeatherSettings {
    WeatherSettings {
        sky: e.sky,
        dynamic: e.changing,
        hour: e.hour,
        month: e.month,
        temperature_offset: e.temperature,
        latitude: e.latitude.unwrap_or(settings.latitude),
        ..settings
    }
}

impl WeatherConfig {
    /// The saved settings, or the track's sky and light when it gives them, with the
    /// command line's weather and time on top.
    pub fn load(args: &Args, environment: Option<Environment>) -> Self {
        let mut config: Self = bindings::load_config(SETTINGS_FILE);
        config.environment = environment;
        if config.track
            && let Some(e) = &environment
        {
            config.settings = with_track(config.settings, e);
        }
        if let Some(sky) = args.weather.as_deref().and_then(Sky::parse) {
            config.settings.sky = sky;
        }
        if let Some(hour) = args.time {
            config.settings.hour = hour;
        }
        config.settings.seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        // Reproducible captures do not overwrite the user's saved weather.
        if let Ok(seed) = std::env::var("OPEN_RACING_WEATHER_SEED")
            && let Ok(seed) = seed.parse()
        {
            config.settings.seed = seed;
            config.settings.dynamic = false;
        }
        if let Ok(scale) = std::env::var("OPEN_RACING_WEATHER_SPEED")
            && let Ok(scale) = scale.parse::<f64>()
            && scale.is_finite()
            && (0.0..=120.0).contains(&scale)
        {
            config.settings.time_scale = scale;
        }
        config
    }

    pub fn save(&self) {
        bindings::save_config(SETTINGS_FILE, self);
    }
}

/// Parses a time of day such as "14:30" or "14.5" into hours.
pub fn parse_time(s: &str) -> Result<f64, String> {
    let hours = match s.split_once(':') {
        Some((h, m)) => {
            let h: f64 = h.trim().parse().map_err(|_| format!("bad hour in {s}"))?;
            let m: f64 = m
                .trim()
                .parse()
                .map_err(|_| format!("bad minutes in {s}"))?;
            h + m / 60.0
        }
        None => s.trim().parse().map_err(|_| format!("bad time {s}"))?,
    };
    if (0.0..24.0).contains(&hours) {
        Ok(hours)
    } else {
        Err(format!("time {s} is not within the day"))
    }
}

/// Parses a sky state name.
pub fn parse_sky(s: &str) -> Result<String, String> {
    Sky::parse(s).map(|k| k.name().into()).ok_or_else(|| {
        let names: Vec<_> = Sky::ALL.iter().map(|k| k.name()).collect();
        format!("unknown weather {s}; one of: {}", names.join(", "))
    })
}


impl WeatherSource for Simulation {
    fn weather(&self) -> &Weather {
        &self.weather
    }
}

/// Draws the weather as the graphics settings and the track's look ask.
pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(open_racing_sky::SkyPlugin::<Simulation>::default())
            .add_systems(Update, sky_settings);
    }
}

fn sky_settings(
    graphics: Res<GraphicsSettings>,
    weather: Res<WeatherConfig>,
    mut sky: ResMut<SkySettings>,
) {
    let look = weather.environment.unwrap_or_default();
    sky.set_if_neq(SkySettings {
        clouds: graphics.clouds.march(),
        divisor: if graphics.clouds == Level::Low { 4 } else { 2 },
        haze: graphics.haze,
        exposure: look.exposure,
        haze_scale: look.haze,
    });
}
