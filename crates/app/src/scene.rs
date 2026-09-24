//! Track geometry and car visuals generated from the simulation data.

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::light::{GeneratedEnvironmentMapLight, NotShadowCaster};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use glam::{DQuat, DVec3};
use open_racing_car::{CarVisual, Part};
use open_racing_sim::Track;

use crate::driving::{CarModelVisual, Simulation, TrackModel};
use crate::track_model::{self, TrackMaterial, TrackModelPlugin};

/// Simulation is Z-up (ISO 8855), Bevy is Y-up: rotate −90° about X.
pub fn to_bevy(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.z as f32, -v.y as f32)
}

pub fn quat_to_bevy(q: DQuat) -> Quat {
    let c = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    // The sim's glam and Bevy's glam may be different crate versions.
    let q = Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32);
    c * q * c.inverse()
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TrackModelPlugin)
            .add_systems(
                Startup,
                (
                    (spawn_track, track_model::spawn).chain(),
                    spawn_car,
                    spawn_lights,
                ),
            )
            .add_systems(PostUpdate, update_car.before(TransformSystems::Propagate));
    }
}

#[derive(Component)]
struct CarBody;

#[derive(Component)]
pub struct CarWheel(pub usize);

/// The axles of a car model's wheels (see `CarVisual::wheel_axles`), in body axes.
#[derive(Resource, Clone, Copy)]
pub struct WheelAxles(pub [DVec3; 4]);

/// What steers and follows the suspension with a wheel without spinning.
#[derive(Component)]
struct CarHub(usize);

/// The steering wheel of a car model, turning about this axis of the body (Bevy axes).
#[derive(Component)]
struct CarSteeringWheel(Vec3);

/// The driver's eye point in the body frame (ISO axes), for the cockpit camera, when the
/// car's model gives one.
#[derive(Resource)]
pub struct DriverEye(pub DVec3);

/// The front end of the car's body along its x axis relative to the centre of gravity, m,
/// for the bumper camera.
#[derive(Resource)]
pub struct CarNose(pub f64);

/// Top-level entities of the car's visuals, hidden by cameras that put the viewer
/// inside the car.
#[derive(Component)]
pub struct CarVisualRoot;

/// Edge of the sky cube map's faces, in texels. The sky is a smooth gradient.
const SKY_MAP_SIZE: u32 = 64;
/// Luminance scale of the sky light, cd/m². Shade gets about the light that ambient light
/// of 400 cd/m² gives, which Bevy scales by its diffuse BRDF term (≈ 0.45).
const SKY_BRIGHTNESS: f32 = 400.0;

