//! A plan view of a project as an image: the ground in its materials' colours (its
//! painted layers too), shaded by its slope, the scatters' copies as dots, with a 100 m
//! grid, the nodes numbered, and the start line, sector lines, grid slots and pit boxes. Lets an agent (or a person without the editor) see
//! what a project looks like.

use std::collections::HashMap;

use glam::{DVec2, DVec3};

use crate::bake::Scene;
use crate::builtin;
use crate::project::{Project, TextureSource};
use crate::road::{MeshData, Solid};

/// An 8-bit RGB image, rows top (north) to bottom.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Picture {
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("PNG header");
        w.write_image_data(&self.pixels).expect("PNG data");
        w.finish().expect("PNG end");
        out
    }
}

/// Maps the plane onto pixels.
struct View {
    lo: DVec2,
    scale: f64,
    width: usize,
    height: usize,
}

impl View {
    fn px(&self, p: DVec3) -> DVec2 {
        DVec2::new(
            (p.x - self.lo.x) * self.scale,
            self.height as f64 - (p.y - self.lo.y) * self.scale,
        )
    }
}

struct Canvas {
    view: View,
    rgb: Vec<[f32; 3]>,
    depth: Vec<f32>,
}

impl Canvas {
    fn set(&mut self, x: i64, y: i64, c: [f32; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.view.width && (y as usize) < self.view.height {
            self.rgb[y as usize * self.view.width + x as usize] = c;
        }
    }

    /// Fills the mesh's triangles, keeping the highest surface at each pixel.
    fn mesh(&mut self, m: &MeshData, color: [f32; 3]) {
        let sun = DVec3::new(-0.4, 0.5, 0.77).normalize();
        for &[a, b, c] in m.indices.as_chunks::<3>().0 {
            let p = [a, b, c].map(|i| DVec3::from(m.positions[i as usize].map(f64::from)));
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            // Walls seen from above show only their tops.
            if n.z.abs() < 0.2 {
                continue;
            }
            let shade = (0.45 + 0.55 * n.abs().dot(sun).max(0.0)) as f32;
            let c = color.map(|x| x * shade);
            let q = p.map(|p| self.view.px(p));
            let (min, max) = (q[0].min(q[1]).min(q[2]), q[0].max(q[1]).max(q[2]));
            let area = (q[1] - q[0]).perp_dot(q[2] - q[0]);
            if area.abs() < 1e-12 {
                continue;
            }
            for y in (min.y.floor().max(0.0) as usize)
                ..=(max.y.ceil() as usize).min(self.view.height - 1)
            {
                for x in (min.x.floor().max(0.0) as usize)
                    ..=(max.x.ceil() as usize).min(self.view.width - 1)
                {
                    let s = DVec2::new(x as f64 + 0.5, y as f64 + 0.5);
                    let w0 = (q[2] - q[1]).perp_dot(s - q[1]) / area;
                    let w1 = (q[0] - q[2]).perp_dot(s - q[2]) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = (w0 * p[0].z + w1 * p[1].z + w2 * p[2].z) as f32;
                    let i = y * self.view.width + x;
                    if z >= self.depth[i] {
                        self.depth[i] = z;
                        self.rgb[i] = c;
                    }
                }
            }
        }
    }

    fn line(&mut self, a: DVec3, b: DVec3, width: f64, c: [f32; 3]) {
        let (pa, pb) = (self.view.px(a), self.view.px(b));
        let steps = pa.distance(pb).ceil().max(1.0) as usize;
        let r = (width * 0.5).max(0.5);
        for k in 0..=steps {
            let p = pa.lerp(pb, k as f64 / steps as f64);
            self.disc(p, r, c);
        }
    }

    fn disc(&mut self, p: DVec2, r: f64, c: [f32; 3]) {
        let ri = r.ceil() as i64;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                if ((dx * dx + dy * dy) as f64) <= r * r {
                    self.set(p.x as i64 + dx, p.y as i64 + dy, c);
                }
            }
        }
    }

    /// Writes a number in a 3 × 5 pixel font scaled by `size`, outlined in black.
    fn number(&mut self, p: DVec2, n: usize, size: i64, c: [f32; 3]) {
        const DIGITS: [[u8; 5]; 10] = [
            [7, 5, 5, 5, 7],
            [2, 6, 2, 2, 7],
            [7, 1, 7, 4, 7],
            [7, 1, 7, 1, 7],
            [5, 5, 7, 1, 1],
            [7, 4, 7, 1, 7],
            [7, 4, 7, 5, 7],
            [7, 1, 1, 1, 1],
            [7, 5, 7, 5, 7],
            [7, 5, 7, 1, 7],
        ];
        let (x0, y0) = (p.x as i64, p.y as i64);
        let lit: Vec<(i64, i64)> = n
            .to_string()
            .bytes()
            .enumerate()
            .flat_map(|(i, ch)| {
                let glyph = DIGITS[(ch - b'0') as usize];
                (0..5).flat_map(move |row| {
                    (0..3)
                        .filter(move |col| glyph[row] & (4 >> col) != 0)
                        .map(move |col| (x0 + (i as i64 * 4 + col) * size, y0 + row as i64 * size))
                })
            })
            .collect();
        for (pad, colour) in [(1, [0.0; 3]), (0, c)] {
            for &(x, y) in &lit {
                for dy in -pad..size + pad {
                    for dx in -pad..size + pad {
                        self.set(x + dx, y + dy, colour);
                    }
                }
            }
        }
    }
}

