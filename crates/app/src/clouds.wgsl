// Clouds in four layers, from the simulation's weather (see `open_racing_sim::weather`):
// - cumulus and stratus/stratocumulus, ray marched as volumes. Cumulus have flat bases at
//   the condensation level and billowing tops that tower where the cloud map is dense;
//   stratus is a flatter, smoother deck;
// - altostratus/altocumulus and cirrus, thin sheets drawn where the view ray meets them.
//
// Where each layer has cloud comes from the simulation's cloud map (two patterns blended
// by `weights`, above `threshold`, at the layer's scale), so the clouds drawn are those
// that shade the road. Perlin–Worley noise breaks the map into billows and finer Worley
// noise erodes their edges. Sunlight is marched towards the sun through the cloud, with
// an approximation of multiple scattering (Wrenninge) and a two-lobed phase function for
// the silver lining; the sky lights the tops and the ground the bases.
//
// The mesh is a large sphere around the scene drawn at infinite depth, so it only
// covers pixels where the sky shows.

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    mesh_view_bindings::{view, globals},
}

struct Layer {
    // World heights of the base and of the highest tops.
    base: f32,
    top: f32,
    threshold: f32,
    cover: f32,
    // Shift of the layer's pattern (map X, Y), and the patterns' weights.
    offset: vec2<f32>,
    weights: vec2<f32>,
    // Size of the pattern relative to the map, and extinction per m at full density.
    scale: f32,
    extinction: f32,
    // Threshold of the convective cells.
    cell_threshold: f32,
    // Uniform arrays step in 16 bytes.
    _pad: vec3<f32>,
}

struct CloudParams {
    // Towards the sun, world space.
    sun_direction: vec3<f32>,
    steps: u32,
    // Sunlight reaching the clouds, lux.
    sun_illuminance: vec3<f32>,
    light_steps: u32,
    // Radiance of the sky over the clouds and of the ground under them, cd/m².
    sky_radiance: vec3<f32>,
    // Strength of the edge erosion by the detail noise, 0 turns it off.
    detail: f32,
    ground_radiance: vec3<f32>,
    // World height of the planet's centre.
    planet_centre: f32,
    // Radiance of the haze at the horizon, cd/m².
    horizon_radiance: vec3<f32>,
    // 1 to vary the ray start every frame (for TAA), 0 for a fixed pattern.
    temporal: f32,
    // Direction the cirrus streaks along (world X, Z).
    streaks: vec2<f32>,
    // Scale from the simulation's extinction to the drawn clouds'.
    density_scale: f32,
    _pad: f32,
    // How far the cumulus tops lean downwind of their bases (world X, Z), m.
    shear: vec2<f32>,
    _pad2: vec2<f32>,
    // Cumulus, stratus, middle, cirrus.
    layers: array<Layer, 4>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: CloudParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var cloud_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var cloud_map_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var shape_noise: texture_3d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var shape_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var detail_noise: texture_3d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var detail_sampler: sampler;

const PI: f32 = 3.14159265;
const EARTH_RADIUS: f32 = 6360000.0;
// Must match `CLOUD_MAP_PERIOD` and `CLOUD_SOFTNESS` in the simulation.
const MAP_PERIOD: f32 = 16384.0;
const SOFTNESS: f32 = 0.75;
// Must match `CELL_SOFTNESS` in the simulation.
const CELL_SOFTNESS: f32 = 0.15;
// Edge of the tiles of the shape and detail noise, m.
const SHAPE_TILE: f32 = 2600.0;
const DETAIL_TILE: f32 = 320.0;
// Distance over which far cloud fades into the haze, m.
const FADE_DISTANCE: f32 = 38000.0;
// Farthest the volumes are marched, and the farthest sheet drawn, m.
const MAX_MARCH: f32 = 50000.0;
const MAX_SHEET: f32 = 90000.0;
// Sheets are drawn as dense as the simulation's layers: their holes are resolved there.
const SHEET_DENSITY_SCALE: f32 = 1.0;
// Step along the view ray: a share of the distance, within these bounds at 64 steps, m.
const STEP_SHARE: f32 = 0.012;
const STEP_RANGE: vec2<f32> = vec2<f32>(25.0, 700.0);
// First step of the march towards the sun, m; each is longer by `LIGHT_GROWTH`.
const LIGHT_STEP: f32 = 40.0;
const LIGHT_GROWTH: f32 = 1.9;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    var clip = view.clip_from_world * vec4<f32>(out.world_position.xyz, 1.0);
    // Infinitely far (reverse Z): only where nothing else was drawn.
    clip.z = 0.0;
    out.position = clip;
    return out;
}

