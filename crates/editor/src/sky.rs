//! The view in the project's sky and light, as the game draws them: the same weather
//! (see `open_racing_sim::weather`) started from the World tab's time, season,
//! latitude and sky over the main road, drawn by `open_racing_sky` with its physical
//! atmosphere, sun, sky light, volumetric clouds and their shadows, exposure and haze.
//! The weather stands still as it starts; plants sway in its wind. Switched off (the
//! World tab), a clear noon to work in.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use open_racing_sim::weather::Weather;
use open_racing_sim::{Sky, TrackDef, WeatherSettings};
use open_racing_sky::{SkySettings, WeatherSource};
use open_racing_track::Environment;
use open_racing_track_render::Wind;

use crate::preview::Built;
use crate::state::Editor;
use crate::viewport::{EditorCamera, Tool};

/// The weather the view is drawn in, and what it was started from.
#[derive(Resource)]
pub struct ProjectWeather {
    pub weather: Weather,
    made: Option<Key>,
    task: Option<(Key, Task<Option<Weather>>)>,
}

/// What a weather is started from: the settings, and the height of the main road's
/// lowest point, which the planet lies under.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    settings: WeatherSettings,
    low: f64,
}

impl Default for ProjectWeather {
    fn default() -> Self {
        Self {
            weather: Weather::STANDARD,
            made: None,
            task: None,
        }
    }
}

impl ProjectWeather {
    /// Whether a weather has been started for the project yet.
    pub fn ready(&self) -> bool {
        self.made.is_some()
    }
}

impl WeatherSource for ProjectWeather {
    fn weather(&self) -> &Weather {
        &self.weather
    }
}

/// The weather's settings for the environment `e` at `latitude` (unless it gives its
/// own); a clear noon when the view is not lit by it.
fn settings(e: &Environment, latitude: f64, lit: bool) -> WeatherSettings {
    let e = if lit {
        *e
    } else {
        Environment {
            sky: Sky::Clear,
            hour: 12.5,
            month: 6,
            exposure: 0.0,
            haze: 1.0,
            ..*e
        }
    };
    WeatherSettings {
        sky: e.sky,
        dynamic: false,
        hour: e.hour,
        month: e.month,
        time_scale: 0.0,
        temperature_offset: e.temperature,
        latitude: e.latitude.unwrap_or(latitude),
        seed: 1,
    }
}

/// Starts the weather again, in the background, when the project's sky and light or
/// the main road change.
pub fn weather(
    editor: Res<Editor>,
    tool: Res<Tool>,
    built: Res<Built>,
    mut w: ResMut<ProjectWeather>,
) {
    if let Some((key, task)) = &mut w.task
        && let Some(done) = check_ready(task)
    {
        let key = *key;
        w.task = None;
        if let Some(weather) = done {
            w.weather = weather;
        }
        w.made = Some(key);
    }
    let Some(road) = built.centreline.clone() else {
        return;
    };
    let p = &editor.project;
    let latitude = p.geo.map_or(48.0, |g| g.lat);
    let key = Key {
        settings: settings(&p.environment, latitude, tool.overlays.sky),
        low: lowest(&road),
    };
    // The main road changes with every edit, but moves only the planet under it and
    // the road's warmth: it is followed only when it has risen or sunk a way.
    let stale = w
        .made
        .is_none_or(|m| m.settings != key.settings || (m.low - key.low).abs() > 5.0);
    if !stale || w.task.is_some() {
        return;
    }
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let track = open_racing_sim::Track::new(&road).ok()?;
        Some(Weather::new(&track, None, key.settings))
    });
    w.task = Some((key, task));
}

/// The height of a road's lowest point, m.
fn lowest(road: &TrackDef) -> f64 {
    road.points
        .iter()
        .map(|p| p.pos.2)
        .fold(f64::INFINITY, f64::min)
}

/// Plants sway in the weather's wind, 10 m above the ground.
pub fn wind(w: Res<ProjectWeather>, mut wind: ResMut<Wind>) {
    let (speed, from) = w.weather.wind();
    let a = from.to_radians();
    let towards = [-(a.sin() * speed) as f32, -(a.cos() * speed) as f32];
    wind.set_if_neq(Wind(towards));
}

/// The height below which the view is hazed as the game hazes it, m.
const EYE_LEVEL: f32 = 60.0;

/// Draws the sky as the project's look asks: its exposure and haze, and clouds but in
/// views from straight above (orthographic ones), which they would only hide. Seen
/// from high above, everything is far away, and the game's haze would grey it all:
/// the haze thins with the view's height above the road, to a thirtieth of it.
pub fn look(
    editor: Res<Editor>,
    tool: Res<Tool>,
    w: Res<ProjectWeather>,
    camera: Query<(&Projection, &Transform), With<EditorCamera>>,
    mut sky: ResMut<SkySettings>,
) {
    let e = &editor.project.environment;
    let Ok((projection, t)) = camera.single() else {
        return;
    };
    let ortho = matches!(projection, Projection::Orthographic(_));
    let base = w.weather.road().map_or(0.0, |r| r.base_height()) as f32;
    let height = (t.translation.y - base).max(EYE_LEVEL);
    let lit = tool.overlays.sky;
    sky.set_if_neq(SkySettings {
        clouds: if ortho { (0, 0, 0.0) } else { (48, 4, 1.0) },
        divisor: 2,
        haze: lit && e.haze > 0.0,
        exposure: if lit { e.exposure } else { 0.0 },
        haze_scale: e.haze * (EYE_LEVEL / height).max(0.03) as f64,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_weather_starts_from_the_project_s_sky_and_light() {
        let e = Environment {
            sky: Sky::Overcast,
            hour: 18.25,
            month: 10,
            latitude: Some(-35.0),
            ..Default::default()
        };
        let s = settings(&e, 48.0, true);
        assert_eq!(
            (s.sky, s.hour, s.month, s.latitude),
            (Sky::Overcast, 18.25, 10, -35.0)
        );
        assert!(!s.dynamic);
        // Not lit by it: a clear noon, still at its place.
        let s = settings(&e, 48.0, false);
        assert_eq!((s.sky, s.hour, s.latitude), (Sky::Clear, 12.5, -35.0));
    }
}
