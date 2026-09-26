# Machines

Cars are built here the way real ones are: from parts designed on their own — a frame, an engine with its intake and exhaust, a transmission, suspensions, wheels with their tyres, brakes, steering, aero, a body, an interior, a fuel tank, electronics — assembled into a machine with its setup, and baked into the car package the game drives.

A library is a directory of plain-text RON files that refer to each other by name:

- `parts/<kind>/<name>.ron`: one part. Kinds: `frame`, `body`, `aero`, `engine`, `intake`, `exhaust`, `transmission`, `driveline`, `suspension`, `wheel`, `brakes`, `steering`, `interior`, `fuel_tank`, `electronics`.
- `machines/<name>.ron`: which parts a machine uses, where they attach, how its gas passages join, and its setup.

The working library is `<content>/machine-src/`. `machinectl init` writes the samples there (`assets/machine-src/`: a GT3-class machine with a flat-plane V8, and an inline four with a road and a straight exhaust to swap in).

`open-racing-machine-editor` edits libraries in 3D and runs engines live with their sound. `open-racing-machinectl` does everything from the command line. Both change files only through the same operations, applied all or nothing and checked.

```bash
machinectl init                                    # the samples into <content>/machine-src/
machinectl list [--json]                           # parts and machines
machinectl info engine/v8_4l_flatplane [--json]    # displacement, firing order, valve events…
machinectl info machine/gt3_v8 [--json]            # mass, CG, inertia, weight split, parts, warnings
machinectl get engine/i4_2l_na [path]              # the part as JSON: the paths Set takes
machinectl apply ops.ron                           # or ops.json, or - for stdin; --dry-run
machinectl copy engine/i4_2l_na engine/i4_2l_race  # a part or machine under a new name
machinectl check [target]                          # the whole library, or one part or machine
machinectl dyno engine/i4_2l_na [--rpm 1000:7000:250] [--exhaust exhaust/i4_straight] [--png d.png] [--json]
machinectl trace engine/i4_2l_na --rpm 6000 [--pipes primary.1] [--png t.png] [--json]
machinectl sound machine/gt3_v8 --preset sweep --out v8.wav [--mic exhaust,cabin] [--png orders.png] [--json]
machinectl preview machine/gt3_v8 --out gt3.png    # side and top views
machinectl bake machine/gt3_v8                     # <content>/cars/gt3_v8/, then: cargo dev -- --car gt3_v8
machinectl guide                                   # this text
```

(`machinectl` is `cargo machinectl`, short for `cargo run -p open-racing-machine-project --bin open-racing-machinectl --`.)

## Conventions

- **Units.** SI: m, kg, s, N, Pa, K, m³. Engine speeds are in rpm and crank and cam angles in degrees, in fields named `_rpm` and `_deg`. Tyre pressures in bar (gauge), as the game has them.
- **Frames.** x forward, y left, z up. A machine's origin is on the ground under the front axle's centre. A part's shapes and mounts are in its own frame.
- **Crank angles.** 0° is a cylinder's firing TDC and 360° its gas-exchange TDC. An intake cam's centreline is after that TDC, an exhaust cam's before it. Cylinders are numbered from 1.
- **Names.** Parts are `kind/name`, machines `machine/name`. Placed parts have instance names unique in their machine; gas terminals are `instance:terminal`.

## Parts

Every part has a `physical` side and a `design`:

```ron
(
    description: "…",
    physical: (
        shapes: [(name: "block", kind: Box(size: (0.6, 0.62, 0.45)), at: (0.0, 0.0, 0.25), mass: 140.0)],
        mounts: [(name: "bellhousing", at: (-0.34, 0.0, 0.16))],
    ),
    design: Engine((spec: (…), bench: (intake: Some("intake/v8_itb"), exhaust: Some("exhaust/v8_race")))),
)
```

