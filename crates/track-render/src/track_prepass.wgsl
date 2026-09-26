// The track material's vertex shader in the prepasses and shadow maps: Bevy's, with
// plants moved by the wind as in the main pass (see `track_plant.wgsl`), so that their
// depth and shadows follow them. Track meshes are neither skinned nor morphed.

#import bevy_pbr::{
    mesh_functions,
    prepass_io::VertexOutput,
    view_transformations::position_world_to_clip,
}
#import bevy_render::globals::Globals
#import open_racing::track_plant::{impostor, is_impostor, look, moved}

// The prepasses bind the time as their second view binding, where the main pass has
// lights.
@group(0) @binding(1) var<uniform> prepass_globals: Globals;

// Bevy's prepass vertex, with the normals in every prepass (`TRACK_NORMALS`): an
// impostor's say where its quad reaches. Track meshes are neither skinned nor morphed
// and have no tangents.
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(1) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(2) uv_b: vec2<f32>,
#endif
#ifdef TRACK_NORMALS
    @location(3) normal: vec3<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(7) color: vec4<f32>,
#endif
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let origin = world_from_local[3].xyz;

    // The normal only nudges fluttering leaves, which the prepass may lack.
#ifdef TRACK_NORMALS
    let normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#else
    let normal = vec3(0.0, 1.0, 0.0);
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    out.world_normal = normal;
#endif

    let world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
#ifdef VERTEX_UVS_B
    let copy = look(vertex.instance_index, vertex.uv_b, true);
#else
    let copy = look(vertex.instance_index, vec2(0.0), false);
#endif
    out.world_position = vec4(moved(world.xyz, normal, origin, vertex.instance_index, prepass_globals.time, copy), 1.0);
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
    // An impostor's quad turned to the viewer, or to the sun in a shadow map; its normal
    // and UVs are the vertex's, whether or not the pass has normals.
    var impostor_uv = vec2(0.0);
    var axis = vec3(0.0);
#ifdef TRACK_NORMALS
    axis = (world_from_local * vec4(vertex.normal, 0.0)).xyz;
#endif
    let is_picture = is_impostor(vertex.instance_index);
    if is_picture {
#ifdef VERTEX_UVS_A
        impostor_uv = vertex.uv;
#endif
        let corner = impostor(world.xyz, axis, impostor_uv, vertex.instance_index, copy);
        out.world_position = vec4(corner.world, 1.0);
#ifdef VERTEX_UVS_A
        out.uv = corner.uv;
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
        out.world_normal = corner.normal;
#endif
    }
    out.position = position_world_to_clip(out.world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif

#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef MOTION_VECTOR_PREPASS
    // Where it was last frame, taken as moved by the wind as it is now.
    let previous = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    let before = mesh_functions::mesh_position_local_to_world(previous, vec4<f32>(vertex.position, 1.0));
    out.previous_world_position = vec4(before.xyz + out.world_position.xyz - world.xyz, 1.0);
    if is_picture {
        out.previous_world_position = out.world_position;
    }
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex.instance_index, world_from_local[3]);
#endif

    return out;
}