fn remap(x: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    return c + (x - a) / (b - a) * (d - c);
}

// Distance along the ray from inside a sphere to where it leaves it.
fn exit_sphere(ro: vec3<f32>, rd: vec3<f32>, centre: vec3<f32>, radius: f32) -> f32 {
    let oc = ro - centre;
    let b = dot(oc, rd);
    let c = dot(oc, oc) - radius * radius;
    let h = b * b - c;
    if h < 0.0 {
        return -1.0;
    }
    return -b + sqrt(h);
}

// Distance to where the ray meets a sphere from outside, or -1.
fn hit_sphere(ro: vec3<f32>, rd: vec3<f32>, centre: vec3<f32>, radius: f32) -> f32 {
    let oc = ro - centre;
    let b = dot(oc, rd);
    let c = dot(oc, oc) - radius * radius;
    let h = b * b - c;
    if h < 0.0 || b > 0.0 {
        return -1.0;
    }
    return -b - sqrt(h);
}

// Layer `i` over a world point: how far the map's regions allow the layer (0..1), and
// how far in from the edge of its convective cell the point is (0 at the edge, 1 at the
// middle; negative outside).
fn layer_map(i: u32, p: vec3<f32>) -> vec2<f32> {
    let l = params.layers[i];
    if l.cover <= 0.001 {
        return vec2<f32>(0.0, -1.0);
    }
    let at = (vec2<f32>(p.x, -p.z) - l.offset) / l.scale;
    let t = textureSampleLevel(cloud_map, cloud_map_sampler, at / MAP_PERIOD, 0.0);
    let potential = dot(l.weights, (t.rg * 255.0 - 128.0) / 32.0);
    let region = saturate((potential - l.threshold) / SOFTNESS);
    return vec2<f32>(region, (t.b - l.cell_threshold) / max(1.0 - l.cell_threshold, 0.05));
}

// Cover of layer `i` over a world point, 0..1, as `CloudLayer::density` in the
// simulation: the regions, the middle of each cell first.
fn layer_cover(i: u32, p: vec3<f32>) -> f32 {
    let m = layer_map(i, p);
    let l = params.layers[i];
    let cell = m.y * max(1.0 - l.cell_threshold, 0.05);
    return m.x * saturate(cell / CELL_SOFTNESS);
}

// Base cloud shape (Schneider): the Perlin–Worley noise dilated by the Worley FBM, so
// that the billows bulge out of a rounded mass, 0..1.
fn base_shape(q: vec3<f32>) -> f32 {
    let n = textureSampleLevel(shape_noise, shape_sampler, q / SHAPE_TILE, 0.0);
    let worley = n.g * 0.625 + n.b * 0.25 + n.a * 0.125;
    return saturate(remap(n.r, worley - 1.0, 1.0, 0.0, 1.0));
}

// Erodes the edges of density `d` with the detail noise (Schneider): wispy, curly
// fibres at the base, billows higher up.
fn erode(d: f32, q: vec3<f32>, h: f32, strength: f32) -> f32 {
    let n = textureSampleLevel(detail_noise, detail_sampler, q / DETAIL_TILE, 0.0);
    let fine = n.r * 0.625 + n.g * 0.25 + n.b * 0.125;
    let e = mix(fine, 1.0 - fine, saturate(h * 8.0)) * strength * params.detail;
    return saturate(remap(d, e, 1.0, 0.0, 1.0));
}

