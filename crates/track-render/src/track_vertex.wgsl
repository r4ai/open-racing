// The track material's vertex shader: Bevy's, with plants moved by the wind (see
// `track_plant.wgsl`). Track meshes are neither skinned nor morphed.

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
}
#import bevy_pbr::mesh_view_bindings::globals
#import open_racing::track_plant::{impostor, is_impostor, look, moved}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef VERTEX_POSITIONS
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
#ifdef VERTEX_NORMALS
    let normal = out.world_normal;
#else
    let normal = vec3(0.0, 1.0, 0.0);
#endif
#ifdef VERTEX_UVS_B
    let copy = look(vertex.instance_index, vertex.uv_b, true);
#else
    let copy = look(vertex.instance_index, vec2(0.0), false);
#endif
#ifdef VERTEX_NORMALS
#ifdef VERTEX_UVS_A
    if is_impostor(vertex.instance_index) {
        let axis = (world_from_local * vec4(vertex.normal, 0.0)).xyz;
        let corner = impostor(out.world_position.xyz, axis, vertex.uv, vertex.instance_index, copy);
        out.world_position = vec4(corner.world, 1.0);
        out.world_normal = corner.normal;
        out.uv = corner.uv;
    } else {
        out.world_position = vec4(moved(out.world_position.xyz, normal, world_from_local[3].xyz, vertex.instance_index, globals.time, copy), 1.0);
    }
#else
    out.world_position = vec4(moved(out.world_position.xyz, normal, world_from_local[3].xyz, vertex.instance_index, globals.time, copy), 1.0);
#endif
#else
    out.world_position = vec4(moved(out.world_position.xyz, normal, world_from_local[3].xyz, vertex.instance_index, globals.time, copy), 1.0);
#endif
    out.position = position_world_to_clip(out.world_position.xyz);
#endif

#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, vertex.tangent, vertex.instance_index);
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex.instance_index, world_from_local[3]);
#endif

    return out;
}
