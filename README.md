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
| `crates/sim`        | 6-DOF chassis, suspension linkages solved from their hardpoints (double wishbones, MacPherson struts, multi-link, trailing arms) with push- and pullrod rockers, two-stage dampers, anti-roll bars and heave springs, Pacejka tyres (combined slip, relaxation length, load sensitivity, sliding speed, camber, tread temperature and wear), a mean-value engine (throttle, intake manifold, turbocharger), brakes whose friction follows their temperature, the engine's cylinder, coolant and oil temperatures with part wear and failures, clutch / H-pattern, sequential and dual-clutch gearboxes with their electronics, front / rear / all-wheel drive through limited-slip differentials, aero elements sensitive to ride height, pitch, yaw and damage, tracks |
| `crates/env`        | RL environment. The observation uses only quantities that other sims (AC / ACC / iRacing, etc.) also expose as telemetry; ground-truth tyre state can be added via `privileged_obs`          |
| `crates/api`        | `VecEnv` / `Policy` traits, asset loading, `AgentDriver` (lets a policy drive a car simulated elsewhere)                                                                                     |
| `crates/track`      | Track package format: centreline, road meshes and walls, render data                                                                                                                         |
| `crates/car`        | Car package format: physics, tyres, and a 3D model whose parts move with the simulated car                                                                                                    |
| `crates/ac`         | Converter from track and car folders in the Assetto Corsa format to track and car packages                                                                                                    |
| `crates/train-burn` | PPO (GAE, clipping, observation normalisation) and `BurnPolicy`                                                                                                                              |
| `crates/app`        | Driving, AI spectating, replay, HUD                                                                                                                                                          |
| `crates/track-project` | Editable track projects (road splines, cross-sections, barriers, race markers) baked into track packages; `open-racing-trackctl` for scripts and agents |
| `crates/editor`     | Track editor                                                                                                                                                                                 |
| `crates/track-render` | Bevy rendering of track and car models, shared by the app and the editor                                                                                                                  |
| `crates/engine-sim` | Crank-angle resolved engine simulator: 1D gas dynamics in the intake and exhaust, cylinders with combustion, heat loss and friction, any crank layout, a dyno, and the engine's sound synthesised from the flow |
| `crates/machine-project` | Machines built from parts (frame, engine, intake, exhaust, transmission, suspension, wheels and tyres, aero, body, interior…), their operations, and their baking into car packages; `open-racing-machinectl` |
| `crates/machine-editor` | Machine editor: assembly in 3D, and an engine workbench (dyno, cycles, gas network, live run with sound, RAM preview) |

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

### Track editor

Circuits are built as projects: roads laid along splines, with their width, banking and crown, and strips beside them (kerbs, run-off, gravel, grass) limited to stretches of the road. Projects also hold painted lines, barriers, the start line, sectors, the grid and a pit lane. Baking a project writes a track package with its race layout. The app starts cars on the pole slot and shows sector times.

```bash
# The editor (projects live in content/track-src/<name>/)
cargo editor my-track

# The same projects from the command line, for scripts and AI agents
cargo trackctl guide                      # the format and the operations
cargo trackctl info my-track --json       # facts: lengths, node distances, radii, grades, warnings
cargo trackctl apply my-track ops.json    # edits as operations, all or nothing
cargo trackctl preview my-track           # plan view as PNG
cargo trackctl centreline my-track lap.gpx --road gp --main   # a real circuit from GPX/KML/GeoJSON/CSV
cargo trackctl kerbs my-track --outside gravel --wall "tyre wall"   # every corner's kerbs, gravel and wall
cargo trackctl dem my-track dem.tif --terrain   # nodes and the ground round them on elevation data (GeoTIFF, ASC, XYZ)
cargo trackctl paint my-track             # the start line and grid slots
cargo trackctl pitlane my-track           # a pit lane beside the start straight, with boxes and a pit wall
cargo trackctl garages my-track assets/models/garage.glb   # a garage behind each pit box
cargo trackctl boards my-track assets/models/board.glb     # 100/200/300 m boards before the corners
cargo trackctl bake my-track              # package in content/tracks/my-track/, checked with a test lap and a race-pace lap
cargo run --release -p open-racing-app -- --track my-track
```

