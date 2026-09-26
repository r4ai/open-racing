//! Reference engines and systems: a naturally aspirated 2.0 l inline four with a
//! plenum intake and a 4-2-1 exhaust, its 1.8 l high-revving cousin with switched cam lobes,
//! and a 4.0 l flat-plane V8 with individual throttle bodies. They are starting points
//! for new designs, the machine tool's samples, and what the tests check the simulator
//! against.

use crate::spec::*;

fn cd_intake() -> Vec<(f64, f64)> {
    vec![
        (0.0, 0.55),
        (0.05, 0.60),
        (0.10, 0.66),
        (0.20, 0.63),
        (0.30, 0.57),
        (0.40, 0.53),
    ]
}

fn cd_exhaust() -> Vec<(f64, f64)> {
    vec![
        (0.0, 0.55),
        (0.05, 0.60),
        (0.10, 0.64),
        (0.20, 0.60),
        (0.30, 0.55),
        (0.40, 0.50),
    ]
}

/// 2.0 l naturally aspirated inline four: 86 × 86 mm, DOHC four valves, 11.0:1.
pub fn i4() -> EngineSpec {
    EngineSpec {
        bore: 0.086,
        stroke: 0.086,
        rod: 0.145,
        compression_ratio: 11.0,
        pin_offset: 0.0,
        layout: Layout {
            banks: vec![Bank {
                name: "A".into(),
                angle_deg: 0.0,
            }],
            throws_deg: vec![0.0, 180.0, 180.0, 0.0],
            cylinders: (0..4)
                .map(|t| CylinderPlace {
                    bank: "A".into(),
                    throw: t,
                })
                .collect(),
            firing_order: vec![1, 3, 4, 2],
        },
        intake: Head {
            valves: Valves {
                count: 2,
                diameter: 0.034,
                stem: 0.0055,
                cd: cd_intake(),
            },
            cam: Cam {
                lift: 0.0105,
                duration_deg: 260.0,
                centreline_deg: 108.0,
                profile: Profile::Polynomial,
            },
            high_cam: None,
            port: Port {
                length: 0.10,
                diameter: vec![(0.0, 0.038), (1.0, 0.040)],
                wall_temperature: 380.0,
            },
        },
        exhaust: Head {
            valves: Valves {
                count: 2,
                diameter: 0.029,
                stem: 0.0055,
                cd: cd_exhaust(),
            },
            cam: Cam {
                lift: 0.0095,
                duration_deg: 256.0,
                centreline_deg: 112.0,
                profile: Profile::Polynomial,
            },
            high_cam: None,
            port: Port {
                length: 0.08,
                diameter: vec![(0.0, 0.031), (1.0, 0.034)],
                wall_temperature: 700.0,
            },
        },
        crank: Crank {
            flywheel_inertia: 0.09,
            crank_inertia: 0.02,
            reciprocating_mass: 0.45,
            rotating_mass: 0.35,
        },
        combustion: Combustion {
            fuel: Fuel::Gasoline,
            wiebe_a: 5.0,
            wiebe_m: 2.0,
            duration_deg: 55.0,
            reference_rpm: 3000.0,
            speed_exponent: 0.35,
            efficiency: 0.96,
            variation: 0.03,
            seed: 1,
            octane: 98.0,
        },
        heat_transfer: HeatTransfer::default(),
        friction: Friction::default(),
        ecu: Ecu {
            idle_rpm: 850.0,
            limiter_rpm: 7000.0,
            stall_rpm: 350.0,
            limiter_hysteresis_rpm: 150.0,
            idle_opening: 0.009,
            idle_authority: 0.04,
            idle_gain: 0.00005,
            limiter_cut: Cut::Fuel,
            cam_switch: None,
            intake_phase: None,
            exhaust_phase: None,
            overrun_cut_rpm: Some(1500.0),
            pops: None,
            spark_deg: Map2 {
                rpm: vec![1000.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 7000.0],
                load: vec![0.0, 0.5, 1.0],
                values: vec![
                    vec![20.0, 16.0, 10.0],
                    vec![32.0, 26.0, 18.0],
                    vec![38.0, 32.0, 24.0],
                    vec![40.0, 34.0, 27.0],
                    vec![42.0, 36.0, 29.0],
                    vec![42.0, 37.0, 30.0],
                    vec![42.0, 37.0, 31.0],
                ],
            },
            lambda: Map2 {
                rpm: vec![1000.0, 4000.0, 7000.0],
                load: vec![0.0, 0.7, 1.0],
                values: vec![
                    vec![1.0, 1.0, 0.92],
                    vec![1.0, 1.0, 0.89],
                    vec![1.0, 0.98, 0.87],
                ],
            },
        },
    }
}

