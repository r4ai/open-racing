//! Renders a track package's 3D model: standard PBR materials extended with the
//! package's surface textures (reflection, roughness, reflectance), normal maps and
//! mask-blended detail layers. Car packages' models share the format and the materials.
//!
//! Shared by the app and the track editor.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::VisibilityRange;
use bevy::image::{
    CompressedImageFormatSupport, CompressedImageFormats, ImageAddressMode, ImageSampler,
    ImageSamplerDescriptor, ImageType,
};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Face, ShaderType};
use bevy::shader::ShaderRef;
use glam::{DQuat, DVec3};
use open_racing_track::{AlphaMode as TrackAlpha, DetailMask, Visual};

/// Simulation is Z-up (ISO 8855), Bevy is Y-up: rotate −90° about X.
pub fn to_bevy(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.z as f32, -v.y as f32)
}

/// Inverse of `to_bevy`.
pub fn from_bevy(v: Vec3) -> DVec3 {
    DVec3::new(v.x as f64, -v.z as f64, v.y as f64)
}

pub fn quat_to_bevy(q: DQuat) -> Quat {
    let c = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    // The sim's glam and Bevy's glam may be different crate versions.
    let q = Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32);
    c * q * c.inverse()
}

pub type TrackMaterial = ExtendedMaterial<StandardMaterial, TrackExtension>;

pub struct TrackModelPlugin;

impl Plugin for TrackModelPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "track_material.wgsl");
        app.add_plugins(MaterialPlugin::<TrackMaterial>::default());
    }
}

/// `TrackParams::flags`: the detail layers are present.
const DETAIL: u32 = 1;
/// Detail UVs from the world position instead of the mesh UVs.
const WORLD_UV: u32 = 2;
/// The normal map is present.
const NORMAL_MAP: u32 = 4;
/// The surface texture is present.
const SURFACE: u32 = 8;
/// The base colour's alpha masks the R detail layer instead of the mask texture.
const BASE_ALPHA_MASK: u32 = 16;
/// The detail normal map is present.
const DETAIL_NORMAL_MAP: u32 = 32;

#[derive(ShaderType, Clone, Debug, Default, Reflect)]
pub struct TrackParams {
    scales: Vec4,
    enabled: Vec4,
    multiplier: f32,
    reflection: f32,
    flags: u32,
    detail_normal_scale: f32,
    detail_normal_strength: f32,
}

/// The package's surface texture, normal map, reflection of the surroundings and detail
/// layers, see `track_material.wgsl`. Bindings start at 100, after the standard
/// material's. The standard material's own normal mapping would need vertex tangents; this
/// derives the frame from the UVs instead.
///
/// Bindless like the standard material, so that meshes of different materials still draw
/// in few calls: the index table is at binding 120 and the parameters at 121.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
#[data(100, TrackParams, binding_array(121))]
#[bindless(index_table(range(100..117), binding(120)))]
pub struct TrackExtension {
    params: TrackParams,
    #[texture(101)]
    #[sampler(102)]
    mask: Option<Handle<Image>>,
    #[texture(103)]
    #[sampler(104)]
    layer_r: Option<Handle<Image>>,
    #[texture(105)]
    #[sampler(106)]
    layer_g: Option<Handle<Image>>,
    #[texture(107)]
    #[sampler(108)]
    layer_b: Option<Handle<Image>>,
    #[texture(109)]
    #[sampler(110)]
    layer_a: Option<Handle<Image>>,
    #[texture(111)]
    #[sampler(112)]
    normal_map: Option<Handle<Image>>,
    #[texture(113)]
    #[sampler(114)]
    surface: Option<Handle<Image>>,
    #[texture(115)]
    #[sampler(116)]
    detail_normal: Option<Handle<Image>>,
}

impl From<&TrackExtension> for TrackParams {
    fn from(extension: &TrackExtension) -> Self {
        extension.params.clone()
    }
}

impl MaterialExtension for TrackExtension {
    fn fragment_shader() -> ShaderRef {
        "embedded://open_racing_track_render/track_material.wgsl".into()
    }
}

