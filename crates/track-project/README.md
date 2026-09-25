# Track projects

A track project is the editable source of a circuit, kept in a directory:

- `project.ron`: everything about the track. It is plain text, and names tie its parts together.
- Texture files that the materials name, if there are any. The built-in textures need no files.

`open-racing-editor` edits projects in 3D. `open-racing-trackctl` edits them from the command line.
Both apply the same operations and check the result the same way.
Baking turns a project into the track package that the simulator reads, under `content/tracks/<name>/`.

```bash
trackctl new my-track                   # content/track-src/my-track/, with a small circuit
trackctl info my-track [--json]         # roads, nodes and their distances, radii, grades, markers, warnings
trackctl apply my-track ops.ron         # or ops.json, or - for stdin; all or nothing
trackctl preview my-track               # plan view: my-track/preview.png
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
  - Keys and stretches keep their places when nodes move.
  - `trackctl info` prints the distance `s` at each node, so you can convert between `u` and `s`.
- **Names.** Roads, surfaces and materials refer to each other by name. Strips, lines and barriers are named within their road.

## project.ron

```ron
(
    format: 1,
    name: "my-track",                // name of the baked package
    main_road: "circuit",            // the closed road raced on
    roads: [(
        name: "circuit",
        closed: true,
        nodes: [(pos: (0, 0, 0)), (pos: (250, 0, 2), handle: Some((60, 0, 0))), ...],
        //  handle: outgoing Bézier handle offset (the incoming one mirrors it);
        //  left out, the road is smoothed through the neighbouring nodes.
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
        resolution: 2,               // m between cross-sections
    )],
    splines: [                       // kerbs, walls and fences on their own lines
        (name: "T1 sausage", closed: false, drape: true,
         nodes: [(pos: (410, 12, 0)), (pos: (440, 40, 0))], resolution: 0.5,
         shape: Band(width: 0.6, align: Center, profile: Crown(0.12),
                     surface: "kerb", material: "kerb", lift: 0.01)),
        (name: "tyre wall", closed: false, drape: true,
         nodes: [(pos: (500, 100, 0)), (pos: (505, 160, 0))], resolution: 2,
         shape: Wall(height: 1, thickness: 0.8, material: "concrete", collide: true)),
    ],
    markers: (
        start: 0.5,                  // start/finish line on the main road (u)
        sectors: [3.0, 6.5],         // sector boundaries (u)
        grid: (count: 12, spacing: 8, stagger: 2.5, behind: 10, pole: Left),
        pit: Some((road: "pit", speed_limit: 22.2, boxes: [0.5, 0.6],
                   box_side: Right, box_offset: 4)),
    ),
    terrain: (enabled: true, surface: "grass", material: "grass", margin: 200, cell: 8),
    surfaces: [(name: "asphalt", props: (kind: Asphalt, grip: 1.0, drag: 0.0)), ...],
    materials: [(name: "asphalt", color: (1, 1, 1), texture: Builtin(Asphalt),
                 tile: (4, 4), roughness: 0.8, reflectance: 0.5), ...],
)
```

**Profiles.** `width_left`, `width_right` and `bank` are keyed along the road and eased between keys. A single key makes the value constant.

**Strips.** A strip is a band beside the road, such as a kerb, run-off, gravel, grass or a verge.

- `ranges` limits a strip to stretches of the road. An empty list means everywhere.
- On a closed road, `from > to` wraps through the start.
- `fade` narrows and flattens the strip over that many metres at the ends of each stretch.
- The strip's profile takes one of these forms:
  - `Flat` continues the plane of whatever lies inside it.
  - `Crown(h)` rises to `h` in the middle, like a kerb.
  - `Slope(d)` falls by `d` to its outer edge.

**Splines.** A spline is a kerb, wall or fence along its own line, placed anywhere rather than beside a road.

- Its name must differ from every road's and spline's, and the node operations address it by that name as `line`.
- `drape: true` lays it on whatever is under the line (roads, then terrain) and ignores the nodes' heights. Otherwise it follows the nodes.
- The `Band` shape is drivable: it takes a `width` and a profile. It lies centred on the line, or to its `Left` or `Right`. `lift` raises it above what is under it.
- The `Wall` shape stands on the line. A `thickness` of 0 gives a thin rail or fence, which wants a double-sided material. With `collide: false`, cars pass through it.

**Surfaces.** A surface's `kind` is one of `Asphalt`, `Kerb`, `Runoff`, `Grass`, `Turf`, `Gravel` or `Dirt`.

- `grip` multiplies the tyres' friction.
- `drag` adds rolling resistance.
- `Asphalt` and `Kerb` count as track. Everything else is off track.

**Built-in textures.** These are `Asphalt`, `Kerb` (red and white blocks along the road), `Grass`, `Gravel`, `Concrete`, `Armco`, `Paint` and `Dirt`. The alternative is `File("textures/x.png")`, a path relative to the project.

- `tile` gives the metres covered by one repetition of the texture, across the road and then along it.

**Pit lane.** The pit lane is a separate, open road. It starts where it leaves the track and ends where it rejoins.

**Terrain.** The terrain fills in around the roads. It lies just under them and meets their outer edges.

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
| `AddRoad` | `name`, `closed`, `nodes: [(x, y, z)]`, `like?` | adds a road. With `like`, it copies that road's cross-section, running along the whole road. |
| `RemoveRoad` | `road` | removes a road |
| `RenameRoad` | `road`, `to` | renames a road and every reference to it |
| `SetRoad` | `road`, `closed?`, `crown?`, `surface?`, `material?`, `resolution?` | sets the given properties |
| `SetMainRoad` | `road` | makes that road the main road |

**Nodes**

| operation | fields | what it does |
| --- | --- | --- |
| `AddNode` | `line`, `pos`, `before?` | inserts a node before node `before`, or appends one |
| `MoveNode` | `line`, `index`, `pos` | moves a node |
| `SetHandle` | `line`, `index`, `handle` | sets a handle: `Some((x, y, z))`, or `None` for an automatic one |
| `RemoveNode` | `line`, `index` | removes a node |
| `SetNodes` | `line`, `nodes` | replaces the whole polyline, with automatic handles |

**Profiles**

| operation | fields | what it does |
| --- | --- | --- |
| `SetProfile` | `road`, `curve`, `keys: [(u, value)]` | replaces a profile. `curve` is `WidthLeft`, `WidthRight`, `Width` (both sides) or `Bank`. |
| `SetKey` | `road`, `curve`, `u`, `value` | sets or adds one key |

**Strips, lines and barriers**

| operation | fields | what it does |
| --- | --- | --- |
| `PutStrip` | `road`, `side`, `strip`, `at?` | adds a strip, or replaces the one with the same name |
| `RemoveStrip` | `road`, `side`, `name` | removes a strip |
| `PutLine` / `RemoveLine` | `road`, `line` / `name` | adds, replaces or removes a painted line |
| `PutBarrier` / `RemoveBarrier` | `road`, `barrier` / `name` | adds, replaces or removes a barrier |

**Splines**

| operation | fields | what it does |
| --- | --- | --- |
| `PutSpline` / `RemoveSpline` | `spline` / `name` | adds, replaces or removes a spline (kerb, wall, fence) |

**Markers and terrain**

| operation | fields | what it does |
| --- | --- | --- |
| `SetMarkers` | `start?`, `sectors?`, `grid?` | sets the race markers |
| `SetPit` | `pit: Some((...))` or `None` | sets or removes the pit lane |
| `SetTerrain` | `terrain` | sets the terrain |

**Surfaces and materials**

| operation | fields | what it does |
| --- | --- | --- |
| `PutSurface` / `RemoveSurface` | `surface` / `name` | adds, replaces or removes a surface |
| `PutMaterial` / `RemoveMaterial` | `material` / `name` | adds, replaces or removes a material |

## Working on a track

1. Run `trackctl info` for the facts and `trackctl preview` for the picture.
2. Change the project with `trackctl apply`, or edit `project.ron` directly.
3. Run `trackctl check --lap` to confirm the track still drives.

Warnings point out a radius under 10 m, a grade over 20 %, and a road crossing itself on the level.

The editor reloads `project.ron` when it changes on disk, so an agent's edits show up live.