- **Shapes** (`Box`, `Cylinder` along `X`/`Y`/`Z`, `Sphere`) stand in for the part's model, give its mass (uniform density; `mass: Given(…)` overrides the sum) and colour, and say what moves them in the game: `role: Wheel` (spins with a wheel), `Hub` (follows a wheel: uprights, discs, calipers), `Link` (half with the wheel: arms, driveshafts), `SteeringWheel`; the body otherwise. Unsprung masses come from these roles.
- **Mounts** are named frames where other parts attach. A `joint` (stiffness, strength) is kept for damage.
- **Designs** by kind:
  - `Engine`: the simulated engine (below), its cooling for the game, and the intake and exhaust it runs with on its own (`bench`).
  - `Intake`, `Exhaust`: a gas network (below); the intake's `throttle` (`DriveByWire`/`Cable`) for the game.
  - `Suspension`: one axle; `track`, the game's `linkage` hardpoints (the left wheel's, relative to its wheel centre), `actuation`, travels, bump stop rate. Its shapes are the left corner's, relative to the wheel centre; the right corner mirrors them.
  - `Wheel`: the game's tyre (`tire`) and the wheel's spin inertia. Shapes relative to the wheel centre.
  - `Transmission` (clutch, gearbox), `Driveline` (drive, differential), `Brakes`, `Steering`, `Electronics`: the game's parameters.
  - `Aero`: the game's elements, `position` as (forward, up) in the part's frame.
  - `Interior`: seat (hip point), eye, steering wheel centre and column axis (towards the driver).
  - `FuelTank`: capacity (l), density (kg/l). `Frame`, `Body`: structure.

Tyres and suspensions carry the game's own parameters until their workbenches simulate them.

### Engines