/// Average linear colour of each material.
fn material_colours(project: &Project) -> Vec<[f32; 3]> {
    let mut cache: HashMap<crate::project::BuiltinTexture, [f32; 3]> = HashMap::new();
    let to_linear = |c: f32| c.powf(2.2);
    project
        .materials
        .iter()
        .map(|m| {
            let base = match &m.texture {
                TextureSource::Builtin(t) => *cache.entry(*t).or_insert_with(|| {
                    let img = builtin::image(*t);
                    let mut sum = [0.0f32; 3];
                    for px in img.pixels.as_chunks::<4>().0 {
                        for (s, &v) in sum.iter_mut().zip(px) {
                            *s += to_linear(v as f32 / 255.0);
                        }
                    }
                    sum.map(|s| s / (img.width * img.height) as f32)
                }),
                _ => [0.5; 3],
            };
            let tint = m.color.map(to_linear);
            [base[0] * tint[0], base[1] * tint[1], base[2] * tint[2]]
        })
        .collect()
}

/// How a scatter's model looks from above: its colour, linear, and its radius, m. A
/// built-in model's are its own; a file's, a tree's.
fn dot(model: &std::path::Path) -> ([f32; 3], f64) {
    let Some(m) = crate::shapes::name(model).and_then(crate::shapes::model) else {
        return ([0.03, 0.08, 0.02], 2.5);
    };
    // The widest part seen from above is the one on top: its colour and reach.
    let top = m
        .meshes
        .iter()
        .max_by(|a, b| {
            let reach = |mesh: &open_racing_track::Mesh| {
                mesh.positions
                    .iter()
                    .map(|p| p[0].hypot(p[1]))
                    .fold(0.0f32, f32::max)
            };
            reach(a).total_cmp(&reach(b))
        })
        .expect("a model has meshes");
    let c = m.look.materials[top.material as usize].base_color;
    let r = top
        .positions
        .iter()
        .map(|p| p[0].hypot(p[1]))
        .fold(0.0f32, f32::max);
    ([c[0] * 1.5, c[1] * 1.5, c[2] * 1.5], r as f64)
}