/// 1.8 l inline four with switched cam lobes, in the manner of Honda's B18C: 81 × 87.2 mm,
/// 11.1:1, a mild low lobe for the street and a long, high one that the rocker pins lock in
/// above 5200 rpm with the throttle open, to an 8400 rpm limiter.
pub fn i4_vtec() -> EngineSpec {
    let mut e = i4();
    e.bore = 0.081;
    e.stroke = 0.0872;
    e.rod = 0.1385;
    e.compression_ratio = 11.1;
    e.intake.valves.diameter = 0.033;
    e.exhaust.valves.diameter = 0.028;
    e.intake.cam = Cam {
        lift: 0.0078,
        duration_deg: 232.0,
        centreline_deg: 112.0,
        profile: Profile::Polynomial,
    };
    e.intake.high_cam = Some(Cam {
        lift: 0.0115,
        duration_deg: 282.0,
        centreline_deg: 106.0,
        profile: Profile::Polynomial,
    });
    e.exhaust.cam = Cam {
        lift: 0.0076,
        duration_deg: 228.0,
        centreline_deg: 112.0,
        profile: Profile::Polynomial,
    };
    e.exhaust.high_cam = Some(Cam {
        lift: 0.0105,
        duration_deg: 270.0,
        centreline_deg: 108.0,
        profile: Profile::Polynomial,
    });
    e.crank.flywheel_inertia = 0.075;
    e.ecu.limiter_rpm = 8400.0;
    e.ecu.cam_switch = Some(CamSwitch {
        rpm: 5200.0,
        hysteresis_rpm: 200.0,
        min_load: 0.3,
    });
    e.ecu.spark_deg.rpm.push(8500.0);
    let last = e.ecu.spark_deg.values.last().unwrap().clone();
    e.ecu.spark_deg.values.push(last);
    e
}

/// The VTEC four's intake: the plenum's, with a bigger throttle and short runners tuned
/// for its high lobes' speeds.
pub fn i4_vtec_intake() -> Network {
    let mut n = i4_intake();
    for p in &mut n.pipes {
        if p.name.starts_with("runner") {
            p.length = 0.20;
        }
        if let End::Volume {
            restriction: Some(Restriction::Throttle { bore, .. }),
            ..
        } = &mut p.b
        {
            *bore = 0.064;
        }
    }
    n
}

fn pipe(name: &str, length: f64, d: &[(f64, f64)], wall: f64, a: End, b: End) -> PipeSpec {
    PipeSpec {
        name: name.into(),
        length,
        diameter: d.to_vec(),
        wall_temperature: wall,
        roughness: 1e-4,
        friction: 1.0,
        heat: 1.0,
        a,
        b,
    }
}

fn vol(name: &str) -> End {
    End::Volume {
        name: name.into(),
        restriction: None,
    }
}

fn volume(name: &str, litres: f64) -> Volume {
    Volume {
        name: name.into(),
        volume: litres * 1e-3,
        wall_temperature: 320.0,
    }
}

