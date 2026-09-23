//! Renders a track package's 3D model: standard PBR materials, extended with the
//! package's mask-blended detail layers where a material has them.

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats, ImageAddressMode, ImageSampler, ImageSamplerDescriptor, ImageType};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Face, ShaderType};
use bevy::shader::ShaderRef;
use open_racing_track::{AlphaMode as TrackAlpha, Visual};

use crate::driving::TrackModel;

pub type TrackMaterial = ExtendedMaterial<StandardMaterial, DetailLayers>;

pub struct TrackModelPlugin;

impl Plugin for TrackModelPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "track_detail.wgsl");
        app.add_plugins(MaterialPlugin::<TrackMaterial>::default());
    }
}

/// `DetailParams::flags`: detail UVs from the world position instead of the mesh UVs.
const WORLD_UV: u32 = 1;

#[derive(ShaderType, Clone, Debug, Default, Reflect)]
pub struct DetailParams {
    scales: Vec4,
    enabled: Vec4,
    multiplier: f32,
    flags: u32,
}

/// See `track_detail.wgsl`. Bindings start at 100, after the standard material's.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct DetailLayers {
    #[uniform(100)]
    params: DetailParams,
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
}

impl MaterialExtension for DetailLayers {
    fn fragment_shader() -> ShaderRef {
        "embedded://open_racing_app/track_detail.wgsl".into()
    }
}

/// GPU images of the package's textures, created on first use: colour textures as sRGB,
/// detail masks as linear data.
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
        let cache = if srgb { &mut self.srgb } else { &mut self.linear };
        if let Some(handle) = &cache[i] {
            return handle.clone();
        }
        let data = &self.visual.textures[i].data;
        let handle = Image::from_buffer(data, ImageType::Extension("dds"), self.formats, srgb, self.sampler.clone(), RenderAssetUsages::RENDER_WORLD)
            .inspect_err(|e| warn!("track texture {i}: {e}"))
            .ok()
            .map(|img| self.images.add(img));
        cache[i] = Some(handle.clone());
        handle
    }
}

#[derive(Clone)]
enum MaterialHandle {
    Standard(Handle<StandardMaterial>),
    Detail(Handle<TrackMaterial>),
}

/// Spawns the track's model, if it has one: one entity per mesh batch.
pub fn spawn(
    mut commands: Commands,
    mut model: ResMut<TrackModel>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    mut detail: ResMut<Assets<TrackMaterial>>,
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
    let mut images = Images { visual: &visual, formats, sampler, images: &mut images, srgb: vec![None; n], linear: vec![None; n] };
    let mats: Vec<MaterialHandle> = visual
        .materials
        .iter()
        .map(|m| {
            let [r, g, b, a] = m.base_color;
            let base = StandardMaterial {
                base_color: Color::linear_rgba(r, g, b, a),
                base_color_texture: m.base_color_texture.and_then(|i| images.get(i, true)),
                perceptual_roughness: 0.85,
                alpha_mode: match m.alpha_mode {
                    TrackAlpha::Opaque => AlphaMode::Opaque,
                    TrackAlpha::Mask(cutoff) => AlphaMode::Mask(cutoff),
                    TrackAlpha::Blend => AlphaMode::Blend,
                },
                cull_mode: (!m.double_sided).then_some(Face::Back),
                double_sided: m.double_sided,
                ..default()
            };
            let Some(d) = &m.detail else {
                return MaterialHandle::Standard(standard.add(base));
            };
            let layer = |i: usize| d.layers[i];
            let extension = DetailLayers {
                params: DetailParams {
                    scales: Vec4::from_array(std::array::from_fn(|i| layer(i).map_or(1.0, |l| l.scale))),
                    enabled: Vec4::from_array(std::array::from_fn(|i| if layer(i).is_some() { 1.0 } else { 0.0 })),
                    multiplier: d.multiplier,
                    flags: if d.world_uv { WORLD_UV } else { 0 },
                },
                mask: images.get(d.mask, false),
                layer_r: layer(0).and_then(|l| images.get(l.texture, true)),
                layer_g: layer(1).and_then(|l| images.get(l.texture, true)),
                layer_b: layer(2).and_then(|l| images.get(l.texture, true)),
                layer_a: layer(3).and_then(|l| images.get(l.texture, true)),
            };
            MaterialHandle::Detail(detail.add(TrackMaterial { base, extension }))
        })
        .collect();

    for m in visual.meshes {
        // As `to_bevy`, without the round trip through f64.
        let convert = |[x, y, z]: [f32; 3]| [x, z, -y];
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.into_iter().map(convert).collect::<Vec<_>>());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals.into_iter().map(convert).collect::<Vec<_>>());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs);
        mesh.insert_indices(Indices::U32(m.indices));
        let mut entity = commands.spawn(Mesh3d(meshes.add(mesh)));
        match &mats[m.material as usize] {
            MaterialHandle::Standard(h) => entity.insert(MeshMaterial3d(h.clone())),
            MaterialHandle::Detail(h) => entity.insert(MeshMaterial3d(h.clone())),
        };
        if !m.cast_shadows {
            entity.insert(NotShadowCaster);
        }
    }
}
