//! Writes the sample library, `assets/machine-src/`: a GT3-class machine whose frame,
//! suspension, tyres, drivetrain and aero come from `assets/cars/gt3.ron` and
//! `assets/tires/`, with the engine simulator's sample engines, intakes and exhausts.
//! The files are the samples' source from then on; this only makes them afresh.
//!
//! `cargo run -p open-racing-machine-project --example make_samples`

use std::path::Path;

use open_racing_engine_sim::samples;
use open_racing_engine_sim::spec::{End, Network};
use open_racing_machine_project::library::{Library, Target};
use open_racing_machine_project::machine::{
    Attach, Axle, AxleSetup, Driver, FORMAT, Machine, Placed, Setup,
};
use open_racing_machine_project::part::*;
use open_racing_sim::params::{AxleParams, CarParams};
use open_racing_sim::tire::TireParams;

const CARBON: [f32; 3] = [0.12, 0.12, 0.13];
const RED: [f32; 3] = [0.78, 0.08, 0.07];
const METAL: [f32; 3] = [0.62, 0.62, 0.66];
const DARK: [f32; 3] = [0.25, 0.25, 0.27];
const RUBBER: [f32; 3] = [0.07, 0.07, 0.07];

fn b(name: &str, size: [f64; 3], at: [f64; 3], mass: f64, colour: [f32; 3]) -> Shape {
    Shape {
        name: name.into(),
        kind: ShapeKind::Box { size },
        at,
        rotation_deg: [0.0; 3],
        mass,
        colour,
        role: Role::Body,
    }
}

fn cyl(
    name: &str,
    radius: f64,
    length: f64,
    axis: Axis,
    at: [f64; 3],
    mass: f64,
    colour: [f32; 3],
) -> Shape {
    Shape {
        name: name.into(),
        kind: ShapeKind::Cylinder {
            radius,
            length,
            axis,
        },
        at,
        rotation_deg: [0.0; 3],
        mass,
        colour,
        role: Role::Body,
    }
}

fn role(mut s: Shape, r: Role) -> Shape {
    s.role = r;
    s
}

fn mount(name: &str, at: [f64; 3]) -> Mount {
    Mount {
        name: name.into(),
        at,
        rotation_deg: [0.0; 3],
        joint: None,
    }
}

fn part(description: &str, shapes: Vec<Shape>, mounts: Vec<Mount>, design: Design) -> Part {
    Part {
        description: description.into(),
        physical: Physical {
            shapes,
            mounts,
            mass: MassSpec::Shapes,
            model: None,
        },
        design,
    }
}

/// Moves a network's mouths.
fn mouths(mut n: Network, f: impl Fn(&str) -> [f64; 3]) -> Network {
    for p in &mut n.pipes {
        for e in [&mut p.a, &mut p.b] {
            if let End::Ambient { at, .. } = e {
                *at = f(&p.name);
            }
        }
    }
    n
}

fn suspension(a: &AxleParams, track: f64, disc: f64) -> Part {
    part(
        "Double wishbones; the left corner's hardpoints relative to its wheel centre.",
        vec![
            role(
                b("upright", [0.08, 0.06, 0.26], [0.0, -0.07, 0.0], 6.0, DARK),
                Role::Hub,
            ),
            role(
                cyl("disc", 0.18, 0.035, Axis::Y, [0.0, -0.11, 0.0], disc, METAL),
                Role::Hub,
            ),
            role(
                b(
                    "caliper",
                    [0.10, 0.05, 0.14],
                    [-0.12, -0.13, 0.08],
                    4.0,
                    RED,
                ),
                Role::Hub,
            ),
            role(
                b(
                    "upper arm",
                    [0.30, 0.34, 0.02],
                    [0.0, -0.28, 0.13],
                    3.0,
                    DARK,
                ),
                Role::Link,
            ),
            role(
                b(
                    "lower arm",
                    [0.40, 0.48, 0.025],
                    [0.0, -0.32, -0.15],
                    4.5,
                    DARK,
                ),
                Role::Link,
            ),
            b(
                "coil-over",
                [0.07, 0.07, 0.30],
                [0.0, -0.45, 0.10],
                3.0,
                [0.9, 0.7, 0.1],
            ),
        ],
        vec![],
        Design::Suspension(Suspension {
            track,
            linkage: a.linkage.clone(),
            actuation: a.actuation.clone(),
            bump_travel: a.bump_travel,
            droop_travel: a.droop_travel,
            bump_stop_rate: a.bump_stop_rate,
        }),
    )
}

