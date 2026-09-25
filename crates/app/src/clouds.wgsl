// Moisture-driven cloud rendering. HDR radiance is pre-exposed before storage.
// Shape/erosion and scattering follow the published Horizon/Nubis approach;
// the large-scale density comes from the CPU moist-convection model.
struct Cirrus {
    altitude: f32, threshold: f32, cover: f32, scale: f32,
    offset: vec2<f32>, weights: vec2<f32>,
}
struct CloudParams {
    sun_direction: vec3<f32>, steps: u32,
    sun_illuminance: vec3<f32>, light_steps: u32,
    sky_radiance: vec3<f32>, detail: f32,
    ground_radiance: vec3<f32>, base_height: f32,
    horizon_radiance: vec3<f32>, blend: f32,
    shape_offset: vec2<f32>, detail_offset: vec2<f32>,
    frame_ages: vec2<f32>, streaks: vec2<f32>,
    cirrus_phase: vec2<f32>, noise_mean: vec2<f32>,
    upper_wind: vec2<f32>, occupied_bands: u32, march_base: f32,
    winds: array<vec4<f32>, 32>, cirrus: Cirrus,
}
struct Uniforms {
    params: CloudParams,
    world_from_clip: mat4x4<f32>, previous_clip_from_world: mat4x4<f32>,
    camera: vec4<f32>, previous_camera: vec4<f32>,
    resolution: vec2<f32>, motion: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: Uniforms;
struct Output { @location(0) color: vec4<f32>, @location(1) depth: f32 }
fn direction(uv: vec2<f32>) -> vec3<f32> {
    let p = u.world_from_clip * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.5, 1.0);
    return normalize(p.xyz / p.w - u.camera.xyz);
}
fn height(p: vec3<f32>) -> f32 {
    let horizontal = p.xz - u.camera.xz;
    return p.y - u.params.base_height + dot(horizontal, horizontal) / 12720000.0;
}
fn wind(h: f32) -> vec2<f32> {
    if h > 6000.0 { return u.params.upper_wind; }
    let z = clamp(h / 187.5 - 0.5, 0.0, 31.0);
    return mix(u.params.winds[u32(z)].zw, u.params.winds[min(u32(z)+1u,31u)].zw, fract(z));
}

#ifdef MARCH
@group(1) @binding(0) var cloud_map: texture_2d<f32>;
@group(1) @binding(1) var shape_noise: texture_3d<f32>;
@group(1) @binding(2) var detail_noise: texture_3d<f32>;
@group(1) @binding(3) var field: texture_3d<f32>;
@group(1) @binding(4) var repeating: sampler;

fn field_density(p: vec3<f32>) -> f32 {
    let h = height(p);
    if h < 0.0 || h >= 6000.0 { return 0.0; }
    let z = clamp(h / 187.5 - 0.5, 0.0, 31.0);
    let winds = mix(u.params.winds[u32(z)], u.params.winds[min(u32(z)+1u,31u)], fract(z));
    let a = vec2<f32>(p.x, -p.z) - winds.xy * u.params.frame_ages.x;
    let b = vec2<f32>(p.x, -p.z) - winds.zw * u.params.frame_ages.y;
    let y = clamp(h / 6000.0, 0.5/32.0, 31.5/32.0);
    let previous = textureSampleLevel(field, repeating, vec3<f32>(a.x/16384.0, y, a.y/16384.0), 0.0).r;
    let current = textureSampleLevel(field, repeating, vec3<f32>(b.x/16384.0, y, b.y/16384.0), 0.0).g;
    return mix(previous, current, u.params.blend);
}

fn density(p: vec3<f32>, footprint: f32, detailed: bool) -> f32 {
    let coarse = field_density(p);
    if coarse < 0.000015 { return 0.0; }
    let q = p - vec3<f32>(u.params.shape_offset.x, u.params.base_height, -u.params.shape_offset.y);
    let lod = max(log2(max(footprint, 1.0) / (2600.0/64.0)), 0.0);
    let n = textureSampleLevel(shape_noise, repeating, q/2600.0, lod);
    // Broad mass dominates. Detail only sculpts the dilute boundary, never turns
    // the whole volume into a stack of equal Worley bubbles.
    let edge = 1.0 - smoothstep(0.015, 0.05, coarse);
    // The eroded shape was mip-filtered on the CPU. Normalising by its measured
    // mean preserves the coarse liquid-water extinction at every footprint.
    var shaped = coarse * mix(1.0, n.r / max(u.params.noise_mean.x,0.001), edge);
    if detailed && u.params.detail > 0.0 && footprint < 160.0 {
        let d = p - vec3<f32>(u.params.detail_offset.x, u.params.base_height, -u.params.detail_offset.y);
        let fine = textureSampleLevel(detail_noise, repeating, d/320.0, max(log2(max(footprint,1.0)/10.0), 0.0)).r;
        let erosion = 0.28 * edge * u.params.detail * (1.0-smoothstep(60.0,160.0,footprint));
        shaped *= (1.0 - erosion * fine) / (1.0 - erosion * u.params.noise_mean.y);
    }
    return shaped;
}

