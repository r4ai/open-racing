# Track projects

A track project is the editable source of a circuit, kept in a directory:

- `project.ron`: everything about the track. It is plain text, and names tie its parts together.
- `assets/textures/` and `assets/models/`: the textures (PNG, DDS) and glTF models (.glb, .gltf) the project uses, if there are any. The built-in textures need no files.

`open-racing-editor` edits projects in 3D. `open-racing-trackctl` edits them from the command line.
Both apply the same operations and check the result the same way.
Baking turns a project into the track package that the simulator reads, under `content/tracks/<name>/`.

```bash
trackctl new my-track                   # content/track-src/my-track/, with a small circuit
trackctl info my-track [--json]         # roads, nodes and their distances, radii, grades, markers, warnings
trackctl apply my-track ops.ron         # or ops.json, or - for stdin; all or nothing
trackctl preview my-track               # plan view: my-track/preview.png
trackctl import my-track a.png b.glb    # copy into my-track/assets/, print the paths to use
trackctl model my-track assets/models/oak.glb  # triangles, size, leaves; its picture and far cards as PNG
trackctl assets my-track [--json]       # textures and models, what uses each, unused and missing ones
trackctl check my-track --lap           # bake in memory, check, drive a test lap
trackctl bake my-track                  # write content/tracks/<name>/ (then: open-racing-app --track <name>)
trackctl guide                          # this text
```

(`trackctl` is short for `cargo run -p open-racing-track-project --bin open-racing-trackctl --`.)

## Conventions

- **Units.** Metres, radians, m/s. The world frame has X east, Y north and Z up.
- **Driving direction.** Roads are driven in the order of their nodes.
- **Left and right.** These are relative to the driving direction. A positive lateral offset is to the left.
- **Spline parameter `u`.** Places along a road are given as spline parameters rather than as distances. `u = i` is node `i`, and `u = i + 0.5` is halfway to the next node. On a closed road `u` runs from 0 to the node count and wraps around.
  - Keys, stretches, corner parts and markers keep their places when nodes move, and when nodes are added or removed.
  - `trackctl info` prints the distance `s` at each node, so you can convert between `u` and `s`.
- **Names.** Roads, surfaces and materials refer to each other by name. Strips, lines and barriers are named within their road.

## project.ron