/// Air box, 65 mm zip tube, 60 mm throttle, 2.5 l plenum and four 300 mm runners.
pub fn i4_intake() -> Network {
    let mut n = Network {
        volumes: vec![volume("airbox", 8.0), volume("plenum", 2.5)],
        pipes: vec![
            pipe(
                "snorkel",
                0.35,
                &[(0.0, 0.070)],
                300.0,
                End::Ambient {
                    at: [0.9, 0.3, 0.55],
                    restriction: None,
                },
                vol("airbox"),
            ),
            pipe(
                "zip",
                0.30,
                &[(0.0, 0.065)],
                310.0,
                End::Volume {
                    name: "airbox".into(),
                    restriction: Some(Restriction::Fixed {
                        cd: 0.9,
                        diameter: 0.075,
                    }),
                },
                End::Volume {
                    name: "plenum".into(),
                    restriction: Some(Restriction::Throttle {
                        bore: 0.060,
                        shaft: 0.008,
                        closed_angle_deg: 7.0,
                    }),
                },
            ),
        ],
        orifices: vec![],
        compressors: vec![],
        turbines: vec![],
    };
    for c in 1..=4 {
        n.pipes.push(pipe(
            &format!("runner.{c}"),
            0.30,
            &[(0.0, 0.045), (1.0, 0.040)],
            330.0,
            vol("plenum"),
            End::Terminal(format!("intake.{c}")),
        ));
    }
    n
}

/// A road-car 4-2-1 exhaust: 700 mm primaries paired 1-4 and 2-3, secondaries, a
/// catalyst, two expansion-chamber silencers and a tailpipe.
pub fn i4_exhaust() -> Network {
    let mut n = Network {
        volumes: vec![
            volume("merge.14", 0.15),
            volume("merge.23", 0.15),
            volume("merge", 0.3),
            volume("cat.in", 0.8),
            volume("cat.out", 0.8),
        ],
        pipes: vec![],
        orifices: vec![],
        compressors: vec![],
        turbines: vec![],
    };
    for c in 1..=4 {
        let merge = if c == 1 || c == 4 {
            "merge.14"
        } else {
            "merge.23"
        };
        n.pipes.push(pipe(
            &format!("primary.{c}"),
            0.70,
            &[(0.0, 0.036)],
            900.0,
            End::Terminal(format!("exhaust.{c}")),
            vol(merge),
        ));
    }
    for m in ["14", "23"] {
        n.pipes.push(pipe(
            &format!("secondary.{m}"),
            0.40,
            &[(0.0, 0.045)],
            800.0,
            vol(&format!("merge.{m}")),
            vol("merge"),
        ));
    }
    let join = |j: &str| End::Join(j.into());
    n.pipes.push(pipe(
        "downpipe",
        1.0,
        &[(0.0, 0.050)],
        700.0,
        vol("merge"),
        vol("cat.in"),
    ));
    let mut cat = pipe(
        "catalyst",
        0.15,
        &[(0.0, 0.100)],
        750.0,
        vol("cat.in"),
        vol("cat.out"),
    );
    cat.friction = 40.0;
    n.pipes.push(cat);
    n.pipes.push(pipe(
        "midpipe",
        1.2,
        &[(0.0, 0.050)],
        550.0,
        vol("cat.out"),
        join("resonator.in"),
    ));
    n.pipes.push(pipe(
        "resonator",
        0.35,
        &[(0.0, 0.120)],
        500.0,
        join("resonator.in"),
        join("resonator.out"),
    ));
    n.pipes.push(pipe(
        "link",
        0.9,
        &[(0.0, 0.050)],
        450.0,
        join("resonator.out"),
        join("silencer.in"),
    ));
    let mut silencer = pipe(
        "silencer",
        0.45,
        &[(0.0, 0.160)],
        420.0,
        join("silencer.in"),
        join("silencer.out"),
    );
    silencer.friction = 3.0;
    n.pipes.push(silencer);
    n.pipes.push(pipe(
        "tailpipe",
        0.35,
        &[(0.0, 0.050)],
        400.0,
        join("silencer.out"),
        End::Ambient {
            at: [-4.2, 0.4, 0.3],
            restriction: None,
        },
    ));
    n
}