/// GPU images of the package's textures, created on first use: colour textures as sRGB,
/// normal, surface and mask textures as linear data.
struct Images<'a> {
    visual: &'a Visual,
    formats: CompressedImageFormats,
    sampler: ImageSampler,
    images: &'a mut Assets<Image>,
    srgb: Vec<Option<Option<Handle<Image>>>>,
    linear: Vec<Option<Option<Handle<Image>>>>,
}

impl Images<'_> {
    fn get(&mut self, index: u32, srgb: bool) -> Option<Handle<Image>> {
        let i = index as usize;
        let cache = if srgb {
            &mut self.srgb
        } else {
            &mut self.linear
        };
        if let Some(handle) = &cache[i] {
            return handle.clone();
        }
        let data = &self.visual.textures[i].data;
        let handle = Image::from_buffer(
            data,
            ImageType::Extension("dds"),
            self.formats,
            srgb,
            self.sampler.clone(),
            RenderAssetUsages::RENDER_WORLD,
        )
        .inspect_err(|e| warn!("texture {i}: {e}"))
        .ok()
        .map(|img| self.images.add(img));
        cache[i] = Some(handle.clone());
        handle
    }
}

/// Spawns a package's render data: one entity per mesh batch, each with `extra` (e.g. a
/// marker to find and despawn them again).
#[allow(clippy::too_many_arguments)]
pub fn spawn_visual(
    commands: &mut Commands,
    visual: Visual,
    formats: CompressedImageFormats,
    anisotropy: u16,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<TrackMaterial>,
    images: &mut Assets<Image>,
    extra: impl Bundle + Clone,
) {
    let mats = add_materials(&visual, formats, anisotropy, materials, images);
    for mut m in visual.meshes {
        let (material, cast_shadows) = (mats[m.material as usize].clone(), m.cast_shadows);
        let lod = level_of_detail(&mut m);
        let mut entity = commands.spawn((
            Mesh3d(meshes.add(to_mesh(m))),
            MeshMaterial3d(material),
            extra.clone(),
        ));
        if let Some(lod) = lod {
            entity.insert(lod);
        }
        if !cast_shadows {
            entity.insert(NotShadowCaster);
        }
    }
}

/// For a mesh drawn within a range of distances: moves its positions to be about the
/// centre the distances are measured from, and gives where to put it and the range.
/// The levels of detail of the same things then share their place, so that they
/// cross-fade.
pub fn level_of_detail(m: &mut open_racing_track::Mesh) -> Option<(Transform, VisibilityRange)> {
    let lod = m.lod?;
    let [cx, cy, cz] = lod.center;
    for p in &mut m.positions {
        *p = [p[0] - cx, p[1] - cy, p[2] - cz];
    }
    Some((
        Transform::from_translation(to_bevy(DVec3::new(cx as f64, cy as f64, cz as f64))),
        VisibilityRange {
            start_margin: lod.fade_in[0]..lod.fade_in[1],
            end_margin: lod.fade_out[0]..lod.fade_out[1],
            use_aabb: false,
        },
    ))
}

/// The compressed texture formats the GPU supports, BC when not known yet.
pub fn formats(support: Option<&CompressedImageFormatSupport>) -> CompressedImageFormats {
    support.map_or(CompressedImageFormats::BC, |f| f.0)
}

