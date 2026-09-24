# open-racing

A realistic racing simulator built as a platform for developing general-purpose racing AI.
The simulation core is headless pure Rust and runs massively in parallel. Visualisation (Bevy) and training (Burn) are separate layers.

## Architecture (onion)

```
adapters   app (Bevy) · train-burn (Burn) · [future] py (PyO3)
ports      api        ← adapters depend only on this crate (plain &[f32] / u8 data)
app layer  env        episodes, observations, rewards, termination, parallel BatchEnv
domain     sim        vehicle dynamics and tracks (deterministic, zero allocation in step)
```

| crate               | role                                                                                                                                                                                         |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/sim`        | 6-DOF chassis, 4-wheel suspension with unsprung masses, Pacejka tyres (combined slip, relaxation length, load sensitivity, camber, tread temperature and wear), engine / clutch / sequential gearbox, front / rear / all-wheel drive through limited-slip differentials, aero, tracks |
| `crates/env`        | RL environment. The observation uses only quantities that other sims (AC / ACC / iRacing, etc.) also expose as telemetry; ground-truth tyre state can be added via `privileged_obs`          |
| `crates/api`        | `VecEnv` / `Policy` traits, asset loading, `AgentDriver` (lets a policy drive a car simulated elsewhere)                                                                                     |
| `crates/track`      | Track package format: centreline, road meshes and walls, render data                                                                                                                         |
| `crates/car`        | Car package format: physics, tyres, and a 3D model whose parts move with the simulated car                                                                                                    |
| `crates/ac`         | Converter from track and car folders in the Assetto Corsa format to track and car packages                                                                                                    |
| `crates/train-burn` | PPO (GAE, clipping, observation normalisation) and `BurnPolicy`                                                                                                                              |
| `crates/app`        | Driving, AI spectating, replay, HUD                                                                                                                                                          |

Physics runs at a fixed 1 kHz. Everything is in SI units, and steering input is the steering wheel angle in radians, the same physical quantity a real wheel reports.

## Usage

```bash
# Drive with the keyboard (H shows the controls)
cargo run --release -p open-racing-app

# Train (defaults to the wgpu backend: Metal / DX12 / Vulkan)
cargo run --release -p open-racing-train-burn -- train --envs 512 --iterations 400 --out runs/lakeside

# Continue training from a trained policy (--gamma sets the discount, i.e. the planning horizon)
cargo run --release -p open-racing-train-burn -- train --init runs/lakeside --lr 1e-4 --out runs/lakeside-2

# Evaluate: one car from the start line, then 64 cars from random points (crash rate and where they crash)
cargo run --release -p open-racing-train-burn -- eval --model runs/lakeside

# Watch the AI in the app (T switches between AI and human)
cargo run --release -p open-racing-app -- --ai runs/lakeside
```

Use `--features ndarray` or `--features flex` for a CPU backend.

### Track evolution

Rubber builds up on the racing line as cars drive, as in other sims. The road is a grid of patches in track coordinates (4 m along × 0.5 m across), each with its own rubber level:

- **Rubber:** grip runs from 90 % on dusty asphalt to 100 % where the line is fully rubbered in. Tyres lay rubber in proportion to their frictional work (the square of how much of their grip they use), so braking zones, apexes and corner exits rubber in fastest and straights least. Starting conditions put rubber on a racing line estimated from the track (the minimum-curvature line, driven at an estimated speed); off the line the asphalt stays at 94 %.
- **Dirt:** tyres rolling on grass, earth or gravel pick up a coat (grass clippings, earth, grit) that costs them up to 20–30 % grip. On paved ground they shed it within about 100 m (faster when sliding; grit flies off quickest), and what lands on the asphalt costs grip until tyres rolling over it sweep it away a few dozen metres later. The HUD shows the grip under the car and each tyre's coat.
- **Levels:** `dusty` (90 %), `green` (94 %), `fast` (97 %), `optimum` (100 %), or any number such as `0.96`, is the grip where the line is worked hardest. Kerbs, run-off and grass keep their own grip.

```bash
# Drive on a green track that rubbers in by 0.2 % per lap (also on the Esc settings, page "track")
cargo run --release -p open-racing-app -- --track-grip green --grip-gain 0.002

# Train over randomised track conditions (the policy sees them only through the car's behaviour)
cargo run --release -p open-racing-train-burn -- train --track-grip green..optimum --out runs/evolving

