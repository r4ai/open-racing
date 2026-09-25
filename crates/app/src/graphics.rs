//! Image quality, set on the settings screen (Esc, Tab to "graphics") and saved between
//! runs: a quality preset, and the details it sets (anti-aliasing, textures, shadows,
//! ambient occlusion, ...) that can also be tuned one by one. Motion blur and VSync are
//! preferences that the presets leave alone. Applies to the window camera; the shadow
//! settings also to the VR eyes.

use bevy::anti_alias::contrast_adaptive_sharpening::ContrastAdaptiveSharpening;
use bevy::anti_alias::fxaa::Fxaa;
use bevy::anti_alias::smaa::Smaa;
use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::core_pipeline::prepass::{DepthPrepass, MotionVectorPrepass, NormalPrepass};
use bevy::light::{
    CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap,
    ShadowFilteringMethod,
};
use bevy::pbr::{
    ContactShadows, DistanceFog, FogFalloff, ScreenSpaceAmbientOcclusion,
    ScreenSpaceAmbientOcclusionQualityLevel,
};
use bevy::post_process::motion_blur::MotionBlur;
use bevy::prelude::*;
use bevy::render::camera::{MipBias, TemporalJitter};
use bevy::render::render_resource::TextureFormat;
use bevy::render::renderer::RenderAdapter;
use bevy::window::{PresentMode, PrimaryWindow};
use serde::{Deserialize, Serialize};

use crate::Args;
use crate::bindings;
use crate::camera::MainCamera;

const SETTINGS_FILE: &str = "graphics.ron";
/// Distance at which the haze hides objects, m, and its colour: the sky at the horizon.
/// The weather sets both each frame.
const FOG_VISIBILITY: f32 = 20000.0;
const FOG_COLOR: Color = Color::srgb(0.62, 0.74, 0.86);
/// A camera that moves farther than this in a frame (m) cut to a new view, and TAA
/// drops the old frames instead of smearing them over the new one.
const CAMERA_CUT: f32 = 10.0;

const SHARPENING_STEPS: [u32; 11] = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
const ANISOTROPY_STEPS: [u16; 5] = [1, 2, 4, 8, 16];
const VIEW_DISTANCE_STEPS: [u32; 5] = [1000, 2000, 3000, 5000, 8000];
/// Shadow map sizes in texels; 0 turns shadows off.
const SHADOW_RESOLUTION_STEPS: [u32; 5] = [0, 512, 1024, 2048, 4096];
const SHADOW_DISTANCE_STEPS: [u32; 8] = [50, 100, 150, 200, 300, 500, 750, 1000];
const SHADOW_CASCADE_STEPS: [u32; 4] = [1, 2, 3, 4];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum AntiAliasing {
    Off,
    Fxaa,
    Smaa,
    Taa,
    Msaa2,
    Msaa4,
    Msaa8,
}

