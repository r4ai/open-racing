//! What the add menu draws besides roads: a kerb, gravel trap or other band of each of
//! the project's strip types, and a wall of each of its wall types, so that the types
//! made in the Library tab are offered with the built-in ones.

use open_racing_track_project::Project;
use open_racing_track_project::project::{Align, Shape, Spline};

/// A kind of spline to draw.
#[derive(Clone, Debug, PartialEq)]
pub enum Preset {
    /// A band of the strip type of this name.
    Strip(String),
    /// A wall of the wall type of this name.
    Wall(String),
}

/// What the add menu offers: every strip type, then every wall type.
pub fn list(project: &Project) -> Vec<Preset> {
    let strips = project
        .strip_styles
        .iter()
        .map(|s| Preset::Strip(s.name.clone()));
    let walls = project
        .wall_styles
        .iter()
        .map(|s| Preset::Wall(s.name.clone()));
    strips.chain(walls).collect()
}

/// The preset drawing strips or walls of the type called `name`.
#[cfg(test)]
pub fn named(project: &Project, name: &str) -> Option<Preset> {
    list(project).into_iter().find(|p| p.name() == name)
}

impl Preset {
    pub fn name(&self) -> &str {
        match self {
            Preset::Strip(n) | Preset::Wall(n) => n,
        }
    }

    /// The menu's label: the type's name, capitalised.
    pub fn label(&self) -> String {
        let mut c = self.name().chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or_default()
    }

    /// Whether it is a band laid on the ground rather than a wall.
    pub fn is_band(&self) -> bool {
        matches!(self, Preset::Strip(_))
    }

    /// The shape of a spline of this type, and how finely it is built; `None` once the
    /// type is gone.
    pub fn shape(&self, project: &Project) -> Option<(Shape, f64)> {
        match self {
            Preset::Strip(n) => {
                let s = project.strip_style(n)?;
                let resolution = if s.width < 1.0 {
                    0.25
                } else if s.width < 3.0 {
                    0.5
                } else {
                    1.0
                };
                let shape = Shape::Band {
                    width: s.width,
                    align: Align::Center,
                    profile: s.profile.clone(),
                    surface: s.surface.clone(),
                    material: s.material.clone(),
                    lift: 0.01,
                };
                Some((shape, resolution))
            }
            Preset::Wall(n) => {
                let w = project.wall_style(n)?;
                let shape = Shape::Wall {
                    height: w.height,
                    thickness: w.thickness,
                    material: w.material.clone(),
                    collide: true,
                    model: w.model.clone(),
                };
                Some((shape, if w.model.is_some() { 1.0 } else { 2.0 }))
            }
        }
    }

    /// A spline of this type through `nodes`, named after it.
    pub fn spline(&self, project: &Project, nodes: Vec<glam::DVec3>) -> Option<Spline> {
        let (shape, resolution) = self.shape(project)?;
        Some(Spline {
            name: unique_name(project, self.name()),
            closed: false,
            nodes: nodes
                .into_iter()
                .map(open_racing_track_project::Node::new)
                .collect(),
            drape: true,
            shape,
            resolution,
            style: Some(self.name().to_string()),
        })
    }
}

/// `base`, or `base.001`, `base.002`… whichever is not `taken` yet, as Blender names
/// copies.
pub fn free_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    let stem = base
        .rsplit_once('.')
        .filter(|(_, n)| n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()))
        .map_or(base, |(s, _)| s);
    (1..)
        .map(|i| format!("{stem}.{i:03}"))
        .find(|n| !taken(n))
        .expect("some name is free")
}

/// A name no road or spline has yet.
pub fn unique_name(project: &Project, base: &str) -> String {
    free_name(base, |n| project.line(n).is_some())
}

/// A name no prop has yet.
pub fn unique_prop_name(project: &Project, base: &str) -> String {
    free_name(base, |n| project.props.iter().any(|p| p.name == n))
}
