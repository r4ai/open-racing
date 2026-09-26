// Plants: copies swaying in the wind, their leaves fluttering, and each copy's own
// leaf colour, or none at all when it is bare. See `open_racing_track::Varies` and
// `open_racing_track::Instance::tint`.
#define_import_path open_racing::track_plant

#import bevy_pbr::{mesh_bindings::mesh, mesh_functions}
#import bevy_pbr::mesh_view_bindings::view
#import open_racing::track_bindings::{params, wind, IMPOSTOR, TINTED, WIND}

// The wind `sway` and `flutter` are given at, m/s.
const REFERENCE_WIND: f32 = 10.0;
// The brightness a leaf colour of a copy keeps its leaves at: brighter colours lighten
// them, darker ones darken them.
const REFERENCE_LUMINANCE: f32 = 0.2;

fn slot(instance_index: u32) -> u32 {
    return mesh[instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
}

// A copy's leaves' look, its four bytes packed: that of the entity drawing the copy,
// or, where copies are merged into one mesh (with a second set of UVs), that of its
// vertices, each UV two of the bytes.
fn look(instance_index: u32, uv_b: vec2<f32>, merged: bool) -> u32 {
    if merged {
        let v = vec2<u32>(round(uv_b));
        return (v.x & 0xffffu) | ((v.y & 0xffffu) << 16u);
    }
    return mesh_functions::get_tag(instance_index);
}

// A look as (sRGB colour, how much of it, 0 to 1), the amount negative when the copy is
// bare. Copies without one keep their own colour.
fn unpack(look: u32) -> vec4<f32> {
    let amount = look >> 24u;
    let colour = vec3(f32(look & 0xffu), f32((look >> 8u) & 0xffu), f32((look >> 16u) & 0xffu)) / 255.0;
    return vec4(colour, select(f32(amount) / 254.0, -1.0, amount == 255u));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

// A point of a copy (`world`, with its normal, in Bevy's axes) moved by the wind: the
// whole plant bends away from it by the square of the height above its foot `origin`,
// in gusts that sweep over the ground with the wind, and sways back and forth (`t` is
// the time, s); leaves flutter. The leaves of a bare copy (`look`) shrink to its foot,
// drawing nothing.
fn moved(world: vec3<f32>, normal: vec3<f32>, origin: vec3<f32>, instance_index: u32, t: f32, look: u32) -> vec3<f32> {
    let s = slot(instance_index);
    let p = params(s);
    if (p.flags & TINTED) != 0u && unpack(look).w < 0.0 {
        return origin;
    }
    if (p.flags & WIND) == 0u {
        return world;
    }
    let w = wind(s);
    let speed = length(w.xy);
    let dir = select(vec2(1.0, 0.0), w.xy / max(speed, 1e-3), speed > 1e-3);
    let k = speed / REFERENCE_WIND;
    // Gust fronts some 60 m apart, carried along by the wind.
    let along = dot(origin.xz, dir) - speed * t;
    let gust = 0.6 + 0.3 * sin(along * 0.1) + 0.1 * sin(along * 0.37 + 1.3);
    // Each plant's own phase, from where it stands.
    let phase = fract(sin(dot(origin.xz, vec2(12.9898, 78.233))) * 43758.547) * 6.2832;
    let h = max(world.y - origin.y, 0.0);
    let bend = p.sway * h * h * (k * k * gust + 0.2 * k * sin(t * 2.2 + phase));
    var out = world + vec3(dir.x, 0.0, dir.y) * bend;
    if p.flutter > 0.0 {
        let wave = sin(t * 9.0 + dot(world, vec3(1.7, 2.3, 1.3)) * 2.0 + phase);
        out += normal * p.flutter * k * (0.4 + gust) * wave;
    }
    return out;
}

// A base colour in a copy's colour (`look`), for materials that take it, keeping
// its brightness.
fn tinted(base: vec3<f32>, look: u32) -> vec3<f32> {
    let l = unpack(look);
    if l.w <= 0.0 {
        return base;
    }
    let luminance = dot(base, vec3(0.2126, 0.7152, 0.0722));
    let tinted = srgb_to_linear(l.rgb) * (luminance / REFERENCE_LUMINANCE);
    return mix(base, tinted, l.w);
}

// An impostor's pictures are seen from FRAMES × FRAMES sides: see
// `open_racing_track::IMPOSTOR_FRAMES`.
const FRAMES: f32 = 8.0;

// Whether the material in `instance_index`'s slot is an impostor's.
fn is_impostor(instance_index: u32) -> bool {
    return (params(slot(instance_index)).flags & IMPOSTOR) != 0u;
}

// A corner of an impostor's quad, in Bevy's axes: where it is, the quad's normal, and
// the UV of the picture it shows.
struct Corner {
    world: vec3<f32>,
    normal: vec3<f32>,
    uv: vec2<f32>,
}

// Corner `uv` of an impostor's quad about the copy's middle `centre`, whose X axis is
// `axis` (as long as the radius of the sphere round the copy): the quad turned square
// to the side its picture nearest the viewer (the camera, or the sun in a shadow map)
// was drawn from, and the UV of that picture in the texture. The leaves of a bare copy
// (`look`) shrink to its middle, drawing nothing.
fn impostor(centre: vec3<f32>, axis: vec3<f32>, uv: vec2<f32>, instance_index: u32, look: u32) -> Corner {
    var out: Corner;
    let radius = length(axis);
    let x = axis / max(radius, 1e-6);
    let up = vec3(0.0, 1.0, 0.0);
    // The model's y (the simulation's axes: z up) in Bevy's.
    let y = normalize(cross(up, x));
    var toward: vec3<f32>;
    if view.clip_from_view[3][3] == 1.0 {
        toward = normalize(view.world_from_view[2].xyz);
    } else {
        toward = normalize(view.world_position - centre);
    }
    let d = vec3(dot(toward, x), dot(toward, y), max(dot(toward, up), 0.0));
    // Hemi-octahedral coordinates, and the picture whose side is nearest.
    let o = d.xy / max(abs(d.x) + abs(d.y) + d.z, 1e-6);
    let h = vec2(o.x + o.y, o.x - o.y) * 0.5 + 0.5;
    let cell = clamp(floor(h * FRAMES), vec2(0.0), vec2(FRAMES - 1.0));
    let fh = (cell + 0.5) / FRAMES;
    let fo = vec2(fh.x + fh.y - 1.0, fh.x - fh.y);
    let side = normalize(vec3(fo, max(1.0 - abs(fo.x) - abs(fo.y), 0.0)));
    // The picture's axes, as it was drawn (see `impostor::octahedral`).
    var right = vec3(-side.y, side.x, 0.0);
    let across = length(right);
    right = select(vec3(1.0, 0.0, 0.0), right / across, across > 1e-4);
    let above = cross(side, right);
    let world = mat3x3(x, y, up);
    out.normal = world * side;
    out.uv = (cell + uv) / FRAMES;
    let p = params(slot(instance_index));
    if (p.flags & TINTED) != 0u && unpack(look).w < 0.0 {
        out.world = centre;
        return out;
    }
    let c = vec2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    out.world = centre + (world * right * c.x + world * above * c.y) * radius;
    return out;
}
