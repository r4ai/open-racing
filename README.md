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

| crate | role |
|---|---|
| `crates/sim` | 6-DOF chassis, 4-wheel suspension with unsprung masses, Pacejka tyres (combined slip, relaxation length, load sensitivity, camber), engine / clutch / sequential gearbox / LSD, aero, tracks |
| `crates/env` | RL environment. The observation uses only quantities that other sims (AC / ACC / iRacing, etc.) also expose as telemetry; ground-truth tyre state can be added via `privileged_obs` |
| `crates/api` | `VecEnv` / `Policy` traits, asset loading, `AgentDriver` (lets a policy drive a car simulated elsewhere) |
| `crates/train-burn` | PPO (GAE, clipping, observation normalisation) and `BurnPolicy` |
| `crates/app` | Driving, AI spectating, replay, HUD |

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

| key | action |
|---|---|
| W/S, ↑/↓ | throttle / brake |
| A/D, ←/→ | steering |
| E/Q | shift up / down (`--auto-shift` for automatic) |
| C | clutch |
| I | restart the engine after a stall |
| Backspace | put the car back on the track |
| V | switch camera (chase / cockpit / TV / top) |
| P | replay since the last reset |
| T | switch to the AI driver |
| M | mute / unmute sound |
| Tab | choose the input device |
| Esc | input settings (assign steering, pedals and shift buttons) |

Gamepad: left stick to steer, RT/LT for throttle/brake, RB/LB to shift, Select to choose the input device.

With several devices connected, Tab cycles auto → keyboard → each connected gamepad / wheel → custom. In auto, whichever device was used last drives; otherwise only the chosen device is read. Steering wheels (recognised by name, e.g. G29 or T300, or by vendor: MOZA, Fanatec) steer 1:1 over a 900° rotation range.

For a wheel and pedals, open the settings with Esc (the simulation pauses), pick an action and press Enter, then move the control: turn the wheel fully right and back to centre, or press a pedal fully and release it, then press Enter again; shift buttons are assigned by pressing them. Steering, pedals and shifters may come from different devices, and the calibration handles inverted or offset axes. Set the rotation to match the wheel's own setting. Assignments are saved to `%APPDATA%/open-racing/input.ron` (`~/.config/open-racing/input.ron` elsewhere) and select the "custom" input.

Force feedback (Windows, DirectInput) plays the simulated steering torque from the front tyres' aligning moments on the wheel that steers, with the driver's centring spring switched off; the force goes light as the fronts lose grip. Its settings are on the second page of the settings screen (Esc, then Tab) and are saved to `ffb.ron` next to `input.ron`: on/off, the base's peak torque in N·m (e.g. 12 for a MOZA R12, with the wheel software's own FFB gain at 100 %), strength as a share of the car's steering torque (100 % reproduces it 1:1 at the rim; a GT3 car peaks around 12 N·m), max output in N·m, damping and direction. Wheels differ in which way they push, so hold the wheel and run the test first: it must turn the wheel left, otherwise flip the direction. Turning the wheel past the car's steering lock (270° each way for the GT3) pauses the force until it comes back; being driven into lock three times within seconds (an inverted direction) stops it until the settings screen is opened. The HUD shows either. If the wheel itself stops the effect (e.g. MOZA's hands-off protection), it is restarted.

## Adding content

- Tracks: put a centreline control-point file (position, width, bank) in `assets/tracks/<name>.ron` and select it with `--track <name>`.
- Cars: add `assets/cars/<name>.ron` using `gt3.ron` as a template.

## Tests and benchmarks

```bash
cargo test                      # physics plausibility, determinism, zero allocation, API contract
cargo bench -p open-racing-sim  # one physics step
cargo bench -p open-racing-env  # 256-env parallel throughput
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

## Force-feedback wheels (planned)

`Controls` takes the steering wheel angle and pedal travel directly, and `Telemetry::steering_torque` provides the steering torque from the tyre aligning moments. A wheel device only needs one system in `crates/app/src/input.rs` that writes these values.

## Training reference

`train --envs 512 --iterations 400` (13M agent steps, ~12 min on an M5, wgpu): mean distance per episode rose from ~50 m to ~3–8 km, best lap 75.9 s on Lakeside (~4 km). This is still short; more training should make it more stable.
Training throughput is ~18k agent steps/s, and the bottleneck is the NN update, not the simulation.