The editor works as Blender does: Tab switches between object mode (pick roads, kerbs, walls and props, and drag one to move it) and edit mode (their nodes), G/R/S transform, and what is selected is orange and bold (the active item and node brightest; nodes are dots, dark when not selected, white when active, ringed while the pointer is over them). Pointing at a strip, wall or line in the outliner or its properties lights it up in the view. To build a real circuit, import its centreline (File › Import Centreline) or trace it over a satellite image (the Reference image tab, scaled with the Measure tool), then work corner by corner: the Corners tab numbers the turns, lays each one's entry, apex and exit kerbs, gravel or run-off and wall (they stay with their corner as the road changes), and Page Up/Page Down step through them; in the view, drag the road's edges at selected nodes, a kerb's or wall's outer edge, and the ends of their stretches, which catch on nodes and on corners' apexes. To kerb a curve of several nodes at once, select them and use Lay Along Selected Nodes (the Node menu, a node's right-click menu or Shift A): any kerb, run-off or wall type goes along them on the inside or outside of the curve, or either side. A kerb's width and height can change along it: Ctrl + click it in edit mode to add a node, drag the node out to widen it there, Z while dragging to raise it. In edit mode the painted lines are dragged across the road too (they catch on its centre, its edges and each other), and a right click paints a new one. The Library tab holds the kinds of kerbs, strips, walls (optionally a glTF model repeated along them, kerbs too), materials (PNG, JPEG or DDS textures, or blocks of any two colours) and surfaces, made once and used anywhere; a kerb type's cross-section is drawn to drag its points, with a step up from the road and ready shapes. Shift + click or a box selects several items to move, turn, scale, mirror (Ctrl M), duplicate (Shift D) or delete together, and a change to the active item's properties is made to the others of its kind too; Shift G selects similar items, and Ctrl F2 renames them at once. O turns on proportional editing (the nodes near those moved follow, the wheel setting how far), Y cuts a road or spline at a node, Ctrl J joins lines end to end, and Switch Direction turns one round; keys, stretches, corner parts and markers stay where they were on the road through all of them. H hides, Alt H reveals, numpad / looks at the selection alone, and the outliner's eyes and locks and its collections (M moves items to one) keep a dense circuit workable. Ctrl C and Ctrl V copy items as operations, into another project or to an agent. The Terrain tab makes the ground follow elevation data of the real place and shapes it with landforms (hills, banks, level pads, dragged in the view); a road's Rows tab lays trees, boards, lights or garages beside it. The toolbar's brushes work as Blender's sculpting and texture painting: Sculpt Terrain raises, digs, smooths, levels or roughens the ground as you drag (Ctrl the other way, Shift smooths, F sets the radius and Shift F the strength), Paint Ground paints up to three layers over it (dirt, gravel, sand, each driving with its own grip), and Scatter plants woods, bushes, rocks, long grass or crowds of spectators (Ctrl wipes them out); L switches any brush to a lasso that fills the area it outlines (a pad levelled, a gravel patch, a wood); built-in trees, bushes, rocks, grass tufts and cones need no model files, and the Scatter tab sets each scatter's models, spacing and sizes. Ctrl F sets a brush's hardness (how much of its radius acts fully, from a soft falloff to a hard edge), and a stamp (clouds, spots or streaks) makes it act in patches. With the Scatter tool a palette under the view offers kinds of vegetation on shelves (trees, bushes, grass, rocks, spectators, others, your own and the project's scatters) with a picture of each, as a city builder's asset menu: a click paints it. 2 plants single copies where you click (a drag turns them) and 3 selects copies, painted or planted, to move (G), turn (R), resize (S) or delete (X) one by one or boxed together; the tool settings set their model, size and turn. A scatter's models may be your own glTF files (the Assets panel's Scatter button, or the palette's From a Model File), with the project's materials in place of theirs (bark or leaves of your own textures), and "Save to my library" keeps a scatter with its models, materials and brush as a kind to paint in any project. Scatters lay their copies out naturally, evenly or in rows along the roads (spectators facing the track), and are drawn by instancing: their models in full near the camera, and pictures of them on crossed cards (made when the project builds, or a far model of your own) out to their draw distance. Plants sway in the weather's wind, and each copy's leaves have a colour of their own that follows the season: fresh in spring, yellow, orange and red in autumn, bare branches and straw in winter. The World tab sets the sky and the light (time of day, month, latitude, weather, exposure, haze, and the season plants show): the view shows them as the game does, with its physical atmosphere, volumetric clouds and their shadows, and the game starts in them. Each stroke is an operation in `project.ron`, so an agent paints the same way. Kerbs and walls take any shape: drag in the draw tool to sketch them freehand, and Alt S at their nodes sets a kerb's width or a wall's height there; the add menu also draws filled areas (gravel traps, paddocks, car parks). In the view the wheel zooms towards the pointer, holding the right button with W A S D (Q, E) flies, and C selects by painting with a circle. Snapping (grid, angle and what dragged nodes catch on) is set in the sidebar's Tool tab. The Curves area below the view graphs the selected road's elevation, widths and bank as Blender's graph editor: nodes and keys are picked with a click, Shift + click or a box and dragged together (G too), a double-click adds one, X deletes the selected, the numbers along the bottom of the width and bank graphs pick the road's nodes, and the right click adds and deletes nodes from there as well; the wheel zooms, Ctrl + wheel zooms the values and the middle button pans. The Checks tab lists what will not drive well as you edit, View › Walk the Track looks along the road from a driver's eye, Bake drives a test lap and a race-pace lap round the racing line whose path the view shows coloured by speed and View › Replay Test Lap replays, Edit › Undo History goes back to any step, and a backup of `project.ron` is kept every few minutes (File › Restore Backup).

