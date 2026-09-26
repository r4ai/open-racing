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
use open_racing_track::{AlphaMode as TrackAlpha, DetailMask, FAR_AWAY, Instance, Level, Visual};

/// Simulation is Z-up (ISO 8855), Bevy is Y-up: rotate −90° about X.
pub fn to_bevy(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.z as f32, -v.y as f32)
}

/// Inverse of `to_bevy`.
pub fn from_bevy(v: Vec3) -> DVec3 {
    DVec3::new(v.x as f64, -v.z as f64, v.y as f64)
}

pub fn quat_to_bevy(q: DQuat) -> Quat {
    // The sim's glam and Bevy's glam may be different crate versions.
    rotation_to_bevy([q.x as f32, q.y as f32, q.z as f32, q.w as f32])
}

/// A rotation (x, y, z, w) of the simulation's axes in Bevy's.
pub fn rotation_to_bevy([x, y, z, w]: [f32; 4]) -> Quat {
    let c = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    c * Quat::from_xyzw(x, y, z, w) * c.inverse()
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

/// Spawns a package's render data, each entity with `extra` (e.g. a marker to find and
/// despawn them again): one per mesh batch; for copies of shapes, one per copy of each
/// mesh of their nearest level, which share the mesh and its material so that the
/// renderer draws them together by instancing, and their farther levels merged by tile
/// (see `TILE`).
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
    let shapes = add_shapes(&visual, &mats, meshes);
    for m in visual.meshes {
        let (material, cast_shadows) = (mats[m.material as usize].clone(), m.cast_shadows);
        let mut entity = commands.spawn((
            Mesh3d(meshes.add(to_mesh(m))),
            MeshMaterial3d(material),
            extra.clone(),
        ));
        if !cast_shadows {
            entity.insert(NotShadowCaster);
        }
    }
    for set in &visual.instances {
        for level in &set.levels {
            let shape = level.shape as usize;
            if !merged(*level) {
                for part in &shapes[shape] {
                    spawn_copies(commands, part, *level, &set.copies, extra.clone());
                }
                continue;
            }
            for (tile, copies) in tiles(&set.copies) {
                let (at, parts) = merge_tile(&visual.shapes[shape].meshes, &copies, tile);
                for (m, part) in parts.into_iter().zip(&shapes[shape]) {
                    spawn_tile(commands, meshes, part, m, at, *level, extra.clone());
                }
            }
        }
    }
}

/// One mesh of a shape, ready to draw copies of.
#[derive(Clone)]
pub struct ShapePart {
    pub mesh: Handle<Mesh>,
    pub material: Handle<TrackMaterial>,
    pub cast_shadows: bool,
}

/// The meshes of the package's shapes, each added once, with their materials among
/// `mats` (from `add_materials`).
pub fn add_shapes(
    visual: &Visual,
    mats: &[Handle<TrackMaterial>],
    meshes: &mut Assets<Mesh>,
) -> Vec<Vec<ShapePart>> {
    visual
        .shapes
        .iter()
        .map(|s| {
            s.meshes
                .iter()
                .map(|m| ShapePart {
                    mesh: meshes.add(to_mesh(m.clone())),
                    material: mats.get(m.material as usize).cloned().unwrap_or_default(),
                    cast_shadows: m.cast_shadows,
                })
                .collect()
        })
        .collect()
}

/// Where a copy stands, in Bevy's axes.
pub fn instance_transform(c: &Instance) -> Transform {
    let [x, y, z] = c.pos;
    Transform {
        translation: Vec3::new(x, z, -y),
        rotation: rotation_to_bevy(c.rotation),
        scale: Vec3::splat(c.scale),
    }
}

/// The distances from the camera a level of detail shows at, unless it shows at any.
pub fn visibility_range(level: Level) -> Option<VisibilityRange> {
    (level.fade_in[1] > 0.0 || level.fade_out[0] < FAR_AWAY).then(|| VisibilityRange {
        start_margin: level.fade_in[0]..level.fade_in[1],
        end_margin: level.fade_out[0]..level.fade_out[1],
        use_aabb: false,
    })
}

/// Spawns a copy of a shape's mesh at each of `copies`, shown at the distances of
/// `level`: the entities share the mesh and the material, and the renderer draws them
/// in one go.
pub fn spawn_copies(
    commands: &mut Commands,
    part: &ShapePart,
    level: Level,
    copies: &[Instance],
    extra: impl Bundle + Clone,
) {
    let range = visibility_range(level);
    for c in copies {
        let mut e = commands.spawn((
            Mesh3d(part.mesh.clone()),
            MeshMaterial3d(part.material.clone()),
            instance_transform(c),
            extra.clone(),
        ));
        if let Some(range) = &range {
            e.insert(range.clone());
        }
        if !part.cast_shadows {
            e.insert(NotShadowCaster);
        }
    }
}