An `EngineSpec`: `bore`, `stroke`, `rod`, `compression_ratio`, `pin_offset`; `layout` (banks at angles, crank throws at angles, each cylinder on a bank and a throw, and the firing order — checked against the throws); `intake` and `exhaust` heads (valve count, diameter, stem, discharge coefficient against L/D; cam lift, duration, centreline, profile; the port's length, diameters and wall temperature); `crank` (flywheel and crank inertia, reciprocating and rotating masses per cylinder); `combustion` (fuel, Wiebe `a` and `m`, burn duration at a reference speed and its speed exponent, efficiency, cycle-to-cycle spread, octane); `heat_transfer` (Woschni scale and wall temperatures); `friction` (Chen–Flynn terms, Pa); `ecu` (idle, limiter and stall speeds, limiter hysteresis, idle control, overrun cut, and the `spark_deg` and `lambda` maps over rpm and load).

The simulator (`open-racing-engine-sim`) resolves every crank degree: one-dimensional unsteady gas dynamics in every pipe (finite volumes, MUSCL–Hancock with the HLLC Riemann solver, wall friction and heat transfer), volumes, valves and throttles as quasi-steady compressible restrictions on characteristic boundaries, and single-zone cylinders with Wiebe combustion, Woschni heat loss, a knock index, friction, and the crank's own dynamics. The pressure waves tune the breathing, and the flow leaving the tailpipes and entering the intake mouths is also the sound.

The engine exposes a terminal for each port: `intake.N`, `exhaust.N`.

### Networks (intakes and exhausts)

```ron
(
    volumes: [(name: "plenum", volume: 0.0025)],
    pipes: [
        (name: "runner.1", length: 0.30, diameter: [(0.0, 0.045), (1.0, 0.040)],
         wall_temperature: 330.0, a: Volume(name: "plenum"), b: Terminal("intake.1")),
        (name: "tailpipe", length: 0.5, diameter: [(0.0, 0.05)], a: Volume(name: "silencer"),
         b: Ambient(at: (-4.2, 0.4, 0.3))),
    ],
)
```

A pipe runs from end `a` to end `b`, with diameters along it (fraction of the length, m). An end is `Closed`, a `Volume(name, restriction)`, `Ambient(at, restriction)` (a mouth to the air, where the sound leaves; `at` is its position), `Terminal(name)` (joined to another part), or `Join(name)` (straight to the one other pipe end of this network with that joint name: a change of section such as a silencer's chamber). Restrictions are `Fixed(cd, diameter)` or a `Throttle(bore, shaft, closed_angle_deg)` worked by the pedal. `friction` multiplies a pipe's wall friction (a catalyst's brick). `orifices` join two volumes (or a volume and `"ambient"`) through a restriction.

A machine joins terminals in `gas: [("exhaust:exhaust.1", "engine:exhaust.1"), …]`; engine ports not listed join the one intake or exhaust terminal of the same name. An engine port left open opens to the air; a network terminal left open is closed.

## Machines

```ron
(
    parts: [
        (name: "tub", part: "frame/gt3_tub"),
        (name: "engine", part: "engine/v8_4l_flatplane", attach: Some((to: "tub", at: "engine", mount: Some("base")))),
        (name: "front suspension", part: "suspension/gt3_front", attach: Some((to: "tub", at: "front axle")), axle: Some(Front)),
        (name: "front wheels", part: "wheel/gt3_front", axle: Some(Front)),
        …
    ],
    setup: (front: (pressure: 1.38, spring_rate: 120000.0, bump_damping: 5500.0, rebound_damping: 8000.0,
                    anti_roll_rate: 60000.0, camber: -0.061, ride_height: 0.065), rear: (…),
            brake_bias: 0.6, wings: [("rear wing", 0.14)], fuel: 60.0, ballast: [((-0.6, 0.0, 0.1), 60.0)]),
    driver: (mass: 80.0),
)
```

A placed part attaches its own mount (its origin when left out) to a mount of another placed part, offset by `at` and `rotation_deg`; without `attach` it sits at `at` in the machine's frame. Suspensions and wheels name their `axle`: the wheels go to the suspension's wheel centres, both sides. The setup holds what is adjusted between runs: springs, dampers, bars, camber, toe, pressures, ride heights, brake balance, wing angles, gearing, differential, fuel and ballast.

## Operations

Every change is an operation (RON or JSON; one, or a list):

| operation | does |
| --- | --- |
| `Set(target, path, value)` | sets any field by its path in the target's JSON (see `machinectl get`) |
| `PutPart(part, value)` / `RemovePart(part)` | adds or replaces a whole part / removes an unused one |
| `RenamePart(part, to)` / `CopyPart(part, to)` | renames (updating machines and benches) / copies |
| `PutMachine(machine, value)` / `RemoveMachine` / `RenameMachine` / `CopyMachine` | the same for machines |
| `Place(machine, placed)` / `Unplace(machine, name)` | adds or replaces a placed part by instance name / removes it |
| `Connect(machine, a, b)` / `Disconnect(machine, terminal)` | joins two gas terminals / removes a terminal's joins |

Paths are keys and list indices separated by dots, with `[name]` for the element of a list whose `name` is that:

```ron
[
    Set(target: "engine/i4_2l_na", path: "design.Engine.spec.compression_ratio", value: 11.5),
    Set(target: "engine/i4_2l_na", path: "design.Engine.spec.intake.cam.duration_deg", value: 272.0),
    Set(target: "exhaust/i4_road", path: "design.Exhaust.network.pipes[primary.1].length", value: 0.75),
    Set(target: "machine/gt3_v8", path: "parts[exhaust].part", value: "exhaust/v8_merged"),
    Set(target: "machine/gt3_v8", path: "setup.rear.spring_rate", value: 150000.0),
]
```

The same in JSON: `[{"Set": {"target": "engine/i4_2l_na", "path": "design.Engine.spec.bore", "value": 0.087}}]`.

## Workflow

1. `machinectl info` and `get` to read; `apply` to change; `check` to validate.
2. Develop an engine on its bench: `dyno` (torque, power, BMEP, pumping and friction losses, volumetric efficiency, BSFC, peak pressure and its angle, knock index, residuals; `--png` for the sheet), `trace` (one cycle in detail), `sound` (a WAV and, with `--json`, its level, spectral peaks and engine orders along the run, so the sound can be read as numbers). Swap intakes and exhausts with `--intake` / `--exhaust`.
3. Assemble a machine; `info` gives its mass, CG, inertia, weight split and unsprung masses; `preview` draws it.
4. `bake` writes the car package: mass properties from the shapes, the engine's torque and drag curves from the dyno with its intake and exhaust, the setup, the aero referred to the CG, and a model of the shapes. It then drives the car on a straight and reports how it settles and accelerates.

Qualities (`--quality`): `draft` (24 kHz, ≈ 60 mm cells, first order: real time), `normal` (48 kHz: dyno and bake), `high` (96 kHz) and `ultra` (192 kHz) for recordings.
