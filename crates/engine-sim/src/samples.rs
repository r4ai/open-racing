//! Reference engines and systems: a naturally aspirated 2.0 l inline four with a
//! plenum intake and a 4-2-1 exhaust, and a 4.0 l flat-plane V8 with individual throttle
//! bodies. They are starting points for new designs, the machine tool's samples, and what
//! the tests check the simulator against.

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
            overrun_cut_rpm: Some(1500.0),
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

fn pipe(name: &str, length: f64, d: &[(f64, f64)], wall: f64, a: End, b: End) -> PipeSpec {
    PipeSpec {
        name: name.into(),
        length,
        diameter: d.to_vec(),
        wall_temperature: wall,
        roughness: 1e-4,
        friction: 1.0,
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
            idle_rpm: 1200.0,
            limiter_rpm: 9000.0,
            stall_rpm: 500.0,
            limiter_hysteresis_rpm: 200.0,
            idle_opening: 0.01,
            idle_authority: 0.04,
            idle_gain: 0.00005,
            overrun_cut_rpm: Some(2000.0),
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