/// 4.0 l flat-plane V8: 94 × 72 mm, 90° banks, crank throws 0/180/180/0, 12.5:1, firing
/// alternately from bank to bank.
pub fn v8() -> EngineSpec {
    let cylinders = (0..8)
        .map(|i| CylinderPlace {
            bank: if i < 4 { "A".into() } else { "B".into() },
            throw: i % 4,
        })
        .collect();
    EngineSpec {
        bore: 0.094,
        stroke: 0.072,
        rod: 0.138,
        compression_ratio: 12.5,
        pin_offset: 0.0,
        layout: Layout {
            banks: vec![
                Bank {
                    name: "A".into(),
                    angle_deg: 45.0,
                },
                Bank {
                    name: "B".into(),
                    angle_deg: -45.0,
                },
            ],
            throws_deg: vec![0.0, 180.0, 180.0, 0.0],
            cylinders,
            firing_order: vec![1, 6, 2, 5, 4, 7, 3, 8],
        },
        intake: Head {
            valves: Valves {
                count: 2,
                diameter: 0.038,
                stem: 0.006,
                cd: cd_intake(),
            },
            cam: Cam {
                lift: 0.0125,
                duration_deg: 280.0,
                centreline_deg: 105.0,
                profile: Profile::Polynomial,
            },
            high_cam: None,
            port: Port {
                length: 0.09,
                diameter: vec![(0.0, 0.044), (1.0, 0.046)],
                wall_temperature: 380.0,
            },
        },
        exhaust: Head {
            valves: Valves {
                count: 2,
                diameter: 0.032,
                stem: 0.006,
                cd: cd_exhaust(),
            },
            cam: Cam {
                lift: 0.0115,
                duration_deg: 276.0,
                centreline_deg: 108.0,
                profile: Profile::Polynomial,
            },
            high_cam: None,
            port: Port {
                length: 0.08,
                diameter: vec![(0.0, 0.034), (1.0, 0.038)],
                wall_temperature: 720.0,
            },
        },
        crank: Crank {
            flywheel_inertia: 0.06,
            crank_inertia: 0.03,
            reciprocating_mass: 0.40,
            rotating_mass: 0.35,
        },
        combustion: Combustion {
            fuel: Fuel::Gasoline,
            wiebe_a: 5.0,
            wiebe_m: 2.0,
            duration_deg: 50.0,
            reference_rpm: 3000.0,
            speed_exponent: 0.35,
            efficiency: 0.96,
            variation: 0.03,
            seed: 3,
            octane: 100.0,
        },
        heat_transfer: HeatTransfer::default(),
        friction: Friction::default(),
        ecu: Ecu {
            idle_rpm: 1600.0,
            limiter_rpm: 9000.0,
            stall_rpm: 500.0,
            limiter_hysteresis_rpm: 200.0,
            idle_opening: 0.006,
            idle_authority: 0.04,
            idle_gain: 0.00005,
            limiter_cut: Cut::Fuel,
            cam_switch: None,
            intake_phase: None,
            exhaust_phase: None,
            overrun_cut_rpm: Some(2000.0),
            pops: None,
            spark_deg: Map2 {
                rpm: vec![1000.0, 3000.0, 5000.0, 7000.0, 9000.0],
                load: vec![0.0, 0.5, 1.0],
                values: vec![
                    vec![22.0, 16.0, 10.0],
                    vec![38.0, 30.0, 22.0],
                    vec![42.0, 34.0, 27.0],
                    vec![44.0, 36.0, 30.0],
                    vec![44.0, 37.0, 32.0],
                ],
            },
            lambda: Map2 {
                rpm: vec![1000.0, 9000.0],
                load: vec![0.0, 0.7, 1.0],
                values: vec![vec![1.0, 0.98, 0.9], vec![1.0, 0.95, 0.87]],
            },
        },
    }
}

/// A 12 l air box feeding eight 200 mm trumpets, each with its own 48 mm throttle.
pub fn v8_intake() -> Network {
    let mut n = Network {
        volumes: vec![volume("airbox", 12.0)],
        pipes: vec![pipe(
            "snorkel",
            0.25,
            &[(0.0, 0.10)],
            300.0,
            End::Ambient {
                at: [0.4, 0.0, 1.1],
                restriction: None,
            },
            vol("airbox"),
        )],
        orifices: vec![],
        compressors: vec![],
        turbines: vec![],
    };
    for c in 1..=8 {
        n.pipes.push(pipe(
            &format!("trumpet.{c}"),
            0.20,
            &[(0.0, 0.056), (0.4, 0.048), (1.0, 0.046)],
            320.0,
            End::Volume {
                name: "airbox".into(),
                restriction: Some(Restriction::Throttle {
                    bore: 0.048,
                    shaft: 0.006,
                    closed_angle_deg: 6.0,
                }),
            },
            End::Terminal(format!("intake.{c}")),
        ));
    }
    n
}

