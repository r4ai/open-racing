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
| `crates/sim`        | 6-DOF chassis, 4-wheel suspension with unsprung masses, Pacejka tyres (combined slip, relaxation length, load sensitivity, sliding speed, camber and camber gain, tread temperature and wear), a mean-value engine (throttle, intake manifold, turbocharger), brakes whose friction follows their temperature, the engine's cylinder, coolant and oil temperatures with part wear and failures, clutch / H-pattern, sequential and dual-clutch gearboxes with their electronics, front / rear / all-wheel drive through limited-slip differentials, aero elements sensitive to ride height, pitch, yaw and damage, tracks |
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

### Weather

The app simulates the weather with the car, so a drive replays exactly. Rain is not modelled yet.

- **Sky:** clear, fair, partly cloudy, cloudy or overcast, set on the Esc settings (page "weather") or with `--weather`. With changes on, the sky drifts between neighbouring states every hour or so of weather time, and time can run up to 120× faster than real time.
- **Clouds** come in four layers, as a meteorologist reports them: cumulus from daytime convection (none at night, building through the day, their flat bases at the condensation level, 125 m per kelvin of dew-point spread), a stratus or stratocumulus deck, an altostratus or altocumulus sheet at about 3 km, and cirrus at about 9 km. Each drifts with its own wind, veering and strengthening with height. They are drawn as ray-marched volumetric clouds in the manner of *Horizon Zero Dawn* (Schneider) and *Frostbite* (Hillaire), with a physical atmosphere, and the cloud shadows sweep over the track.
- **Air:** the temperature follows the month's climate and a daily cycle damped by cloud, and falls with height; humidity and pressure follow the sky. The air's density (temperature, pressure, humidity) scales drag, downforce and engine power (SAE J1349), and the wind, with gusts, adds to or takes from the airspeed.
- **Road temperature:** each 8 m patch of the road, in three lanes, balances sunshine on its slope, diffuse daylight from the part of the sky it sees, long-wave radiation to and from the sky, convection and conduction into the ground. On tracks with a 3D model the scenery shades the road (rays cast once per half hour of the sun's path), so a stretch under a grandstand stays cool while open asphalt gets 20–30 °C above the air on a summer afternoon. Tyres are heated or cooled by the road under them and by the air around them.

```bash
# A partly cloudy afternoon
cargo run --release -p open-racing-app -- --weather partly-cloudy --time 15:30
```

Training (`open-racing-env`) keeps fixed standard conditions (25 °C air and road, 1.225 kg/m³, no wind), as before, and runs as fast as before.

### Controls

| key       | action                                                     |
| --------- | ---------------------------------------------------------- |
| W/S, ↑/↓  | throttle / brake                                           |
| A/D, ←/→  | steering                                                   |
| E/Q       | shift up / down                                            |
| C         | clutch                                                     |
| I         | restart the engine after a stall                           |
| Backspace | put the car back on the track                              |
| V         | switch camera (chase / cockpit / TV / top)                 |
| P         | replay since the last reset                                |
| T         | switch to the AI driver                                    |
| M         | mute / unmute sound                                        |
| Tab       | choose the input device                                    |
| Esc       | settings: input, force feedback, track, weather, graphics, assists, realism |

Gamepad: left stick to steer, RT/LT for throttle/brake, RB/LB to shift, Select to choose the input device.

With several devices connected, Tab cycles auto → keyboard → each connected gamepad / wheel → custom. In auto, whichever device was used last drives; otherwise only the chosen device is read. Steering wheels (recognised by name, e.g. G29 or T300, or by vendor: MOZA, Fanatec) steer 1:1 over a 900° rotation range.

For a wheel and pedals, open the settings with Esc (the simulation pauses), pick an action and press Enter, then move the control: turn the wheel fully right and back to centre, or press a pedal fully and release it, then press Enter again; shift buttons are assigned by pressing them. Steering, pedals and shifters may come from different devices, and the calibration handles inverted or offset axes. Set the rotation to match the wheel's own setting. Assignments are saved to `%APPDATA%/open-racing/input.ron` (`~/.config/open-racing/input.ron` elsewhere) and select the "custom" input.

Force feedback (Windows, DirectInput) plays the simulated steering torque on the wheel that steers, with the driver's centring spring switched off. The torque is taken about each front wheel's steering axis from everything acting on its contact patch: the tyre's aligning moment, the lateral force on the mechanical trail, the longitudinal force on the scrub radius and the load on the caster and kingpin inclination, so the force goes light as the fronts lose grip and bumps, undulations and kerb ridges kick through the rim. The pneumatic trail grows with the tyre's load, swaps ends in reverse and shrinks on loose ground, where the tyre ploughs; at a standstill the contact patch winds up against the steering like rubber, ever more softly as it slips round, and springs back when the wheel is let go; loose ground holds it less. On top of the torque the wheel plays two vibrations: the grain of the surface under the front tyres, rougher off the track, and the scrub of front tyres sliding. Its settings are on the second page of the settings screen (Esc, then Tab) and are saved to `ffb.ron` next to `input.ron`: on/off, the base's peak torque in N·m (e.g. 12 for a MOZA R12, with the wheel software's own FFB gain at 100 %), strength as a share of the car's steering torque (100 % reproduces it 1:1 at the rim; a GT3 car peaks around 20 N·m), max output in N·m, road detail (the share of fast changes such as bumps and kerbs reproduced, 100 % as simulated), effects (strength of the two vibrations), damping and direction. Keep the torque below the max output: a clipped force is flat and carries no detail. Wheels differ in which way they push, so hold the wheel and run the test first: it must turn the wheel left, otherwise flip the direction. Turning the wheel past the car's steering lock (270° each way for the GT3) pauses the force until it comes back; being driven into lock three times within seconds (an inverted direction) stops it until the settings screen is opened. The HUD shows either. If the wheel itself stops the effect (e.g. MOZA's hands-off protection), it is restarted.

### Engine, clutch and gearbox

The engine is a mean-value model of a four-stroke: air flows through the throttle (compressible orifice flow) into the intake manifold, the cylinders draw it out in proportion to the engine speed and their volumetric efficiency, and the torque follows the air they trap, less the work of pumping it through the throttle and the engine's friction. It is calibrated so that in standard air a steady engine gives the car's `torque_curve` with the throttle wide open and its `drag_curve` on the overrun; everything else follows from the flows: the manifold fills in a few tens of milliseconds, part throttle and engine braking come from the manifold's vacuum, thin or hot air weakens the engine. The injectors cut on the overrun, at the rev limiter and for a gearbox's ignition cut, with the throttle left open, so the torque returns at once. A drive-by-wire throttle (the default) is opened by a motor so that the torque follows the pedal evenly; a cable throttle (`throttle: Cable`) turns the butterfly with the pedal, which uncovers most of the air a low engine speed needs early in its travel.

A turbocharger (`turbo` in `engine`) is a shaft whose kinetic energy the exhaust feeds through the turbine and the compressor draws on, so the boost builds with the air flow and lags behind the throttle: little below its `reference_rpm`, spooling up over `spool_time`, held at `max_boost` by a wastegate that opens over `wastegate_band`. With the fuel cut the cool exhaust barely drives it; a blow-off valve vents the boost when the throttle shuts, and the shaft keeps turning for the next time it opens. The torque curve of a turbocharged engine is the engine's without boost; the manifold's pressure carries the boost into the torque. The HUD shows the manifold pressure, or the boost.

The engine, the gearbox input shaft and the driven wheels turn as three bodies joined by friction couplings — the clutch, a synchroniser, or a dual clutch's two clutches — each stuck or slipping (with less friction while slipping) and meshed gears. Three kinds of gearbox are modelled:

- **H-pattern manual** (`fr_sports`): the lever moves through neutral, and the selected gear's synchroniser has to match the input shaft to it before the dogs mesh. With the clutch down that takes a fraction of a second; with the engine still connected it cannot, and the gears grind. The clutch pedal bites over the top of its travel. Drop it at rest and the engine stalls; let it in with the car rolling and the engine turns with the wheels (a bump start works).
- **Sequential dog box with paddles** (`gt3` and the other road cars): the dogs let go only once the torque through them falls, which is what the ignition cut on upshifts is for. They then mesh at whatever speed the engine turns, and the clutch absorbs the difference, so a downshift without a blip snaps the driven wheels.
- **Dual clutch**: the next gear is preselected and a shift hands the torque from one clutch to the other without a break. Its control unit works the clutches: it slips them to pull away, creeps with neither pedal pressed and opens them before a stall.

Each car lists the electronics it is fitted with (`electronics` in its RON file): anti-stall, auto-blip, ignition cut and downshift protection. The GT3 has them all; the `fr_sports` has none. Whatever the car has, three driver aids on the settings screen (Esc, page "assists", saved to `assists.ron`) work the controls as a driver would:

- **Clutch** (on by default): lets the clutch in as the engine revs to pull away, so the car rests in gear with the throttle closed; opens it before a stall; works it through H-pattern shifts.
- **Auto-blip** (on by default): matches revs on downshifts, as heel-and-toe does.
- **Auto shift** (off by default, or `--auto-shift`): picks the gears.

An H-pattern shifter's gates (1–7 and R) can be assigned on the input page like the other buttons; with them assigned, the shifter selects the gear directly.

The behaviour of each part is set in the car's RON file; every value below has a default that matches the bundled cars, so a car file only names what it changes:

- `engine`: `idle_authority` and `idle_band_rpm` (how far the idle control opens the throttle, and how many rpm below idle it takes to open it fully); `displacement` and `manifold_volume` in litres (estimated from the torque curve, and 1.5 × the displacement, when left out); `throttle` (`DriveByWire` or `Cable`); `turbo: Some((max_boost: …, reference_rpm: …))` with optional `spool_time`, `flow_exponent` (how steeply the boost grows with the air flow), `wastegate_band` and `blow_off_valve`.
- `clutch`: `bite_point` and `engagement_exponent` (how the capacity grows as the pedal comes back from the bite point: 1 is linear, 2 the default).
- `gearbox`: `input_inertia` and `input_drag` (the input shaft's inertia and oil drag). `HPattern` adds `sync_window` (speed difference, rad/s, at which the dogs mesh), `grind_time` (how long a synchroniser fights before the gear grinds) and `reverse_synchro` (with `false`, reverse goes in only once the input shaft has stopped; until then it grinds). `DualClutch` adds `control`: the speeds above idle at which its clutch starts to bite with the throttle closed (`bite_rpm`), closes fully (`lock_slip_rpm`, `lock_rpm`) and drops a gear (`downshift_rpm`), and the range over which it engages (`engage_band_rpm`).
- `electronics`: `blip_band_rpm` (how far short of the incoming gear's speed the auto-blip opens the throttle fully; it also blips a dual clutch's downshifts) and the anti-stall's `band_rpm` (the range above its `rpm` over which it opens the clutch).

A sequential gearbox's actuator gives up on a gear change whose dogs have not let go within half a second, so a downshift asked for on full throttle is dropped rather than carried out once the car has gone much faster.

### Brakes, engine temperatures and failures

Each wheel's brake is three bodies that heat and cool: the disc, the caliper with its pads, pistons and fluid, and the rim with the hub. The kinetic energy the brake takes from the wheel heats the disc (and 6 % of it the pads); the disc sheds heat to the air through its vanes and duct (forced convection, growing with the wheel's speed to the 0.8) and by radiation, and passes some through the pads into the caliper and through its bell into the rim. The pads' friction follows the disc's temperature (`pad_friction`): racing pads bite weakly cold and are at their best from about 350 to 650 °C, road pads fade from about 450 °C, so a road car driven hard on a track fades stop after stop while its tyres could still stop it. A caliper hotter than the fluid's boiling point fills with vapour, which takes much of the pressure the pedal builds. The rim heats the tyre's carcass and the air inside it, so hot brakes raise the tyre's pressure, and a cooler rim takes heat out of it. The HUD shows each disc's and caliper's temperature and the share of its torque the brake gives ("bite").

The engine holds its heat in three bodies: the cylinders (heads, liners and pistons), the coolant with the block, and the oil. A share of the fuel's energy heats the cylinder walls and friction heats the oil and the liners; the coolant carries the heat to the radiator, whose flow the thermostat opens to hold its temperature, and the oil sheds heat in its cooler and to the coolant. The radiator and the oil cooler take the air coming into the nose, or at low speed a fan's (racing cars often have none and overheat standing still); a hit on the nose crushes them. The temperatures change what the engine gives, always:

- cold oil is thick and adds friction (about twice the warm friction at 20 °C), hot oil takes some away;
- cylinders above 170 °C knock, and the control unit retards the ignition, costing up to a quarter of the torque;
- coolant above its boiling point leaves the cylinder walls in steam, which carries little heat, so the cylinders overheat.

Two options on the settings screen (Esc, page "realism", saved to `realism.ron`) decide whether anything lasting happens, as in other sims:

- **Damage** (on by default): hits into walls cost downforce and crush the radiator and oil cooler. Off, the car takes no damage.
- **Failures** (on by default): engine parts beyond their limits wear — the pistons and head gasket in cylinders above 230 °C, the bearings in oil above 150 °C (their life halves for every 15 K more), the valvetrain above the over-rev speed (7 % over the limiter unless the car says otherwise; far over it the valves hit the pistons at once). Worn pistons and valves lose compression and power, worn bearings add friction, and a broken part stops the engine for good. Off, parts do not wear, though heat still costs power.

Putting the car back on the track (Backspace) repairs it. Every value has a default that matches the bundled cars:

- `brakes`: `pad_friction` (friction against the disc temperature, °C, relative to its peak, where `max_torque` applies; road pads when left out), `disc_mass` and `disc_cooling` for the front and rear discs (kg, and W/K at 50 m/s; sized from the brake torque when left out), `fluid_boiling_point` (260 °C) and `start_temperature` (the discs' temperature after a reset, 150 °C). The GT3 runs racing pads on ducted 11 / 8.5 kg discs with racing fluid, warmed to 300 °C on the way to the grid.
- `engine`: `cooling` with `radiator` and `oil_cooler` (W/K at 50 m/s with the thermostat open; sized from the engine's peak power when left out), `thermostat` (85 °C), `boiling_point` (125 °C, under the cap's pressure) and `fan` (`true`; the GT3 has none); `over_rev_rpm`.

## Adding content

- Tracks: put a centreline control-point file (position, width, bank) in `assets/tracks/<name>.ron` and select it with `--track <name>`.
- Cars: add `assets/cars/<name>.ron` using `gt3.ron` as a template, or convert a car into a car package (below). Each axle names its tyre (`tire: "<name>"` for `assets/tires/<name>.ron`, or a `.ron` path relative to the car file) and sets the cold pressure in bar, as teams set it in the garage.
  `drive` picks the driven wheels: `Rear` (the default), `Front`, or `All(front_share: …, centre_differential: (…), front_differential: (…))`, where a centre differential splits the torque between the axles; `differential` is the driven axle's, the rear one's for all-wheel drive. Keep the front and rear tyres the same size on an all-wheel-drive car with a locking centre differential: like a real one, it fights a difference in wheel speed.
  Where the engine sits shows in `front_weight` and in `inertia`: masses near the centre lower the pitch and yaw inertia. With the same parts, a mid-engined car puts more power down, turns in quicker, understeers less and rotates more when the driver lifts mid-corner than a front-engined one, and a nose-heavy front-drive car understeers most (tests in `crates/sim/tests/physics.rs`).
- Bundled cars (`--car <name>`), all fictional and representative of their class: `gt3` (mid-engined GT3 racer on slicks, the default), `hot_hatch` (front-wheel drive, 61 % front), `awd_sedan` (all-wheel drive, 40 % of the torque to the front), `fr_coupe` and `mr_coupe` (the same 3.0 l coupé with the engine in front, 52 % front, and in the middle, 42 % front), `fr_sports` (a light 2.0 l boxer sports car with a six-speed H-pattern manual and no gearbox electronics). The road cars run on the Velloni Strada R road tyre.
- Aero: `aero` holds the static `ride_height` at the axles and a list of `elements` (body, wings, splitter, floor). Each has an area and a position from the CG, downforce and drag coefficients against its angle of attack (its `angle` plus the car's pitch, nose down positive), multipliers against the ride height of the floor under it (`height_lift`, `height_drag`, from the static ride height plus the suspension's travel and the tyres' deflection), a loss of downforce with sideslip (`yaw_lift`) and with damage (`damage_lift`, `damage_drag`, per m/s of impact into walls, by the zone hit: front, rear, left, right). The forces act where the element sits, so they pitch the car. The bundled GT3 has a splitter and a floor in ground effect and a rear wing: its balance moves forward as the car squats at speed. The HUD shows the downforce, its balance and the ride heights, and any damage.
- Suspension: `camber_gain` on an axle (rad per metre of bump travel) sets how the camber changes as the wheel rises; double wishbones gain negative camber in bump.
- Tyres: add `assets/tires/<name>.ron` using `velloni_zeta_gt_front.ron` as a template. A tyre file holds the size, the force curves, how pressure changes stiffness, peak slip, rolling resistance and grip, and the thermal model (inner / middle / outer tread zones and carcass, cooling, operating window, wear). Load sensitivity is a power law (`load_exponent_x` / `load_exponent_y`: the friction force grows with the load raised to them), grip against the tread temperature a `grip_curve`, and `speed_sensitivity` (optional) costs grip per m/s of sliding speed. Hot pressure follows the carcass temperature (gas law), so the cold pressure, camber and driving style show up in the tread temperatures on the HUD as they would on a real car. The bundled GT3 runs on the fictional Velloni Zeta GT, modelled on public figures for GT3 slicks (30/68-18 front, 31/71-18 rear).

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
- **Engine:** the power curve is the engine without boost; each `[TURBO_n]` adds its `MAX_BOOST` (capped by its `WASTEGATE`) to one simulated turbocharger that reaches it at `REFERENCE_RPM`, with the game's `LAG_UP` as its spool time and `GAMMA` as how steeply its boost grows with the air flow. `LAG_DN` has no counterpart: the shaft slows by itself.
- **Tyres:** the power-law load sensitivity (`LS_EXPX`, `LS_EXPY`), the grip left past the peak (`FALLOFF_LEVEL`, as the Magic Formula's shape), the loss with sliding speed (`SPEED_SENSITIVITY`), the camber that grips most and how fast grip falls away from it (`DCAMBER_0`, `DCAMBER_1`), the pressure's effect on stiffness (`PRESSURE_SPRING_GAIN`) and grip (`PRESSURE_D_GAIN`, fitted a quarter of a bar off the ideal) and the thermal `PERFORMANCE_CURVE` carry over.
- **Suspension:** the camber gain of double-wishbone (`DWB`) and strut suspensions follows from their geometry in front view (the instant centre of the arms).
- **Aero:** every `[WING_n]` becomes an element with its lift and drag against the angle of attack (`LUT_AOA_CL`, `LUT_AOA_CD` and their gains, at its `ANGLE`), against the ride height (`LUT_GH_CL`, `LUT_GH_CD`, read at the ride heights of `[RIDE]` in `car.ini`), its `YAW_CL_GAIN` and its damage zones (`ZONE_*_CL`, `ZONE_*_CD`, taking the game's damage as the impact speed in km/h).
- **Engine damage:** `[DAMAGE] RPM_THRESHOLD` in `engine.ini` becomes the over-rev speed above which the valvetrain wears.
- **Gearbox:** a car with `[GEARBOX] SUPPORTS_SHIFTER=1` gets an H-pattern manual, the others a sequential gearbox with an ignition cut. `[AUTOBLIP] ELECTRONIC` and `[DOWNSHIFT_PROTECTION]` become the car's auto-blip and downshift protection, and an automatic clutch the car always has (`[AUTOCLUTCH] FORCED_ON=1`) becomes anti-stall that opens the clutch between `MAX_RPM` and `MIN_RPM`.

Limitations:

- The game's electronic and viscous centre couplings (`AWD2`) and active differentials become limited-slip differentials.
- The game's tyres run on open-racing's tyre model: its Magic Formula curves and thermal model take the values above, but the game's own slip curves (`CX_MULT`, `XMU`, `FALLOFF_SPEED`), relaxation length, flex, graining and blistering and wear curve are not read.
- Several turbochargers become one; the game's turbo `LAG_DN` is not read. Aero fins (`[FIN_n]`) and active aero (`[DYNAMIC_CONTROLLER_n]`) are not simulated. Damage costs aerodynamics and cooling; the game's turbo and suspension damage are not read, and its brakes keep open-racing's thermal defaults.
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