// Stable local curved-layer intersection: no subtraction of Earth-radius squares.
fn shell(rd: vec3<f32>, altitude: f32) -> f32 {
    let a = max(dot(rd.xz, rd.xz) / 12720000.0, 1e-12);
    let c = altitude - (u.camera.y - u.params.base_height);
    let root = sqrt(max(rd.y*rd.y + 4.0*a*c, 0.0));
    if rd.y >= 0.0 { return max(2.0*c/max(rd.y+root,1e-8),0.0); }
    return max((-rd.y + root)/(2.0*a),0.0);
}
fn hg(c: f32, g: f32) -> f32 {
    return (1.0-g*g) / (12.5663706 * pow(max(1.0+g*g-2.0*g*c,0.01),1.5));
}
fn phase(c: f32, g: f32) -> f32 { return 0.7*hg(c,0.75*g) + 0.3*hg(c,-0.2*g); }
fn noise(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189*fract(dot(pixel,vec2<f32>(0.06711056,0.00583715))) + u.motion.z*0.61803398875);
}
@fragment
fn fragment(@location(0) uv: vec2<f32>, @builtin(position) pixel: vec4<f32>) -> Output {
    let rd = direction(uv);
    var out: Output;
    out.color = vec4<f32>(0.0,0.0,0.0,1.0);
    out.depth = 60000.0;
    if rd.y < -0.035 { return out; }
    let start = shell(rd, u.params.march_base);
    let end = min(shell(rd, 6000.0), 50000.0);
    // Spend the view budget inside potentially occupied intervals, not in the
    // kilometres of clear air between low cloud and the top of the domain.
    var occupied_distance = 0.0;
    for (var band=0u;band<32u;band++) {
        if (u.params.occupied_bands & (1u << band)) != 0u {
            let a = max(start,shell(rd,f32(band)*187.5));
            let b = min(end,shell(rd,f32(band+1u)*187.5));
            occupied_distance += max(b-a,0.0);
        }
    }
    let dt = max(occupied_distance/f32(u.params.steps), 1.0);
    var t = start;
    let jitter = noise(pixel.xy);
    var light = vec3<f32>(0.0);
    var transmittance = 1.0;
    var depth_sum = 0.0;
    var weight = 0.0;
    let cos_theta = dot(rd,u.params.sun_direction);
    let phase0 = phase(cos_theta,1.0);
    let phase1 = phase(cos_theta,0.5);
    let phase2 = phase(cos_theta,0.25);
    var jumps = 0u;
    for (var i=0u; i<u.params.steps;) {
        if t >= end || transmittance < 0.01 { break; }
        let step_length = min(dt,end-t);
        let sample_t = t + jitter*step_length;
        let p = u.camera.xyz + rd*sample_t;
        let band = u32(clamp(height(p)/187.5,0.0,31.0));
        if (u.params.occupied_bands & (1u << band)) == 0u {
            // Jump to the next occupied height interval. Occupancy is dilated
            // over both snapshots, so interpolation/wind cannot reveal a hole.
            t = max(t+step_length,shell(rd,f32(band+1u)*187.5)+0.01);
            jumps += 1u;
            if jumps > 32u { break; }
            continue;
        }
        i += 1u;
        let footprint = max(sample_t*u.motion.w,step_length*0.5);
        let sigma = density(p,footprint,true);
        if sigma > 0.0 {
            var tau = 0.0;
            var distance = 0.0;
            var ls = 100.0;
            for (var j=0u;j<u.params.light_steps;j++) {
                tau += density(p + u.params.sun_direction*(distance+ls*0.5),ls,false)*ls;
                distance += ls;
                ls *= 2.0;
            }
            let sun = phase0*exp(-tau) + 0.5*phase1*exp(-tau*0.45) + 0.25*phase2*exp(-tau*0.2);
            let sky_visibility = exp(-field_density(p+vec3<f32>(0.0,250.0,0.0))*200.0);
            let ambient = u.params.sky_radiance*(0.2+0.55*sky_visibility) + u.params.ground_radiance*0.18;
            let step_t = exp(-sigma*step_length);
            let added = transmittance*(1.0-step_t);
            let haze = exp(-sample_t/50000.0);
            light += added * mix(u.params.horizon_radiance,u.params.sun_illuminance*sun+ambient,haze);
            depth_sum += sample_t*added;
            weight += added;
            transmittance *= step_t;
        }
        // Empty samples skip the expensive light march.
        t += step_length;
    }
    // Thin ice cloud above the moist simulation domain.
    let cirrus = u.params.cirrus;
    if cirrus.cover > 0.001 && transmittance > 0.01 {
        let ct = shell(rd,cirrus.altitude);
        if ct < 90000.0 {
            let p = u.camera.xyz + rd*ct;
            let at = (vec2<f32>(p.x,-p.z)-cirrus.offset)/cirrus.scale/16384.0;
            let map = textureSampleLevel(cloud_map,repeating,at,0.0);
            let cover = clamp((dot(cirrus.weights,(map.rg*255.0-128.0)/32.0)-cirrus.threshold)/0.75,0.0,1.0);
            let along = dot(p.xz,u.params.streaks)/7000.0 - u.params.cirrus_phase.x;
            let across = dot(p.xz,vec2<f32>(-u.params.streaks.y,u.params.streaks.x))/1200.0 - u.params.cirrus_phase.y;
            let fibres = textureSampleLevel(detail_noise,repeating,vec3<f32>(along,across,0.4),1.0).r;
            let alpha = 1.0-exp(-cover*fibres*0.45/max(rd.y,0.12));
            let added = transmittance*alpha;
            light += added * mix(u.params.horizon_radiance,u.params.sky_radiance+u.params.sun_illuminance*phase1*0.4,exp(-ct/50000.0));
            depth_sum += ct*added; weight += added; transmittance *= 1.0-alpha;
        }
    }
    out.color = vec4<f32>(light*u.camera.w, transmittance);
    if weight > 0.0001 { out.depth = depth_sum/weight; }
    return out;
}
#endif