// Coverage-shaped density (Schneider): the shape where the cover lets it through, and
// thinner where the cover is light.
fn with_cover(shape: f32, cover: f32) -> f32 {
    return saturate(remap(shape, 1.0 - cover, 1.0, 0.0, 1.0)) * cover;
}

// Extinction per m at a point of the low volumes (cumulus and stratus), world height
// `y` of the point on the curved Earth; no detail for the light march.
fn extinction(p: vec3<f32>, y: f32, detailed: bool) -> f32 {
    var sigma = 0.0;
    // Cumulus: a flat base at the condensation level; each cell a dome, rising highest
    // over the middle of the cell, its top leaning downwind with the shear.
    let cu = params.layers[0];
    if y > cu.base && y < cu.top {
        let h = (y - cu.base) / (cu.top - cu.base);
        let lean = vec3<f32>(params.shear.x, 0.0, params.shear.y) * h;
        let m = layer_map(0u, p - lean);
        // A dome over the cell: as high as a hemisphere at this distance from the middle,
        // lower where the region's cover is thin.
        let x = saturate(m.y);
        let reach = sqrt(x * (2.0 - x)) * (0.45 + 0.55 * m.x);
        if m.x > 0.0 && m.y > 0.0 && h < reach {
            let hc = h / reach;
            let gradient = saturate(remap(hc, 0.0, 0.06, 0.0, 1.0))
                * saturate(remap(hc, 0.6, 1.0, 1.0, 0.0));
            // The shape noise always carves the surface into billows.
            let c = min(m.x * smoothstep(0.0, 0.35, m.y), 0.55);
            let q = p - vec3<f32>(cu.offset.x, 0.0, -cu.offset.y) - lean;
            var d = with_cover(base_shape(q) * gradient, c);
            if detailed && d > 0.0 {
                d = erode(d, q, hc, 0.6);
            }
            sigma += d * cu.extinction;
        }
    }
    // Stratus: a deck with a soft base and top, lumpy while it is broken into
    // stratocumulus cells, almost smooth when closed.
    let st = params.layers[1];
    if y > st.base && y < st.top {
        let c = layer_cover(1u, p);
        if c > 0.0 {
            let h = (y - st.base) / (st.top - st.base);
            let gradient = saturate(remap(h, 0.0, 0.2, 0.0, 1.0))
                * saturate(remap(h, 0.6, 1.0, 1.0, 0.0));
            let q = p - vec3<f32>(st.offset.x, 0.0, -st.offset.y);
            // Rolls of lumpy cloud; a closing deck smooths over.
            let shape = mix(base_shape(q * vec3<f32>(1.2, 2.0, 1.2)), 1.0, 0.35 * st.cover * st.cover);
            var d = with_cover(shape * gradient, min(c, 0.7 + 0.3 * st.cover));
            if detailed && d > 0.0 {
                d = erode(d, q, h, 0.5);
            }
            sigma += d * st.extinction;
        }
    }
    return sigma * params.density_scale;
}

fn henyey_greenstein(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(1.0 + g2 - 2.0 * g * cos_theta, 1.5));
}

// Strong forward scattering (the silver lining) with some back scattering; `scale`
// flattens it for the higher orders of scattering.
fn phase(cos_theta: f32, scale: f32) -> f32 {
    return mix(
        henyey_greenstein(cos_theta, -0.2 * scale),
        henyey_greenstein(cos_theta, 0.8 * scale),
        0.6,
    );
}

// Interleaved gradient noise.
fn ign(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
}

// Height of a point above the planet's centre, as a world height.
fn height(p: vec3<f32>, centre: vec3<f32>) -> f32 {
    return length(p - centre) + params.planet_centre;
}

struct Result {
    light: vec3<f32>,
    transmittance: f32,
    distance: f32,
}

