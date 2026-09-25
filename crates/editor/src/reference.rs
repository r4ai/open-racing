//! The reference image: a picture of the real place (a satellite image, a track map)
//! laid flat in the 3D view to trace the roads over, and scaling it so that a distance
//! measured on it comes out right.

use std::path::PathBuf;

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use glam::{DVec2, DVec3};
use open_racing_track_project::ops::Op;
use open_racing_track_project::project::Reference;
use open_racing_track_render::to_bevy;

use crate::assets::Library;
use crate::state::Editor;

/// An image file read into a texture, with its size in pixels, or why it could not be.
type Loaded = Result<(Handle<Image>, UVec2), String>;

/// What is shown now: the image as loaded, and the entity showing it.
#[derive(Resource, Default)]
pub struct Shown {
    /// The image file and the asset files' revision it was read at, with its size in
    /// pixels, or why it could not be read.
    loaded: Option<(PathBuf, u64, Loaded)>,
    /// The reference the entity was made for.
    placed: Option<Reference>,
    entity: Option<Entity>,
    material: Option<Handle<StandardMaterial>>,
}

impl Shown {
    /// Why the image cannot be shown, if it cannot.
    pub fn error(&self) -> Option<&str> {
        match &self.loaded {
            Some((_, _, Err(e))) => Some(e),
            _ => None,
        }
    }

    /// The image's size in pixels, once loaded.
    pub fn size(&self) -> Option<UVec2> {
        match &self.loaded {
            Some((_, _, Ok((_, size)))) => Some(*size),
            _ => None,
        }
    }
}

/// The corners of the image on the ground, in the order of its texture's corners:
/// top left, top right, bottom right, bottom left.
pub fn corners(r: &Reference, aspect: f64) -> [DVec2; 4] {
    let half = DVec2::new(0.5 * r.width, 0.5 * r.width / aspect.max(1e-6));
    let turn = DVec2::from_angle(r.rotation);
    [
        DVec2::new(-half.x, half.y),
        DVec2::new(half.x, half.y),
        DVec2::new(half.x, -half.y),
        DVec2::new(-half.x, -half.y),
    ]
    .map(|c| r.center + turn.rotate(c))
}

fn quad(r: &Reference, aspect: f64) -> Mesh {
    let positions: Vec<[f32; 3]> = corners(r, aspect)
        .map(|c| to_bevy(c.extend(r.height)).to_array())
        .to_vec();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; 4]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    );
    mesh.insert_indices(Indices::U32(vec![0, 3, 2, 0, 2, 1]));
    mesh
}

fn load(path: &std::path::Path) -> Result<(Image, UVec2), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let img = open_racing_track::texture::decode(&bytes)?;
    let size = UVec2::new(img.width as u32, img.height as u32);
    if size.x == 0 || size.y == 0 {
        return Err("the image is empty".into());
    }
    let image = Image::new(
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        img.pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    Ok((image, size))
}

/// Keeps the reference image's quad in step with the project.
#[allow(clippy::too_many_arguments)]
pub fn show(
    mut commands: Commands,
    editor: Res<Editor>,
    library: Res<Library>,
    mut shown: ResMut<Shown>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let wanted = editor.project.reference.as_ref();
    let Some(r) = wanted.filter(|r| r.visible) else {
        if let Some(e) = shown.entity.take() {
            commands.entity(e).despawn();
        }
        shown.placed = None;
        return;
    };
    // The file, read again when it or the assets change.
    let stale = shown
        .loaded
        .as_ref()
        .is_none_or(|(path, rev, _)| *path != r.image || *rev != library.revision);
    if stale {
        let loaded = load(&editor.dir.join(&r.image)).map(|(img, size)| (images.add(img), size));
        shown.loaded = Some((r.image.clone(), library.revision, loaded));
        shown.placed = None;
    }
    if shown.placed.as_ref() == Some(r) {
        return;
    }
    shown.placed = Some(r.clone());
    if let Some(e) = shown.entity.take() {
        commands.entity(e).despawn();
    }
    let Some((_, _, Ok((image, size)))) = &shown.loaded else {
        return;
    };
    let aspect = size.x as f64 / size.y as f64;
    let color = Color::srgba(1.0, 1.0, 1.0, r.opacity as f32);
    let material = match shown.material.clone() {
        Some(m) => {
            if let Some(mut mat) = materials.get_mut(&m) {
                mat.base_color = color;
                mat.base_color_texture = Some(image.clone());
            }
            m
        }
        None => materials.add(StandardMaterial {
            base_color: color,
            base_color_texture: Some(image.clone()),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            double_sided: true,
            ..default()
        }),
    };
    shown.material = Some(material.clone());
    let e = commands
        .spawn((
            Mesh3d(meshes.add(quad(r, aspect))),
            MeshMaterial3d(material),
            Transform::default(),
            NotShadowCaster,
        ))
        .id();
    shown.entity = Some(e);
}

/// A reference image over the whole view the roads span, or a 1 km square.
pub fn new_reference(editor: &Editor, image: PathBuf) -> Reference {
    let p = &editor.project;
    let points = p
        .roads
        .iter()
        .flat_map(|r| &r.nodes)
        .map(|n| n.pos.truncate());
    let (lo, hi) = points.fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(lo, hi), q| (lo.min(q), hi.max(q)),
    );
    let (center, width) = if lo.x.is_finite() {
        (0.5 * (lo + hi), ((hi - lo).max_element() * 1.5).max(200.0))
    } else {
        (DVec2::ZERO, 1000.0)
    };
    Reference {
        image,
        center,
        width,
        rotation: 0.0,
        height: 0.5,
        opacity: 0.6,
        visible: true,
    }
}

/// The reference scaled about `a` so that the distance from `a` to `b` on it becomes
/// `metres`.
pub fn calibrated(r: &Reference, a: DVec3, b: DVec3, metres: f64) -> Option<Reference> {
    let measured = (b - a).truncate().length();
    if measured < 1e-6 || metres <= 0.0 || !metres.is_finite() {
        return None;
    }
    let k = metres / measured;
    let a = a.truncate();
    Some(Reference {
        center: a + (r.center - a) * k,
        width: r.width * k,
        ..r.clone()
    })
}

/// Sets the reference image, as one undo step while `key` repeats.
pub fn set(editor: &mut Editor, reference: Option<Reference>, key: Option<&str>) {
    editor.apply(vec![Op::SetReference { reference }], key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrating_keeps_the_first_point_and_scales_the_rest() {
        let r = Reference {
            image: "a.png".into(),
            center: DVec2::new(100.0, 0.0),
            width: 400.0,
            rotation: 0.3,
            height: 0.0,
            opacity: 1.0,
            visible: true,
        };
        let (a, b) = (DVec3::new(0.0, 0.0, 0.0), DVec3::new(50.0, 0.0, 0.0));
        let c = calibrated(&r, a, b, 100.0).unwrap();
        assert_eq!(c.width, 800.0);
        assert_eq!(c.center, DVec2::new(200.0, 0.0));
        assert!(calibrated(&r, a, a, 100.0).is_none());
        // Corners turn with the image.
        let [tl, tr, ..] = corners(&r, 2.0);
        assert!(((tr - tl).length() - 400.0).abs() < 1e-9);
        assert!(((tr - tl).to_angle() - 0.3).abs() < 1e-9);
    }
}