# Evaluate on a given condition (defaults to what the policy was trained with)
cargo run --release -p open-racing-train-burn -- eval --model runs/evolving --track-grip dusty
```

Without `--track-grip` training keeps the whole asphalt at the tyres' nominal grip, as before. The laid rubber is off in training by default (`--grip-gain`), since one car adds little within an episode; tyre dirt from going off track always applies. The app shows the coat on the tyres; the rubber and the dirt on the road are not drawn.

### Controls

| key       | action                                                     |
| --------- | ---------------------------------------------------------- |
| W/S, ↑/↓  | throttle / brake                                           |
| A/D, ←/→  | steering                                                   |
| E/Q       | shift up / down (`--auto-shift` for automatic)             |
| C         | clutch                                                     |
| I         | restart the engine after a stall                           |
| Backspace | put the car back on the track                              |
| V         | switch camera (chase / cockpit / TV / top)                 |
| P         | replay since the last reset                                |
| T         | switch to the AI driver                                    |
| M         | mute / unmute sound                                        |
| Tab       | choose the input device                                    |
| Esc       | settings: input devices, force feedback, track condition   |

Gamepad: left stick to steer, RT/LT for throttle/brake, RB/LB to shift, Select to choose the input device.

With several devices connected, Tab cycles auto → keyboard → each connected gamepad / wheel → custom. In auto, whichever device was used last drives; otherwise only the chosen device is read. Steering wheels (recognised by name, e.g. G29 or T300, or by vendor: MOZA, Fanatec) steer 1:1 over a 900° rotation range.

For a wheel and pedals, open the settings with Esc (the simulation pauses), pick an action and press Enter, then move the control: turn the wheel fully right and back to centre, or press a pedal fully and release it, then press Enter again; shift buttons are assigned by pressing them. Steering, pedals and shifters may come from different devices, and the calibration handles inverted or offset axes. Set the rotation to match the wheel's own setting. Assignments are saved to `%APPDATA%/open-racing/input.ron` (`~/.config/open-racing/input.ron` elsewhere) and select the "custom" input.

Force feedback (Windows, DirectInput) plays the simulated steering torque on the wheel that steers, with the driver's centring spring switched off. The torque is taken about each front wheel's steering axis from everything acting on its contact patch: the tyre's aligning moment, the lateral force on the mechanical trail, the longitudinal force on the scrub radius and the load on the caster and kingpin inclination, so the force goes light as the fronts lose grip and bumps, undulations and kerb ridges kick through the rim. Its settings are on the second page of the settings screen (Esc, then Tab) and are saved to `ffb.ron` next to `input.ron`: on/off, the base's peak torque in N·m (e.g. 12 for a MOZA R12, with the wheel software's own FFB gain at 100 %), strength as a share of the car's steering torque (100 % reproduces it 1:1 at the rim; a GT3 car peaks around 20 N·m), max output in N·m, road detail (the share of fast changes such as bumps and kerbs reproduced, 100 % as simulated), damping and direction. Keep the torque below the max output: a clipped force is flat and carries no detail. Wheels differ in which way they push, so hold the wheel and run the test first: it must turn the wheel left, otherwise flip the direction. Turning the wheel past the car's steering lock (270° each way for the GT3) pauses the force until it comes back; being driven into lock three times within seconds (an inverted direction) stops it until the settings screen is opened. The HUD shows either. If the wheel itself stops the effect (e.g. MOZA's hands-off protection), it is restarted.

## Adding content

- Tracks: put a centreline control-point file (position, width, bank) in `assets/tracks/<name>.ron` and select it with `--track <name>`.
- Cars: add `assets/cars/<name>.ron` using `gt3.ron` as a template, or convert a car into a car package (below). Each axle names its tyre (`tire: "<name>"` for `assets/tires/<name>.ron`, or a `.ron` path relative to the car file) and sets the cold pressure in bar, as teams set it in the garage.
  `drive` picks the driven wheels: `Rear` (the default), `Front`, or `All(front_share: …, centre_differential: (…), front_differential: (…))`, where a centre differential splits the torque between the axles; `differential` is the driven axle's, the rear one's for all-wheel drive. Keep the front and rear tyres the same size on an all-wheel-drive car with a locking centre differential: like a real one, it fights a difference in wheel speed.
  Where the engine sits shows in `front_weight` and in `inertia`: masses near the centre lower the pitch and yaw inertia. With the same parts, a mid-engined car puts more power down, turns in quicker, understeers less and rotates more when the driver lifts mid-corner than a front-engined one, and a nose-heavy front-drive car understeers most (tests in `crates/sim/tests/physics.rs`).
- Bundled cars (`--car <name>`), all fictional and representative of their class: `gt3` (mid-engined GT3 racer on slicks, the default), `hot_hatch` (front-wheel drive, 61 % front), `awd_sedan` (all-wheel drive, 40 % of the torque to the front), `fr_coupe` and `mr_coupe` (the same 3.0 l coupé with the engine in front, 52 % front, and in the middle, 42 % front). The road cars run on the Velloni Strada R road tyre.
- Tyres: add `assets/tires/<name>.ron` using `velloni_zeta_gt_front.ron` as a template. A tyre file holds the size, the force curves, how pressure changes stiffness, peak slip, rolling resistance and grip, and the thermal model (inner / middle / outer tread zones and carcass, cooling, operating window, wear). Hot pressure follows the carcass temperature (gas law), so the cold pressure, camber and driving style show up in the tread temperatures on the HUD as they would on a real car. The bundled GT3 runs on the fictional Velloni Zeta GT, modelled on public figures for GT3 slicks (30/68-18 front, 31/71-18 rear).

### Track packages

Tracks with 3D models are loaded from open-racing's own package format: a directory `content/tracks/<name>/` holding

- `track.ron`: format version, centreline and surface types (grip, rolling resistance),
- `ground.bin`: road meshes the tyres ride on, and walls,
- `visual.bin`: meshes, materials and textures, read only by the app.

Select one with `--track <name>`. `content/` is git-ignored; set `OPEN_RACING_CONTENT` to use a different directory. The physics data is a few MB, so training and evaluation start quickly and never read the render data.

Packages are made ahead of time by converters from other formats. The runtime reads packages only.

### Converting tracks

No third-party tracks ship with open-racing. Only convert and use tracks whose licence allows it.

**Assetto Corsa format** (`open-racing-ac`): reads KN5 models, `ai/fast_lane.ai` and `data/surfaces.ini` from a track folder anywhere on disk. The folder itself is not modified.

```bash
# Writes content/tracks/my_track/ and prints how well the centreline fits the road meshes
cargo run --release -p open-racing-ac -- path/to/<track folder> --layout <layout> --name my_track
cargo run --release -p open-racing-app -- --track my_track
cargo run --release -p open-racing-train-burn -- train --track my_track
```

`--layout` picks a layout of a multi-layout folder (the `<layout>` in `models_<layout>.ini`); leave it out for a single-layout folder. The name defaults to the folder name.

How the folder is converted:

- **Tyres:** a mesh is physical when its name starts with digits followed by a surface key from `surfaces.ini` (e.g. `1ROAD_05`). Grip is `FRICTION` relative to the grippiest valid-track surface. The kind of surface comes from its key (`KERB`/`CURB`/`RUMBLE`, `SAND`/`GRAVEL`, `DIRT`/`MUD`, `CARPET`/`TURF`, `GRASS`/`GRS`), else its sound (`kerb.wav`, `sand.wav`, `grass.wav`, `extraturf.wav`), else paved (valid track: asphalt, otherwise run-off) or grass when `DIRT_ADDITIVE` is high; `DIRT_ADDITIVE` also sets how readily tyres pick up dirt there. Anything but asphalt and kerbs counts as off track in rewards. Packages converted before these surface kinds existed treat everything off track as grass; convert them again to get gravel, earth, turf and run-off. Meshes named `<digits>WALL…` are solid walls.
- **Centreline:** the AI line only defines the centreline and the track widths, which give progress, observations and lap timing. It is not used as a driving line. The start/finish line is at the timing markers.
- **Materials:** diffuse textures, normal maps (`txNormal`) and the mask and detail layers of multi-layer materials carry over. The game's highlights (`ksSpecular`, `ksSpecularEXP`) and reflections (`fresnelMaxLevel`) become a PBR roughness and reflectance, per texel where `txMaps` varies them. Surfaces reflect the sky only where the game reflects its surroundings.

Limitations:

- Point-to-point tracks (open AI line) are not supported.
- Encrypted or otherwise protected KN5 files are rejected.
- Detail textures other than the multi-layer ones (`txDetail`, `txNormalDetail`, `txDetailNM`), object-space normal maps and emissive surfaces are not converted.
- Model rotations in `models_*.ini` are ignored.
- Custom Shaders Patch extensions (`extension/`: generated trees, lights, mesh adjustments) are not applied, so a track looks as it does without the patch.

### Car packages

Cars with 3D models are loaded from open-racing's car package format: a directory `content/cars/<name>/` holding

- `car.ron`: the physics, in the same format as `assets/cars/*.ron`, naming its tyres,
- `front_tire.ron`, `rear_tire.ron`: the tyres, in the format of `assets/tires/*.ron`,
- `visual.ron`: format version, and the part of the car each mesh belongs to (body, a wheel, a wheel's hub, the steering wheel), which says how it moves; plus the steering wheel's axis and the driver's eye point,
- `visual.bin`: meshes, materials and textures, in the same encoding as a track package's render data, read only by the app.

Select one with `--car <name>`. Training and evaluation read only `car.ron` and the tyres. The files are plain RON, so the physics of a converted car can be tuned by hand.

### Converting cars

No third-party cars ship with open-racing. Only convert and use cars whose licence allows it.

**Assetto Corsa format** (`open-racing-ac`, the same command as for tracks; a folder with `ui/ui_car.json`, `data.acd` or `data/car.ini` is taken as a car):

```bash
# Writes content/cars/my_car/ and prints the car's figures and a quick acceleration test
cargo run --release -p open-racing-ac -- path/to/<car folder> --name my_car
cargo run --release -p open-racing-app -- --car my_car
cargo run --release -p open-racing-train-burn -- train --car my_car
```

Options: `--skin <skin>` picks a livery from `skins/` (default: the first), `--data <dir>` points to a folder of physics files, `--base <car.ron>` sets the car that stands in for values the folder does not give (default: the bundled GT3), and `--max-texture <texels>` caps the texture size (default 4096; larger textures keep their smaller mip levels).

How the folder is converted:

- **Model:** `<folder>.kn5` (or the largest KN5 file), with the chosen skin's textures replacing those of the same name. Parts move by node name, as in the game: `WHEEL_LF`/`RF`/`LR`/`RR` spin, steer and follow the suspension; `SUSP_<corner>`, and whatever shares a parent node with a single wheel, steer and follow the suspension without spinning; `STEER_HR` turns with the steering input. Low-detail copies (`COCKPIT_LR`, `STEER_LR`), motion-blurred wheels, damaged parts and unfastened belts are left out. Materials are converted as for tracks.
- **Geometry:** wheelbase, tracks and tyre sizes come from the wheels in the model, so the model and the simulated wheels line up.
- **Physics:** read from the physics files (`car.ini`, `suspensions.ini`, `tyres.ini`, `engine.ini`, `drivetrain.ini`, `brakes.ini`, `aero.ini` and the tables they name) in the folder's `data/` or in `--data`. Physics packed into `data.acd` are **not** read: open-racing does not unpack them. Without physics files, the mass, the engine's torque curve and rev limit, the driven wheels (the `fwd`, `rwd`, `awd` or `4wd` tag) and the final drive (for the stated top speed) come from `ui/ui_car.json`, and everything else from the base car. The converter lists where each group of values came from.
- **Drive:** `[TRACTION] TYPE` sets the driven wheels. An `AWD` car takes its torque split and front, centre and rear differentials from `[AWD]`. The newer `AWD2` model is simulated as all-wheel drive through a limited-slip centre differential.

Limitations:

- The game's electronic and viscous centre couplings (`AWD2`) and active differentials become limited-slip differentials.
- The game's tyre model, aero maps (ride height sensitivity, per-wing damage) and turbo dynamics are reduced to open-racing's parameters: peak grip and slip, load sensitivity, pressures, operating temperature; drag and downforce areas; the torque curve at full boost.
- Skinned meshes (the driver, animated belts), animations (doors, wipers), lights and Custom Shaders Patch extensions are not converted.

## Tests and benchmarks

```bash
cargo test                      # physics plausibility, determinism, zero allocation, API contract
cargo bench -p open-racing-sim  # one physics step
cargo bench -p open-racing-env  # 256-env parallel throughput
# A lap on every track package in content/tracks/ (ignored by default)
cargo test -p open-racing-api --test content_tracks -- --ignored
# A drive in every car package in content/cars/ (ignored by default)
cargo test -p open-racing-api --test content_cars -- --ignored
```

Reference results (Apple M5, 10 cores): 1 physics step ≈ 0.4 µs; `BatchEnv` ≈ 9.8M physics steps/s (≈ 9,800× real time).
GT3 car: 0–100 km/h 3.2 s, top speed ≈ 274 km/h, 100–0 km/h ≈ 34 m, steady-state skidpad ≈ 1.43 g.
Road cars, launched with the throttle eased off as the driven wheels spin (`crates/sim/tests/physics.rs`): hot hatch 0–100 km/h 5.8 s, skidpad ≈ 0.96 g; AWD sedan 4.9 s, ≈ 0.92 g; FR coupé 4.8 s, MR coupé 4.4 s.

## Development

For the app, iterate in the dev profile with Bevy linked as a DLL. The dev profile builds the workspace with `opt-level = 1` and dependencies with `opt-level = 3` (without debug info), so a physics step is as fast as in `--release`:

```bash
cargo dev                       # = cargo run -p open-racing-app --features dev
cargo dev -- --ai runs/lakeside
```

A rebuild after editing the app takes about 5 s this way (Windows, 16 threads), against 13–18 s for `--release`. `cargo test` without `--release` is enough for the same reason. Keep `--release` for training, evaluation and benchmarks. Don't combine the `dev` feature with `--release` or distribute its binary: it needs the Bevy DLL from `target/`.

## Training reference

`train --envs 512 --iterations 400` (13M agent steps, ~12 min on an M5, wgpu): mean distance per episode rose from ~50 m to ~3–8 km, best lap 75.9 s on Lakeside (~4 km). This is still short; more training should make it more stable.
Training throughput is ~18k agent steps/s, and the bottleneck is the NN update, not the simulation.

Tsukuba (2.08 km) on an RX 9070 (wgpu, ~90k agent steps/s at 50 Hz, ~57k at 25 Hz with 2048 envs), in three stages of about 16, 20 and 65 minutes:

```bash
T="train --track tsukuba --envs 2048 --minibatch 16384 --control-hz 25 --edge-obs --gamma 0.995 --crash-penalty 50 --steer-change-penalty 0.5"
cargo run --release -p open-racing-train-burn -- $T --iterations 400 --entropy 0.002 --final-std 0.15 --out runs/tsukuba-1
cargo run --release -p open-racing-train-burn -- $T --iterations 640 --lr 1.5e-4 --entropy 0 --init-std 0.15 --final-std 0.05 \
  --off-track-penalty 0.06 --init runs/tsukuba-1 --out runs/tsukuba-2
cargo run --release -p open-racing-train-burn -- $T --iterations 1800 --abs --lr 1.5e-4 --entropy 0 --init-std 0.11 --final-std 0.05 \
  --off-track-penalty 0.06 --init runs/tsukuba-2 --out runs/tsukuba-3
```

(The second stage was stopped at iteration 640 of a 2300-iteration schedule.) The result laps in 55.2 s on the second lap from a standing start (best 54.8 s), with one crash in 141 laps from random starts. What mattered, from single-seed runs of 400 iterations:

- `--edge-obs`: without the track widths ahead the policy cannot tell where the road ends, and did not get past Tsukuba's first hairpin; with them it lapped in 57.9 s after 10 minutes.
- `--control-hz 25`: half as many decisions per second halves the steering jitter the exploration noise adds, and the same discount looks twice as far ahead. The policy then used the apexes (0.7–1.9 m from the inside edge instead of 3.3–5 m).
- `--final-std`: with actions clipped to their range, a policy trained with large noise behaves differently when run deterministically; several runs lapped cleanly with noise and crashed without it. Shrinking the noise over training removed that, and with `--steer-change-penalty` the steering stopped sawing between full left and full right every step.
- `--abs`: without it the GT3 locks its wheels at 80 % pedal, so policies braked at about half pedal and still overheated the front tyre that went light over bumps.
- Hitting a wall ends the episode; before, policies braked for the hairpin by bouncing off the outside wall.

## License

open-racing's source code and the files in `assets/` are licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

### Distributing binaries

Built executables contain third-party crates under their own licences, whose notices must accompany the binaries. Most are MIT or Apache-2.0; others include BSD, ISC, Zlib, Unicode-3.0 and MPL-2.0 (`colored` and `option-ext`, pulled in by Burn). For the MPL-2.0 crates, tell recipients where their source is available (crates.io). The app also embeds Bevy's default font, FiraMono, under the SIL Open Font License 1.1, which Cargo's licence metadata does not show.

Generate the notices with [cargo-about](https://github.com/EmbarkStudios/cargo-about), using `about.toml` and `about.hbs` in this repository, and ship the result with the binaries:

```bash
cargo about generate about.hbs -o THIRD-PARTY-LICENSES.html
```