// Ray marches the low volumes between `t_start` and `t_end`.
fn march(ro: vec3<f32>, rd: vec3<f32>, centre: vec3<f32>, t_start: f32, t_end: f32, jitter: f32) -> Result {
    var r: Result;
    r.transmittance = 1.0;
    r.light = vec3<f32>(0.0);
    let cos_theta = dot(rd, params.sun_direction);
    let quality = 64.0 / f32(params.steps);
    let low = min(params.layers[0].base, params.layers[1].base);
    let high = max(params.layers[0].top, params.layers[1].top);
    var weight_sum = 0.0;
    var depth_sum = 0.0;
    var t = t_start + clamp(t_start * STEP_SHARE, STEP_RANGE.x, STEP_RANGE.y) * quality * jitter;
    for (var i = 0u; i < params.steps; i++) {
        if t > t_end {
            break;
        }
        let dt = clamp(t * STEP_SHARE, STEP_RANGE.x, STEP_RANGE.y) * quality;
        let p = ro + rd * (t + 0.5 * dt);
        let y = height(p, centre);
        let sigma = extinction(p, y, true);
        if sigma <= 0.0 {
            // Empty air: stride on.
            t += dt * 1.6;
            continue;
        }
        // Optical depth towards the sun, in longer and longer steps.
        var tau = 0.0;
        var s = 0.0;
        var ls = LIGHT_STEP;
        for (var j = 0u; j < params.light_steps; j++) {
            let q = p + params.sun_direction * (s + 0.5 * ls);
            let yq = height(q, centre);
            if yq > high {
                break;
            }
            tau += extinction(q, yq, false) * ls;
            s += ls;
            ls *= LIGHT_GROWTH;
        }
        // Multiple scattering: octaves with less extinction and a flatter phase.
        // Thick cloud scatters light many times over: each octave keeps more of the
        // energy (a), spreads it more evenly (b) and reaches deeper (c).
        var sun = 0.0;
        var a = 1.0;
        var b = 1.0;
        var c = 1.0;
        for (var k = 0; k < 4; k++) {
            sun += a * phase(cos_theta, b) * exp(-tau * c);
            a *= 0.6;
            b *= 0.5;
            c *= 0.5;
        }
        // Powder (Schneider): light scattered towards the viewer has to come from inside,
        // so edges seen away from the sun are darker.
        let powder = mix(1.0 - exp(-sigma * 300.0), 1.0, saturate(cos_theta * 0.5 + 0.5));
        // Light scattered in from the sky above and the ground below: bright tops, grey
        // bases (Frostbite).
        let up = saturate((y - low) / max(high - low, 1.0));
        let ambient = params.sky_radiance * (0.15 + 0.4 * up) + params.ground_radiance * 0.35 * (1.0 - up);
        let scattering = params.sun_illuminance * sun * powder + ambient;
        let step_t = exp(-sigma * dt);
        let added = r.transmittance * (1.0 - step_t);
        r.light += scattering * added;
        depth_sum += t * added;
        weight_sum += added;
        r.transmittance *= step_t;
        if r.transmittance < 0.01 {
            break;
        }
        t += dt;
    }
    r.distance = depth_sum / max(weight_sum, 1e-6);
    return r;
}

