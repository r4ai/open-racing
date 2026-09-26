//! Kerbs, walls and fences along their own splines, placed anywhere: draped over the
//! roads and the terrain, or at the heights of their nodes.

use glam::DVec3;
use open_racing_sim::GroundMesh;

use crate::curve::Sampled;
use crate::project::{Align, Project, Shape};
use crate::road::{
    BARRIER_SINK, Layer, LinePoint, ModelLine, Solid, SolidPart, VisualPart, add_band, add_wall,
    column_xs,
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
    /// The wall, if a model shows it.
    pub models: Vec<ModelLine>,
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
    let (mut visual, mut solid, mut models) = (Vec::new(), Vec::new(), Vec::new());

    match &sp.shape {
        Shape::Band {
            width,
            align,
            profile,
            surface,
            lift,
            model,
            ..
        } => {
            let (d0, d1) = match align {
                Align::Center => (-0.5 * width, 0.5 * width),
                Align::Left => (0.0, *width),
                Align::Right => (-width, 0.0),
            };
            // Its right edge on what is under it, then up any step its profile starts
            // with.
            let xs: Vec<f64> = std::iter::once(0.0)
                .chain(column_xs(profile, *width))
                .collect();
            let height = |x: f64| if x == 0.0 { 0.0 } else { profile.height(x) };
            // From the right edge to the left, so that the band faces up.
            let rows: Vec<Vec<(DVec3, f64)>> = frames
                .iter()
                .map(|f| {
                    xs.iter()
                        .map(|&x| {
                            let d = d0 + (d1 - d0) * x;
                            let h = lift + height(x);
                            (drape(f.pos + f.lateral * d) + DVec3::Z * h, d - d0)
                        })
                        .collect()
                })
                .collect();
            if let Some(run) = model {
                // At the foot of its right edge halfway across, facing the line's left.
                let points: Vec<LinePoint> = frames
                    .iter()
                    .map(|f| LinePoint {
                        pos: drape(f.pos + f.lateral * (0.5 * (d0 + d1))) + DVec3::Z * *lift,
                        toward: f.lateral.with_z(0.0).normalize_or(DVec3::Y),
                    })
                    .collect();
                models.push(ModelLine::new(run, &points, sampled.closed, |_| true));
            }
            let mut hidden = Vec::new();
            add_band(
                if model.is_some() {
                    &mut hidden
                } else {
                    &mut visual
                },
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
            model,
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
            let mut hidden = Vec::new();
            if let Some(run) = model {
                // Standing on the ground, facing the line's left.
                let points: Vec<LinePoint> = corners
                    .iter()
                    .zip(frames)
                    .map(|(c, f)| LinePoint {
                        pos: (c[0] + c[3]) * 0.5 + DVec3::Z * BARRIER_SINK,
                        toward: f.lateral.with_z(0.0).normalize_or(DVec3::Y),
                    })
                    .collect();
                models.push(ModelLine::new(run, &points, sampled.closed, |_| true));
            }
            add_wall(
                if model.is_some() {
                    &mut hidden
                } else {
                    &mut visual
                },
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
        models,
    }
}