#ifdef RESOLVE
@group(1) @binding(0) var current: texture_2d<f32>;
@group(1) @binding(1) var current_depth: texture_2d<f32>;
@group(1) @binding(2) var history: texture_2d<f32>;
@group(1) @binding(3) var history_depth: texture_2d<f32>;
@group(1) @binding(4) var linear: sampler;
@fragment
fn fragment(@location(0) uv: vec2<f32>, @builtin(position) pixel: vec4<f32>) -> Output {
    let size = vec2<i32>(u.resolution.xy);
    let at = clamp(vec2<i32>(pixel.xy),vec2<i32>(0),size-1);
    let now = textureLoad(current,at,0);
    let depth = textureLoad(current_depth,at,0).r;
    var out = Output(now,depth);
    if u.motion.y <= 0.0 { return out; }
    let world = u.camera.xyz + direction(uv)*depth;
    let velocity = wind(height(world));
    let previous_world = world - vec3<f32>(velocity.x,0.0,-velocity.y)*u.motion.x;
    let clip = u.previous_clip_from_world*vec4<f32>(previous_world,1.0);
    let previous_uv = clip.xy/clip.w*vec2<f32>(0.5,-0.5)+0.5;
    if clip.w <= 0.0 || any(previous_uv < vec2<f32>(0.0)) || any(previous_uv > vec2<f32>(1.0)) { return out; }
    let old_at = clamp(vec2<i32>(previous_uv*u.resolution.xy),vec2<i32>(0),size-1);
    let old_depth = textureLoad(history_depth,old_at,0).r;
    let expected_depth = distance(previous_world,u.previous_camera.xyz);
    if abs(old_depth-expected_depth) > max(200.0,depth*0.08) { return out; }
    var old = textureSampleLevel(history,linear,previous_uv,0.0);
    old = vec4<f32>(old.rgb*u.camera.w/max(u.previous_camera.w,1e-8),old.a);
    var lo = now; var hi = now;
    for(var y=-1;y<=1;y++) { for(var x=-1;x<=1;x++) {
        let c = textureLoad(current,clamp(at+vec2<i32>(x,y),vec2<i32>(0),size-1),0);
        lo = min(lo,c); hi = max(hi,c);
    }}
    let weight = u.motion.y * (1.0-smoothstep(0.04,0.25,abs(old.a-now.a)));
    out.color = mix(now,clamp(old,lo,hi),weight);
    return out;
}
#endif

#ifdef COMPOSITE
@group(1) @binding(0) var resolved: texture_2d<f32>;
@group(1) @binding(1) var linear: sampler;
struct Composite { @location(0) color: vec4<f32>, @builtin(frag_depth) depth: f32 }
@fragment
fn fragment(@location(0) uv: vec2<f32>) -> Composite {
    let cloud = textureSampleLevel(resolved,linear,uv,0.0);
    // Reverse-Z sky only, per MSAA sample: geometry silhouettes stay crisp.
    return Composite(vec4<f32>(cloud.rgb,1.0-cloud.a),0.0);
}
#endif
