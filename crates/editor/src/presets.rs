//! What the add menu draws besides roads: a kerb, gravel trap or other band of each of
//! the project's strip types, a wall of each of its wall types, and an area filled with
//! each strip type that is not a kerb (a gravel trap, a paddock), so that the types made
//! in the Library tab are offered with the built-in ones.

use open_racing_track_project::Project;
use open_racing_track_project::project::{Align, Shape, Spline};

/// A kind of spline to draw.
#[derive(Clone, Debug, PartialEq)]
pub enum Preset {
    /// A band of the strip type of this name.
    Strip(String),
    /// A wall of the wall type of this name.
    Wall(String),
    /// A closed area filled with the strip type of this name.
    Area(String),
}

/// What the add menu offers: every strip type, then every wall type, then an area of
/// every strip type that is not a kerb.
pub fn list(project: &Project) -> Vec<Preset> {
    let strips = project
        .strip_styles
        .iter()
        .map(|s| Preset::Strip(s.name.clone()));
    let walls = project
        .wall_styles
        .iter()
        .map(|s| Preset::Wall(s.name.clone()));
    let kerb = |surface: &str| {
        project
            .surfaces
            .iter()
            .find(|s| s.name == surface)
            .is_some_and(|s| s.props.kind == open_racing_sim::Surface::Kerb)
    };
    let areas = project
        .strip_styles
        .iter()
        .filter(|s| !kerb(&s.surface))
        .map(|s| Preset::Area(s.name.clone()));
    strips.chain(walls).chain(areas).collect()
}

/// The preset drawing strips or walls of the type called `name`.
#[cfg(test)]
pub fn named(project: &Project, name: &str) -> Option<Preset> {
    list(project).into_iter().find(|p| p.name() == name)
}

impl Preset {
    pub fn name(&self) -> &str {
        match self {
            Preset::Strip(n) | Preset::Wall(n) | Preset::Area(n) => n,
        }
    }

    /// The menu's label: the type's name, capitalised (an area's, as an area).
    pub fn label(&self) -> String {
        let mut c = self.name().chars();
        let name: String = c
            .next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or_default();
        match self {
            Preset::Area(_) => format!("{name} area"),
            _ => name,
        }
    }

    /// Whether it is a band laid on the ground rather than a wall.
    pub fn is_band(&self) -> bool {
        matches!(self, Preset::Strip(_))
    }

    /// Whether it fills a closed line.
    pub fn is_area(&self) -> bool {
        matches!(self, Preset::Area(_))
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
                    model: s.model.clone(),
                };
                Some((shape, resolution))
            }
            Preset::Area(n) => {
                let s = project.strip_style(n)?;
                let shape = Shape::Area {
                    surface: s.surface.clone(),
                    material: s.material.clone(),
                    lift: 0.02,
                };
                Some((shape, 1.0))
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
        let name = match self {
            Preset::Area(n) => format!("{n} area"),
            _ => self.name().to_string(),
        };
        Some(Spline {
            group: None,
            name: unique_name(project, &name),
            closed: self.is_area(),
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
