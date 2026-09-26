// The track material's own bindings and parameters, shared by its vertex and fragment
// shaders: see `TrackExtension`.
#define_import_path open_racing::track_bindings

#import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d}

struct Params {
    scales: vec4<f32>,
    // 1 for the detail layers that are present, 0 otherwise.
    enabled: vec4<f32>,
    multiplier: f32,
    // Scale of the reflection of the surroundings.
    reflection: f32,
    flags: u32,
    detail_normal_scale: f32,
    detail_normal_strength: f32,
    // How far the plant bends in a 10 m/s wind, m at 1 m up (growing with the square of
    // the height), and how far its leaves flutter, m.
    sway: f32,
    flutter: f32,
}

const DETAIL: u32 = 1u;
const WORLD_UV: u32 = 2u;
const NORMAL_MAP: u32 = 4u;
const SURFACE: u32 = 8u;
const BASE_ALPHA_MASK: u32 = 16u;
const DETAIL_NORMAL_MAP: u32 = 32u;
// Takes each copy's colour (leaves, clothes), and is left out of a copy without.
const TINTED: u32 = 64u;
// Moves in the wind: `sway` and `flutter` apply, and the wind texture is present.
const WIND: u32 = 128u;
// Pictures of a model from many sides, turned to the viewer: see `track_plant::impostor`.
const IMPOSTOR: u32 = 256u;

// Textures, as indices into the bindless index table below.
const MASK: u32 = 0u;
const LAYER_R: u32 = 1u;
const LAYER_G: u32 = 2u;
const LAYER_B: u32 = 3u;
const LAYER_A: u32 = 4u;
const NORMAL: u32 = 5u;
const SURFACE_TEXTURE: u32 = 6u;
const DETAIL_NORMAL: u32 = 7u;

#ifdef BINDLESS
struct Indices {
    params: u32,
    // Texture and sampler of each texture, in the order of the constants above, then
    // the wind texture.
    textures: array<u32, 17>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(120) var<storage> indices: array<Indices>;
@group(#{MATERIAL_BIND_GROUP}) @binding(121) var<storage> params_array: array<Params>;
#else
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> material_params: Params;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var mask_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var mask_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var layer_r_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var layer_r_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var layer_g_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var layer_g_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var layer_b_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var layer_b_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var layer_a_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var layer_a_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var normal_map_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(112) var normal_map_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(113) var surface_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(114) var surface_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(115) var detail_normal_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(116) var detail_normal_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(117) var wind_texture: texture_2d<f32>;
#endif

fn params(slot: u32) -> Params {
#ifdef BINDLESS
    return params_array[indices[slot].params];
#else
    return material_params;
#endif
}

// Samples texture `t` (MASK, …, DETAIL_NORMAL) of the material in `slot`. `t` is a constant at
// every call, so control flow stays uniform.
fn sample(slot: u32, t: u32, uv: vec2<f32>) -> vec4<f32> {
#ifdef BINDLESS
    let ids = indices[slot];
    return textureSample(bindless_textures_2d[ids.textures[2u * t]], bindless_samplers_filtering[ids.textures[2u * t + 1u]], uv);
#else
    switch t {
        case MASK: { return textureSample(mask_texture, mask_sampler, uv); }
        case LAYER_R: { return textureSample(layer_r_texture, layer_r_sampler, uv); }
        case LAYER_G: { return textureSample(layer_g_texture, layer_g_sampler, uv); }
        case LAYER_B: { return textureSample(layer_b_texture, layer_b_sampler, uv); }
        case LAYER_A: { return textureSample(layer_a_texture, layer_a_sampler, uv); }
        case NORMAL: { return textureSample(normal_map_texture, normal_map_sampler, uv); }
        case SURFACE_TEXTURE: { return textureSample(surface_texture, surface_sampler, uv); }
        default: { return textureSample(detail_normal_texture, detail_normal_sampler, uv); }
    }
#endif
}

// The wind of the moment for a material with WIND: its (x, y) is the wind at 10 m,
// along Bevy's x and z, m/s.
fn wind(slot: u32) -> vec4<f32> {
#ifdef BINDLESS
    return textureLoad(bindless_textures_2d[indices[slot].textures[16]], vec2(0u, 0u), 0);
#else
    return textureLoad(wind_texture, vec2(0u, 0u), 0);
#endif
}
