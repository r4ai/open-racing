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
            idle_authority: 0.06,
            idle_gain: 0.0004,
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
/// catalyst, a two-chamber silencer and a tailpipe.
pub fn i4_exhaust() -> Network {
    let mut n = Network {
        volumes: vec![
            volume("merge.14", 0.15),
            volume("merge.23", 0.15),
            volume("merge", 0.3),
            volume("cat.in", 0.8),
            volume("cat.out", 0.8),
            volume("silencer.1", 6.0),
            volume("silencer.2", 8.0),
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
        vol("silencer.1"),
    ));
    n.pipes.push(pipe(
        "silencer.pipe",
        0.25,
        &[(0.0, 0.045)],
        450.0,
        vol("silencer.1"),
        vol("silencer.2"),
    ));
    n.pipes.push(pipe(
        "tailpipe",
        0.50,
        &[(0.0, 0.050)],
        400.0,
        vol("silencer.2"),
        End::Ambient {
            at: [-4.2, 0.4, 0.3],
            restriction: None,
        },
    ));
    n
}
