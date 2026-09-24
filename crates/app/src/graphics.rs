//! Image quality: anti-aliasing, shadows and distance haze, set on the settings
//! screen (Esc, Tab to "graphics") and saved between runs. Applies to the window camera;
//! the shadow quality also to the VR eyes.

use bevy::anti_alias::fxaa::Fxaa;
use bevy::anti_alias::smaa::Smaa;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bindings;
use crate::camera::MainCamera;

const SETTINGS_FILE: &str = "graphics.ron";
/// Distance at which the haze hides objects, m, and its colour: the sky at the horizon.
const FOG_VISIBILITY: f32 = 6000.0;
const FOG_COLOR: Color = Color::srgb(0.62, 0.74, 0.86);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum AntiAliasing {
    Off,
    Fxaa,
    Smaa,
    Msaa4,
}

impl AntiAliasing {
    pub const ALL: [Self; 4] = [Self::Off, Self::Fxaa, Self::Smaa, Self::Msaa4];

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Fxaa => "FXAA",
            Self::Smaa => "SMAA",
            Self::Msaa4 => "MSAA 4x",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Shadows {
    Off,
    Low,
    Medium,
    High,
}

impl Shadows {
    pub const ALL: [Self; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Edge of each cascade's shadow map in texels, distance of the first cascade's far
    /// bound and the farthest shadow, m.
    fn quality(self) -> (usize, f32, f32) {
        match self {
            Self::Off | Self::Low => (1024, 8.0, 80.0),
            Self::Medium => (2048, 10.0, 150.0),
            Self::High => (4096, 15.0, 300.0),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Low,
    Medium,
    High,
}

impl Preset {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn settings(self) -> GraphicsSettings {
        match self {
            Self::Low => GraphicsSettings {
                anti_aliasing: AntiAliasing::Fxaa,
                shadows: Shadows::Low,
                fog: false,
            },
            Self::Medium => GraphicsSettings {
                anti_aliasing: AntiAliasing::Msaa4,
                shadows: Shadows::Medium,
                fog: true,
            },
            Self::High => GraphicsSettings {
                anti_aliasing: AntiAliasing::Msaa4,
                shadows: Shadows::High,
                fog: true,
            },
        }
    }
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    pub anti_aliasing: AntiAliasing,
    pub shadows: Shadows,
    /// Haze that fades distant scenery into the sky.
    pub fog: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Preset::High.settings()
    }
}

impl GraphicsSettings {
    pub fn load() -> Self {
        bindings::load_config(SETTINGS_FILE)
    }

    pub fn save(&self) {
        bindings::save_config(SETTINGS_FILE, self);
    }

    /// The preset these settings match, if any.
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.settings() == *self)
    }
}

pub struct GraphicsPlugin;

impl Plugin for GraphicsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GraphicsSettings::load())
            .add_systems(PostStartup, (apply_camera, apply_shadows))
            .add_systems(
                Update,
                (apply_camera, apply_shadows).run_if(resource_changed::<GraphicsSettings>),
            );
    }
}

fn apply_camera(
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    cameras: Query<Entity, With<MainCamera>>,
) {
    for camera in &cameras {
        let mut camera = commands.entity(camera);
        camera
            .remove::<(Fxaa, Smaa)>()
            .insert(match settings.anti_aliasing {
                AntiAliasing::Msaa4 => Msaa::Sample4,
                _ => Msaa::Off,
            });
        match settings.anti_aliasing {
            AntiAliasing::Fxaa => {
                camera.insert(Fxaa::default());
            }
            AntiAliasing::Smaa => {
                camera.insert(Smaa::default());
            }
            AntiAliasing::Off | AntiAliasing::Msaa4 => {}
        }
        if settings.fog {
            camera.insert(DistanceFog {
                color: FOG_COLOR,
                falloff: FogFalloff::from_visibility(FOG_VISIBILITY),
                ..default()
            });
        } else {
            camera.remove::<DistanceFog>();
        }
    }
}

fn apply_shadows(
    settings: Res<GraphicsSettings>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut lights: Query<(&mut DirectionalLight, &mut CascadeShadowConfig)>,
) {
    let (size, first, max) = settings.shadows.quality();
    shadow_map.size = size;
    for (mut light, mut cascades) in &mut lights {
        light.shadow_maps_enabled = settings.shadows != Shadows::Off;
        *cascades = CascadeShadowConfigBuilder {
            first_cascade_far_bound: first,
            maximum_distance: max,
            ..default()
        }
        .build();
    }
}
