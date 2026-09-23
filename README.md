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
| `crates/sim`        | 6-DOF chassis, 4-wheel suspension with unsprung masses, Pacejka tyres (combined slip, relaxation length, load sensitivity, camber, tread temperature and wear), engine / clutch / sequential gearbox / LSD, aero, tracks |
| `crates/env`        | RL environment. The observation uses only quantities that other sims (AC / ACC / iRacing, etc.) also expose as telemetry; ground-truth tyre state can be added via `privileged_obs`          |
| `crates/api`        | `VecEnv` / `Policy` traits, asset loading, `AgentDriver` (lets a policy drive a car simulated elsewhere)                                                                                     |
| `crates/track`      | Track package format: centreline, road meshes and walls, render data                                                                                                                         |
| `crates/ac`         | Converter from track folders in the Assetto Corsa format to track packages                                                                                                                   |
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
| Esc       | input settings (assign steering, pedals and shift buttons) |

Gamepad: left stick to steer, RT/LT for throttle/brake, RB/LB to shift, Select to choose the input device.

With several devices connected, Tab cycles auto → keyboard → each connected gamepad / wheel → custom. In auto, whichever device was used last drives; otherwise only the chosen device is read. Steering wheels (recognised by name, e.g. G29 or T300, or by vendor: MOZA, Fanatec) steer 1:1 over a 900° rotation range.

For a wheel and pedals, open the settings with Esc (the simulation pauses), pick an action and press Enter, then move the control: turn the wheel fully right and back to centre, or press a pedal fully and release it, then press Enter again; shift buttons are assigned by pressing them. Steering, pedals and shifters may come from different devices, and the calibration handles inverted or offset axes. Set the rotation to match the wheel's own setting. Assignments are saved to `%APPDATA%/open-racing/input.ron` (`~/.config/open-racing/input.ron` elsewhere) and select the "custom" input.

Force feedback (Windows, DirectInput) plays the simulated steering torque from the front tyres' aligning moments on the wheel that steers, with the driver's centring spring switched off; the force goes light as the fronts lose grip. Its settings are on the second page of the settings screen (Esc, then Tab) and are saved to `ffb.ron` next to `input.ron`: on/off, the base's peak torque in N·m (e.g. 12 for a MOZA R12, with the wheel software's own FFB gain at 100 %), strength as a share of the car's steering torque (100 % reproduces it 1:1 at the rim; a GT3 car peaks around 12 N·m), max output in N·m, damping and direction. Wheels differ in which way they push, so hold the wheel and run the test first: it must turn the wheel left, otherwise flip the direction. Turning the wheel past the car's steering lock (270° each way for the GT3) pauses the force until it comes back; being driven into lock three times within seconds (an inverted direction) stops it until the settings screen is opened. The HUD shows either. If the wheel itself stops the effect (e.g. MOZA's hands-off protection), it is restarted.

## Adding content

- Tracks: put a centreline control-point file (position, width, bank) in `assets/tracks/<name>.ron` and select it with `--track <name>`.
- Cars: add `assets/cars/<name>.ron` using `gt3.ron` as a template. Each axle names its tyre (`tire: "<name>"` for `assets/tires/<name>.ron`, or a `.ron` path relative to the car file) and sets the cold pressure in bar, as teams set it in the garage.
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

- **Tyres:** a mesh is physical when its name starts with digits followed by a surface key from `surfaces.ini` (e.g. `1ROAD_05`). Grip is `FRICTION` relative to the grippiest valid-track surface, and surfaces that are not valid track count as off track in rewards. Meshes named `<digits>WALL…` are solid walls.
- **Centreline:** the AI line only defines the centreline and the track widths, which give progress, observations and lap timing. It is not used as a driving line. The start/finish line is at the timing markers.
- **Materials:** diffuse textures, normal maps (`txNormal`) and the mask and detail layers of multi-layer materials carry over. The game's highlights (`ksSpecular`, `ksSpecularEXP`) and reflections (`fresnelMaxLevel`) become a PBR roughness and reflectance, per texel where `txMaps` varies them. Surfaces reflect the sky only where the game reflects its surroundings.

Limitations:

- Point-to-point tracks (open AI line) are not supported.
- Encrypted or otherwise protected KN5 files are rejected.
- Detail textures other than the multi-layer ones (`txDetail`, `txNormalDetail`, `txDetailNM`), object-space normal maps and emissive surfaces are not converted.
- Model rotations in `models_*.ini` are ignored.
- Custom Shaders Patch extensions (`extension/`: generated trees, lights, mesh adjustments) are not applied, so a track looks as it does without the patch.

## Tests and benchmarks

```bash
cargo test                      # physics plausibility, determinism, zero allocation, API contract
cargo bench -p open-racing-sim  # one physics step
cargo bench -p open-racing-env  # 256-env parallel throughput
# A lap on every track package in content/tracks/ (ignored by default)
cargo test -p open-racing-api --test content_tracks -- --ignored
```

Reference results (Apple M5, 10 cores): 1 physics step ≈ 0.4 µs; `BatchEnv` ≈ 9.8M physics steps/s (≈ 9,800× real time).
GT3 car: 0–100 km/h 3.2 s, top speed ≈ 274 km/h, 100–0 km/h ≈ 34 m, steady-state skidpad ≈ 1.43 g.

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