/// Adds the materials of a package's render data, and the textures they use, filtered
/// with up to `anisotropy` samples.
pub fn add_materials(
    visual: &Visual,
    formats: CompressedImageFormats,
    anisotropy: u16,
    materials: &mut Assets<TrackMaterial>,
    images: &mut Assets<Image>,
) -> Vec<Handle<TrackMaterial>> {
    let sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: anisotropy,
        ..ImageSamplerDescriptor::linear()
    });
    let n = visual.textures.len();
    let mut images = Images {
        visual,
        formats,
        sampler,
        images,
        srgb: vec![None; n],
        linear: vec![None; n],
    };
    visual
        .materials
        .iter()
        .map(|m| {
            let [r, g, b, a] = m.base_color;
            let base = StandardMaterial {
                base_color: Color::linear_rgba(r, g, b, a),
                base_color_texture: m.base_color_texture.and_then(|i| images.get(i, true)),
                perceptual_roughness: m.roughness,
                reflectance: m.reflectance,
                alpha_mode: match m.alpha_mode {
                    TrackAlpha::Opaque => AlphaMode::Opaque,
                    TrackAlpha::Mask(cutoff) => AlphaMode::Mask(cutoff),
                    TrackAlpha::Blend => AlphaMode::Blend,
                },
                cull_mode: (!m.double_sided).then_some(Face::Back),
                double_sided: m.double_sided,
                ..default()
            };
            let mut extension = TrackExtension {
                normal_map: m.normal_texture.and_then(|i| images.get(i, false)),
                surface: m.surface_texture.and_then(|i| images.get(i, false)),
                ..default()
            };
            extension.params.reflection = m.reflection;
            if extension.normal_map.is_some() {
                extension.params.flags |= NORMAL_MAP;
            }
            if extension.surface.is_some() {
                extension.params.flags |= SURFACE;
            }
            if let Some(d) = &m.detail {
                let layer = |i: usize| d.layers[i];
                extension.params.scales =
                    Vec4::from_array(std::array::from_fn(|i| layer(i).map_or(1.0, |l| l.scale)));
                extension.params.enabled = Vec4::from_array(std::array::from_fn(|i| {
                    if layer(i).is_some() { 1.0 } else { 0.0 }
                }));
                extension.params.multiplier = d.multiplier;
                extension.params.flags |= DETAIL | if d.world_uv { WORLD_UV } else { 0 };
                match d.mask {
                    DetailMask::Texture(t) => extension.mask = images.get(t, false),
                    DetailMask::BaseAlpha => extension.params.flags |= BASE_ALPHA_MASK,
                }
                if let Some(n) = d.normal {
                    extension.detail_normal = images.get(n.texture, false);
                    extension.params.detail_normal_scale = n.scale;
                    extension.params.detail_normal_strength = n.strength;
                    if extension.detail_normal.is_some() {
                        extension.params.flags |= DETAIL_NORMAL_MAP;
                    }
                }
                [
                    extension.layer_r,
                    extension.layer_g,
                    extension.layer_b,
                    extension.layer_a,
                ] = std::array::from_fn(|i| layer(i).and_then(|l| images.get(l.texture, true)));
            }
            materials.add(TrackMaterial { base, extension })
        })
        .collect()
}

/// A material blending up to four textures by a mask, as painted ground: `base` with its
/// colour and roughness, the layers' textures and repetitions per metre laid by the
/// world's x and y, and `mask` (linear RGBA, laid by the mesh's UVs) weighing them.
pub fn layered_material(
    base: StandardMaterial,
    mask: Handle<Image>,
    layers: [Option<(Handle<Image>, f32)>; 4],
) -> TrackMaterial {
    let mut extension = TrackExtension {
        mask: Some(mask),
        ..default()
    };
    extension.params.flags = DETAIL | WORLD_UV;
    extension.params.multiplier = 1.0;
    extension.params.scales = Vec4::from_array(std::array::from_fn(|i| {
        layers[i].as_ref().map_or(1.0, |l| l.1)
    }));
    extension.params.enabled = Vec4::from_array(std::array::from_fn(|i| {
        if layers[i].is_some() { 1.0 } else { 0.0 }
    }));
    let [r, g, b, a] = layers.map(|l| l.map(|l| l.0));
    (
        extension.layer_r,
        extension.layer_g,
        extension.layer_b,
        extension.layer_a,
    ) = (r, g, b, a);
    TrackMaterial {
        base: StandardMaterial {
            base_color_texture: None,
            ..base
        },
        extension,
    }
}

/// A Bevy mesh from a package mesh, whose positions and normals are in the simulation's
/// Z-up axes.
pub fn to_mesh(m: open_racing_track::Mesh) -> Mesh {
    // As `to_bevy`, without the round trip through f64.
    let convert = |[x, y, z]: [f32; 3]| [x, z, -y];
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        m.positions.into_iter().map(convert).collect::<Vec<_>>(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        m.normals.into_iter().map(convert).collect::<Vec<_>>(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs);
    mesh.insert_indices(Indices::U32(m.indices));
    mesh
}