impl AntiAliasing {
    const ALL: [Self; 7] = [
        Self::Off,
        Self::Fxaa,
        Self::Smaa,
        Self::Taa,
        Self::Msaa2,
        Self::Msaa4,
        Self::Msaa8,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Fxaa => "FXAA",
            Self::Smaa => "SMAA",
            Self::Taa => "TAA",
            Self::Msaa2 => "MSAA 2x",
            Self::Msaa4 => "MSAA 4x",
            Self::Msaa8 => "MSAA 8x",
        }
    }

    fn msaa(self) -> Msaa {
        match self {
            Self::Msaa2 => Msaa::Sample2,
            Self::Msaa4 => Msaa::Sample4,
            Self::Msaa8 => Msaa::Sample8,
            _ => Msaa::Off,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ShadowFilter {
    Hard,
    Soft,
    /// Soft edges that change every frame, smoothed by TAA.
    Temporal,
}

impl ShadowFilter {
    const ALL: [Self; 3] = [Self::Hard, Self::Soft, Self::Temporal];

    fn name(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::Soft => "soft",
            Self::Temporal => "temporal",
        }
    }

    fn method(self) -> ShadowFilteringMethod {
        match self {
            Self::Hard => ShadowFilteringMethod::Hardware2x2,
            Self::Soft => ShadowFilteringMethod::Gaussian,
            Self::Temporal => ShadowFilteringMethod::Temporal,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Level {
    Off,
    Low,
    Medium,
    High,
    Ultra,
}

impl Level {
    const ALL: [Self; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Ultra];
    /// Motion blur has no ultra.
    const TO_HIGH: [Self; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    fn ambient_occlusion(self) -> Option<ScreenSpaceAmbientOcclusionQualityLevel> {
        use ScreenSpaceAmbientOcclusionQualityLevel as Q;
        match self {
            Self::Off => None,
            Self::Low => Some(Q::Low),
            Self::Medium => Some(Q::Medium),
            Self::High => Some(Q::High),
            Self::Ultra => Some(Q::Ultra),
        }
    }

    /// Ray-march steps through the clouds and towards the sun, and the strength of the
    /// fine erosion of their edges; no steps draws no clouds.
    pub fn march(self) -> (u32, u32, f32) {
        match self {
            Self::Off => (0, 0, 0.0),
            Self::Low => (32, 3, 0.0),
            Self::Medium => (48, 4, 1.0),
            Self::High => (64, 5, 1.0),
            Self::Ultra => (96, 6, 1.0),
        }
    }

    fn motion_blur(self) -> Option<MotionBlur> {
        let (shutter_angle, samples) = match self {
            Self::Off => return None,
            Self::Low => (0.25, 2),
            Self::Medium => (0.5, 3),
            Self::High | Self::Ultra => (1.0, 4),
        };
        Some(MotionBlur {
            shutter_angle,
            samples,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    Low,
    Medium,
    High,
    Ultra,
}

impl Preset {
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }

    /// `prefs` with this preset's quality settings; its preferences are kept.
    pub fn apply(self, prefs: GraphicsSettings) -> GraphicsSettings {
        match self {
            Self::Low => GraphicsSettings {
                anti_aliasing: AntiAliasing::Fxaa,
                sharpening: 0,
                anisotropy: 4,
                view_distance: 2000,
                shadow_resolution: 1024,
                shadow_distance: 100,
                shadow_cascades: 2,
                shadow_filter: ShadowFilter::Hard,
                contact_shadows: false,
                ambient_occlusion: Level::Off,
                haze: false,
                clouds: Level::Low,
                ..prefs
            },
            Self::Medium => GraphicsSettings {
                anti_aliasing: AntiAliasing::Msaa4,
                sharpening: 0,
                anisotropy: 8,
                view_distance: 3000,
                shadow_resolution: 2048,
                shadow_distance: 150,
                shadow_cascades: 3,
                shadow_filter: ShadowFilter::Soft,
                contact_shadows: false,
                ambient_occlusion: Level::Off,
                haze: true,
                clouds: Level::Medium,
                ..prefs
            },
            Self::High => GraphicsSettings {
                anti_aliasing: AntiAliasing::Msaa4,
                sharpening: 0,
                anisotropy: 16,
                view_distance: 8000,
                shadow_resolution: 4096,
                shadow_distance: 300,
                shadow_cascades: 4,
                shadow_filter: ShadowFilter::Soft,
                contact_shadows: true,
                ambient_occlusion: Level::Off,
                haze: true,
                clouds: Level::High,
                ..prefs
            },
            Self::Ultra => GraphicsSettings {
                anti_aliasing: AntiAliasing::Taa,
                sharpening: 30,
                anisotropy: 16,
                view_distance: 8000,
                shadow_resolution: 4096,
                shadow_distance: 500,
                shadow_cascades: 4,
                shadow_filter: ShadowFilter::Temporal,
                contact_shadows: true,
                ambient_occlusion: Level::High,
                haze: true,
                clouds: Level::Ultra,
                ..prefs
            },
        }
    }
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    pub anti_aliasing: AntiAliasing,
    /// Contrast adaptive sharpening, percent.
    pub sharpening: u32,
    /// Most samples of the anisotropic texture filter; 1 is trilinear.
    pub anisotropy: u16,
    /// Farthest scenery drawn, m.
    pub view_distance: u32,
    /// Edge of each cascade's shadow map, texels; 0 turns shadows off.
    pub shadow_resolution: u32,
    /// Farthest shadow, m.
    pub shadow_distance: u32,
    pub shadow_cascades: u32,
    pub shadow_filter: ShadowFilter,
    /// Screen-space shadows where objects touch, finer than the shadow map.
    pub contact_shadows: bool,
    pub ambient_occlusion: Level,
    /// Haze that fades distant scenery into the sky.
    pub haze: bool,
    /// Volumetric clouds.
    pub clouds: Level,
    pub motion_blur: Level,
    pub vsync: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Preset::High.apply(GraphicsSettings {
            anti_aliasing: AntiAliasing::Off,
            sharpening: 0,
            anisotropy: 1,
            view_distance: 0,
            shadow_resolution: 0,
            shadow_distance: 0,
            shadow_cascades: 1,
            shadow_filter: ShadowFilter::Soft,
            contact_shadows: false,
            ambient_occlusion: Level::Off,
            haze: false,
            clouds: Level::Off,
            motion_blur: Level::Off,
            vsync: true,
        })
    }
}

/// One line of the settings screen's graphics page below the preset.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting {
    AntiAliasing,
    Sharpening,
    Textures,
    ViewDistance,
    ShadowResolution,
    ShadowDistance,
    ShadowCascades,
    ShadowFilter,
    ContactShadows,
    AmbientOcclusion,
    Haze,
    Clouds,
    MotionBlur,
    VSync,
}

impl Setting {
    pub const ALL: [Self; 14] = [
        Self::AntiAliasing,
        Self::Sharpening,
        Self::Textures,
        Self::ViewDistance,
        Self::ShadowResolution,
        Self::ShadowDistance,
        Self::ShadowCascades,
        Self::ShadowFilter,
        Self::ContactShadows,
        Self::AmbientOcclusion,
        Self::Haze,
        Self::Clouds,
        Self::MotionBlur,
        Self::VSync,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::AntiAliasing => "Anti-aliasing",
            Self::Sharpening => "Sharpening",
            Self::Textures => "Texture filter",
            Self::ViewDistance => "View distance",
            Self::ShadowResolution => "Shadow quality",
            Self::ShadowDistance => "Shadow distance",
            Self::ShadowCascades => "Shadow cascades",
            Self::ShadowFilter => "Shadow edges",
            Self::ContactShadows => "Contact shadows",
            Self::AmbientOcclusion => "Ambient occl.",
            Self::Haze => "Haze",
            Self::Clouds => "Clouds",
            Self::MotionBlur => "Motion blur",
            Self::VSync => "VSync",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::AntiAliasing => "smooths edges; TAA also stills shimmer",
            Self::Sharpening => "restores detail blurred by FXAA and TAA",
            Self::Textures => "sharp road at a distance; on restart",
            Self::ViewDistance => "farthest scenery drawn",
            Self::ShadowResolution => "sharpness of shadows",
            Self::ShadowDistance => "how far shadows reach",
            Self::ShadowCascades => "more keeps far shadows sharp",
            Self::ShadowFilter => "temporal is best with TAA",
            Self::ContactShadows => "fine shadows where objects touch",
            Self::AmbientOcclusion => "shade in creases; not with MSAA",
            Self::Haze => "fades distant scenery into the sky",
            Self::Clouds => "volumetric clouds; costly on slow GPUs",
            Self::MotionBlur => "not set by the presets",
            Self::VSync => "no tearing; not set by the presets",
        }
    }

    /// Switched by Enter as well as Left/Right.
    pub fn is_toggle(self) -> bool {
        matches!(self, Self::ContactShadows | Self::Haze | Self::VSync)
    }
}

/// What the GPU can do, for the options it cannot.
#[derive(Resource, Clone, Copy)]
pub struct GraphicsSupport {
    msaa8: bool,
}

/// The value after `current` (`up`) or before it in `steps`, stopping at the ends.
/// A value not in `steps` moves to the first.
fn step<T: Copy + PartialEq>(steps: &[T], current: T, up: bool) -> T {
    match steps.iter().position(|&x| x == current) {
        Some(i) if up => steps[(i + 1).min(steps.len() - 1)],
        Some(i) => steps[i.saturating_sub(1)],
        None => steps[0],
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

impl GraphicsSettings {
    pub fn load() -> Self {
        let mut s: Self = bindings::load_config(SETTINGS_FILE);
        if let Ok(name) = std::env::var("OPEN_RACING_CAPTURE_QUALITY")
            && let Some(preset) = Preset::ALL.into_iter().find(|p| p.name() == name)
        {
            s = preset.apply(s);
            s.vsync = false;
            s.motion_blur = Level::Off;
        }
        // A file saved before the clouds setting existed keeps the preset it was on.
        if s.preset().is_none()
            && let Some(clouds) = Level::ALL
                .into_iter()
                .find(|&clouds| Self { clouds, ..s }.preset().is_some())
        {
            s.clouds = clouds;
        }
        s
    }

    pub fn save(&self) {
        bindings::save_config(SETTINGS_FILE, self);
    }

    /// The preset whose quality settings these are, if any.
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.apply(*self) == *self)
    }

    /// The anti-aliasing in use: MSAA 8x falls back to 4x where the GPU lacks it.
    fn anti_aliasing(&self, support: GraphicsSupport) -> AntiAliasing {
        match self.anti_aliasing {
            AntiAliasing::Msaa8 if !support.msaa8 => AntiAliasing::Msaa4,
            aa => aa,
        }
    }

    /// The ambient occlusion in use: it needs a camera without MSAA.
    fn ambient_occlusion(&self, support: GraphicsSupport) -> Level {
        if self.anti_aliasing(support).msaa() == Msaa::Off {
            self.ambient_occlusion
        } else {
            Level::Off
        }
    }

    pub fn value(&self, setting: Setting, support: GraphicsSupport) -> String {
        match setting {
            Setting::AntiAliasing => self.anti_aliasing(support).name().into(),
            Setting::Sharpening => match self.sharpening {
                0 => "off".into(),
                s => format!("{s} %"),
            },
            Setting::Textures => match self.anisotropy {
                1 => "trilinear".into(),
                n => format!("{n}x aniso"),
            },
            Setting::ViewDistance => format!("{} m", self.view_distance),
            Setting::ShadowResolution => match self.shadow_resolution {
                0 => "off".into(),
                n => format!("{n} px"),
            },
            Setting::ShadowDistance => format!("{} m", self.shadow_distance),
            Setting::ShadowCascades => self.shadow_cascades.to_string(),
            Setting::ShadowFilter => self.shadow_filter.name().into(),
            Setting::ContactShadows => on_off(self.contact_shadows).into(),
            Setting::AmbientOcclusion => match self.ambient_occlusion(support) {
                Level::Off if self.ambient_occlusion != Level::Off => "off (MSAA)".into(),
                level => level.name().into(),
            },
            Setting::Haze => on_off(self.haze).into(),
            Setting::Clouds => self.clouds.name().into(),
            Setting::MotionBlur => self.motion_blur.name().into(),
            Setting::VSync => on_off(self.vsync).into(),
        }
    }

    /// Moves `setting` one step up or down; toggles switch either way.
    pub fn step(&mut self, setting: Setting, up: bool, support: GraphicsSupport) {
        match setting {
            Setting::AntiAliasing => {
                let all: Vec<_> = AntiAliasing::ALL
                    .into_iter()
                    .filter(|&aa| aa != AntiAliasing::Msaa8 || support.msaa8)
                    .collect();
                self.anti_aliasing = step(&all, self.anti_aliasing(support), up);
            }
            Setting::Sharpening => self.sharpening = step(&SHARPENING_STEPS, self.sharpening, up),
            Setting::Textures => self.anisotropy = step(&ANISOTROPY_STEPS, self.anisotropy, up),
            Setting::ViewDistance => {
                self.view_distance = step(&VIEW_DISTANCE_STEPS, self.view_distance, up);
            }
            Setting::ShadowResolution => {
                self.shadow_resolution = step(&SHADOW_RESOLUTION_STEPS, self.shadow_resolution, up);
            }
            Setting::ShadowDistance => {
                self.shadow_distance = step(&SHADOW_DISTANCE_STEPS, self.shadow_distance, up);
            }
            Setting::ShadowCascades => {
                self.shadow_cascades = step(&SHADOW_CASCADE_STEPS, self.shadow_cascades, up);
            }
            Setting::ShadowFilter => {
                self.shadow_filter = step(&ShadowFilter::ALL, self.shadow_filter, up);
            }
            Setting::ContactShadows => self.contact_shadows = !self.contact_shadows,
            Setting::AmbientOcclusion => {
                self.ambient_occlusion = step(&Level::ALL, self.ambient_occlusion, up);
            }
            Setting::Haze => self.haze = !self.haze,
            Setting::Clouds => self.clouds = step(&Level::ALL, self.clouds, up),
            Setting::MotionBlur => self.motion_blur = step(&Level::TO_HIGH, self.motion_blur, up),
            Setting::VSync => self.vsync = !self.vsync,
        }
    }
}

pub struct GraphicsPlugin;

impl Plugin for GraphicsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GraphicsSettings::load())
            .add_systems(Startup, detect_support)
            .add_systems(PostStartup, (apply_camera, apply_shadows, apply_vsync))
            .add_systems(
                Update,
                (
                    (apply_camera, apply_shadows, apply_vsync)
                        .run_if(resource_changed::<GraphicsSettings>),
                    reset_taa_on_cut,
                ),
            );
    }
}

fn detect_support(mut commands: Commands, adapter: Option<Res<RenderAdapter>>) {
    let msaa8 = adapter.is_some_and(|a| {
        [TextureFormat::Rgba8UnormSrgb, TextureFormat::Depth32Float]
            .into_iter()
            .all(|f| {
                a.get_texture_format_features(f)
                    .flags
                    .sample_count_supported(8)
            })
    });
    commands.insert_resource(GraphicsSupport { msaa8 });
}

fn apply_camera(
    settings: Res<GraphicsSettings>,
    support: Res<GraphicsSupport>,
    mut commands: Commands,
    mut cameras: Query<(Entity, &mut Projection), With<MainCamera>>,
) {
    let s = *settings;
    let aa = s.anti_aliasing(*support);
    let taa = aa == AntiAliasing::Taa;
    let ambient_occlusion = s.ambient_occlusion(*support).ambient_occlusion();
    let motion_blur = s.motion_blur.motion_blur();
    for (camera, mut projection) in &mut cameras {
        if let Projection::Perspective(p) = &mut *projection {
            p.far = s.view_distance as f32;
        }
        let mut camera = commands.entity(camera);
        camera
            .remove::<(
                Fxaa,
                Smaa,
                TemporalAntiAliasing,
                ContrastAdaptiveSharpening,
                ScreenSpaceAmbientOcclusion,
                ContactShadows,
                MotionBlur,
                DistanceFog,
            )>()
            .insert((aa.msaa(), s.shadow_filter.method()));
        match aa {
            AntiAliasing::Fxaa => {
                camera.insert(Fxaa::default());
            }
            AntiAliasing::Smaa => {
                camera.insert(Smaa::default());
            }
            AntiAliasing::Taa => {
                camera.insert(TemporalAntiAliasing::default());
            }
            _ => {}
        }
        if s.sharpening > 0 {
            camera.insert(ContrastAdaptiveSharpening {
                sharpening_strength: s.sharpening as f32 / 100.0,
                ..default()
            });
        }
        if let Some(quality_level) = ambient_occlusion {
            camera.insert(ScreenSpaceAmbientOcclusion {
                quality_level,
                ..default()
            });
        }
        if s.contact_shadows {
            camera.insert(ContactShadows::default());
        }
        if let Some(blur) = motion_blur.clone() {
            camera.insert(blur);
        }
        if s.haze {
            camera.insert(DistanceFog {
                color: FOG_COLOR,
                falloff: FogFalloff::from_visibility(FOG_VISIBILITY),
                ..default()
            });
        }
        // The effects above require their prepasses; drop those no effect needs now.
        if !(taa || ambient_occlusion.is_some() || s.contact_shadows || motion_blur.is_some()) {
            camera.remove::<DepthPrepass>();
        }
        if ambient_occlusion.is_none() {
            camera.remove::<NormalPrepass>();
        }
        if !(taa || motion_blur.is_some()) {
            camera.remove::<MotionVectorPrepass>();
        }
        if !taa {
            camera.remove::<(TemporalJitter, MipBias)>();
        }
    }
}

fn apply_shadows(
    settings: Res<GraphicsSettings>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut lights: Query<(&mut DirectionalLight, &mut CascadeShadowConfig)>,
) {
    let s = *settings;
    if s.shadow_resolution > 0 {
        shadow_map.size = s.shadow_resolution as usize;
    }
    let maximum_distance = s.shadow_distance as f32;
    for (mut light, mut cascades) in &mut lights {
        light.shadow_maps_enabled = s.shadow_resolution > 0;
        light.contact_shadows_enabled = s.contact_shadows;
        *cascades = CascadeShadowConfigBuilder {
            num_cascades: s.shadow_cascades as usize,
            // The first cascade covers the car and the road just ahead.
            first_cascade_far_bound: (maximum_distance / 20.0).clamp(5.0, 25.0),
            maximum_distance,
            ..default()
        }
        .build();
    }
}

/// In VR the headset paces the frames, and the window keeps VSync off.
fn apply_vsync(
    settings: Res<GraphicsSettings>,
    args: Res<Args>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if args.vr {
        return;
    }
    for mut window in &mut windows {
        window.present_mode = if settings.vsync {
            PresentMode::AutoVsync
        } else {
            PresentMode::AutoNoVsync
        };
    }
}

/// Clears TAA's history when the camera jumps to a new view (camera change, TV camera
/// switch, reset), so the old view does not ghost over the new one.
fn reset_taa_on_cut(
    mut last: Local<Option<Vec3>>,
    mut cameras: Query<(&Transform, &mut TemporalAntiAliasing), With<MainCamera>>,
) {
    let Ok((transform, mut taa)) = cameras.single_mut() else {
        *last = None;
        return;
    };
    if last.is_some_and(|p| p.distance(transform.translation) > CAMERA_CUT) {
        taa.reset = true;
    }
    *last = Some(transform.translation);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_recognised_whatever_the_preferences() {
        let prefs = GraphicsSettings {
            motion_blur: Level::High,
            vsync: false,
            ..default()
        };
        for preset in Preset::ALL {
            assert_eq!(preset.apply(prefs).preset(), Some(preset));
        }
    }

    #[test]
    fn changing_a_detail_makes_the_settings_custom() {
        let support = GraphicsSupport { msaa8: false };
        let mut s = GraphicsSettings::default();
        s.step(Setting::ShadowDistance, true, support);
        assert_eq!(s.preset(), None);
        s.step(Setting::ShadowDistance, false, support);
        assert_eq!(s.preset(), Some(Preset::High));
    }

    #[test]
    fn msaa_8x_is_skipped_without_support() {
        let support = GraphicsSupport { msaa8: false };
        let mut s = GraphicsSettings::default();
        s.step(Setting::AntiAliasing, true, support);
        assert_eq!(s.anti_aliasing, AntiAliasing::Msaa4);
    }
}