Every edit in the editor is one of the operations `trackctl apply` takes, and the editor saves each one to `project.ron` straight away. When the file changes on disk, the editor reloads it (the selection staying on the same items), so a person and an agent can work on the same track at once. `open-racing-editor <project> --screenshot view.png` saves the 3D view and quits (with `--focus <name>`, `--at <x,y>`, `--corner <n>`, `--edit <nodes>`, `--top`, `--tab <tab>`, `--curves <graph>`, `--tool <tool>`, `--scatter <name>`, `--scatter-mode <paint|plant|select>`, `--distance <m>`, `--pitch <deg>` and `--yaw <deg>` to choose what it shows).

### Machine tool

Cars are built the way real ones are: from parts designed on their own and kept in their own files (`<content>/machine-src/parts/<kind>/<name>.ron`) — a frame, an engine with its intake and exhaust, a transmission, suspensions, wheels and tyres, brakes, steering, aero, a body, an interior — assembled into a machine (`machines/<name>.ron`) by attaching parts to each other's mounts, with the machine's setup. One engine can go into several machines, and swapping an exhaust changes both the power and the sound. Parts have stand-in shapes that give their mass, centre of mass and inertia (and later their models and crush zones), and mounts whose joints are kept for damage.

Engines are simulated in detail, apart from the game: the intake and exhaust as one-dimensional unsteady gas dynamics (finite volumes, MUSCL–Hancock with the HLLC Riemann solver, wall friction and heat transfer), valves and throttles as compressible restrictions, and each cylinder with Wiebe combustion, Woschni heat transfer, a knock index and Chen–Flynn friction, on a crank of any layout. The pressure waves tune the breathing, and what leaves the tailpipes and intake mouths is the sound, heard through microphones at the tailpipe, beside the car or in the cabin. Tyres and suspensions carry the game's own parameters until their workbenches simulate them.

```bash
cargo machinectl init                           # the samples: a GT3-class machine, a V8 and an inline four
cargo machinectl info machine/gt3_v8            # mass, CG, inertia, weight split
cargo machinectl dyno engine/i4_2l_na --png dyno.png
cargo machinectl sound machine/gt3_v8 --preset rev --mic exhaust,cabin --out v8.wav
cargo machinectl bake machine/gt3_v8            # a car package, from the parts and a dyno sweep
cargo dev -- --car gt3_v8
cargo machine machine/gt3_v8                    # the editor
```

`cargo machinectl guide` prints the format and the operations. As with tracks, every edit is an operation (`machinectl apply`, or `Set` any field by its path), the editor saves each at once and reloads files an agent changes, and `--screenshot` lets scripts see it. In the editor's Engine workspace the engine runs live at Draft quality, the pedal on W; where the machine cannot keep up (or for finer quality), the RAM preview renders a script or the last live take into memory and plays it back.

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

