//! Track geometry and car visuals generated from the simulation data.

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use glam::{DQuat, DVec3};
use open_racing_car::{CarVisual, Part};
use open_racing_sim::Track;

use crate::driving::{CarModelVisual, Simulation, TrackModel};
use crate::graphics::GraphicsSettings;
use open_racing_track_render::{self as track_model, TrackMaterial, TrackModelPlugin};
pub use open_racing_track_render::{quat_to_bevy, to_bevy};

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(TrackModelPlugin)
            .add_systems(
                Startup,
                (
                    (spawn_track, spawn_track_model).chain(),
                    spawn_car,
                    spawn_lights,
                ),
            )
            .add_systems(PostUpdate, update_car.before(TransformSystems::Propagate))
            .add_systems(Update, update_wind);
    }
}

/// Plants sway in the weather's wind (10 m above the ground; gusts come in the shader).
fn update_wind(sim: Res<Simulation>, mut wind: ResMut<track_model::Wind>) {
    let (speed, from) = sim.weather.wind();
    let a = from.to_radians();
    let towards = [-(a.sin() * speed) as f32, -(a.cos() * speed) as f32];
    wind.set_if_neq(track_model::Wind(towards));
}

/// Spawns the track's model, if it has one.
fn spawn_track_model(
    mut commands: Commands,
    mut model: ResMut<TrackModel>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    graphics: Res<GraphicsSettings>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<TrackMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(visual) = model.0.take() else { return };
    track_model::spawn_visual(
        &mut commands,
        visual,
        track_model::formats(formats.as_deref()),
        graphics.anisotropy,
        &mut meshes,
        &mut materials,
        &mut images,
        (),
    );
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

/// One link, rod or rocker arm of a wheel's suspension: its index among the wheel's
/// `Car::linkage_segments`.
#[derive(Component)]
struct CarLink {
    wheel: usize,
    index: usize,
}

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

fn spawn_lights(mut commands: Commands) {
    // The weather's sky light replaces ambient light, and it places the sun.
    commands.insert_resource(GlobalAmbientLight::NONE);
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
    // `spawn_track_model` draws tracks that come with a 3D model.
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
    graphics: Res<GraphicsSettings>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut model_materials: ResMut<Assets<TrackMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if let Some(visual) = model.0.take() {
        let formats = formats.map_or(CompressedImageFormats::BC, |f| f.0);
        let mats = track_model::add_materials(
            &visual.visual,
            formats,
            graphics.anisotropy,
            &mut model_materials,
            &mut images,
        );
        spawn_car_model(&mut commands, visual, &mats, &mut meshes);
        return;
    }
    let car = &sim.car;
    let p = &car.model.params;
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
    let metal = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.56, 0.6),
        metallic: 0.9,
        perceptual_roughness: 0.35,
        ..default()
    });

    // Body dimensions derived from the chassis so other cars look right too.
    let front_x = car.model.corners[0].origin.x as f32;
    let rear_x = car.model.corners[2].origin.x as f32;
    let track = p.track_front.min(p.track_rear) as f32;
    let floor = 0.1 - p.cg_height as f32; // ground clearance relative to CG

    // Mesh axes in the car's local Bevy frame: x forward, y up, z = right.
    let mut boxes: Vec<(Vec3, Vec3, &Handle<StandardMaterial>)> = Vec::new();
    let wing = |name: &str| p.aero.elements.iter().find(|e| e.name.contains(name));
    let nose = if let Some(front_wing) = wing("front wing") {
        // An open-wheeler: a narrow tub and nose, sidepods, and wings where its aero
        // elements are.
        let floor =
            0.5 * (p.aero.ride_height[0] + p.aero.ride_height[1]) as f32 - p.cg_height as f32;
        let (wing_x, wing_y) = (front_wing.position[0] as f32, front_wing.position[1] as f32);
        let tub = (rear_x - 0.35, front_x + 0.15);
        let at = |x0: f32, x1: f32, y: f32, z: f32| Vec3::new(0.5 * (x0 + x1), y, z);
        boxes.push((
            at(tub.0, tub.1, floor + 0.25, 0.0),
            Vec3::new(tub.1 - tub.0, 0.5, 0.56),
            &paint,
        ));
        boxes.push((
            at(tub.1, wing_x, floor + 0.2, 0.0),
            Vec3::new(wing_x - tub.1, 0.24, 0.3),
            &paint,
        ));
        let pods = (rear_x + 0.45, front_x - 1.1);
        for z in [-0.47, 0.47] {
            boxes.push((
                at(pods.0, pods.1, floor + 0.2, z),
                Vec3::new(pods.1 - pods.0, 0.36, 0.38),
                &paint,
            ));
        }
        // The driver sits in the middle, the airbox and engine cover behind the head;
        // then the floor.
        let eye = Vec3::new(front_x - 1.4, floor + 0.8, 0.0);
        commands.insert_resource(DriverEye(DVec3::new(eye.x.into(), 0.0, eye.y.into())));
        let airbox = (rear_x - 0.1, eye.x - 0.3);
        boxes.push((
            at(airbox.0, airbox.1, floor + 0.7, 0.0),
            Vec3::new(airbox.1 - airbox.0, 0.4, 0.3),
            &paint,
        ));
        boxes.push((
            at(rear_x + 0.2, front_x - 0.5, floor + 0.01, 0.0),
            Vec3::new(front_x - rear_x - 0.7, 0.02, track - 0.45),
            &dark,
        ));
        boxes.push((
            Vec3::new(wing_x, wing_y, 0.0),
            Vec3::new(0.35, 0.03, p.track_front as f32 + 0.1),
            &dark,
        ));
        for z in [-1.0, 1.0] {
            boxes.push((
                Vec3::new(
                    wing_x,
                    wing_y + 0.08,
                    z * (0.5 * p.track_front as f32 + 0.05),
                ),
                Vec3::new(0.45, 0.2, 0.02),
                &dark,
            ));
        }
        if let Some(rear_wing) = wing("rear wing") {
            let (x, y) = (rear_wing.position[0] as f32, rear_wing.position[1] as f32);
            boxes.push((Vec3::new(x, y, 0.0), Vec3::new(0.3, 0.03, 0.9), &dark));
            boxes.push((
                Vec3::new(x, y + 0.1, 0.0),
                Vec3::new(0.15, 0.02, 0.9),
                &dark,
            ));
            for z in [-0.46, 0.46] {
                boxes.push((Vec3::new(x, y, z), Vec3::new(0.5, 0.4, 0.02), &dark));
            }
            boxes.push((
                at(x, rear_x - 0.1, 0.5 * (floor + 0.5 + y), 0.0),
                Vec3::new(0.12, y - floor - 0.5, 0.04),
                &dark,
            ));
        }
        wing_x + 0.2
    } else {
        let length = (p.wheelbase + 1.9) as f32;
        // Narrower than the track so the wheels stand out at the corners.
        let width = track - 0.25;
        let center_x = 0.5 * (front_x + rear_x) + 0.1;
        let body_top = floor + 0.55;
        let (wing_x, wing_y) = (center_x - length * 0.5 + 0.2, floor + 1.12);
        // Stays run from the body top to the underside of the wing plate.
        let stay_height = wing_y - 0.02 - body_top;
        boxes.push((
            Vec3::new(center_x, floor + 0.3, 0.0),
            Vec3::new(length, 0.5, width),
            &paint,
        ));
        boxes.push((
            Vec3::new(center_x - 0.3, floor + 0.78, 0.0),
            Vec3::new(1.8, 0.45, width * 0.72),
            &glass,
        ));
        boxes.push((
            Vec3::new(wing_x, wing_y, 0.0),
            Vec3::new(0.35, 0.04, width + 0.3),
            &dark,
        ));
        for z in [-0.45, 0.45] {
            boxes.push((
                Vec3::new(wing_x, body_top + 0.5 * stay_height, z),
                Vec3::new(0.2, stay_height, 0.04),
                &dark,
            ));
        }
        center_x + 0.5 * length
    };
    commands.insert_resource(CarNose(nose.into()));
    commands
        .spawn((
            CarBody,
            CarVisualRoot,
            Transform::default(),
            Visibility::default(),
        ))
        .with_children(|body| {
            for (centre, size, material) in boxes {
                body.spawn((
                    Mesh3d(meshes.add(Cuboid::from_size(size))),
                    MeshMaterial3d(material.clone()),
                    Transform::from_translation(centre),
                ));
            }
        });

    // The suspension's links, rods and rockers, placed each frame by `update_car`.
    let rod = meshes.add(Cylinder::new(0.012, 1.0));
    let mut segments = Vec::new();
    for i in 0..4 {
        segments.clear();
        car.linkage_segments(i, &mut segments);
        for index in 0..segments.len() {
            commands.spawn((
                CarLink { wheel: i, index },
                CarVisualRoot,
                Mesh3d(rod.clone()),
                MeshMaterial3d(metal.clone()),
                Transform::default(),
                Visibility::default(),
            ));
        }
    }

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

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_car(
    sim: Res<Simulation>,
    axles: Option<Res<WheelAxles>>,
    mut segments: Local<[Vec<(DVec3, DVec3)>; 4]>,
    mut bodies: Query<&mut Transform, (With<CarBody>, Without<CarWheel>, Without<CarHub>)>,
    mut wheels: Query<(&CarWheel, &mut Transform), (Without<CarBody>, Without<CarHub>)>,
    mut hubs: Query<(&CarHub, &mut Transform), (Without<CarBody>, Without<CarWheel>)>,
    mut steering: Query<
        (&CarSteeringWheel, &mut Transform),
        (Without<CarBody>, Without<CarWheel>, Without<CarHub>),
    >,
    mut links: Query<
        (&CarLink, &mut Transform),
        (
            Without<CarBody>,
            Without<CarWheel>,
            Without<CarHub>,
            Without<CarSteeringWheel>,
        ),
    >,
) {
    let (pos, rot) = sim.body_pose();
    for mut t in &mut bodies {
        t.translation = to_bevy(pos);
        t.rotation = quat_to_bevy(rot);
    }
    let car = &sim.car;
    // Wheel centre and orientation in the world: turned as the linkage holds the upright
    // (steered, cambered), then, if it spins, about the axle (rolling forward is a
    // positive rotation in ISO coordinates). A model's wheels carry their static camber
    // and toe; the stand-in's are square to the body and take the upright's axle.
    let wheel = |i: usize, spin: bool| {
        let pose = car.pose(i);
        let angle = if spin { car.state.wheels[i].angle } else { 0.0 };
        let upright = match &axles {
            Some(a) => pose.rotation * DQuat::from_axis_angle(a.0[i], angle),
            None => DQuat::from_rotation_arc(DVec3::Y, pose.axis) * DQuat::from_rotation_y(angle),
        };
        (
            to_bevy(pos + rot * car.wheel_center_body(i)),
            quat_to_bevy(rot * upright),
        )
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
    // Each link a unit rod stretched between its joints.
    for (i, s) in segments.iter_mut().enumerate() {
        s.clear();
        car.linkage_segments(i, s);
    }
    for (link, mut t) in &mut links {
        let Some(&(a, b)) = segments[link.wheel].get(link.index) else {
            continue;
        };
        let (a, b) = (to_bevy(pos + rot * a), to_bevy(pos + rot * b));
        let d = b - a;
        t.translation = 0.5 * (a + b);
        t.rotation = Quat::from_rotation_arc(Vec3::Y, d.normalize_or(Vec3::Y));
        t.scale = Vec3::new(1.0, d.length(), 1.0);
    }
}