/// Racing exhaust: a 4-into-1 for each bank, a short silencer, and two tailpipes.
pub fn v8_exhaust() -> Network {
    let mut n = Network {
        volumes: vec![
            volume("collector.A", 0.3),
            volume("collector.B", 0.3),
            volume("silencer.A", 4.0),
            volume("silencer.B", 4.0),
        ],
        pipes: vec![],
        orifices: vec![],
        compressors: vec![],
        turbines: vec![],
    };
    for c in 1..=8 {
        let bank = if c <= 4 { "A" } else { "B" };
        n.pipes.push(pipe(
            &format!("primary.{c}"),
            0.55,
            &[(0.0, 0.042)],
            950.0,
            End::Terminal(format!("exhaust.{c}")),
            vol(&format!("collector.{bank}")),
        ));
    }
    for (bank, y) in [("A", 0.45), ("B", -0.45)] {
        n.pipes.push(pipe(
            &format!("collector.{bank}.pipe"),
            0.8,
            &[(0.0, 0.072)],
            800.0,
            vol(&format!("collector.{bank}")),
            vol(&format!("silencer.{bank}")),
        ));
        n.pipes.push(pipe(
            &format!("tail.{bank}"),
            0.5,
            &[(0.0, 0.070)],
            600.0,
            vol(&format!("silencer.{bank}")),
            End::Ambient {
                at: [-4.3, y, 0.35],
                restriction: None,
            },
        ));
    }
    n
}

/// A turbocharger's compressor with a small road car's map: about 2.5:1 at 130 000 rpm on
/// a 56 mm wheel.
fn compressor(
    name: &str,
    shaft: &str,
    inlet: &str,
    outlet: &str,
    wheel: f64,
    at: [f64; 3],
) -> Compressor {
    Compressor {
        name: name.into(),
        shaft: shaft.into(),
        inlet: inlet.into(),
        outlet: outlet.into(),
        wheel,
        inducer: 0.72 * wheel,
        blades: 6,
        head: 0.62,
        shutoff: 0.85,
        surge_flow: 0.05,
        choke_flow: 0.17,
        efficiency: 0.76,
        duct_length: 0.25,
        at,
    }
}

/// A turbine with a wastegate holding `boost` Pa.
fn turbine(name: &str, shaft: &str, inlet: &str, outlet: &str, area: f64, boost: f64) -> Turbine {
    Turbine {
        name: name.into(),
        shaft: shaft.into(),
        inlet: inlet.into(),
        outlet: outlet.into(),
        wheel: 0.052,
        area,
        efficiency: 0.70,
        inertia: 3.0e-5,
        wastegate: Some(Wastegate {
            diameter: 0.030,
            opens: boost - 0.15e5,
            open: boost + 0.15e5,
        }),
    }
}

