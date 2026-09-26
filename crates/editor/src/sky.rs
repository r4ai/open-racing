//! The view lit as the project's sky and light say: the sun where it runs at the time,
//! season and latitude set in the World tab, warmer and dimmer near the horizon, the
//! daylight from the sky, the clouds dulling both, the sky's colour behind and the
//! haze in the distance. A preview of what the game renders physically, cheap enough
//! for editing; switched off (the World tab), an even light to work in.

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use glam::DVec3;
use open_racing_sim::Sky;
use open_racing_track::Environment;
use open_racing_track_render::{Wind, to_bevy};

use crate::state::Editor;
use crate::viewport::{EditorCamera, Tool};

/// The sun's light in the view.
#[derive(Component)]
pub struct Sun;

/// How the view is lit: the sun's direction (towards it, Z up), its light and colour,
/// the sky's light and colour, the colour behind everything, and how far one sees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    pub toward_sun: DVec3,
    pub sun_lux: f32,
    pub sun_colour: Vec3,
    pub sky_light: f32,
    pub sky_colour: Vec3,
    pub background: Vec3,
    pub visibility: Option<f32>,
}

/// The even light the view had before it followed the sky.
pub fn neutral() -> Lighting {
    Lighting {
        toward_sun: DVec3::new(0.4, 0.5, 1.0).normalize(),
        sun_lux: 20_000.0,
        sun_colour: Vec3::ONE,
        sky_light: 2500.0,
        sky_colour: Vec3::ONE,
        background: Vec3::new(0.55, 0.7, 0.88),
        visibility: None,
    }
}

/// How much of the sky clouds cover in each state.
fn cover(sky: Sky) -> f32 {
    match sky {
        Sky::Clear => 0.0,
        Sky::Fair => 0.15,
        Sky::PartlyCloudy => 0.4,
        Sky::Cloudy => 0.7,
        Sky::Overcast => 1.0,
    }
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The light of `e` at `latitude` (unless it gives its own).
pub fn lighting(e: &Environment, latitude: f64) -> Lighting {
    let latitude = e.latitude.unwrap_or(latitude);
    let toward_sun = open_racing_sim::weather::sun_direction(latitude, e.day_of_year(), e.hour);
    let up = toward_sun.z as f32;
    let cloud = cover(e.sky);
    let exposure = 2f32.powf(e.exposure as f32);
    // Day comes in over the civil twilight; the sun's light is warm and weak low down,
    // where it passes through more air.
    let day = smooth(-0.1, 0.25, up);
    let high = smooth(0.0, 0.4, up);
    let sun_colour = Vec3::new(1.0, 0.55, 0.3).lerp(Vec3::new(1.0, 0.97, 0.92), high);
    let sun_lux = 24_000.0 * smooth(-0.02, 0.3, up) * (1.0 - 0.9 * cloud) * exposure;
    let sky_light = (150.0 + 2600.0 * day) * (1.0 + 0.25 * cloud) * exposure;
    let blue = Vec3::new(0.45, 0.62, 0.9);
    let grey = Vec3::new(0.62, 0.65, 0.68);
    let dusk = Vec3::new(0.85, 0.58, 0.45);
    let night = Vec3::new(0.02, 0.03, 0.06);
    // The sky reddens only as the sun nears the horizon.
    let low = 1.0 - smooth(-0.05, 0.15, up);
    let lit = blue.lerp(dusk, low).lerp(grey, cloud * 0.9);
    let background = night.lerp(lit, day) * exposure.sqrt();
    let sky_colour = Vec3::new(0.75, 0.85, 1.0).lerp(Vec3::ONE, cloud);
    let visibility = (e.haze > 0.0)
        .then(|| 30_000.0 * (1.0 - 0.3 * cloud) / e.haze as f32)
        .map(|v| v.max(300.0));
    Lighting {
        toward_sun,
        sun_lux,
        sun_colour,
        sky_light,
        sky_colour,
        background,
        visibility,
    }
}

/// The wind plants sway in: the mean wind of the project's sky, from the south-west.
pub fn wind(editor: Res<Editor>, mut wind: ResMut<Wind>) {
    let speed = editor.project.environment.sky.wind() as f32;
    let towards = Vec2::ONE.normalize() * speed;
    wind.set_if_neq(Wind(towards.to_array()));
}

/// Lights the view by the project's sky and light, when they or the switch change.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn light(
    mut commands: Commands,
    editor: Res<Editor>,
    tool: Res<Tool>,
    mut last: Local<Option<Lighting>>,
    mut sun: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    camera: Query<(Entity, Option<&DistanceFog>), With<EditorCamera>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
) {
    let p = &editor.project;
    let now = if tool.overlays.sky {
        lighting(&p.environment, p.geo.map_or(48.0, |g| g.lat))
    } else {
        neutral()
    };
    if *last == Some(now) {
        return;
    }
    *last = Some(now);
    for (mut light, mut t) in &mut sun {
        light.illuminance = now.sun_lux;
        light.color = Color::linear_rgb(now.sun_colour.x, now.sun_colour.y, now.sun_colour.z);
        // Below the horizon it shines up from under the ground: none of it.
        let toward = now
            .toward_sun
            .with_z(now.toward_sun.z.max(0.02))
            .normalize();
        *t = Transform::from_translation(to_bevy(toward)).looking_at(Vec3::ZERO, Vec3::Y);
    }
    ambient.brightness = now.sky_light;
    ambient.color = Color::linear_rgb(now.sky_colour.x, now.sky_colour.y, now.sky_colour.z);
    let bg = now.background;
    clear.0 = Color::srgb(bg.x.min(1.0), bg.y.min(1.0), bg.z.min(1.0));
    for (entity, fog) in &camera {
        match now.visibility {
            Some(v) => {
                commands.entity(entity).insert(DistanceFog {
                    color: clear.0,
                    falloff: FogFalloff::from_visibility(v),
                    ..fog.cloned().unwrap_or_default()
                });
            }
            None if fog.is_some() => {
                commands.entity(entity).remove::<DistanceFog>();
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_light_follows_the_time_the_clouds_and_the_exposure() {
        let noon = Environment {
            hour: 12.0,
            sky: Sky::Clear,
            ..Default::default()
        };
        let a = lighting(&noon, 48.0);
        assert!(
            a.toward_sun.z > 0.5 && a.toward_sun.y < 0.0,
            "{:?}",
            a.toward_sun
        );
        let evening = lighting(&Environment { hour: 19.8, ..noon }, 48.0);
        assert!(evening.sun_lux < 0.5 * a.sun_lux);
        assert!(evening.sun_colour.z < a.sun_colour.z);
        let night = lighting(&Environment { hour: 1.0, ..noon }, 48.0);
        assert_eq!(night.sun_lux, 0.0);
        assert!(night.background.length() < 0.1);
        let overcast = lighting(
            &Environment {
                sky: Sky::Overcast,
                ..noon
            },
            48.0,
        );
        assert!(overcast.sun_lux < 0.2 * a.sun_lux);
        let brighter = lighting(
            &Environment {
                exposure: 1.0,
                ..noon
            },
            48.0,
        );
        assert!((brighter.sun_lux / a.sun_lux - 2.0).abs() < 1e-3);
        // South of the equator the noon sun is in the north.
        assert!(lighting(&noon, -35.0).toward_sun.y > 0.0);
        // No haze: nothing fogs the distance.
        assert!(
            lighting(&Environment { haze: 0.0, ..noon }, 48.0)
                .visibility
                .is_none()
        );
    }
}
