// Standard PBR with the track package's surface texture, normal map, reflection of the
// surroundings and detail layers, see `open_racing_track::Material`. The detail layers
// multiply into the base colour: base × multiplier × Σ maskᵢ · layerᵢ(scaleᵢ · uv).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_bindings::mesh,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}
#import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d}

struct Params {
    scales: vec4<f32>,
    // 1 for the detail layers that are present, 0 otherwise.
    enabled: vec4<f32>,
    multiplier: f32,
    // Scale of the reflection of the surroundings.
    reflection: f32,
    flags: u32,
}

const DETAIL: u32 = 1u;
const WORLD_UV: u32 = 2u;
const NORMAL_MAP: u32 = 4u;
const SURFACE: u32 = 8u;

// Textures, as indices into the bindless index table below.
const MASK: u32 = 0u;
const LAYER_R: u32 = 1u;
const LAYER_G: u32 = 2u;
const LAYER_B: u32 = 3u;
const LAYER_A: u32 = 4u;
const NORMAL: u32 = 5u;
const SURFACE_TEXTURE: u32 = 6u;

#ifdef BINDLESS
struct Indices {
    params: u32,
    // Texture and sampler of each texture, in the order of the constants above.
    textures: array<u32, 14>,
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
#endif

fn params(slot: u32) -> Params {
#ifdef BINDLESS
    return params_array[indices[slot].params];
#else
    return material_params;
#endif
}

// Samples texture `t` (MASK, …, SURFACE_TEXTURE) of the material in `slot`. `t` is a constant at
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
        default: { return textureSample(surface_texture, surface_sampler, uv); }
    }
#endif
}

// The package's tangent frame: B along increasing V on the surface, T = B × N. The
// gradient of V follows from the screen-space derivatives of position and UV
// (Schüler, "Followup: Normal Mapping Without Precomputed Tangents").
fn map_normal(N: vec3<f32>, position: vec3<f32>, uv: vec2<f32>, texel: vec3<f32>) -> vec3<f32> {
    let dp1 = dpdx(position);
    let dp2 = dpdy(position);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let grad_v = (cross(dp2, N) * duv1.y + cross(N, dp1) * duv2.y) * sign(dot(N, cross(dp1, dp2)));
    let length_squared = dot(grad_v, grad_v);
    if !(length_squared > 0.0) {
        return N;
    }
    let B = grad_v * inverseSqrt(length_squared);
    let T = cross(B, N);
    // Z from X and Y: the same for normalised maps, and right for two-channel ones.
    let xy = texel.xy * 2.0 - 1.0;
    let z = sqrt(max(1.0 - dot(xy, xy), 0.0));
    return normalize(xy.x * T + xy.y * B + z * N);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

#ifdef VERTEX_UVS_A
    let slot = mesh[in.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
    let p = params(slot);

    var reflection = p.reflection;
    if (p.flags & SURFACE) != 0u {
        let surface = sample(slot, SURFACE_TEXTURE, in.uv);
        reflection *= surface.r;
        pbr_input.material.perceptual_roughness *= surface.g;
        pbr_input.material.reflectance *= surface.a;
    }
    // Environment lighting's specular term only; highlights of lights stay.
    pbr_input.specular_occlusion *= reflection;

    if (p.flags & NORMAL_MAP) != 0u {
        let texel = sample(slot, NORMAL, in.uv).rgb;
        pbr_input.N = map_normal(pbr_input.world_normal, in.world_position.xyz, in.uv, texel);
    }

    if (p.flags & DETAIL) != 0u {
        // Simulation (x, y) is Bevy (x, -z).
        let uv = select(in.uv, vec2(in.world_position.x, -in.world_position.z), (p.flags & WORLD_UV) != 0u);
        let w = sample(slot, MASK, in.uv) * p.enabled;
        let d = w.r * sample(slot, LAYER_R, uv * p.scales.r).rgb
            + w.g * sample(slot, LAYER_G, uv * p.scales.g).rgb
            + w.b * sample(slot, LAYER_B, uv * p.scales.b).rgb
            + w.a * sample(slot, LAYER_A, uv * p.scales.a).rgb;
        let base = pbr_input.material.base_color;
        pbr_input.material.base_color = vec4(base.rgb * d * p.multiplier, base.a);
    }
#endif

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