/// 2.0 l turbocharged flat four in the manner of Subaru's EJ20: 92 × 75 mm, 8.0:1, an
/// intake cam phaser, and on the exhaust side headers of unequal length into one turbine,
/// whose unevenly spaced pulses are the "boxer rumble".
pub fn flat4_turbo() -> EngineSpec {
    let mut e = i4();
    e.bore = 0.092;
    e.stroke = 0.075;
    e.rod = 0.1305;
    e.compression_ratio = 8.0;
    e.layout = Layout {
        banks: vec![
            Bank {
                name: "R".into(),
                angle_deg: 90.0,
            },
            Bank {
                name: "L".into(),
                angle_deg: -90.0,
            },
        ],
        throws_deg: vec![0.0, 180.0, 180.0, 0.0],
        cylinders: ["R", "L", "R", "L"]
            .iter()
            .enumerate()
            .map(|(t, b)| CylinderPlace {
                bank: (*b).into(),
                throw: t,
            })
            .collect(),
        firing_order: vec![1, 3, 2, 4],
    };
    e.intake.valves.diameter = 0.036;
    e.exhaust.valves.diameter = 0.031;
    e.intake.cam = Cam {
        lift: 0.0096,
        duration_deg: 248.0,
        centreline_deg: 118.0,
        profile: Profile::Polynomial,
    };
    e.exhaust.cam = Cam {
        lift: 0.0091,
        duration_deg: 244.0,
        centreline_deg: 115.0,
        profile: Profile::Polynomial,
    };
    e.crank.flywheel_inertia = 0.11;
    e.ecu.limiter_rpm = 7900.0;
    e.ecu.idle_rpm = 800.0;
    e.ecu.intake_phase = Some(Map2 {
        rpm: vec![1000.0, 2500.0, 4500.0, 6500.0],
        load: vec![0.0, 0.5, 1.0],
        values: vec![
            vec![0.0, 5.0, 10.0],
            vec![5.0, 20.0, 25.0],
            vec![5.0, 20.0, 20.0],
            vec![0.0, 5.0, 5.0],
        ],
    });
    // Boost takes timing out and fuel in.
    e.ecu.spark_deg = Map2 {
        rpm: vec![
            1000.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 7000.0, 8000.0,
        ],
        load: vec![0.0, 0.5, 1.0],
        values: vec![
            vec![20.0, 14.0, 8.0],
            vec![32.0, 22.0, 10.0],
            vec![38.0, 26.0, 12.0],
            vec![40.0, 28.0, 14.0],
            vec![42.0, 30.0, 16.0],
            vec![42.0, 31.0, 17.0],
            vec![42.0, 32.0, 18.0],
            vec![42.0, 32.0, 18.0],
        ],
    };
    e.ecu.lambda = Map2 {
        rpm: vec![1000.0, 4000.0, 8000.0],
        load: vec![0.0, 0.6, 1.0],
        values: vec![
            vec![1.0, 1.0, 0.85],
            vec![1.0, 0.95, 0.80],
            vec![1.0, 0.92, 0.78],
        ],
    };
    e
}

/// The flat four's intake: air box, compressor, top-mounted intercooler, throttle,
/// plenum and runners, with a blow-off valve venting the charge to the air.
pub fn flat4_turbo_intake() -> Network {
    let mut n = Network {
        volumes: vec![
            volume("airbox", 6.0),
            volume("compressor.in", 0.4),
            volume("compressor.out", 0.4),
            volume("plenum", 2.5),
        ],
        pipes: vec![
            pipe(
                "snorkel",
                0.35,
                &[(0.0, 0.070)],
                300.0,
                End::Ambient {
                    at: [0.9, 0.3, 0.55],
                    restriction: None,
                },
                vol("airbox"),
            ),
            pipe(
                "inlet",
                0.45,
                &[(0.0, 0.065)],
                310.0,
                vol("airbox"),
                vol("compressor.in"),
            ),
            pipe(
                "charge.hot",
                0.5,
                &[(0.0, 0.055)],
                380.0,
                vol("compressor.out"),
                End::Join("intercooler.in".into()),
            ),
            PipeSpec {
                friction: 4.0,
                heat: 25.0,
                ..pipe(
                    "intercooler",
                    0.5,
                    &[(0.0, 0.09)],
                    320.0,
                    End::Join("intercooler.in".into()),
                    End::Join("intercooler.out".into()),
                )
            },
            pipe(
                "charge.cold",
                0.35,
                &[(0.0, 0.060)],
                320.0,
                End::Join("intercooler.out".into()),
                End::Volume {
                    name: "plenum".into(),
                    restriction: Some(Restriction::Throttle {
                        bore: 0.065,
                        shaft: 0.008,
                        closed_angle_deg: 7.0,
                    }),
                },
            ),
        ],
        orifices: vec![Orifice {
            name: "blow-off".into(),
            between: ("compressor.out".into(), "ambient".into()),
            restriction: Restriction::BlowOff {
                diameter: 0.025,
                reference: "plenum".into(),
                opens: 0.3e5,
                span: 0.2e5,
            },
            at: [0.4, -0.2, 0.6],
        }],
        compressors: vec![compressor(
            "compressor",
            "turbo",
            "compressor.in",
            "compressor.out",
            0.056,
            [0.35, 0.25, 0.5],
        )],
        turbines: vec![],
    };
    for c in 1..=4 {
        n.pipes.push(pipe(
            &format!("runner.{c}"),
            0.28,
            &[(0.0, 0.042), (1.0, 0.038)],
            330.0,
            vol("plenum"),
            End::Terminal(format!("intake.{c}")),
        ));
    }
    n
}

