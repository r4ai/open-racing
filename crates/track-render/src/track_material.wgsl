// Standard PBR with the track package's surface texture, normal map, reflection of the
// surroundings and detail layers, see `open_racing_track::Material`. The detail layers
// multiply into the base colour: base × multiplier × Σ maskᵢ · layerᵢ(scaleᵢ · uv), or,
// masked by the base colour's alpha, base × multiplier × lerp(layer_R(scale_R · uv), 1, α).
// A detail normal map, weighed like the R layer, adds its bumps to the normal map's.
// A plant's leaves take each copy's colour (see `track_plant.wgsl`).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_bindings::mesh,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}
#import open_racing::track_bindings::{
    params, sample, DETAIL, WORLD_UV, NORMAL_MAP, SURFACE, BASE_ALPHA_MASK, DETAIL_NORMAL_MAP,
    LEAVES, MASK, LAYER_R, LAYER_G, LAYER_B, LAYER_A, NORMAL, SURFACE_TEXTURE, DETAIL_NORMAL,
}
#import open_racing::track_plant::{leaf_colour, look}

// The package's tangent frame: B along increasing V on the surface, T = B × N. The
// gradient of V follows from the screen-space derivatives of position and UV
// (Schüler, "Followup: Normal Mapping Without Precomputed Tangents").
fn map_normal(N: vec3<f32>, position: vec3<f32>, uv: vec2<f32>, tangent_space: vec3<f32>) -> vec3<f32> {
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
    return normalize(tangent_space.x * T + tangent_space.y * B + tangent_space.z * N);
}

// The tangent-space normal of a normal map texel. Z from X and Y: the same for normalised
// maps, and right for two-channel ones.
fn unpack_normal(texel: vec3<f32>) -> vec3<f32> {
    let xy = texel.xy * 2.0 - 1.0;
    return vec3(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
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

    // Simulation (x, y) is Bevy (x, -z).
    let detail_uv = select(in.uv, vec2(in.world_position.x, -in.world_position.z), (p.flags & WORLD_UV) != 0u);
    let base = pbr_input.material.base_color;
    var w = vec4(1.0 - base.a, 0.0, 0.0, 0.0);
    if (p.flags & (DETAIL | BASE_ALPHA_MASK)) == DETAIL {
        w = sample(slot, MASK, in.uv) * p.enabled;
    }

    var normal = pbr_input.world_normal;
    if (p.flags & NORMAL_MAP) != 0u {
        normal = map_normal(normal, in.world_position.xyz, in.uv, unpack_normal(sample(slot, NORMAL, in.uv).rgb));
    }
    if (p.flags & DETAIL_NORMAL_MAP) != 0u {
        let bump = unpack_normal(sample(slot, DETAIL_NORMAL, detail_uv * p.detail_normal_scale).rgb);
        // A single detail map on a base-alpha material follows the R mask. Multi-layer
        // materials instead have one normal map over their whole world-projected surface.
        let weight = select(w.r, 1.0, (p.flags & WORLD_UV) != 0u);
        let detail_normal = map_normal(
            pbr_input.world_normal,
            in.world_position.xyz,
            detail_uv,
            vec3(bump.xy * p.detail_normal_strength * weight, 1.0),
        );
        normal = normalize(normal + detail_normal - pbr_input.world_normal);
    }
    if (p.flags & (NORMAL_MAP | DETAIL_NORMAL_MAP)) != 0u {
        pbr_input.N = normal;
    }

    if (p.flags & DETAIL) != 0u {
        let uv = detail_uv;
        var d: vec3<f32>;
        if (p.flags & BASE_ALPHA_MASK) != 0u {
            d = mix(sample(slot, LAYER_R, uv * p.scales.r).rgb, vec3(1.0), base.a);
        } else {
            d = w.r * sample(slot, LAYER_R, uv * p.scales.r).rgb
                + w.g * sample(slot, LAYER_G, uv * p.scales.g).rgb
                + w.b * sample(slot, LAYER_B, uv * p.scales.b).rgb
                + w.a * sample(slot, LAYER_A, uv * p.scales.a).rgb;
        }
        pbr_input.material.base_color = vec4(base.rgb * d * p.multiplier, base.a);
    }
    if (p.flags & LEAVES) != 0u {
        let c = pbr_input.material.base_color;
#ifdef VERTEX_UVS_B
        let copy = look(in.instance_index, in.uv_b, true);
#else
        let copy = look(in.instance_index, vec2(0.0), false);
#endif
        pbr_input.material.base_color = vec4(leaf_colour(c.rgb, copy), c.a);
    }
#endif

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
