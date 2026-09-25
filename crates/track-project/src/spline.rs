//! Kerbs, walls and fences along their own splines, placed anywhere: draped over the
//! roads and the terrain, or at the heights of their nodes.

use glam::DVec3;
use open_racing_sim::GroundMesh;

use crate::curve::Sampled;
use crate::project::{Align, Project, Shape};
use crate::road::{
    BARRIER_SINK, Layer, Solid, SolidPart, VisualPart, add_band, add_wall, columns, profile_height,
};

/// How far above a point draping looks for the ground first, m: enough for ground a
/// little above the node, while a spline under a bridge stays under it.
const DRAPE_REACH: f64 = 3.0;

/// A built spline.
#[derive(Clone, Debug)]
pub struct SplineBuild {
    pub sampled: Sampled,
    pub visual: Vec<VisualPart>,
    pub solid: Vec<SolidPart>,
}

/// Builds spline `index` of the project. Draped splines follow `ground`, the roads and
/// terrain built before them.
pub fn build(project: &Project, index: usize, ground: Option<&GroundMesh>) -> SplineBuild {
    let sp = &project.splines[index];
    let ground = ground.filter(|_| sp.drape);
    let drape = |p: DVec3| match ground {
        Some(g) => g
            .raycast_down(p, DRAPE_REACH)
            .or_else(|| g.raycast_down(p, 1e4))
            .map_or(p, |h| h.point),
        None => p,
    };
    let sampled = Sampled::line(&sp.nodes, sp.closed, sp.resolution, drape);
    let frames = &sampled.frames;
    // Unknown names (in a project being edited) fall back to the first entry.
    let material = project.material_index(sp.material()).unwrap_or(0);
    let tile = project
        .materials
        .get(material)
        .map_or([1.0; 2], |m| m.tile.map(|t| t.max(1e-3) as f64));
    let (mut visual, mut solid) = (Vec::new(), Vec::new());

    match &sp.shape {
        Shape::Band {
            width,
            align,
            profile,
            surface,
            lift,
            ..
        } => {
            let (d0, d1) = match align {
                Align::Center => (-0.5 * width, 0.5 * width),
                Align::Left => (0.0, *width),
                Align::Right => (-width, 0.0),
            };
            let cols = columns(*profile, *width);
            // From the right edge to the left, so that the band faces up.
            let rows: Vec<Vec<(DVec3, f64)>> = frames
                .iter()
                .map(|f| {
                    (0..=cols)
                        .map(|c| {
                            let x = c as f64 / cols as f64;
                            let d = d0 + (d1 - d0) * x;
                            let h = lift + profile_height(*profile, x);
                            (drape(f.pos + f.lateral * d) + DVec3::Z * h, d - d0)
                        })
                        .collect()
                })
                .collect();
            add_band(
                &mut visual,
                &mut solid,
                &sampled,
                &rows,
                Layer::Strip,
                Some(Solid::Ground(project.surface_index(surface).unwrap_or(0))),
                material,
                tile,
                false,
                |_| true,
            );
        }
        Shape::Wall {
            height,
            thickness,
            collide,
            ..
        } => {
            let corners: Vec<[DVec3; 4]> = frames
                .iter()
                .map(|f| {
                    let a = drape(f.pos - f.lateral * (0.5 * thickness));
                    let b = drape(f.pos + f.lateral * (0.5 * thickness));
                    let top = a.z.max(b.z) + height;
                    [
                        a - DVec3::Z * BARRIER_SINK,
                        a.with_z(top),
                        b.with_z(top),
                        b - DVec3::Z * BARRIER_SINK,
                    ]
                })
                .collect();
            add_wall(
                &mut visual,
                &mut solid,
                &sampled,
                &corners,
                *thickness > 0.0,
                *collide,
                material,
                tile,
                |_| true,
            );
        }
    }
    SplineBuild {
        sampled,
        visual,
        solid,
    }
}