// A thin sheet (altostratus/altocumulus, cirrus) where the view ray meets its middle.
fn sheet(i: u32, ro: vec3<f32>, rd: vec3<f32>, centre: vec3<f32>) -> Result {
    var r: Result;
    r.transmittance = 1.0;
    r.light = vec3<f32>(0.0);
    let l = params.layers[i];
    if l.cover <= 0.001 {
        return r;
    }
    let middle = 0.5 * (l.base + l.top);
    let t = exit_sphere(ro, rd, centre, middle - params.planet_centre);
    if t <= 0.0 || t > MAX_SHEET {
        return r;
    }
    let p = ro + rd * t;
    let c = layer_cover(i, p);
    if c <= 0.0 {
        return r;
    }
    let q = p - vec3<f32>(l.offset.x, 0.0, -l.offset.y);
    var d: f32;
    if i == 3u {
        // Cirrus: fibres drawn out along the upper wind and curled by it (a warp by the
        // coarser noise).
        let warp = textureSampleLevel(shape_noise, shape_sampler, vec3<f32>(q.xz / 9000.0, 0.7).xzy, 0.0).gb - 0.5;
        let w = q.xz + warp * 2500.0;
        let along = dot(w, params.streaks);
        let across = dot(w, vec2<f32>(-params.streaks.y, params.streaks.x));
        let n = textureSampleLevel(
            detail_noise, detail_sampler, vec3<f32>(along / 7000.0, across / 1400.0, 0.3), 0.0
        );
        let fibres = n.r * 0.5 + n.g * 0.35 + n.b * 0.15;
        d = saturate(remap(fibres, 1.0 - c, 1.0, 0.0, 1.0)) * c;
    } else {
        // Altocumulus: rows of small cells while broken; altostratus: a smooth sheet.
        let n = textureSampleLevel(shape_noise, shape_sampler, vec3<f32>(q.xz / 3000.0, 0.5).xzy, 0.0);
        let fine = textureSampleLevel(detail_noise, detail_sampler, vec3<f32>(q.xz / 1600.0, 0.5).xzy, 0.0);
        let cells = mix(n.g * 0.7 + n.b * 0.3, 1.0, c * c) * (0.55 + 0.45 * fine.r);
        d = saturate(remap(cells, 1.0 - c, 1.0, 0.0, 1.0)) * c;
    }
    // Along the view ray through the sheet, and the sun's ray down to its middle.
    let up = normalize(p - centre);
    let thickness = l.top - l.base;
    let k = d * l.extinction * SHEET_DENSITY_SCALE * thickness;
    let tau = k / max(abs(dot(rd, up)), 0.06);
    let tau_sun = 0.5 * k / max(dot(params.sun_direction, up), 0.06);
    let alpha = 1.0 - exp(-tau);
    let cos_theta = dot(rd, params.sun_direction);
    let sun = params.sun_illuminance * (phase(cos_theta, 1.0) * exp(-tau_sun) + 0.25 * phase(cos_theta, 0.3));
    r.light = (sun + params.sky_radiance) * alpha;
    r.transmittance = 1.0 - alpha;
    r.distance = t;
    return r;
}

// Fades light seen at `distance` into the haze at the horizon.
fn haze(light: vec3<f32>, alpha: f32, distance: f32) -> vec3<f32> {
    let fade = exp(-distance / FADE_DISTANCE);
    return light * fade + params.horizon_radiance * alpha * (1.0 - fade);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let ro = view.world_position;
    let rd = normalize(in.world_position.xyz - ro);
    let centre = vec3<f32>(ro.x, params.planet_centre, ro.z);
    // No cloud below the horizon.
    if hit_sphere(ro, rd, centre, EARTH_RADIUS) > 0.0 {
        return vec4<f32>(0.0);
    }
    var jitter = ign(in.position.xy);
    if params.temporal > 0.5 {
        jitter = fract(jitter + f32(globals.frame_count % 64u) * 0.618034);
    }

    // Low volumes.
    var light = vec3<f32>(0.0);
    var transmittance = 1.0;
    let low = min(params.layers[0].base, params.layers[1].base);
    let high = max(params.layers[0].top, params.layers[1].top);
    if params.layers[0].cover > 0.001 || params.layers[1].cover > 0.001 {
        let t_start = max(exit_sphere(ro, rd, centre, low - params.planet_centre), 0.0);
        let t_end = min(exit_sphere(ro, rd, centre, high - params.planet_centre), MAX_MARCH);
        if t_end > t_start {
            let r = march(ro, rd, centre, t_start, t_end, jitter);
            light = haze(r.light, 1.0 - r.transmittance, r.distance);
            transmittance = r.transmittance;
        }
    }
    // Sheets behind them, the higher behind the lower.
    for (var i = 2u; i < 4u; i++) {
        if transmittance < 0.01 {
            break;
        }
        let r = sheet(i, ro, rd, centre);
        light += transmittance * haze(r.light, 1.0 - r.transmittance, r.distance);
        transmittance *= r.transmittance;
    }
    let alpha = 1.0 - transmittance;
    if alpha <= 0.0 {
        return vec4<f32>(0.0);
    }
    // Premultiplied: the sky behind shows through by the transmittance.
    return vec4<f32>(light * view.exposure, alpha);
}