/// Environment light for cameras: a sky from blue at the zenith to haze at the horizon,
/// over sunlit ground. It fills shadows as ambient light would, and gives glossy surfaces
/// something to reflect. Its colours are muted so that shade stays close to neutral.
pub fn sky_light(images: &mut Assets<Image>) -> GeneratedEnvironmentMapLight {
    let zenith = LinearRgba::from(Color::srgb(0.5, 0.64, 0.85));
    let horizon = LinearRgba::from(Color::srgb(0.88, 0.9, 0.92));
    let ground = LinearRgba::from(Color::srgb(0.55, 0.5, 0.43));
    let n = SKY_MAP_SIZE as usize;
    let mut data = Vec::with_capacity(6 * n * n * 4);
    // Faces +X, −X, +Y, −Y, +Z, −Z; rows run down on the side faces.
    for face in 0..6 {
        for row in 0..n {
            for col in 0..n {
                let [u, v] = [col, row].map(|i| (i as f32 + 0.5) / n as f32 * 2.0 - 1.0);
                let up = match face {
                    2 => 1.0,
                    3 => -1.0,
                    _ => -v,
                } / (1.0 + u * u + v * v).sqrt();
                let c = if up >= 0.0 {
                    horizon.mix(&zenith, up.sqrt())
                } else {
                    horizon.mix(&ground, (-4.0 * up).min(1.0))
                };
                data.extend(Color::from(c).to_srgba().to_u8_array());
            }
        }
    }
    let size = Extent3d {
        width: SKY_MAP_SIZE,
        height: SKY_MAP_SIZE,
        depth_or_array_layers: 6,
    };
    let mut image = Image::new(
        size,
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    GeneratedEnvironmentMapLight {
        environment_map: images.add(image),
        intensity: SKY_BRIGHTNESS,
        ..default()
    }
}

fn spawn_lights(mut commands: Commands) {
    // The sky light (see `sky_light`) replaces ambient light.
    commands.insert_resource(GlobalAmbientLight::NONE);
    commands.spawn((
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(100.0, 300.0, 150.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Triangle mesh built from ribbons that follow the track.
struct Strip {
    positions: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Strip {
    fn new() -> Self {
        Self {
            positions: Vec::new(),
            colors: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// Adds a closed ribbon along the track between two edges given per sample as
    /// (lateral offset, height above the surface). `left` must be left of `right`.
    fn ribbon(
        &mut self,
        track: &Track,
        left: impl Fn(usize) -> (f64, f64),
        right: impl Fn(usize) -> (f64, f64),
        color: impl Fn(usize) -> [f32; 4],
    ) {
        let base = self.positions.len() as u32;
        let n = track.samples.len();
        for i in 0..=n {
            let k = i % n;
            let smp = &track.samples[k];
            for (offset, lift) in [left(k), right(k)] {
                let p = smp.pos + smp.lateral * offset + smp.normal * lift;
                self.positions.push(to_bevy(p).to_array());
                self.colors.push(color(i));
            }
        }
        for i in 0..n as u32 {
            let v = base + 2 * i;
            // Counter-clockwise seen from above, so the surface faces up.
            self.indices
                .extend_from_slice(&[v, v + 1, v + 2, v + 1, v + 3, v + 2]);
        }
    }

    fn into_mesh(self) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
        .with_inserted_indices(Indices::U32(self.indices));
        mesh.compute_normals();
        mesh
    }
}

fn spawn_track(
    mut commands: Commands,
    sim: Res<Simulation>,
    model: Res<TrackModel>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // `track_model::spawn` draws tracks that come with a 3D model.
    if model.0.is_some() {
        return;
    }
    let track = &*sim.track;
    let kerb = track.kerb_width;
    let lift = 0.01; // Keep strips from z-fighting.
    let spacing = track.spacing;

    // Vertex colours are linear; author them in sRGB.
    let srgb = |r, g, b| Color::srgb(r, g, b).to_linear().to_f32_array();
    let asphalt = srgb(0.22, 0.22, 0.24);
    let grass = srgb(0.27, 0.45, 0.2);
    let red = srgb(0.8, 0.1, 0.08);
    let white = srgb(0.92, 0.92, 0.92);
    let kerb_color = |i: usize| {
        if (((i as f64 * spacing) / 3.0) as usize).is_multiple_of(2) {
            red
        } else {
            white
        }
    };

    let mut road = Strip::new();
    road.ribbon(
        track,
        |k| (track.samples[k].width_left, 0.0),
        |k| (-track.samples[k].width_right, 0.0),
        |_| asphalt,
    );
    // Start/finish line.
    let mut line = Strip::new();
    let first = &track.samples[0];
    let fwd = first.tangent * 0.5;
    for (p, off) in [
        (first.pos - fwd, first.width_left),
        (first.pos - fwd, -first.width_right),
        (first.pos + fwd, first.width_left),
        (first.pos + fwd, -first.width_right),
    ] {
        line.positions
            .push(to_bevy(p + first.lateral * off + first.normal * 0.02).to_array());
        line.colors.push(white);
    }
    line.indices.extend_from_slice(&[0, 1, 2, 1, 3, 2]);

    let mut kerbs = Strip::new();
    kerbs.ribbon(
        track,
        |k| (track.samples[k].width_left + kerb, lift),
        |k| (track.samples[k].width_left, lift),
        kerb_color,
    );
    kerbs.ribbon(
        track,
        |k| (-track.samples[k].width_right, lift),
        |k| (-track.samples[k].width_right - kerb, lift),
        kerb_color,
    );

    // Flat, in the plane of the road, like the simulated grass surface.
    let mut runoff = Strip::new();
    let run = track.runoff_width;
    runoff.ribbon(
        track,
        |k| (track.samples[k].width_left + kerb + run, 0.0),
        |k| (track.samples[k].width_left + kerb, 0.0),
        |_| grass,
    );
    runoff.ribbon(
        track,
        |k| (-track.samples[k].width_right - kerb, 0.0),
        |k| (-track.samples[k].width_right - kerb - run, 0.0),
        |_| grass,
    );

    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.9,
        ..default()
    });
    for strip in [road, line, kerbs, runoff] {
        commands.spawn((
            Mesh3d(meshes.add(strip.into_mesh())),
            MeshMaterial3d(material.clone()),
        ));
    }

    // Ground plane below the lowest point of the track.
    let min_z = track
        .samples
        .iter()
        .map(|s| s.pos.z)
        .fold(f64::INFINITY, f64::min);
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(6000.0, 6000.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.2, 0.36, 0.14),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, (min_z - 1.2) as f32, 0.0),
    ));
}

#[allow(clippy::too_many_arguments)]
fn spawn_car(
    mut commands: Commands,
    sim: Res<Simulation>,
    mut model: ResMut<CarModelVisual>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut model_materials: ResMut<Assets<TrackMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if let Some(visual) = model.0.take() {
        let formats = formats.map_or(CompressedImageFormats::BC, |f| f.0);
        let mats =
            track_model::add_materials(&visual.visual, formats, &mut model_materials, &mut images);
        spawn_car_model(&mut commands, visual, &mats, &mut meshes);
        return;
    }
    let p = &sim.car.model.params;
    let paint = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.45, 0.05),
        metallic: 0.3,
        perceptual_roughness: 0.35,
        ..default()
    });
    let dark = materials.add(StandardMaterial {
        base_color: Color::srgb(0.05, 0.05, 0.06),
        perceptual_roughness: 0.6,
        ..default()
    });
    let glass = materials.add(StandardMaterial {
        base_color: Color::srgb(0.1, 0.12, 0.15),
        perceptual_roughness: 0.1,
        ..default()
    });

    // Body dimensions derived from the chassis so other cars look right too.
    let length = (p.wheelbase + 1.9) as f32;
    // Narrower than the track so the wheels stand out at the corners.
    let width = (p.track_front.min(p.track_rear) - 0.25) as f32;
    let front_x = p.wheelbase * (1.0 - p.front_weight);
    let rear_x = -p.wheelbase * p.front_weight;
    let center_x = (0.5 * (front_x + rear_x) + 0.1) as f32;
    let floor = 0.1 - p.cg_height as f32; // ground clearance relative to CG

    let body_top = floor + 0.55;
    let (wing_x, wing_y) = (center_x - length * 0.5 + 0.2, floor + 1.12);
    // Stays run from the body top to the underside of the wing plate.
    let stay_height = wing_y - 0.02 - body_top;
    commands.insert_resource(CarNose((center_x + 0.5 * length).into()));

    // Mesh axes in the car's local Bevy frame: x forward, y up, z = right.
    commands
        .spawn((
            CarBody,
            CarVisualRoot,
            Transform::default(),
            Visibility::default(),
        ))
        .with_children(|car| {
            car.spawn((
                Mesh3d(meshes.add(Cuboid::new(length, 0.5, width))),
                MeshMaterial3d(paint.clone()),
                Transform::from_xyz(center_x, floor + 0.3, 0.0),
            ));
            car.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.8, 0.45, width * 0.72))),
                MeshMaterial3d(glass),
                Transform::from_xyz(center_x - 0.3, floor + 0.78, 0.0),
            ));
            // Rear wing.
            car.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.35, 0.04, width + 0.3))),
                MeshMaterial3d(dark.clone()),
                Transform::from_xyz(wing_x, wing_y, 0.0),
            ));
            for z in [-0.45, 0.45] {
                car.spawn((
                    Mesh3d(meshes.add(Cuboid::new(0.2, stay_height, 0.04))),
                    MeshMaterial3d(dark.clone()),
                    Transform::from_xyz(wing_x, body_top + 0.5 * stay_height, z),
                ));
            }
        });

    for i in 0..4 {
        let tire = &sim.car.model.tire(i).p;
        let r = tire.radius as f32;
        commands
            .spawn((
                CarWheel(i),
                CarVisualRoot,
                Transform::default(),
                Visibility::default(),
            ))
            .with_children(|w| {
                // Cylinder axis is Y; turn it onto the wheel's spin axis (local Z).
                let axis = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
                w.spawn((
                    Mesh3d(meshes.add(Cylinder::new(r, tire.width as f32))),
                    MeshMaterial3d(dark.clone()),
                    Transform::from_rotation(axis),
                ));
                // A spoke so wheel rotation is visible.
                w.spawn((
                    Mesh3d(meshes.add(Cuboid::new(r * 1.6, 0.08, 0.32))),
                    MeshMaterial3d(paint.clone()),
                    Transform::default(),
                ));
            });
    }
}

