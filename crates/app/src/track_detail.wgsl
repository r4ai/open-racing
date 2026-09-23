// Standard PBR with the track package's detail layers multiplied into the base colour:
// base × multiplier × Σ maskᵢ · layerᵢ(scaleᵢ · uv), see `open_racing_track::Detail`.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}

struct Detail {
    scales: vec4<f32>,
    // 1 for the layers that are present, 0 otherwise.
    enabled: vec4<f32>,
    multiplier: f32,
    flags: u32,
}

const WORLD_UV: u32 = 1u;

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> detail: Detail;
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

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

#ifdef VERTEX_UVS_A
    // Simulation (x, y) is Bevy (x, -z).
    let uv = select(in.uv, vec2(in.world_position.x, -in.world_position.z), (detail.flags & WORLD_UV) != 0u);
    let w = textureSample(mask_texture, mask_sampler, in.uv) * detail.enabled;
    let d = w.r * textureSample(layer_r_texture, layer_r_sampler, uv * detail.scales.r).rgb
        + w.g * textureSample(layer_g_texture, layer_g_sampler, uv * detail.scales.g).rgb
        + w.b * textureSample(layer_b_texture, layer_b_sampler, uv * detail.scales.b).rgb
        + w.a * textureSample(layer_a_texture, layer_a_sampler, uv * detail.scales.a).rgb;
    let base = pbr_input.material.base_color;
    pbr_input.material.base_color = vec4(base.rgb * d * detail.multiplier, base.a);
#endif

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
