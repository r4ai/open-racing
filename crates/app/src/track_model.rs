//! Renders a track package's 3D model: standard PBR materials extended with the
//! package's surface textures (reflection, roughness, reflectance), normal maps and
//! mask-blended detail layers.

use bevy::asset::{RenderAssetUsages, embedded_asset};
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
use open_racing_track::{AlphaMode as TrackAlpha, Visual};

use crate::driving::TrackModel;

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

#[derive(ShaderType, Clone, Debug, Default, Reflect)]
pub struct TrackParams {
    scales: Vec4,
    enabled: Vec4,
    multiplier: f32,
    reflection: f32,
    flags: u32,
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
#[bindless(index_table(range(100..115), binding(120)))]
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
}

impl From<&TrackExtension> for TrackParams {
    fn from(extension: &TrackExtension) -> Self {
        extension.params.clone()
    }
}

impl MaterialExtension for TrackExtension {
    fn fragment_shader() -> ShaderRef {
        "embedded://open_racing_app/track_material.wgsl".into()
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
        .inspect_err(|e| warn!("track texture {i}: {e}"))
        .ok()
        .map(|img| self.images.add(img));
        cache[i] = Some(handle.clone());
        handle
    }
}

/// Spawns the track's model, if it has one: one entity per mesh batch.
pub fn spawn(
    mut commands: Commands,
    mut model: ResMut<TrackModel>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TrackMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(visual) = model.0.take() else { return };
    let formats = formats.map_or(CompressedImageFormats::BC, |f| f.0);
    let sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 8,
        ..ImageSamplerDescriptor::linear()
    });
    let n = visual.textures.len();
    let mut images = Images {
        visual: &visual,
        formats,
        sampler,
        images: &mut images,
        srgb: vec![None; n],
        linear: vec![None; n],
    };
    let mats: Vec<Handle<TrackMaterial>> = visual
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
                extension.mask = images.get(d.mask, false);
                [
                    extension.layer_r,
                    extension.layer_g,
                    extension.layer_b,
                    extension.layer_a,
                ] = std::array::from_fn(|i| layer(i).and_then(|l| images.get(l.texture, true)));
            }
            materials.add(TrackMaterial { base, extension })
        })
        .collect();

    for m in visual.meshes {
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
        let mut entity = commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(mats[m.material as usize].clone()),
        ));
        if !m.cast_shadows {
            entity.insert(NotShadowCaster);
        }
    }
}