/// Spawns a car package's model: the body's meshes on the body, the wheels' and hubs'
/// on entities that follow the simulated wheels, and the steering wheel on the body,
/// turning with the steering.
fn spawn_car_model(
    commands: &mut Commands,
    car: CarVisual,
    materials: &[Handle<TrackMaterial>],
    meshes: &mut Assets<Mesh>,
) {
    // As `to_bevy`, in f32.
    let bevy = |[x, y, z]: [f32; 3]| Vec3::new(x, z, -y);
    let nose = car
        .visual
        .meshes
        .iter()
        .zip(&car.mesh_parts)
        .filter(|(_, part)| matches!(part, Part::Body))
        .flat_map(|(m, _)| &m.positions)
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max);
    if nose.is_finite() {
        commands.insert_resource(CarNose(nose.into()));
    }
    let body = commands
        .spawn((
            CarBody,
            CarVisualRoot,
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    let wheels: [Entity; 4] = std::array::from_fn(|i| {
        commands
            .spawn((
                CarWheel(i),
                CarVisualRoot,
                Transform::default(),
                Visibility::default(),
            ))
            .id()
    });
    let hubs: [Entity; 4] = std::array::from_fn(|i| {
        commands
            .spawn((
                CarHub(i),
                CarVisualRoot,
                Transform::default(),
                Visibility::default(),
            ))
            .id()
    });
    let steering = car.steering_wheel.map(|s| {
        commands
            .spawn((
                CarSteeringWheel(bevy(s.axis)),
                Transform::from_translation(bevy(s.pivot)),
                Visibility::default(),
                ChildOf(body),
            ))
            .id()
    });
    for (m, part) in car.visual.meshes.into_iter().zip(car.mesh_parts) {
        let parent = match part {
            Part::Body => body,
            Part::Wheel(i) => wheels[i as usize],
            Part::Hub(i) => hubs[i as usize],
            Part::SteeringWheel => steering.unwrap_or(body),
        };
        let (material, cast_shadows) = (materials[m.material as usize].clone(), m.cast_shadows);
        let mut entity = commands.spawn((
            Mesh3d(meshes.add(track_model::to_mesh(m))),
            MeshMaterial3d(material),
            ChildOf(parent),
        ));
        if !cast_shadows {
            entity.insert(NotShadowCaster);
        }
    }
    let axles = car.wheel_axles.map_or([DVec3::Y; 4], |a| {
        a.map(|[x, y, z]| DVec3::new(x.into(), y.into(), z.into()))
    });
    commands.insert_resource(WheelAxles(axles));
    if let Some([x, y, z]) = car.driver_eye {
        commands.insert_resource(DriverEye(DVec3::new(x.into(), y.into(), z.into())));
    }
}

#[allow(clippy::type_complexity)]
fn update_car(
    sim: Res<Simulation>,
    axles: Option<Res<WheelAxles>>,
    mut bodies: Query<&mut Transform, (With<CarBody>, Without<CarWheel>, Without<CarHub>)>,
    mut wheels: Query<(&CarWheel, &mut Transform), (Without<CarBody>, Without<CarHub>)>,
    mut hubs: Query<(&CarHub, &mut Transform), (Without<CarBody>, Without<CarWheel>)>,
    mut steering: Query<
        (&CarSteeringWheel, &mut Transform),
        (Without<CarBody>, Without<CarWheel>, Without<CarHub>),
    >,
) {
    let (pos, rot) = sim.body_pose();
    for mut t in &mut bodies {
        t.translation = to_bevy(pos);
        t.rotation = quat_to_bevy(rot);
    }
    let car = &sim.car;
    // Wheel centre and orientation in the world: steered about z, then, if it spins,
    // turned about the axle (about +y; rolling forward is a positive rotation in ISO
    // coordinates).
    let axles = axles.map_or([DVec3::Y; 4], |a| a.0);
    let wheel = |i: usize, spin: bool| {
        let corner = &car.model.corners[i];
        let w = &car.state.wheels[i];
        let local = corner.hardpoint - DVec3::Z * w.extension;
        let steer = car.telemetry.wheels[i].steer;
        let angle = if spin { w.angle } else { 0.0 };
        let q = rot * DQuat::from_rotation_z(steer) * DQuat::from_axis_angle(axles[i], angle);
        (to_bevy(pos + rot * local), quat_to_bevy(q))
    };
    for (w, mut t) in &mut wheels {
        (t.translation, t.rotation) = wheel(w.0, true);
    }
    for (h, mut t) in &mut hubs {
        (t.translation, t.rotation) = wheel(h.0, false);
    }
    let lock = car.model.params.steering.lock;
    let angle = sim.controls.steer_wheel_angle.clamp(-lock, lock) as f32;
    for (s, mut t) in &mut steering {
        t.rotation = Quat::from_axis_angle(s.0, angle);
    }
}