/// The flat four's exhaust: the right bank's headers short, the left's crossing under the
/// engine long, into the turbine; downpipe, catalyst, silencer.
pub fn flat4_turbo_exhaust() -> Network {
    let mut n = Network {
        volumes: vec![
            volume("crossover", 0.5),
            volume("turbine.out", 0.4),
            volume("cat", 1.2),
            volume("silencer", 6.0),
        ],
        pipes: vec![
            pipe(
                "downpipe",
                0.8,
                &[(0.0, 0.076)],
                800.0,
                vol("turbine.out"),
                vol("cat"),
            ),
            pipe(
                "centre",
                1.6,
                &[(0.0, 0.065)],
                600.0,
                vol("cat"),
                vol("silencer"),
            ),
            pipe(
                "tail",
                0.35,
                &[(0.0, 0.075)],
                500.0,
                vol("silencer"),
                End::Ambient {
                    at: [-4.2, 0.35, 0.3],
                    restriction: None,
                },
            ),
        ],
        orifices: vec![],
        compressors: vec![],
        turbines: vec![turbine(
            "turbine",
            "turbo",
            "crossover",
            "turbine.out",
            7.5e-4,
            1.0e5,
        )],
    };
    for c in 1..=4 {
        // Cylinders 1 and 3 on the right, next to the turbine; 2 and 4 cross over.
        let length = if c % 2 == 1 { 0.5 } else { 1.0 };
        n.pipes.push(pipe(
            &format!("primary.{c}"),
            length,
            &[(0.0, 0.038)],
            950.0,
            End::Terminal(format!("exhaust.{c}")),
            vol("crossover"),
        ));
    }
    n
}

/// 4.0 l twin-turbocharged cross-plane V8 in the manner of Mercedes-AMG's M177: 83 × 92 mm,
/// 8.6:1, 90° apart, firing 1-5-4-8-6-3-7-2, each bank into its own turbocharger.
pub fn v8_tt() -> EngineSpec {
    let mut e = v8();
    e.bore = 0.083;
    e.stroke = 0.092;
    e.rod = 0.1435;
    e.compression_ratio = 8.6;
    e.layout.banks[0].angle_deg = -45.0;
    e.layout.banks[1].angle_deg = 45.0;
    e.layout.throws_deg = vec![0.0, 90.0, 270.0, 180.0];
    e.layout.firing_order = vec![1, 5, 4, 8, 6, 3, 7, 2];
    e.intake.valves.diameter = 0.033;
    e.exhaust.valves.diameter = 0.028;
    e.intake.cam = Cam {
        lift: 0.0100,
        duration_deg: 250.0,
        centreline_deg: 112.0,
        profile: Profile::Polynomial,
    };
    e.exhaust.cam = Cam {
        lift: 0.0095,
        duration_deg: 246.0,
        centreline_deg: 112.0,
        profile: Profile::Polynomial,
    };
    e.crank.flywheel_inertia = 0.14;
    e.ecu.idle_rpm = 750.0;
    e.ecu.idle_opening = 0.008;
    e.ecu.limiter_rpm = 7000.0;
    let f4 = flat4_turbo();
    e.ecu.intake_phase = f4.ecu.intake_phase;
    e.ecu.spark_deg = f4.ecu.spark_deg;
    e.ecu.lambda = f4.ecu.lambda;
    e
}