```ron
(
    format: 2,
    name: "my-track",                // name of the baked package
    main_road: "circuit",            // the closed road raced on
    roads: [(
        name: "circuit",
        closed: true,
        nodes: [(pos: (0, 0, 0)),
                (pos: (250, 0, 2), handles: Aligned(outgoing: (60, 0, 0), incoming_length: 30)), ...],
        //  handles: Auto (default), Aligned(outgoing, incoming_length),
        //  or Free(incoming, outgoing).
        width_left: (keys: [(u: 0, value: 6)]),      // centre to left edge, m
        width_right: (keys: [(u: 0, value: 6)]),
        bank: (keys: [(u: 0, value: 0), (u: 3, value: 0.05)]),  // + raises the right edge
        crown: 0.05,                 // centre above the edges, m
        surface: "asphalt",
        material: "asphalt",
        left: [                      // strips from the road's edge outwards
            (name: "kerb", width: 1.2, surface: "kerb", material: "kerb",
             profile: Crown(0.03), ranges: [(from: 1.6, to: 4.4)], fade: 4),
            (name: "grass", width: 18, surface: "grass", material: "grass",
             profile: Slope(0.3), ranges: [], fade: 0),
        ],
        right: [...],
        lines: [(name: "left edge", offset: 5.7, width: 0.12, material: "paint",
                 ranges: [], dash: None)],   // dash: Some((3, 9)) for 3 m on, 9 m off
        barriers: [(name: "wall", side: Left, offset: 20, height: 1, thickness: 0.5,
                    material: "concrete", ranges: [])],   // offset from the road's edge
        rows: [(name: "trees", model: "assets/models/tree.glb", side: Left, offset: 30,
                spacing: 15, ranges: [(from: 20, to: 60)],
                jitter: (offset: 6, yaw: 3.14, scale: 0.3))],   // models beside it
        resolution: 2,               // m between cross-sections
    )],
    splines: [                       // kerbs, walls and fences on their own lines
        (name: "T1 sausage", closed: false, drape: true,
         nodes: [(pos: (410, 12, 0)), (pos: (440, 40, 0))], resolution: 0.5,
         shape: Band(width: 0.6, align: Center, profile: Crown(0.12),
                     surface: "kerb", material: "kerb", lift: 0.01)),
        (name: "tyre wall", closed: false, drape: true,
         nodes: [(pos: (500, 100, 0)), (pos: (505, 160, 0), radius: 1.5)], resolution: 2,
         shape: Wall(height: 1, thickness: 0.8, material: "concrete", collide: true)),
        (name: "T1 gravel", closed: true, drape: true, resolution: 1,
         nodes: [(pos: (430, 60, 0)), (pos: (470, 50, 0)), (pos: (480, 100, 0))],
         shape: Area(surface: "gravel", material: "gravel", lift: 0.02)),
    ],
    markers: (
        start: 0.5,                  // start/finish line on the main road (u)
        sectors: [3.0, 6.5],         // sector boundaries (u)
        grid: (count: 12, spacing: 8, stagger: 2.5, behind: 10, pole: Left),
        pit: Some((road: "pit", speed_limit: 22.2, boxes: [0.5, 0.6],
                   box_side: Right, box_offset: 4)),
    ),
    terrain: (enabled: true, surface: "grass", material: "grass", margin: 200, cell: 8,
              heights: Some("assets/terrain/dem.tif"), heights_offset: 0.3,   // optional
              landforms: [(name: "bank", center: (-560, -600), to: Some((-300, -520)),
                           radius: 15, falloff: 30, kind: Raise(8)),
                          (name: "paddock", center: (-750, -500), radius: 60,
                           falloff: 40, kind: Level(-22))],
              sculpt: [(brush: Raise, radius: 40, strength: 6,        // brush strokes
                        points: [(60, 120), (160, 150)])],
              layers: [(name: "sand", surface: "gravel", material: "gravel")],
              paint: [(layer: Some("sand"), stroke: (brush: Paint, radius: 10,
                        strength: 1, points: [(470, 170), (500, 130)]))],
              paint_texel: 1),                // m per texel of the painted layers
    scatter: [(name: "woods", models: [(model: "builtin:pine", weight: 3),
                                       (model: "assets/models/oak.glb", weight: 1,
                                        far: Some("assets/models/oak_far.glb"),   // optional
                                        materials: [(slot: 1, material: "oak leaves")],
                                        kind: Some(Deciduous))],
               spacing: 7, scale: (0.8, 1.25), tilt: 0.1, clearance: 3, max_slope: 35,
               collide: false, shadows: true, detail: 150, draw: 2500, variety: 0.4,
               strokes: [(brush: Paint, radius: 60, strength: 1, hardness: 0.8,
                          points: [(-300, -150), (-250, -80)]),
                         (brush: Erase, radius: 15, strength: 1, points: [(-270, -110)])],
               removed: [(-43, -19)],                  // painted copies taken out, by cell
               placed: [(model: 1, pos: (-262, -96), yaw: 0.4, scale: 1.3)])],
    surfaces: [(name: "asphalt", props: (kind: Asphalt, grip: 1.0, drag: 0.0)), ...],
    materials: [(name: "asphalt", color: (1, 1, 1), texture: Builtin(Asphalt),
                 tile: (4, 4), roughness: 0.8, reflectance: 0.5), ...,
                (name: "sponsor", color: (1, 1, 1), texture: File("assets/textures/sponsor.png"),
                 normal: File("assets/textures/sponsor_n.png"),   // optional normal map
                 alpha: Mask(0.5),            // Opaque (default), Mask(cut-off) or Blend
                 tile: (6, 1), roughness: 0.6, reflectance: 0.5, double_sided: true)],
    props: [(name: "main stand", model: "assets/models/stand.glb", pos: (120, 35, 0),
             yaw: 0.0, scale: 1.0, drape: true, collide: true)],
    environment: (sky: PartlyCloudy, changing: true, hour: 17.5, month: 9,
                  latitude: None, temperature: 0, exposure: 0.3, haze: 1.5),
)
```

