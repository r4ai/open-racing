//! Kinds of kerbs, walls and fences the add menu offers.

use open_racing_track_project::Project;
use open_racing_track_project::project::{Align, Profile, Shape, Spline};

pub struct Preset {
    pub label: &'static str,
    /// Name new splines of this kind start from.
    pub name: &'static str,
    pub shape: fn(&Project) -> Shape,
    pub resolution: f64,
}

/// The material called `name`, or the project's first one.
fn material(p: &Project, name: &str) -> String {
    p.material_index(name)
        .or((!p.materials.is_empty()).then_some(0))
        .map_or(name.to_string(), |i| p.materials[i].name.clone())
}

fn surface(p: &Project, name: &str) -> String {
    p.surface_index(name)
        .or((!p.surfaces.is_empty()).then_some(0))
        .map_or(name.to_string(), |i| p.surfaces[i].name.clone())
}

pub const PRESETS: &[Preset] = &[
    Preset {
        label: "Kerb",
        name: "kerb",
        shape: |p| Shape::Band {
            width: 1.2,
            align: Align::Center,
            profile: Profile::Crown(0.03),
            surface: surface(p, "kerb"),
            material: material(p, "kerb"),
            lift: 0.01,
        },
        resolution: 0.5,
    },
    Preset {
        label: "Sausage kerb",
        name: "sausage",
        shape: |p| Shape::Band {
            width: 0.5,
            align: Align::Center,
            profile: Profile::Crown(0.12),
            surface: surface(p, "kerb"),
            material: material(p, "kerb"),
            lift: 0.01,
        },
        resolution: 0.25,
    },
    Preset {
        label: "Run-off patch",
        name: "run-off",
        shape: |p| Shape::Band {
            width: 8.0,
            align: Align::Center,
            profile: Profile::Flat,
            surface: surface(p, "runoff"),
            material: material(p, "asphalt"),
            lift: 0.005,
        },
        resolution: 1.0,
    },
    Preset {
        label: "Gravel trap",
        name: "gravel",
        shape: |p| Shape::Band {
            width: 12.0,
            align: Align::Center,
            profile: Profile::Flat,
            surface: surface(p, "gravel"),
            material: material(p, "gravel"),
            lift: 0.005,
        },
        resolution: 1.0,
    },
    Preset {
        label: "Concrete wall",
        name: "wall",
        shape: |p| Shape::Wall {
            height: 1.0,
            thickness: 0.5,
            material: material(p, "concrete"),
            collide: true,
        },
        resolution: 2.0,
    },
    Preset {
        label: "Armco",
        name: "armco",
        shape: |p| Shape::Wall {
            height: 0.8,
            thickness: 0.0,
            material: material(p, "armco"),
            collide: true,
        },
        resolution: 2.0,
    },
    Preset {
        label: "Fence",
        name: "fence",
        shape: |p| Shape::Wall {
            height: 3.0,
            thickness: 0.0,
            material: material(p, "fence"),
            collide: true,
        },
        resolution: 2.0,
    },
    Preset {
        label: "Tyre wall",
        name: "tyres",
        shape: |p| Shape::Wall {
            height: 0.9,
            thickness: 1.0,
            material: material(p, "tyres"),
            collide: true,
        },
        resolution: 1.0,
    },
];

/// `base`, or `base.001`, `base.002`… whichever no road or spline is called yet.
pub fn unique_name(project: &Project, base: &str) -> String {
    let taken = |n: &str| project.road(n).is_some() || project.spline(n).is_some();
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

impl Preset {
    pub fn spline(&self, project: &Project, nodes: Vec<glam::DVec3>) -> Spline {
        Spline {
            name: unique_name(project, self.name),
            closed: false,
            nodes: nodes
                .into_iter()
                .map(|pos| open_racing_track_project::Node { pos, handle: None })
                .collect(),
            drape: true,
            shape: (self.shape)(project),
            resolution: self.resolution,
        }
    }

    /// Whether it is a band laid on the ground rather than a wall.
    pub fn is_band(&self, project: &Project) -> bool {
        matches!((self.shape)(project), Shape::Band { .. })
    }
}