fn wheel(t: TireParams, inertia: f64) -> Part {
    let (r, w) = (t.radius, t.width);
    part(
        &format!("18-inch centre-lock wheel with a {}.", t.name),
        vec![
            role(
                cyl("tyre", r, w, Axis::Y, [0.0; 3], 20.0, RUBBER),
                Role::Wheel,
            ),
            role(
                cyl("rim", 0.235, w + 0.01, Axis::Y, [0.0; 3], 9.0, METAL),
                Role::Wheel,
            ),
        ],
        vec![],
        Design::Wheel(Wheel { tire: t, inertia }),
    )
}

fn setup(a: &AxleParams, ride_height: f64) -> AxleSetup {
    AxleSetup {
        pressure: a.pressure,
        spring_rate: a.spring_rate,
        bump_damping: a.bump_damping,
        rebound_damping: a.rebound_damping,
        fast_bump_damping: a.fast_bump_damping,
        fast_rebound_damping: a.fast_rebound_damping,
        damper_knee: a.damper_knee,
        anti_roll_rate: a.anti_roll_rate,
        heave: a.heave.clone(),
        camber: a.static_camber,
        toe: a.static_toe,
        ride_height,
    }
}

fn placed(
    name: &str,
    part: &str,
    attach: Option<(&str, &str, Option<&str>)>,
    axle: Option<Axle>,
) -> Placed {
    Placed {
        name: name.into(),
        part: part.into(),
        attach: attach.map(|(to, at, mount)| Attach {
            to: to.into(),
            at: at.into(),
            mount: mount.map(String::from),
        }),
        at: [0.0; 3],
        rotation_deg: [0.0; 3],
        axle,
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let gt3 = CarParams::load(root.join("cars/gt3.ron")).expect("gt3.ron");
    let tire = |n: &str| TireParams::load(root.join(format!("tires/{n}.ron"))).expect("tyre");
    let mut lib = Library::new(root.join("machine-src"));
    let mut put = |t: &str, p: Part| {
        let Target::Part(r) = Target::parse(t).unwrap() else {
            unreachable!()
        };
        lib.parts.insert(r, p);
    };
    let (zf, zr) = (
        tire("velloni_zeta_gt_front").radius,
        tire("velloni_zeta_gt_rear").radius,
    );
    let wb = gt3.wheelbase;

    put(
        "frame/gt3_tub",
        part(
            "Steel space frame and roll cage of a GT3-class car; the mounts name where the rest goes.",
            vec![
                b("tub", [2.3, 1.25, 0.45], [-1.25, 0.0, 0.33], 180.0, CARBON),
                b(
                    "front subframe",
                    [0.8, 1.0, 0.25],
                    [0.25, 0.0, 0.35],
                    45.0,
                    DARK,
                ),
                b(
                    "rear subframe",
                    [0.9, 1.0, 0.3],
                    [-2.75, 0.0, 0.40],
                    50.0,
                    DARK,
                ),
                b(
                    "roll cage",
                    [1.2, 1.3, 0.6],
                    [-1.2, 0.0, 0.95],
                    45.0,
                    [0.85, 0.85, 0.85],
                ),
            ],
            vec![
                mount("front axle", [0.0, 0.0, zf]),
                mount("rear axle", [-wb, 0.0, zr]),
                mount("engine", [-1.95, 0.0, 0.12]),
                mount("seat", [-1.15, 0.36, 0.12]),
                mount("fuel", [-1.6, 0.0, 0.25]),
                mount("steering", [0.1, 0.0, 0.3]),
                mount("pedals", [-0.3, 0.3, 0.3]),
                mount("electronics", [-0.4, -0.35, 0.3]),
                mount("origin", [0.0; 3]),
            ],
            Design::Frame(Frame {
                torsional_stiffness: 35_000.0 * 57.3,
            }),
        ),
    );
    put(
        "body/gt3_body",
        part(
            "Composite bodywork, as blocks until its model is imported.",
            vec![
                b("nose", [0.9, 1.9, 0.35], [0.55, 0.0, 0.42], 25.0, RED),
                b("cabin", [2.0, 1.55, 0.5], [-1.25, 0.0, 1.05], 40.0, RED),
                b("sides", [2.6, 2.0, 0.45], [-1.2, 0.0, 0.55], 45.0, RED),
                b("tail", [1.2, 2.0, 0.55], [-3.2, 0.0, 0.6], 40.0, RED),
            ],
            vec![],
            Design::Body(Body {}),
        ),
    );
    // Aero element positions: gt3.ron's are relative to its CG (1.4575 m behind the front
    // axle, 0.42 m up); here they are in the machine's frame.
    let (cgx, cgz) = (-wb * (1.0 - gt3.front_weight), gt3.cg_height);
    let mut elements = gt3.aero.elements.clone();
    for e in &mut elements {
        e.position = [e.position[0] + cgx, e.position[1] + cgz];
    }
    put(
        "aero/gt3_aero",
        part(
            "Splitter, floor with diffuser, and the rear wing; the body's own drag.",
            vec![
                b(
                    "splitter",
                    [0.35, 1.9, 0.02],
                    [0.95, 0.0, 0.08],
                    6.0,
                    CARBON,
                ),
                b(
                    "rear wing",
                    [0.30, 1.8, 0.03],
                    [-3.46, 0.0, 1.22],
                    10.0,
                    CARBON,
                ),
                b(
                    "pylon left",
                    [0.2, 0.02, 0.4],
                    [-3.4, 0.3, 1.0],
                    3.0,
                    CARBON,
                ),
                b(
                    "pylon right",
                    [0.2, 0.02, 0.4],
                    [-3.4, -0.3, 1.0],
                    3.0,
                    CARBON,
                ),
                b("diffuser", [0.6, 1.6, 0.05], [-3.2, 0.0, 0.15], 8.0, CARBON),
            ],
            vec![],
            Design::Aero(Aero { elements }),
        ),
    );
    put(
        "engine/v8_4l_flatplane",
        part(
            "4.0 l flat-plane V8, 94 × 72 mm, dry sump, 9000 rpm.",
            vec![
                b("block", [0.60, 0.62, 0.45], [0.0, 0.0, 0.25], 140.0, METAL),
                cyl(
                    "flywheel",
                    0.12,
                    0.04,
                    Axis::X,
                    [-0.32, 0.0, 0.16],
                    8.0,
                    DARK,
                ),
                b(
                    "ancillaries",
                    [0.15, 0.3, 0.15],
                    [0.30, 0.1, 0.3],
                    12.0,
                    DARK,
                ),
            ],
            vec![
                mount("base", [0.0; 3]),
                mount("bellhousing", [-0.34, 0.0, 0.16]),
                mount("intake", [0.0, 0.0, 0.5]),
            ],
            Design::Engine(EnginePart {
                spec: samples::v8(),
                cooling: gt3.engine.cooling.clone(),
                bench: Bench {
                    intake: Some("intake/v8_itb".into()),
                    exhaust: Some("exhaust/v8_race".into()),
                },
            }),
        ),
    );
    put(
        "engine/i4_2l_na",
        part(
            "2.0 l naturally aspirated inline four, 86 × 86 mm, DOHC four valves.",
            vec![
                b("block", [0.45, 0.35, 0.45], [0.0, 0.0, 0.25], 100.0, METAL),
                cyl(
                    "flywheel",
                    0.14,
                    0.035,
                    Axis::X,
                    [-0.26, 0.0, 0.14],
                    9.0,
                    DARK,
                ),
                b(
                    "ancillaries",
                    [0.15, 0.3, 0.15],
                    [0.25, 0.15, 0.3],
                    10.0,
                    DARK,
                ),
            ],
            vec![
                mount("base", [0.0; 3]),
                mount("bellhousing", [-0.28, 0.0, 0.14]),
                mount("intake", [0.0, 0.25, 0.4]),
            ],
            Design::Engine(EnginePart {
                spec: samples::i4(),
                cooling: Default::default(),
                bench: Bench {
                    intake: Some("intake/i4_plenum".into()),
                    exhaust: Some("exhaust/i4_road".into()),
                },
            }),
        ),
    );
    put(
        "intake/v8_itb",
        part(
            "Carbon air box over eight trumpets with individual throttles.",
            vec![
                b("air box", [0.55, 0.5, 0.18], [0.0, 0.0, 0.12], 7.0, CARBON),
                b("trumpets", [0.4, 0.5, 0.1], [0.0, 0.0, 0.0], 3.0, METAL),
            ],
            vec![],
            Design::Intake(IntakePart {
                network: mouths(samples::v8_intake(), |_| [0.25, 0.0, 0.3]),
                throttle: Default::default(),
            }),
        ),
    );
    put(
        "intake/i4_plenum",
        part(
            "Air box, 60 mm drive-by-wire throttle, 2.5 l plenum and 300 mm runners.",
            vec![
                b("air box", [0.35, 0.3, 0.2], [0.3, 0.1, 0.1], 3.0, DARK),
                b("plenum", [0.4, 0.15, 0.15], [0.0, 0.0, 0.0], 4.0, METAL),
            ],
            vec![],
            Design::Intake(IntakePart {
                network: mouths(samples::i4_intake(), |_| [0.5, 0.25, 0.1]),
                throttle: Default::default(),
            }),
        ),
    );
    let tails = |name: &str| {
        let y = if name.ends_with('B') {
            -0.45
        } else if name.ends_with('A') {
            0.45
        } else {
            0.0
        };
        [-3.9, y, 0.3]
    };
    put(
        "exhaust/v8_race",
        part(
            "Racing exhaust: a 4-into-1 per bank, short silencers, two tailpipes. Placed at the machine's origin: its mouths are in the machine's frame.",
            vec![
                b("headers", [0.5, 0.9, 0.2], [-2.0, 0.0, 0.2], 8.0, METAL),
                cyl(
                    "silencer A",
                    0.08,
                    0.35,
                    Axis::X,
                    [-3.3, 0.45, 0.3],
                    5.0,
                    METAL,
                ),
                cyl(
                    "silencer B",
                    0.08,
                    0.35,
                    Axis::X,
                    [-3.3, -0.45, 0.3],
                    5.0,
                    METAL,
                ),
            ],
            vec![],
            Design::Exhaust(ExhaustPart {
                network: mouths(samples::v8_exhaust(), tails),
            }),
        ),
    );
    // Both banks into one silencer and tailpipe: the tailpipe hears all eight cylinders.
    let mut merged = samples::v8_exhaust();
    merged
        .pipes
        .retain(|p| !p.name.starts_with("tail") && !p.name.contains(".pipe"));
    merged.volumes.retain(|v| !v.name.starts_with("silencer"));
    merged.volumes.push(open_racing_engine_sim::spec::Volume {
        name: "merge".into(),
        volume: 0.6e-3,
        wall_temperature: 320.0,
    });
    merged.volumes.push(open_racing_engine_sim::spec::Volume {
        name: "silencer".into(),
        volume: 7e-3,
        wall_temperature: 320.0,
    });
    let pipe = |name: &str, length: f64, d: f64, wall: f64, a: End, b: End| {
        open_racing_engine_sim::spec::PipeSpec {
            name: name.into(),
            length,
            diameter: vec![(0.0, d)],
            wall_temperature: wall,
            roughness: 1e-4,
            friction: 1.0,
            a,
            b,
        }
    };
    let vol = |n: &str| End::Volume {
        name: n.into(),
        restriction: None,
    };
    for bank in ["A", "B"] {
        merged.pipes.push(pipe(
            &format!("collector.{bank}.pipe"),
            0.7,
            0.072,
            800.0,
            vol(&format!("collector.{bank}")),
            vol("merge"),
        ));
    }
    merged.pipes.push(pipe(
        "mid",
        0.5,
        0.090,
        650.0,
        vol("merge"),
        vol("silencer"),
    ));
    merged.pipes.push(pipe(
        "tail",
        0.5,
        0.090,
        550.0,
        vol("silencer"),
        End::Ambient {
            at: [-3.9, 0.0, 0.3],
            restriction: None,
        },
    ));
    put(
        "exhaust/v8_merged",
        part(
            "Both banks' collectors into one silencer and a single central tailpipe.",
            vec![
                b("headers", [0.5, 0.9, 0.2], [-2.0, 0.0, 0.2], 8.0, METAL),
                cyl(
                    "silencer",
                    0.12,
                    0.45,
                    Axis::X,
                    [-3.3, 0.0, 0.3],
                    9.0,
                    METAL,
                ),
            ],
            vec![],
            Design::Exhaust(ExhaustPart { network: merged }),
        ),
    );
    put(
        "exhaust/i4_road",
        part(
            "Road-car 4-2-1 with a catalyst, a resonator and a silencer.",
            vec![
                b("manifold", [0.3, 0.4, 0.2], [0.0, -0.3, 0.2], 6.0, METAL),
                cyl(
                    "silencer",
                    0.08,
                    0.45,
                    Axis::X,
                    [-3.6, 0.4, 0.3],
                    7.0,
                    METAL,
                ),
            ],
            vec![],
            Design::Exhaust(ExhaustPart {
                network: samples::i4_exhaust(),
            }),
        ),
    );
    let mut straight = Network::default();
    straight.volumes.push(open_racing_engine_sim::spec::Volume {
        name: "merge".into(),
        volume: 0.3e-3,
        wall_temperature: 320.0,
    });
    for c in 1..=4 {
        straight.pipes.push(pipe(
            &format!("primary.{c}"),
            0.7,
            0.036,
            900.0,
            End::Terminal(format!("exhaust.{c}")),
            vol("merge"),
        ));
    }
    straight.pipes.push(pipe(
        "tailpipe",
        1.8,
        0.055,
        650.0,
        vol("merge"),
        End::Ambient {
            at: [-3.9, 0.4, 0.3],
            restriction: None,
        },
    ));
    put(
        "exhaust/i4_straight",
        part(
            "A 4-into-1 and a straight pipe: no catalyst, no silencer.",
            vec![b("manifold", [0.3, 0.4, 0.2], [0.0, -0.3, 0.2], 5.0, METAL)],
            vec![],
            Design::Exhaust(ExhaustPart { network: straight }),
        ),
    );
    put(
        "transmission/gt3_seq6",
        part(
            "Six-speed sequential transaxle with paddle actuation.",
            vec![b("case", [0.6, 0.42, 0.38], [-0.3, 0.0, 0.0], 65.0, METAL)],
            vec![mount("front", [0.0; 3])],
            Design::Transmission(Transmission {
                clutch: gt3.clutch.clone(),
                gearbox: gt3.gearbox.clone(),
            }),
        ),
    );
    put(
        "driveline/gt3_rwd",
        part(
            "Rear drive through a plated limited-slip differential; the driveshafts.",
            vec![role(
                cyl("driveshafts", 0.03, 1.4, Axis::Y, [0.0; 3], 12.0, DARK),
                Role::Link,
            )],
            vec![],
            Design::Driveline(Driveline {
                drive: gt3.drive.clone(),
                differential: gt3.differential.clone(),
            }),
        ),
    );
    put(
        "suspension/gt3_front",
        suspension(&gt3.front, gt3.track_front, 11.0),
    );
    put(
        "suspension/gt3_rear",
        suspension(&gt3.rear, gt3.track_rear, 8.5),
    );
    put(
        "wheel/gt3_front",
        wheel(tire("velloni_zeta_gt_front"), gt3.front.wheel_inertia),
    );
    put(
        "wheel/gt3_rear",
        wheel(tire("velloni_zeta_gt_rear"), gt3.rear.wheel_inertia),
    );
    put(
        "brakes/gt3_iron",
        part(
            "Ventilated iron discs, six-piston calipers front, four rear (discs and calipers are in the suspensions).",
            vec![b("master cylinders", [0.15, 0.2, 0.1], [0.0; 3], 3.0, DARK)],
            vec![],
            Design::Brakes(Brakes {
                brakes: gt3.brakes.clone(),
            }),
        ),
    );
    put(
        "steering/gt3_rack",
        part(
            "Rack and pinion with power assistance.",
            vec![b("rack", [0.1, 0.9, 0.08], [0.0; 3], 9.0, DARK)],
            vec![],
            Design::Steering(Steering {
                steering: gt3.steering.clone(),
            }),
        ),
    );
    put(
        "interior/gt3_cockpit",
        part(
            "Racing seat, dash, steering wheel and pedal box; origin at the seat's mount.",
            vec![
                b("seat", [0.5, 0.5, 0.6], [0.05, 0.0, 0.3], 12.0, CARBON),
                b("dash", [0.3, 1.3, 0.25], [0.75, -0.36, 0.6], 8.0, CARBON),
                role(
                    cyl(
                        "steering wheel",
                        0.15,
                        0.04,
                        Axis::X,
                        [0.45, 0.0, 0.62],
                        2.0,
                        CARBON,
                    ),
                    Role::SteeringWheel,
                ),
                b(
                    "pedal box",
                    [0.35, 0.4, 0.25],
                    [1.15, 0.0, 0.15],
                    6.0,
                    METAL,
                ),
                b(
                    "extinguisher",
                    [0.12, 0.12, 0.4],
                    [-0.2, -0.3, 0.2],
                    5.0,
                    RED,
                ),
            ],
            vec![],
            Design::Interior(Interior {
                seat: [0.0, 0.0, 0.05],
                eye: [0.12, 0.0, 0.93],
                steering_wheel: [0.45, 0.0, 0.62],
                steering_axis: [-0.95, 0.0, 0.3],
            }),
        ),
    );
    put(
        "fuel_tank/gt3_120l",
        part(
            "120 l FIA safety cell.",
            vec![b("cell", [0.5, 0.8, 0.35], [0.0; 3], 14.0, [0.2, 0.2, 0.2])],
            vec![],
            Design::FuelTank(FuelTank {
                capacity: 120.0,
                density: 0.745,
            }),
        ),
    );
    put(
        "electronics/gt3_paddle",
        part(
            "Engine and gearbox control units, loom and paddle shift.",
            vec![b("units", [0.3, 0.25, 0.1], [0.0; 3], 12.0, DARK)],
            vec![],
            Design::Electronics(Electronics {
                electronics: gt3.electronics.clone(),
            }),
        ),
    );
    let machine = Machine {
        format: FORMAT,
        description: "A GT3-class machine with the flat-plane V8 and its racing exhaust.".into(),
        parts: vec![
            placed("tub", "frame/gt3_tub", None, None),
            placed("body", "body/gt3_body", Some(("tub", "origin", None)), None),
            placed("aero", "aero/gt3_aero", Some(("tub", "origin", None)), None),
            placed(
                "engine",
                "engine/v8_4l_flatplane",
                Some(("tub", "engine", Some("base"))),
                None,
            ),
            placed(
                "intake",
                "intake/v8_itb",
                Some(("engine", "intake", None)),
                None,
            ),
            placed(
                "exhaust",
                "exhaust/v8_race",
                Some(("tub", "origin", None)),
                None,
            ),
            placed(
                "gearbox",
                "transmission/gt3_seq6",
                Some(("engine", "bellhousing", Some("front"))),
                None,
            ),
            placed(
                "driveline",
                "driveline/gt3_rwd",
                Some(("tub", "rear axle", None)),
                None,
            ),
            placed(
                "front suspension",
                "suspension/gt3_front",
                Some(("tub", "front axle", None)),
                Some(Axle::Front),
            ),
            placed(
                "rear suspension",
                "suspension/gt3_rear",
                Some(("tub", "rear axle", None)),
                Some(Axle::Rear),
            ),
            placed("front wheels", "wheel/gt3_front", None, Some(Axle::Front)),
            placed("rear wheels", "wheel/gt3_rear", None, Some(Axle::Rear)),
            placed(
                "brakes",
                "brakes/gt3_iron",
                Some(("tub", "pedals", None)),
                None,
            ),
            placed(
                "steering",
                "steering/gt3_rack",
                Some(("tub", "steering", None)),
                None,
            ),
            placed(
                "cockpit",
                "interior/gt3_cockpit",
                Some(("tub", "seat", None)),
                None,
            ),
            placed(
                "fuel cell",
                "fuel_tank/gt3_120l",
                Some(("tub", "fuel", None)),
                None,
            ),
            placed(
                "electronics",
                "electronics/gt3_paddle",
                Some(("tub", "electronics", None)),
                None,
            ),
        ],
        gas: vec![],
        setup: Setup {
            front: setup(&gt3.front, gt3.aero.ride_height[0]),
            rear: setup(&gt3.rear, gt3.aero.ride_height[1]),
            brake_bias: gt3.brakes.front_bias,
            wings: vec![("rear wing".into(), 0.14)],
            gearing: None,
            differential: None,
            fuel: 60.0,
            ballast: vec![([-0.6, 0.0, 0.1], 60.0)],
        },
        driver: Driver { mass: 80.0 },
    };
    lib.machines.insert("gt3_v8".into(), machine);
    open_racing_machine_project::validate::library(&lib).expect("the samples check");
    let written = lib.save().expect("written");
    println!("{} files in {}", written.len(), lib.dir.display());
}