/// Edge of the square tiles the copies of far levels are merged in, m.
///
/// A far level shows for nearly every copy in view, and the renderer's work on the CPU
/// grows with the entities in view: tens of thousands of copies of a few triangles each
/// cost far more as entities than as the vertices they add. The levels in full show
/// only near the camera, so their copies stay entities of their own.
pub const TILE: f32 = 32.0;

/// Whether a level's copies are merged by tile: those of the levels that fade in,
/// farther than the nearest.
pub fn merged(level: Level) -> bool {
    level.fade_in[1] > 0.0
}

/// The tile a copy stands in.
pub fn tile_of(c: &Instance) -> [i32; 2] {
    [
        (c.pos[0] / TILE).floor() as i32,
        (c.pos[1] / TILE).floor() as i32,
    ]
}

/// Copies grouped by their tile, in the order of the tiles.
pub fn tiles(copies: &[Instance]) -> Vec<([i32; 2], Vec<Instance>)> {
    let mut by: std::collections::BTreeMap<[i32; 2], Vec<Instance>> = Default::default();
    for c in copies {
        by.entry(tile_of(c)).or_default().push(*c);
    }
    by.into_iter().collect()
}

/// The copies in a tile merged: each of the shape's meshes at every copy, about the
/// tile's middle, and where the middle is, whose distance from the camera the tile
/// shows by.
pub fn merge_tile(
    meshes: &[open_racing_track::Mesh],
    copies: &[Instance],
    tile: [i32; 2],
) -> (Transform, Vec<open_racing_track::Mesh>) {
    let z = copies.iter().map(|c| c.pos[2]).sum::<f32>() / copies.len().max(1) as f32;
    let middle = Vec3::new(
        (tile[0] as f32 + 0.5) * TILE,
        (tile[1] as f32 + 0.5) * TILE,
        z,
    );
    let merged = meshes
        .iter()
        .map(|part| {
            let mut m = open_racing_track::Mesh {
                material: part.material,
                cast_shadows: part.cast_shadows,
                ..Default::default()
            };
            for c in copies {
                let [x, y, z, w] = c.rotation;
                let turn = Quat::from_xyzw(x, y, z, w);
                let at = Vec3::from_array(c.pos) - middle;
                let base = m.positions.len() as u32;
                for (p, n) in part.positions.iter().zip(&part.normals) {
                    m.positions
                        .push((at + turn * (Vec3::from_array(*p) * c.scale)).to_array());
                    m.normals.push((turn * Vec3::from_array(*n)).to_array());
                }
                m.uvs.extend(&part.uvs);
                m.indices.extend(part.indices.iter().map(|i| i + base));
            }
            m
        })
        .collect();
    let [x, y, z] = middle.to_array();
    (Transform::from_translation(Vec3::new(x, z, -y)), merged)
}

/// The distances a far level's tile shows at: the level's, measured from the tile's
/// middle rather than from each copy, so widened by half the tile's diagonal. A tile is
/// in full by the time the copies of it nearest the camera have faded out of the level
/// before, so that none go missing between the levels.
pub fn tile_range(level: Level) -> Option<VisibilityRange> {
    let h = TILE * std::f32::consts::FRAC_1_SQRT_2;
    visibility_range(Level {
        fade_in: level.fade_in.map(|d| (d - h).max(0.0)),
        fade_out: level
            .fade_out
            .map(|d| if d >= FAR_AWAY { d } else { d + h }),
        ..level
    })
}

/// Spawns a tile of a far level: one of its shape's meshes merged at every copy in it
/// (`merge_tile`), in the look of `part`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_tile(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    part: &ShapePart,
    mesh: open_racing_track::Mesh,
    at: Transform,
    level: Level,
    extra: impl Bundle,
) -> Entity {
    let mut e = commands.spawn((
        Mesh3d(meshes.add(to_mesh(mesh))),
        MeshMaterial3d(part.material.clone()),
        at,
        extra,
    ));
    if let Some(range) = tile_range(level) {
        e.insert(range);
    }
    if !part.cast_shadows {
        e.insert(NotShadowCaster);
    }
    e.id()
}

// The compressed texture formats the GPU supports, BC when not known yet.
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