/// The V8's intake: an air box and a compressor per bank, their charge pipes through
/// intercoolers into one throttle, plenum and runners; a diverter valve returns the
/// charge to a compressor's inlet when the throttle shuts.
pub fn v8_tt_intake() -> Network {
    let mut n = Network {
        volumes: vec![volume("charge", 1.0), volume("plenum", 5.0)],
        pipes: vec![pipe(
            "throttle.body",
            0.25,
            &[(0.0, 0.080)],
            320.0,
            vol("charge"),
            End::Volume {
                name: "plenum".into(),
                restriction: Some(Restriction::Throttle {
                    bore: 0.080,
                    shaft: 0.009,
                    closed_angle_deg: 7.0,
                }),
            },
        )],
        orifices: vec![],
        compressors: vec![],
        turbines: vec![],
    };
    for (bank, y) in [("A", 0.35), ("B", -0.35)] {
        for (v, size) in [
            ("airbox", 4.0),
            ("compressor.in", 0.3),
            ("compressor.out", 0.3),
        ] {
            n.volumes.push(volume(&format!("{v}.{bank}"), size));
        }
        n.pipes.push(pipe(
            &format!("snorkel.{bank}"),
            0.35,
            &[(0.0, 0.070)],
            300.0,
            End::Ambient {
                at: [1.0, y, 0.6],
                restriction: None,
            },
            vol(&format!("airbox.{bank}")),
        ));
        n.pipes.push(pipe(
            &format!("inlet.{bank}"),
            0.4,
            &[(0.0, 0.065)],
            310.0,
            vol(&format!("airbox.{bank}")),
            vol(&format!("compressor.in.{bank}")),
        ));
        n.pipes.push(PipeSpec {
            friction: 4.0,
            heat: 25.0,
            ..pipe(
                &format!("intercooler.{bank}"),
                0.5,
                &[(0.0, 0.07)],
                330.0,
                vol(&format!("compressor.out.{bank}")),
                vol("charge"),
            )
        });
        n.compressors.push(compressor(
            &format!("compressor.{bank}"),
            &format!("turbo.{bank}"),
            &format!("compressor.in.{bank}"),
            &format!("compressor.out.{bank}"),
            0.054,
            [0.2, 0.5 * y, 0.75],
        ));
    }
    n.orifices.push(Orifice {
        name: "diverter".into(),
        between: ("charge".into(), "compressor.in.A".into()),
        restriction: Restriction::BlowOff {
            diameter: 0.028,
            reference: "plenum".into(),
            opens: 0.3e5,
            span: 0.2e5,
        },
        at: [0.3, 0.0, 0.7],
    });
    for c in 1..=8 {
        n.pipes.push(pipe(
            &format!("runner.{c}"),
            0.22,
            &[(0.0, 0.042), (1.0, 0.038)],
            330.0,
            vol("plenum"),
            End::Terminal(format!("intake.{c}")),
        ));
    }
    n
}

/// The V8's exhaust: each bank's four primaries into its turbine, and a pipe of its own
/// through a catalyst and a silencer to its tailpipe.
pub fn v8_tt_exhaust() -> Network {
    let mut n = Network::default();
    for (bank, y) in [("A", 0.45), ("B", -0.45)] {
        for (v, size) in [
            ("collector", 0.4),
            ("turbine.out", 0.4),
            ("cat", 1.2),
            ("silencer", 5.0),
        ] {
            n.volumes.push(volume(&format!("{v}.{bank}"), size));
        }
        let v = |name: &str| vol(&format!("{name}.{bank}"));
        n.pipes.push(pipe(
            &format!("downpipe.{bank}"),
            0.7,
            &[(0.0, 0.070)],
            800.0,
            v("turbine.out"),
            v("cat"),
        ));
        n.pipes.push(pipe(
            &format!("centre.{bank}"),
            1.3,
            &[(0.0, 0.065)],
            600.0,
            v("cat"),
            v("silencer"),
        ));
        n.pipes.push(pipe(
            &format!("tail.{bank}"),
            0.4,
            &[(0.0, 0.075)],
            500.0,
            v("silencer"),
            End::Ambient {
                at: [-4.4, y, 0.35],
                restriction: None,
            },
        ));
        n.turbines.push(turbine(
            &format!("turbine.{bank}"),
            &format!("turbo.{bank}"),
            &format!("collector.{bank}"),
            &format!("turbine.out.{bank}"),
            6.5e-4,
            0.9e5,
        ));
    }
    for c in 1..=8 {
        let bank = if c <= 4 { "A" } else { "B" };
        n.pipes.push(pipe(
            &format!("primary.{c}"),
            0.40,
            &[(0.0, 0.036)],
            950.0,
            End::Terminal(format!("exhaust.{c}")),
            vol(&format!("collector.{bank}")),
        ));
    }
    n
}