/// Draws the project at `size` pixels along its longer side.
pub fn render(project: &Project, scene: &Scene, size: usize) -> Picture {
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for b in &scene.roads {
        for f in &b.sampled.frames {
            lo = lo.min(f.pos.truncate());
            hi = hi.max(f.pos.truncate());
        }
        for part in &b.visual {
            for p in &part.mesh.positions {
                lo = lo.min(DVec2::new(p[0] as f64, p[1] as f64));
                hi = hi.max(DVec2::new(p[0] as f64, p[1] as f64));
            }
        }
    }
    let margin = 0.05 * (hi - lo).max_element().max(50.0);
    lo -= DVec2::splat(margin);
    hi += DVec2::splat(margin);
    let extent = hi - lo;
    let scale = size as f64 / extent.max_element();
    let view = View {
        lo,
        scale,
        width: (extent.x * scale).ceil() as usize,
        height: (extent.y * scale).ceil() as usize,
    };
    let n = view.width * view.height;
    let mut canvas = Canvas {
        view,
        rgb: vec![[0.02, 0.03, 0.04]; n],
        depth: vec![f32::NEG_INFINITY; n],
    };
    let colours = material_colours(project);
    let colour = |name: &str| {
        project
            .material_index(name)
            .map_or([0.3; 3], |i| colours[i])
    };
    if let Some(t) = &scene.terrain {
        let own = colour(&project.terrain.material);
        for m in &t.chunks {
            canvas.mesh(m, own);
        }
        // The painted layers, as the mask blends them.
        if let Some(mask) = &t.mask {
            let layers: Vec<[f32; 3]> = std::iter::once(own)
                .chain(project.terrain.layers.iter().map(|l| colour(&l.material)))
                .collect();
            let v = &canvas.view;
            for y in 0..v.height {
                for x in 0..v.width {
                    let p = DVec2::new(
                        v.lo.x + (x as f64 + 0.5) / v.scale,
                        v.lo.y + (v.height as f64 - y as f64 - 0.5) / v.scale,
                    );
                    let q = (p - mask.lo) / mask.size;
                    if !(0.0..1.0).contains(&q.x) || !(0.0..1.0).contains(&q.y) {
                        continue;
                    }
                    let w = mask.weights(p);
                    let mut c = [0.0; 3];
                    for (layer, &wi) in layers.iter().zip(&w) {
                        for k in 0..3 {
                            c[k] += layer[k] * wi as f32 / 255.0;
                        }
                    }
                    let px = &mut canvas.rgb[y * v.width + x];
                    for k in 0..3 {
                        px[k] *= c[k] / own[k].max(1e-3);
                    }
                }
            }
        }
    }
    let parts = scene.visual_parts();
    for part in parts {
        let c = colours.get(part.material).copied().unwrap_or([0.3; 3]);
        canvas.mesh(&part.mesh, c);
    }
    // Walls as built, seen from above as lines.
    for b in &scene.roads {
        for part in b.solid.iter().filter(|p| p.kind == Solid::Wall) {
            let m = &part.mesh;
            let at = |i: u32| DVec3::from(m.positions[i as usize].map(f64::from));
            for &[a, b, c] in m.indices.as_chunks::<3>().0 {
                for (x, y) in [(a, b), (b, c)] {
                    canvas.line(at(x), at(y), 2.0, [0.9, 0.1, 0.6]);
                }
            }
        }
    }

    // The scatters' copies, as dots about as wide as their models.
    if !project.scatter.is_empty() {
        let props: Vec<_> = project.surfaces.iter().map(|s| s.props).collect();
        let ground = scene.ground.build(&props);
        let keepout = crate::scatter::Keepout::new(&scene.roads);
        for s in &project.scatter {
            let looks: Vec<([f32; 3], f64)> = s.models.iter().map(|m| dot(&m.model)).collect();
            for c in crate::scatter::copies(s, &keepout, &ground) {
                let (colour, r) = looks[c.model];
                let q = canvas.view.px(c.pos);
                canvas.disc(q, (r * c.scale * canvas.view.scale).max(1.0), colour);
            }
        }
    }

    // Linear to sRGB, then the overlays in sRGB.
    for c in &mut canvas.rgb {
        *c = c.map(|x| x.clamp(0.0, 1.0).powf(1.0 / 2.2));
    }
    // 100 m grid, with its coordinates every 500 m.
    let grid = [0.35, 0.4, 0.45];
    let first = (lo / 100.0).ceil() * 100.0;
    let (w, h) = (canvas.view.width, canvas.view.height);
    let mut x = first.x;
    while x < hi.x {
        let px = canvas.view.px(DVec3::new(x, 0.0, 0.0)).x as i64;
        for y in (0..h as i64).step_by(3) {
            canvas.set(px, y, grid);
        }
        x += 100.0;
    }
    let mut y = first.y;
    while y < hi.y {
        let py = canvas.view.px(DVec3::new(0.0, y, 0.0)).y as i64;
        for x in (0..w as i64).step_by(3) {
            canvas.set(x, py, grid);
        }
        y += 100.0;
    }

    let main = project.road_index(&project.main_road);
    for (i, (road, b)) in project.roads.iter().zip(&scene.roads).enumerate() {
        let smp = &b.sampled;
        if Some(i) == main {
            let m = &project.markers;
            let across = |u: f64, c: [f32; 3]| {
                let f = smp.frame_at(smp.s_at(u));
                let (a, b) = (
                    f.pos + f.lateral * f.width_left,
                    f.pos - f.lateral * f.width_right,
                );
                (a, b, c)
            };
            let mut lines = vec![across(m.start, [1.0, 1.0, 1.0])];
            lines.extend(m.sectors.iter().map(|&u| across(u, [1.0, 0.85, 0.1])));
            for (a, b, c) in lines {
                canvas.line(a, b, 3.0, c);
            }
            let start = smp.s_at(m.start);
            for k in 0..m.grid.count {
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                let f = smp.frame_at(start - m.grid.behind - k as f64 * m.grid.spacing);
                let p = f.pos + f.lateral * (m.grid.pole.sign() * side * m.grid.stagger);
                let q = canvas.view.px(p);
                canvas.disc(q, 2.5, [0.2, 0.6, 1.0]);
            }
        }
        if let Some(pit) = &project.markers.pit
            && pit.road == road.name
        {
            for &u in &pit.boxes {
                let f = smp.frame_at(smp.s_at(u));
                let p = f.pos + f.lateral * (pit.box_side.sign() * pit.box_offset);
                let q = canvas.view.px(p);
                canvas.disc(q, 3.0, [1.0, 0.5, 0.1]);
            }
        }
        // Nodes, numbered; the first one larger.
        for (k, node) in road.nodes.iter().enumerate() {
            let q = canvas.view.px(node.pos);
            let r = if k == 0 { 6.0 } else { 4.0 };
            canvas.disc(q, r + 1.5, [0.0; 3]);
            canvas.disc(q, r, [0.3, 0.9, 1.0]);
            canvas.number(q + DVec2::new(7.0, -14.0), k, 2, [1.0; 3]);
        }
    }

    let pixels = canvas
        .rgb
        .iter()
        .flat_map(|c| c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8))
        .collect();
    Picture {
        width: w,
        height: h,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_plan() {
        let p = Project::new("t");
        let pic = render(&p, &crate::bake::build(&p), 400);
        assert_eq!(pic.width.max(pic.height), 400);
        let png = pic.to_png();
        assert_eq!(&png[1..4], b"PNG");
    }
}
