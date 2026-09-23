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

# Evaluate
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

Gamepad: left stick to steer, RT/LT for throttle/brake, RB/LB to shift.

## Adding content

- Tracks: put a centreline control-point file (position, width, bank) in `assets/tracks/<name>.ron` and select it with `--track <name>`.
- Cars: add `assets/cars/<name>.ron` using `gt3.ron` as a template.

## Tests and benchmarks

```bash
cargo test --release            # physics plausibility, determinism, zero allocation, API contract
cargo bench -p open-racing-sim  # one physics step
cargo bench -p open-racing-env  # 256-env parallel throughput
```

Reference results (Apple M5, 10 cores): 1 physics step ≈ 0.4 µs; `BatchEnv` ≈ 9.8M physics steps/s (≈ 9,800× real time).
GT3 car: 0–100 km/h 3.2 s, top speed ≈ 274 km/h, 100–0 km/h ≈ 34 m, steady-state skidpad ≈ 1.43 g.

## Force-feedback wheels (planned)

`Controls` takes the steering wheel angle and pedal travel directly, and `Telemetry::steering_torque` provides the steering torque from the tyre aligning moments. A wheel device only needs one system in `crates/app/src/input.rs` that writes these values.

## Training reference

`train --envs 512 --iterations 400` (13M agent steps, ~12 min on an M5, wgpu): mean distance per episode rose from ~50 m to ~3–8 km, best lap 75.9 s on Lakeside (~4 km). This is still short; more training should make it more stable.
Training throughput is ~18k agent steps/s, and the bottleneck is the NN update, not the simulation.