**Profiles.** `width_left`, `width_right` and `bank` are keyed along the road. Keys may set `slope_in` and `slope_out` (value per unit `u`) to control how quickly the value changes on either side. Their default is zero, which gives the eased interpolation. A single key makes the value constant.

**Strips.** A strip is a band beside the road, such as a kerb, run-off, gravel, grass or a verge.

- `ranges` limits a strip to stretches of the road. An empty list means everywhere.
- On a closed road, `from > to` wraps through the start.
- `fade` narrows and flattens the strip over that many metres at the ends of each stretch.
- The strip's profile takes one of these forms:
  - `Flat` continues the plane of whatever lies inside it.
  - `Crown(h)` rises to `h` in the middle, like a kerb.
  - `Slope(d)` falls by `d` to its outer edge.
  - `Shape([(x, height)])` is any cross-section: heights (m) at fractions across, joined by straight lines. A shape starting above 0 at `x` 0 steps straight up from the road there (a raised kerb's edge).
- `keys: [(u, width, height)]` make a strip wider or narrower, and its profile higher or lower (`height` times), at places along the road: it eases from one key to the next and keeps to the first and last beyond them. Without keys it is `width` wide all along.
- `model: Some((model, length?, bend?, flip?))` repeats a glTF model along it in place of its plain look (its origin at the foot of the strip's inner edge, halfway across; +X along, +Z up, +Y towards the road). Cars still drive on its profile.

**Splines.** A spline is a kerb, wall or fence along its own line, placed anywhere rather than beside a road.

- Its name must differ from every road's and spline's, and the node operations address it by that name as `line`.
- `drape: true` lays it on whatever is under the line (roads, then terrain) and ignores the nodes' heights. Otherwise it follows the nodes.
- The `Band` shape is drivable: it takes a `width` and a profile. It lies centred on the line, or to its `Left` or `Right`. `lift` raises it above what is under it.
- The `Wall` shape stands on the line. A `thickness` of 0 gives a thin rail or fence, which wants a double-sided material. With `collide: false`, cars pass through it.
- The `Area` shape fills a closed line (of 3 nodes or more) with a surface and material: a gravel trap, a paddock, a car park, a patch of run-off. It is cut into cells so that, draped, it follows the ground.
- A node's `radius` (1 when left out) scales the spline there, easing to the next node's: a band's width, a wall's height. A kerb tapering to nothing at its ends has radius 0 at its end nodes.
- `group: Some("T1 kerbs")` keeps a spline in a collection, as props may be too: the editor's outliner lists, hides, locks and selects a collection as one.

**Surfaces.** A surface's `kind` is one of `Asphalt`, `Kerb`, `Runoff`, `Grass`, `Turf`, `Gravel` or `Dirt`.

- `grip` multiplies the tyres' friction.
- `drag` adds rolling resistance.
- `Asphalt` and `Kerb` count as track. Everything else is off track.

**Built-in textures.** These are `Asphalt`, `Kerb` (red and white blocks along the road), `Stripes((r, g, b), (r, g, b))` (blocks of any two sRGB colours, 0–255, along the road: a kerb's blue and yellow), `Grass`, `Gravel`, `Concrete`, `Armco`, `Paint`, `Dirt`, `Fence` (chain link, see-through with `alpha: Mask`) and `Tyres`. The alternative is `File("assets/textures/x.png")`, a path relative to the project; `trackctl import` puts files there.

- `tile` gives the metres covered by one repetition of the texture, across the road and then along it.
- `normal` is a tangent-space normal map, tiled like the texture.
- `alpha` says what the texture's alpha does: nothing (`Opaque`), cut out below a value (`Mask`, for fences and foliage) or blend (`Blend`, for glass).

**Props.** A prop is a glTF model placed in the scene: a grandstand, a sign, a tree, a building.

- `pos` is where the model's origin goes. `yaw` turns it anticlockwise seen from above, in radians, and `scale` resizes it.
- `drape: true` stands it on the road or terrain under `pos`, ignoring the height.
- `collide: true` makes its triangles walls for the cars. Leave it off for things out of reach, as it costs physics time.
- Models are Y-up as glTF has them. Their own materials and textures come along; the project's materials are not used.
- Wherever a model's path is taken (props, rows, scatters, models along walls), a built-in model may be named instead, needing no file: `builtin:pine`, `builtin:tree` (broadleaf), `builtin:poplar`, `builtin:bush`, `builtin:rock`, `builtin:grass` (a tuft), `builtin:cone` (a traffic cone) and `builtin:spectator` (a person standing, facing +X, in clothes each copy of a scatter colours its own way). They are low in triangles, for thousands of copies.
- The files a project refers to, and those it does not, are listed by `trackctl assets`.

**Rows.** A row repeats a glTF model beside a road: trees, cones, distance boards, lamp
posts, spectators' stands, pit garages.

- Each copy stands `offset` metres beyond the road's edge on `side`, facing the road: the
  model's +X runs along the road and its +Y towards it, as a wall's models do; `yaw`
  turns it from there.
- Copies are `spacing` metres apart along `ranges` (everywhere when empty), or one at each
  place of `at` (spline parameters).
- `jitter` varies each copy's place across, turn and size by up to that much, the same way
  every build. `drape` stands them on the ground; `collide` makes them solid.

**Pit lane.** The pit lane is a separate, open road. It starts where it leaves the track and ends where it rejoins.

**Terrain.** The terrain fills in around the roads. It lies just under them and meets their outer edges.

- `heights` is elevation data the ground away from the roads follows (a GeoTIFF, an ESRI ASCII grid or x y z points, in metres, longitudes and latitudes, or WGS 84 UTM metres), with `heights_offset` added. Within about 30 m of the roads' outer edges it eases from their edges to the data.
- `landforms` shape the ground away from the roads: `Raise(m)` raises a hill or bank (negative digs a hollow) and `Level(m)` levels a pad at that height, round `center` or, with `to`, along the line from `center` to `to`, at full effect out to `radius` and easing to nothing over `falloff` more.
- `sculpt` is brush strokes shaping the ground after the landforms, in order, as the editor's Sculpt Terrain tool paints them. A stroke acts fully within `hardness` (0 to 1, 0.5 when left out) of its `radius` of its `points` and eases to nothing at `radius`: 0 eases out from its path, 1 has a hard edge. It acts once wherever it passes. With `fill: true` its points are a closed outline, as a lasso: it acts fully inside it and eases to nothing outside it over twice the soft part of its radius (`radius` at the usual hardness): a pad levelled, a patch of gravel, a wood. `stamp: Some((shape: Spots, size: 30))` makes it act through a shape of texture lying still on the ground, in patches: `Clouds` (ragged patches as wide as they are apart), `Spots` (round spots `size` apart, about half as wide) or `Streaks` (six times as long as they are wide, along `angle`, radians from the east); strokes over the same place build up the same shapes, and scatters' and painted layers' strokes take stamps too. Its `brush` is `Raise` (by `strength` m; negative lowers), `Smooth` (evens bumps, `strength` 0 to 1), `Flatten(height)` (levels towards that height, `strength` 0 to 1) or `Noise` (roughens by up to `strength` m, the same every build). Near the roads' outer edges strokes and landforms ease out, so the ground still meets the roads. A small `cell` shows finer shapes.
- `layers` are up to three materials painted over the ground's own (dirt, gravel, sand), each with its `surface`: where a layer covers most of a cell, cars drive on it. `paint` is the strokes painting them in order (brush `Paint`, `strength` 0 to 1 of the way to all of it); `layer: None` paints the ground's own material back. `paint_texel` is the painted texels' size, m.

**Scatters.** A scatter paints models over the ground: woods, bushes, rocks, long grass,
spectators.

- Copies stand on a grid `spacing` metres apart, each jittered in its cell. Its `strokes`
  (`Paint` and `Erase`, `strength` 0 to 1, as a sculpting stroke reaches) say how much of
  it each place has, in order: a light stroke plants some, painting again more, and
  erasing takes them away. The same strokes give the same copies every build.
- `layout`: `Grid` (left out: the jittered grid, natural and a little clumped),
  `Even` (blue noise: `spacing` apart on the whole, none nearer than about half of
  it) or `Rows` (rows `spacing` apart along the roads and across, from `clearance`
  beyond their outer edges, every other row staggered, each facing its road: avenues,
  orchards, spectators' rows; a row keeps to the road it is nearest).
- Each copy is one of `models`, picked in proportion to its `weight`, turned at random
  and sized from `scale[0]` to `scale[1]` times its own; `tilt` leans it with the slope (0
  upright, 1 square to it).
- Copies keep `clearance` metres beyond the roads' outer edges (strips included), off
  asphalt, kerbs, run-off and gravel (drivable splines and painted layers too), and off
  ground steeper than `max_slope` degrees. `collide: true` makes them solid for the cars;
  `shadows: false` casts none (long grass).
- Single copies: `removed` takes painted copies out by their cell of the grid (as
  `RemoveCopies` names them, `Cell((i, j))`), however the scatter is painted again;
  `placed` are copies planted one by one, each `model` (an index into `models`) standing
  on the ground at `pos` (x, y) turned by `yaw` and sized by `scale`, wherever it is put
  but on a road or kerb. `seed` (drawn from the name when 0) is where the painted copies'
  places come from; a rename keeps it, so the copies stay where they are.
- Levels of detail: copies show their models in full out to `detail` metres from the
  camera, then a lighter far model out to `draw` metres (0: however far), crossfading.
  A model's far model is its `far` file, or else pictures of it drawn from three sides
  onto crossed cards when the project is built, so a wood of thousands of trees costs a
  few triangles a tree in the distance. With `draw` no more than `detail` the models
  are shown in full and nothing beyond.
- The package keeps each model once, with a list of where each copy stands (32 bytes a
  copy), and the game draws the copies by instancing: a wood of tens of thousands of
  trees adds little to the file and the memory. The far models' copies are merged by
  32 m tiles when the track loads, as few triangles cost less than many entities.
- A model's `materials: [(slot, material)]` use the project's materials in place of its
  own, by their place in the model (its first material is slot 0): a bark or leaf
  texture of your own, laid by the model's UVs.
- Plants: a model's `kind` is `Evergreen` (sways in the wind, stays green),
  `Deciduous` (sways; its leaves turn yellow, orange and red in autumn, fall in winter
  and come out fresh in spring), `Grass` (bends far, dries in late summer, straw in
  winter) or `Rigid` (rocks, cones: stands still). Left out, a built-in model's own
  kind (`pine` evergreen, `tree`, `poplar` and `bush` broadleaf, `grass` grass, `rock`
  and `cone` rigid, `spectator` a crowd), or else `Evergreen`. `Crowd` is spectators:
  they stand still, facing the nearest road, each in clothes of its own colour (the
  model's materials named or cut out as leaves take it). The game sways them in its weather's wind, in
  gusts sweeping over the ground. A model's leaves are its materials cut out by alpha
  or named as leaves (leaf, foliage, needle, grass, canopy, frond, twig); they take
  each copy's colour, and none are drawn of a bare copy. `variety` (0 to 1, 0.4 when
  left out) is how much the copies' leaf colours differ from each other.
- `trackctl info` counts each scatter's copies, planted and taken out, and says where
  they stand.

**Sky and light.** `environment` is the sky and the light the track is shown in; the
game starts in them unless its weather settings say otherwise (Esc, "weather", "Track's").

- `hour` (0 to 24, local solar time), `month` (1 to 12) and `latitude` (degrees north;
  the project's `geo` when left out) set where the sun runs.
- `sky` is `Clear`, `Fair`, `PartlyCloudy`, `Cloudy` or `Overcast`; `changing: true` lets
  it drift between states as time goes by. `temperature` warms (or cools) the air the
  month and the sky give, K.
- `exposure` brightens (above 0) or darkens the picture from what the daylight calls
  for, EV; `haze` thickens (above 1) or thins the haze the weather gives, 0 for none.
- `season: Some(Autumn)` (`Spring`, `Summer`, `Autumn` or `Winter`) is the season
  scattered plants show; left out, the month's (half a year on south of the equator).

## Operations

`trackctl apply` takes a list of operations, as RON or as JSON:

- RON: `[SetKey(road: "circuit", curve: Bank, u: 3, value: 0.05)]`
- JSON: `[{"SetKey": {"road": "circuit", "curve": "Bank", "u": 3, "value": 0.05}}]`

Fields marked `?` below are optional. The editor records its own edits as the same operations.

**The project**

| operation | fields | what it does |
| --- | --- | --- |
| `SetName` | `name` | renames the project and its package |

**Roads**

| operation | fields | what it does |
| --- | --- | --- |
| `AddRoad` | `name`, `closed`, `nodes: [(x, y, z)]`, `like?` | adds a road. With `like`, it copies that road's cross-section: widths, banking, and the strips, lines and barriers that run its whole length (not those limited to stretches). |
| `RemoveRoad` | `road` | removes a road |
| `RenameRoad` | `road`, `to` | renames a road and every reference to it |
| `SetRoad` | `road`, `closed?`, `crown?`, `surface?`, `material?`, `resolution?` | sets the given properties |
| `SetMainRoad` | `road` | makes that road the main road; the start line moves to its first node and the sectors split it evenly |
| `PutRoad` | `road` | adds a road as given (every field of `project.ron`'s roads), or replaces the one with the same name |
| `SplitLine` | `line`, `at`, `to?` | cuts a road or spline at node `at`: an open line becomes two, itself up to the node and `to` from it on; a loop opens there. What lies along a road stays where it was |
| `JoinLines` | `line`, `with` | joins open line `with` onto the end of open line `line`, turning either round so that their nearest ends meet; strips, lines and barriers of the same name run on across the join |
| `ReverseLine` | `line` | turns a road or spline round: a road's left and right, banking, and corner entries and exits swap with it |

**Nodes**

| operation | fields | what it does |
| --- | --- | --- |
| `AddNode` | `line`, `pos`, `before?` | inserts a node before node `before`, or appends one |
| `MoveNode` | `line`, `index`, `pos` | moves a node |
| `SetNodeHandles` | `line`, `index`, `mode`, `incoming`, `outgoing` | sets both offsets; `mode` is `Auto`, `Aligned` (opposite directions, independent lengths), or `Free` |
| `RemoveNode` | `line`, `index` | removes a node |
| `Subdivide` | `line`, `segments` | splits each segment (segment `i` runs from node `i` to the next) at its middle, keeping the line's shape |
| `SetNodes` | `line`, `nodes` | replaces the whole polyline, with automatic handles |
| `SetNodeRadius` | `line`, `index`, `radius` | sets a spline node's radius: its band's width or wall's height there, times its own |

**Profiles**

| operation | fields | what it does |
| --- | --- | --- |
| `SetProfile` | `road`, `curve`, `keys: [(u, value)]` | replaces a profile. `curve` is `WidthLeft`, `WidthRight`, `Width` (both sides) or `Bank`. |
| `SetKey` | `road`, `curve`, `u`, `value` | sets or adds one key |
| `SetKeyTangents` | `road`, `curve`, `u`, `slope_in`, `slope_out` | sets slopes on an existing key, in value per unit `u` |

**Strips, lines and barriers**

| operation | fields | what it does |
| --- | --- | --- |
| `PutStrip` | `road`, `side`, `strip`, `at?` | adds a strip, or replaces the one with the same name |
| `RemoveStrip` | `road`, `side`, `name` | removes a strip |
| `PutLine` / `RemoveLine` | `road`, `line` / `name` | adds, replaces or removes a painted line |
| `PutBarrier` / `RemoveBarrier` | `road`, `barrier` / `name` | adds, replaces or removes a barrier |
| `PutRow` / `RemoveRow` | `road`, `row` / `name` | adds, replaces or removes a row of a model beside the road |
| `PutMark` / `RemoveMark` | `road`, `mark: (name, at, length, from, to, material)` / `name` | paints a mark across the road (a start line, a grid slot, a pit speed limit line) |
| `FitCorners` | `road` | puts the road's strips and barriers laid round corners back round them; every change to a road does this anyway |

A strip or barrier may carry `style` (the strip or wall type it was made from) and
`corner: (part, apex, shift)`: laid round a corner (`part` is `Entry`, `Apex`, `Exit` or
`Outside`), it stays with that corner however the corners are renumbered, and is
refitted round it whenever the road changes, `shift` metres beyond its usual place. A
barrier may carry `model: (model, length?, bend?, flip?)`, a glTF model repeated along it
in place of its plain shape (its +X along the wall, +Z up, +Y towards the road); cars
still hit the plain wall. A strip's `profile` may be `Shape([(x, height)])`: any
cross-section, points at fractions across; its `keys` and `model` are described under
Strips.

**Strip and wall types**

| operation | fields | what it does |
| --- | --- | --- |
| `PutStripStyle` / `RemoveStripStyle` | `style: (name, width, profile, surface, material, fade, model?)` / `name` | adds or replaces a kind of kerb, gravel, run-off or verge; strips and splines made from it take its look and model (keeping their widths and keys) |
| `PutWallStyle` / `RemoveWallStyle` | `style: (name, height, thickness, material, model?)` / `name` | adds or replaces a kind of wall, rail, fence or tyre stack; barriers and walls made from it take its shape and model |

New projects start with kerb, flat, raised, sausage and stepped kerbs, gravel, run-off
and grass, and concrete walls, guard rails, tyre walls and catch fences.

**Splines**

| operation | fields | what it does |
| --- | --- | --- |
| `PutSpline` / `RemoveSpline` | `spline` / `name` | adds, replaces or removes a spline (kerb, wall, fence, area) |
| `RenameSpline` | `name`, `to` | renames a spline; fails if a road or spline has that name |

**Markers and terrain**

| operation | fields | what it does |
| --- | --- | --- |
| `SetMarkers` | `start?`, `sectors?`, `grid?` | sets the race markers |
| `SetPit` | `pit: Some((...))` or `None` | sets or removes the pit lane |
| `SetTerrain` | `terrain` | sets the terrain |
| `PutLandform` / `RemoveLandform` | `landform` / `name` | adds or replaces, or removes, a hill, bank, hollow or level pad of the terrain |
| `AddStroke` | `to`, `stroke: (brush, radius, strength, points, fill?, hardness?)` | adds a brush stroke: `to` is `Sculpt` (the ground's shape), `Paint(Some("sand"))` (a ground layer; `Paint(None)` the ground's own material) or `Scatter("woods")` |
| `ClearStrokes` | `of` | removes every stroke of `Sculpt`, of one layer's painting, or of a scatter |
| `PutGroundLayer` / `RemoveGroundLayer` | `layer: (name, surface, material)` / `name` | adds or replaces a painted ground layer, or removes it with its strokes |
| `PutScatter` / `RemoveScatter` | `scatter` / `name` | adds, replaces or removes a scatter of models |
| `RenameScatter` | `name`, `to` | renames a scatter; its copies stay where they are |
| `PlantCopies` | `scatter`, `plants: [(model, pos, yaw?, scale?)]` | plants copies one by one |
| `RemoveCopies` | `scatter`, `copies: [Cell((i, j)) or Placed(k)]` | takes painted copies out (they stay out) and removes planted ones |
| `SetCopies` | `scatter`, `copies: [(Cell((i, j)) or Placed(k), (model, pos, yaw, scale))]` | moves, turns, resizes or changes the model of copies: each becomes a planted copy as given (a painted one is taken out and planted after the others) |
| `SetEnvironment` | `environment` | sets the sky and the light: time, month, latitude, weather, exposure, haze |
| `SetReference` | `reference: Some((image, center, width, rotation?, height?, opacity?, visible?))` or `None` | sets or removes the image the editor shows to trace a real circuit over; not part of the track |
| `SetGeo` | `geo: Some((lon, lat))` or `None` | sets where the project's (0, 0) lies on the Earth |

**Surfaces and materials**

| operation | fields | what it does |
| --- | --- | --- |
| `PutSurface` / `RemoveSurface` | `surface` / `name` | adds, replaces or removes a surface |
| `PutMaterial` / `RemoveMaterial` | `material` / `name` | adds, replaces or removes a material |

**Props**

| operation | fields | what it does |
| --- | --- | --- |
| `PutProp` / `RemoveProp` | `prop` / `name` | places, replaces or removes a prop |
| `MoveProp` | `name`, `pos?`, `yaw?`, `scale?` | moves, turns or resizes a prop |
| `RenameProp` | `name`, `to` | renames a prop; fails if another prop has that name |

## Real circuits

`trackctl centreline <project> <file> --road <name> [--main]` lays a road along a real
circuit's centreline from a GPS track (`.gpx`), a KML line, a GeoJSON line (as
OpenStreetMap exports give) or a CSV of `x, y[, z]` metres or `lon, lat[, ele]` under a
header. Longitudes and latitudes become metres east and north of the project's place on
the Earth (the first line sets it, `SetGeo`), so later lines line up with it. The line
is smoothed as a smoothing spline would (GPS jitter does not make wobbly roads or false
corners) and thinned to the nodes a spline needs to stay within `--tolerance` metres of
it. The editor does the same from File › Import Centreline, and can lay a satellite
image or track map (PNG, JPEG, DDS) under the view to trace over, placed by a distance
measured on it or by the longitudes and latitudes of its edges (the Reference image tab;
`SetReference`).

`trackctl dem <project> <file> [--lines a,b] [--offset m] [--terrain]` puts roads' and
splines' nodes on the ground of elevation data: a GeoTIFF (`.tif`), an ESRI ASCII grid
(`.asc`) or `x y z` points (`.xyz`, `.csv`), in metres, longitudes and latitudes, or
UTM metres. With `--terrain` the file is copied into the project and the terrain away
from the roads follows it too. The editor does the same from File › Heights from
Elevation Data and the Terrain tab, where landforms are added and dragged in the view.

`trackctl kerbs <project> [--corners 1,4] [--style kerb] [--width m] [--no-entry]
[--no-apex] [--no-exit] [--outside gravel] [--outside-width m] [--wall "tyre wall"]
[--wall-offset m]` lays kerbs, gravel or run-off and a wall round the road's corners, of
the project's types. The editor's Corners tab does the same corner by corner, or on
every corner at once.

`trackctl paint <project>` paints the start/finish line and the grid slots' lines.

`trackctl garages <project> <model> [--offset m]` puts a garage behind each pit box, and
`trackctl boards <project> <model> [--distances 100,200,300] [--least deg]` distance
boards before each corner on its outside, both as rows. The editor offers them in Race
markers › Pit lane and the Corners tab; a road's Rows tab lays any model beside it, and
its stretches and distance drag in the view.

`trackctl pitlane <project> [--from u --to u] [--left] [--gap m] [--width m] [--boxes n]`
lays a pit lane road beside a stretch of the main road (by default round the start line):
it leaves the track, runs parallel to it past the boxes, and rejoins it, with white
edge lines and speed limit lines across it. The editor offers the same in Race markers ›
Pit lane.

## Working on a track

1. Run `trackctl info` for the facts and `trackctl preview` for the picture.
2. Change the project with `trackctl apply`, or edit `project.ron` directly.
3. Run `trackctl check --lap` to confirm the track still drives. Once the steady test
   lap gets round, a GT3 drives a lap at race pace round the racing line (85 % of the
   speeds it allows): the report gives its lap time and top speed, and where it went off
   or left the ground (a crest or kerb too sharp). The editor's bake shows that lap's
   path coloured by speed, and View › Replay Test Lap replays it;
   `cargo run -p open-racing-track-project --example pace_lap -- <project>` prints it
   second by second.

Warnings point out a radius under 10 m, a grade over 20 %, a road crossing itself or
another road on the level, and roads whose surfaces overlap away from where one starts
or ends on the other.

`cargo run -p open-racing-track-project --example build_time -- <project>` shows how
long each step of a rebuild takes, as the editor rebuilds on every edit.

The editor reloads `project.ron` when it changes on disk, so an agent's edits show up live.