Without `--track-grip` training keeps the whole asphalt at the tyres' nominal grip, as before. The laid rubber is off in training by default (`--grip-gain`), since one car adds little within an episode; tyre dirt from going off track always applies. The app shows the coat on the tyres; the rubber and the dirt on the road show in the road's [debug views](#debug-views).

### Weather

The app simulates the weather with the car, so a drive replays exactly. Rain is not modelled yet.

- **Sky:** clear, fair, partly cloudy, cloudy or overcast, set on the Esc settings (page "weather") or with `--weather`. With changes on, the sky drifts between neighbouring states every hour or so of weather time, and time can run up to 120× faster than real time.
- **Clouds** come in four layers, as a meteorologist reports them: cumulus from daytime convection (none at night, building through the day, their flat bases at the condensation level, 125 m per kelvin of dew-point spread), a stratus or stratocumulus deck, an altostratus or altocumulus sheet at about 3 km, and cirrus at about 9 km. Each drifts with its own wind, veering and strengthening with height. They are drawn as ray-marched volumetric clouds in the manner of *Horizon Zero Dawn* (Schneider) and *Frostbite* (Hillaire), with a physical atmosphere, and the cloud shadows sweep over the track.
- **Air:** the temperature follows the month's climate and a daily cycle damped by cloud, and falls with height; humidity and pressure follow the sky. The air's density (temperature, pressure, humidity) scales drag, downforce and engine power (SAE J1349), and the wind, with gusts, adds to or takes from the airspeed.
- **Road temperature:** each 8 m patch of the road, in three lanes, balances sunshine on its slope, diffuse daylight from the part of the sky it sees, long-wave radiation to and from the sky, convection and conduction into the ground. On tracks with a 3D model the scenery shades the road (rays cast once per half hour of the sun's path), so a stretch under a grandstand stays cool while open asphalt gets 20–30 °C above the air on a summer afternoon. Tyres are heated or cooled by the road under them and by the air around them.
- **The car in the weather:** a reset starts the car from the air where it stands. The tyres are inflated to their cold pressure in that air (so a tyre set on a cold morning runs higher once warm), and start at its temperature unless they come out of blankets; the calipers and rims start part of the way from the air to the discs'; the engine and gearbox are warmed on an out-lap in that air, so on a winter morning their oils start colder and thicker. The coolers and the brakes cool with the air's mass flow (Reynolds number to the 0.8): hot or thin air cools less, and a headwind more.

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

Each wheel's brake is three bodies that heat and cool: the disc, the caliper with its pads, pistons and fluid, and the rim with the hub. The kinetic energy the brake takes from the wheel heats the disc (and 6 % of it the pads); the disc sheds heat to the air through its duct, fed by the car's airspeed with the wind, and its vanes, pumping as the wheel turns (forced convection, growing with the air's mass flow to the 0.8, so a locked wheel still cools) and by radiation, and passes some through the pads into the caliper and through its bell into the rim. The pads' friction follows the disc's temperature (`pad_friction`): racing pads bite weakly cold and are at their best from about 350 to 650 °C, road pads fade from about 450 °C, so a road car driven hard on a track fades stop after stop while its tyres could still stop it. A caliper hotter than the fluid's boiling point fills with vapour, which takes much of the pressure the pedal builds. The rim heats the tyre's carcass and the air inside it, so hot brakes raise the tyre's pressure, and a cooler rim takes heat out of it. The HUD shows each disc's and caliper's temperature and the share of its torque the brake gives ("bite").

The engine holds its heat in three bodies: the cylinders (heads, liners and pistons), the coolant with the block, and the oil. A share of the fuel's energy heats the cylinder walls and friction heats the oil and the liners; the coolant carries the heat to the radiator, whose flow the thermostat opens to hold its temperature, and the oil sheds heat in its cooler and to the coolant. The radiator and the oil cooler take the air coming into the nose, or at low speed a fan's (racing cars often have none and overheat standing still); a hit on the nose crushes them. The temperatures change what the engine gives, always:

- cold oil is thick and adds friction (about twice the warm friction at 20 °C), hot oil takes some away;
- cylinders above 170 °C knock, and the control unit retards the ignition, costing up to a quarter of the torque;
- coolant above its boiling point leaves the cylinder walls in steam, which carries little heat, so the cylinders overheat;
- the heat the engine sheds warms the air in its bay, the more the less air flows through: worked hard standing or crawling, the bay gets hot. The intake draws part of its air from the bay, and warm intake air costs power; the gearbox sits in the bay and its oil, heated by its own losses and by the engine through the bellhousing, adds churning losses when cold; the bay's air flows out past the tyres of the axle nearest the engine (`engine.position`: `Front`, `Mid` or `Rear`). The HUD shows the intake and gearbox temperatures.

Two options on the settings screen (Esc, page "realism", saved to `realism.ron`) decide whether anything lasting happens, as in other sims:

- **Damage** (on by default): hits into walls cost downforce and crush the radiator and oil cooler. Off, the car takes no damage.
- **Failures** (on by default): engine parts beyond their limits wear — the pistons and head gasket in cylinders above 230 °C, the bearings in oil above 150 °C (their life halves for every 15 K more), the valvetrain above the over-rev speed (7 % over the limiter unless the car says otherwise; far over it the valves hit the pistons at once). Worn pistons and valves lose compression and power, worn bearings add friction, and a broken part stops the engine for good. Off, parts do not wear, though heat still costs power.

Putting the car back on the track (Backspace) repairs it. Every value has a default that matches the bundled cars:

- `brakes`: `pad_friction` (friction against the disc temperature, °C, relative to its peak, where `max_torque` applies; road pads when left out), `disc_mass` and `disc_cooling` for the front and rear discs (kg, and W/K at 50 m/s; sized from the brake torque when left out), `fluid_boiling_point` (260 °C) and `start_temperature` (the discs' temperature after a reset, 150 °C). The GT3 runs racing pads on ducted 11 / 8.5 kg discs with racing fluid, warmed to 300 °C on the way to the grid.
- `engine`: `cooling` with `radiator` and `oil_cooler` (W/K at 50 m/s with the thermostat open; sized from the engine's peak power when left out), `thermostat` (85 °C), `boiling_point` (125 °C, under the cap's pressure) and `fan` (`true`; the GT3 has none); `over_rev_rpm`.

### Suspension

Each axle's `linkage` guides the upright, the part that carries the wheel, by its hardpoints: the ball joints and pivots of its arms and links, given for the left wheel relative to its wheel centre at static ride height (m, x forward, y left, z up; the right wheel mirrors them).

- `DoubleWishbone(upper, lower, tie_rod)`: two A-arms (`front` and `rear` pivot on the body, `outer` ball joint on the upright) and a tie rod (`inner`, `outer`), the toe link on an unsteered axle.
- `MacPherson(lower, strut_top, strut_bottom, tie_rod)`: a lower arm, and a strut fixed to the upright (through `strut_bottom`) sliding through its top mount.
- `MultiLink(links, tie_rod)`: four links and a tie rod, each with its own ball joints.
- `TrailingArm(pivots)`: the upright on an arm pivoting about the axis through two points, trailing or semi-trailing; a twist beam comes close to one. It cannot steer.

The linkage is solved ahead of time over the wheel's travel and the rack's, so a step costs a table lookup. What the hardpoints do follows from it, as on a real car: camber and toe change with the travel (camber gain, bump steer), the steering rack turns the wheels about the axis the linkage makes (caster, kingpin inclination, trail and scrub radius, Ackermann), and the tyre's forces act on the wheel's travel as far as their point moves with it: braking pulls the wheel down where the contact patch moves forward as it rises (anti-dive, anti-lift), the drive where the wheel centre moves back (anti-squat), and cornering forces lift the body on a roll centre above the ground (jacking). The steering's feel is the rack's share of the same forces.

`actuation` decides how the wheel works the springs: `Wheel` (the default: rates are wheel rates), `Direct(chassis, outer, on)` for a coil-over from the body to the upright or an arm (`on: Upright`, `LowerArm`, `UpperArm`), or `Rocker(rod, on, pivot, axis, rod_end, spring_end, chassis)` for a pushrod or pullrod working a rocker on the body, which compresses the coil-over. The rates are then per metre of the coil-over's compression, and the motion ratio (and so the wheel rate) changes with the travel. The anti-roll bar works on the difference of the two sides' actuations, a `heave` spring (`rate`, `gap`, `damping`) on their mean: it holds a car up under downforce without stiffening it in roll. The dampers have a low- and a high-speed rate each way (`fast_bump_damping`, `fast_rebound_damping` above `damper_knee`). `static_camber` and `static_toe` turn the hub on the upright; the spring's preload holds the car at its static ride height.

```bash
# What each bundled car's suspension does at static ride height
cargo run -p open-racing-sim --example suspension
```

It prints, per axle, the camber gain, bump steer, roll centre height, motion ratio, anti-dive / anti-lift / anti-squat (from the brake balance and the drive's split), the wheel rates and ride frequency, and at the front the caster, kingpin inclination, trail, scrub radius and Ackermann.

### Debug views

The app can draw what the simulation computes in false colour, with the scene's own colours set aside: blue for cold or little, through green at working temperature, to red for hot or much; grip goes from red (poor) to green (full). Turn them on with F1–F4 or on the settings screen (Esc, page "debug", saved to `debug.ron`). A panel at the bottom right gives the numbers and colour scales.

- **Road** (F1 steps through the views): the grip the rubber and dirt give, the rubber level, the dirt on the asphalt (coloured by kind), or the surface temperature, laid over the road. Tyre marks are hidden while a road view is on.
- **Environment** (F2): the ray towards the sun (yellow in sunshine, grey behind cloud), the cloud shadows on the ground round the car, the wind at the car with its gusts and the 10 m wind above it; air, sunlight and cloud figures in the panel.
- **Aero** (F3): each aero element's downforce and drag at its centre of pressure, the ride height at each axle against its static value, the air coming at the car, and the temperature of the air round each tyre (warmed by the road and the engine bay), in the bay and at the intake.
- **Car** (F4): the car's model is hidden. In its place the view draws the body, the suspension's links, rods and rockers (coloured by travel from full droop to the bump stops), each tyre's inner, middle and outer tread and its carcass (coloured by temperature against the grip peak), the brake discs and calipers, the engine block, sump, gearbox and radiator (all coloured by temperature), the load and grip force at each contact patch, and the car's velocity and acceleration.

## Adding content

- Tracks: put a centreline control-point file (position, width, bank) in `assets/tracks/<name>.ron` and select it with `--track <name>`. Bundled fictional layouts include Lakeside, Redwood Speedway (banked oval), Pine Ridge Club (compact technical circuit), and Harbor Street Circuit (narrow stop-and-go street course); for example, `cargo run --release -p open-racing-app -- --track harbor_street`.
- Cars: add `assets/cars/<name>.ron` using `gt3.ron` as a template, or convert a car into a car package (below). Each axle names its tyre (`tire: "<name>"` for `assets/tires/<name>.ron`, or a `.ron` path relative to the car file) and sets the cold pressure in bar, as teams set it in the garage.
  `drive` picks the driven wheels: `Rear` (the default), `Front`, or `All(front_share: …, centre_differential: (…), front_differential: (…))`, where a centre differential splits the torque between the axles; `differential` is the driven axle's, the rear one's for all-wheel drive. Keep the front and rear tyres the same size on an all-wheel-drive car with a locking centre differential: like a real one, it fights a difference in wheel speed.
  Where the engine sits shows in `front_weight` and in `inertia`: masses near the centre lower the pitch and yaw inertia. With the same parts, a mid-engined car puts more power down, turns in quicker, understeers less and rotates more when the driver lifts mid-corner than a front-engined one, and a nose-heavy front-drive car understeers most (tests in `crates/sim/tests/physics.rs`).
- Bundled cars (`--car <name>`), all fictional and representative of their class: `gt3` (mid-engined GT3 racer on slicks, the default), `hot_hatch` (front-wheel drive, 61 % front), `awd_sedan` (all-wheel drive, 40 % of the torque to the front), `fr_coupe` and `mr_coupe` (the same 3.0 l coupé with the engine in front, 52 % front, and in the middle, 42 % front), `fr_sports` (a light 2.0 l boxer sports car with a six-speed H-pattern manual and no gearbox electronics), `formula` (a Formula 3 class single-seater: pushrod and pullrod double wishbones with heave springs, wings and a floor in ground effect, on the Velloni Zeta F slick). The road cars run on the Velloni Strada R road tyre.
- Aero: `aero` holds the static `ride_height` at the axles and a list of `elements` (body, wings, splitter, floor). Each has an area and a position from the CG, downforce and drag coefficients against its angle of attack (its `angle` plus the car's pitch, nose down positive), multipliers against the ride height of the floor under it (`height_lift`, `height_drag`, from the static ride height plus the suspension's travel and the tyres' deflection), a loss of downforce with sideslip (`yaw_lift`) and with damage (`damage_lift`, `damage_drag`, per m/s of impact into walls, by the zone hit: front, rear, left, right). The forces act where the element sits, so they pitch the car. The bundled GT3 has a splitter and a floor in ground effect and a rear wing: its balance moves forward as the car squats at speed. The HUD shows the downforce, its balance and the ride heights, and any damage.
- Suspension: each axle's `linkage` and `actuation` give its hardpoints (see [Suspension](#suspension)); `cargo run -p open-racing-sim --example suspension -- <car.ron>` prints what they do.
- Tyres: add `assets/tires/<name>.ron` using `velloni_zeta_gt_front.ron` as a template. A tyre file holds the size, the force curves, how pressure changes stiffness, peak slip, rolling resistance and grip, and the thermal model (inner / middle / outer tread zones and carcass, cooling, operating window, wear). Load sensitivity is a power law (`load_exponent_x` / `load_exponent_y`: the friction force grows with the load raised to them), grip against the tread temperature a `grip_curve`, and `speed_sensitivity` (optional) costs grip per m/s of sliding speed. Hot pressure follows the carcass temperature (gas law), so the cold pressure, camber and driving style show up in the tread temperatures on the HUD as they would on a real car. The bundled GT3 runs on the fictional Velloni Zeta GT, modelled on public figures for GT3 slicks (30/68-18 front, 31/71-18 rear).

### Track packages

Tracks with 3D models are loaded from open-racing's own package format: a directory `content/tracks/<name>/` holding

- `track.ron`: format version, centreline and surface types (grip, rolling resistance),
- `ground.bin`: road meshes the tyres ride on, and walls,
- `visual.bin`: meshes, materials and textures, and models repeated many times (woods, rocks) kept once with where each copy stands, read only by the app.

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
- **Suspension:** double-wishbone (`DWB`) and strut (`STRUT`) suspensions become linkages with their hardpoints (`WBCAR_*`, `WBTYRE_*`, `STRUT_*`, the steering or toe link `*_STEER`); an axle without steering points gets a toe link beside its lower arm. The game's rates are wheel rates, so the springs, dampers and bars act at the wheel.
- **Aero:** every `[WING_n]` becomes an element with its lift and drag against the angle of attack (`LUT_AOA_CL`, `LUT_AOA_CD` and their gains, at its `ANGLE`), against the ride height (`LUT_GH_CL`, `LUT_GH_CD`, read at the ride heights of `[RIDE]` in `car.ini`), its `YAW_CL_GAIN` and its damage zones (`ZONE_*_CL`, `ZONE_*_CD`, taking the game's damage as the impact speed in km/h).
- **Engine damage:** `[DAMAGE] RPM_THRESHOLD` in `engine.ini` becomes the over-rev speed above which the valvetrain wears.
- **Gearbox:** a car with `[GEARBOX] SUPPORTS_SHIFTER=1` gets an H-pattern manual, the others a sequential gearbox with an ignition cut. `[AUTOBLIP] ELECTRONIC` and `[DOWNSHIFT_PROTECTION]` become the car's auto-blip and downshift protection, and an automatic clutch the car always has (`[AUTOCLUTCH] FORCED_ON=1`) becomes anti-stall that opens the clutch between `MAX_RPM` and `MIN_RPM`.

Limitations:

- The game's electronic and viscous centre couplings (`AWD2`) and active differentials become limited-slip differentials.
- The game's tyres run on open-racing's tyre model: its Magic Formula curves and thermal model take the values above, but the game's own slip curves (`CX_MULT`, `XMU`, `FALLOFF_SPEED`), relaxation length, flex, graining and blistering and wear curve are not read.
- Solid axles (`AXLE`) and the other suspension kinds keep the base car's linkage, and the game's static toe, packers and heave springs are not read.
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
