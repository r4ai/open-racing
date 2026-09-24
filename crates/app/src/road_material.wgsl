// Rubber, marbles and dirt over the road, see `track_surface.rs`. A small state texture
// holds one texel per patch of road (R rubber, G marbles, B dirt cover / 2, A kind of
// dirt: 0 grass, 0.5 earth, 1 grit), filtered smoothly between patches; a tiling detail
// texture breaks it up into streaks, clumps and pellets a few centimetres across.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}

struct RoadParams {
    // Size of the state grid in metres: across, along the track.
    size: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> road: RoadParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var state_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var state_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var detail_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var detail_sampler: sampler;

// Linear colours.
const DUST: vec3<f32> = vec3(0.37, 0.33, 0.25);
const RUBBER: vec3<f32> = vec3(0.004, 0.004, 0.004);
const MARBLE: vec3<f32> = vec3(0.006, 0.006, 0.006);
const GRASS: vec3<f32> = vec3(0.07, 0.08, 0.025);
const EARTH: vec3<f32> = vec3(0.11, 0.06, 0.025);
const GRIT: vec3<f32> = vec3(0.34, 0.29, 0.19);

// Layer `color` with opacity `a` over the premultiplied `acc`.
fn over(acc: vec4<f32>, color: vec3<f32>, a: f32) -> vec4<f32> {
    let k = clamp(a, 0.0, 1.0);
    return vec4(acc.rgb * (1.0 - k) + color * k, acc.a * (1.0 - k) + k);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

#ifdef VERTEX_UVS_A
    let state = textureSample(state_texture, state_sampler, in.uv);
    // Metres across and along the track.
    let m = in.uv * road.size.xy;
    // Clumps smeared along the direction of travel, fine grain, tyre streaks, pellets.
    let clumps = textureSample(detail_texture, detail_sampler, m / vec2(1.6, 3.2)).r;
    let grain = textureSample(detail_texture, detail_sampler, m / vec2(0.3, 0.45)).a;
    let streaks = textureSample(detail_texture, detail_sampler, m / vec2(0.9, 14.0)).g;
    let pellets = textureSample(detail_texture, detail_sampler, m / vec2(0.35, 0.35)).b;

    let rubber = state.r;
    let marbles = state.g;
    let cover = state.b * 2.0;
    let kind = state.a;

    var acc = vec4(0.0);
    acc = over(acc, DUST, 0.22 * (1.0 - rubber) * (0.5 + clumps));
    let rubbered = smoothstep(0.35, 1.0, rubber);
    acc = over(acc, RUBBER, 0.8 * rubbered * (0.6 + 0.6 * streaks));
    // Pellets appear one by one as the marbles thicken.
    acc = over(acc, MARBLE, 0.85 * smoothstep(0.9, 0.96, pellets + 0.45 * marbles));
    // Dirt: thin cover shows as scattered clumps that grow together as it thickens,
    // and break up again as tyres sweep it away.
    let coverage = 1.0 - exp(-3.0 * cover);
    let threshold = 0.75 * clumps + 0.25 * grain;
    let dirt = smoothstep(threshold - 0.06, threshold + 0.06, coverage) * step(0.001, cover);
    let color = select(mix(GRASS, EARTH, kind * 2.0), mix(EARTH, GRIT, kind * 2.0 - 1.0), kind > 0.5);
    acc = over(acc, color * (0.75 + 0.5 * grain), 0.92 * dirt);

    // The vertex colour's alpha masks everything but asphalt.
    let alpha = acc.a * pbr_input.material.base_color.a;
    var rgb = vec3(0.0);
    if acc.a > 1e-4 {
        rgb = acc.rgb / acc.a;
    }
    pbr_input.material.base_color = vec4(rgb, alpha);
    // Rubbered asphalt is smoother; dust and dirt are matt.
    pbr_input.material.perceptual_roughness = mix(0.9, 0.55, rubbered * (1.0 - dirt));
#endif

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
