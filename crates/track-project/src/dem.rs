//! Heights of the ground from elevation data, for giving a real circuit's roads the
//! rise and fall the place has: an ESRI ASCII grid (.asc, as most elevation services
//! export) or a list of points (.xyz, .csv or .txt: x y z per line). Coordinates are
//! the project's metres, or longitudes and latitudes when the project knows where it
//! lies on the Earth.

use glam::{DVec2, DVec3};

use crate::Error;
use crate::geo::{Geo, looks_geographic};
use crate::ops::Op;
use crate::project::Project;

/// Heights to look up at points of the project.
pub enum Heights {
    /// A regular grid: its south-west corner's middle, cell size, columns, rows (north
    /// to south, as the file has them) and whether its coordinates are degrees.
    Grid {
        origin: DVec2,
        cell: f64,
        cols: usize,
        rows: usize,
        values: Vec<Option<f64>>,
        geo: Option<Geo>,
    },
    /// Scattered points in the project's metres, in buckets.
    Points {
        points: Vec<DVec3>,
        lo: DVec2,
        bucket: f64,
        size: (usize, usize),
        buckets: Vec<Vec<usize>>,
    },
}

/// Reads elevation data; `name` (the file's name) tells the format by its extension.
/// Degrees need `geo`, the project's place on the Earth.
pub fn read(name: &str, src: &str, geo: Option<Geo>) -> Result<Heights, Error> {
    let ext = name
        .rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_ascii_lowercase());
    let fail = |what: String| Error::Invalid(format!("{name}: {what}"));
    if ext == "asc" {
        return grid(src, geo).map_err(fail);
    }
    let mut points = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let v: Vec<f64> = line
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect();
        match v[..] {
            [x, y, z, ..] => points.push(DVec3::new(x, y, z)),
            // A header row.
            _ if i == 0 => {}
            _ => return Err(fail(format!("line {}: expected x y z", i + 1))),
        }
    }
    if points.is_empty() {
        return Err(fail("no points".into()));
    }
    if looks_geographic(points.iter().map(|p| p.truncate())) {
        let Some(g) = geo else {
            return Err(fail(
                "longitudes and latitudes, but the project does not know where it lies on the Earth yet: import a GPS centreline first".into(),
            ));
        };
        for p in &mut points {
            *p = g.to_local(p.x, p.y).extend(p.z);
        }
    }
    Ok(scattered(points))
}

fn grid(src: &str, geo: Option<Geo>) -> Result<Heights, String> {
    let mut header = std::collections::HashMap::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.peek() {
        let mut parts = line.split_whitespace();
        let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
            lines.next();
            continue;
        };
        if key.parse::<f64>().is_ok() {
            break;
        }
        let value: f64 = value
            .parse()
            .map_err(|_| format!("header {key}: not a number"))?;
        header.insert(key.to_ascii_lowercase(), value);
        lines.next();
    }
    let get = |k: &str| header.get(k).copied();
    let (Some(cols), Some(rows), Some(cell)) = (get("ncols"), get("nrows"), get("cellsize")) else {
        return Err("not an ASCII grid (ncols, nrows, cellsize)".into());
    };
    let (cols, rows) = (cols as usize, rows as usize);
    // The middle of the south-west cell.
    let origin = match (
        get("xllcenter"),
        get("yllcenter"),
        get("xllcorner"),
        get("yllcorner"),
    ) {
        (Some(x), Some(y), ..) => DVec2::new(x, y),
        (_, _, Some(x), Some(y)) => DVec2::new(x, y) + 0.5 * cell,
        _ => return Err("no xllcorner/yllcorner".into()),
    };
    let nodata = get("nodata_value");
    let values: Vec<Option<f64>> = lines
        .flat_map(|l| l.split_whitespace())
        .map(|s| {
            s.parse::<f64>()
                .ok()
                .filter(|v| nodata.is_none_or(|n| *v != n))
        })
        .collect();
    if values.len() < cols * rows || cols < 2 || rows < 2 {
        return Err(format!(
            "{} values for a grid of {cols} × {rows}",
            values.len()
        ));
    }
    let degrees = cell < 0.1 && looks_geographic([origin]);
    if degrees && geo.is_none() {
        return Err("a grid in longitudes and latitudes, but the project does not know where it lies on the Earth yet: import a GPS centreline first".into());
    }
    Ok(Heights::Grid {
        origin,
        cell,
        cols,
        rows,
        values,
        geo: geo.filter(|_| degrees),
    })
}

/// Points put in buckets for looking up those near a place.
fn scattered(points: Vec<DVec3>) -> Heights {
    let (mut lo, mut hi) = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
    for p in &points {
        lo = lo.min(p.truncate());
        hi = hi.max(p.truncate());
    }
    // About a few points per bucket.
    let area = ((hi.x - lo.x) * (hi.y - lo.y)).max(1.0);
    let bucket = (area / points.len() as f64 * 4.0).sqrt().max(1.0);
    let size = (
        ((hi.x - lo.x) / bucket) as usize + 1,
        ((hi.y - lo.y) / bucket) as usize + 1,
    );
    let mut buckets = vec![Vec::new(); size.0 * size.1];
    for (i, p) in points.iter().enumerate() {
        let c = ((p.truncate() - lo) / bucket).as_uvec2();
        buckets[c.y as usize * size.0 + c.x as usize].push(i);
    }
    Heights::Points {
        points,
        lo,
        bucket,
        size,
        buckets,
    }
}

impl Heights {
    /// The ground's height at a point of the project, if the data covers it.
    pub fn at(&self, p: DVec2) -> Option<f64> {
        match self {
            Heights::Grid {
                origin,
                cell,
                cols,
                rows,
                values,
                geo,
            } => {
                let q = match geo {
                    Some(g) => {
                        let (lon, lat) = g.to_geo(p);
                        DVec2::new(lon, lat)
                    }
                    None => p,
                };
                // Column from the west, row from the south.
                let f = (q - *origin) / *cell;
                if f.x < 0.0 || f.y < 0.0 || f.x > (cols - 1) as f64 || f.y > (rows - 1) as f64 {
                    return None;
                }
                let (i, j) = ((f.x as usize).min(cols - 2), (f.y as usize).min(rows - 2));
                let (tx, ty) = (f.x - i as f64, f.y - j as f64);
                // Rows are stored north to south.
                let v = |i: usize, j: usize| values[(rows - 1 - j) * cols + i];
                let (a, b, c, d) = (v(i, j)?, v(i + 1, j)?, v(i, j + 1)?, v(i + 1, j + 1)?);
                Some(
                    a * (1.0 - tx) * (1.0 - ty)
                        + b * tx * (1.0 - ty)
                        + c * (1.0 - tx) * ty
                        + d * tx * ty,
                )
            }
            Heights::Points {
                points,
                lo,
                bucket,
                size,
                buckets,
            } => {
                // The points within a couple of buckets, weighted by nearness.
                let c = ((p - *lo) / *bucket).floor();
                let (cx, cy) = (c.x as isize, c.y as isize);
                let (mut sum, mut total) = (0.0, 0.0);
                for y in cy - 2..=cy + 2 {
                    for x in cx - 2..=cx + 2 {
                        if x < 0 || y < 0 || x >= size.0 as isize || y >= size.1 as isize {
                            continue;
                        }
                        for &i in &buckets[y as usize * size.0 + x as usize] {
                            let d2 = points[i].truncate().distance_squared(p);
                            if d2 < 1e-6 {
                                return Some(points[i].z);
                            }
                            if d2 <= (2.0 * bucket).powi(2) {
                                sum += points[i].z / d2;
                                total += 1.0 / d2;
                            }
                        }
                    }
                }
                (total > 0.0).then(|| sum / total)
            }
        }
    }
}

/// The operations that put the nodes of the roads and splines named in `lines` (all of
/// them when empty) at the ground's height, raised by `offset` m. Nodes the data does
/// not cover stay as they are; the count of those moved comes back too.
pub fn node_ops(
    project: &Project,
    heights: &Heights,
    lines: &[String],
    offset: f64,
) -> (Vec<Op>, usize) {
    let mut ops = Vec::new();
    let names = project
        .roads
        .iter()
        .map(|r| (&r.name, &r.nodes))
        .chain(project.splines.iter().map(|s| (&s.name, &s.nodes)));
    for (name, nodes) in names {
        if !lines.is_empty() && !lines.contains(name) {
            continue;
        }
        for (index, n) in nodes.iter().enumerate() {
            if let Some(h) = heights.at(n.pos.truncate()) {
                ops.push(Op::MoveNode {
                    line: name.clone(),
                    index,
                    pos: n.pos.with_z(h + offset),
                });
            }
        }
    }
    let n = ops.len();
    (ops, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grids_and_points_give_heights_in_metres_or_degrees() {
        // A 3 × 3 grid rising 1 m per 10 m east, from (0, 0).
        let asc = "ncols 3\nnrows 3\nxllcorner -5\nyllcorner -5\ncellsize 10\nNODATA_value -9999\n0 1 2\n0 1 2\n0 1 2\n";
        let h = read("ground.asc", asc, None).unwrap();
        assert!((h.at(DVec2::new(5.0, 10.0)).unwrap() - 0.5).abs() < 1e-9);
        assert!(h.at(DVec2::new(50.0, 0.0)).is_none());
        // Points in degrees round the project's origin.
        let geo = Geo {
            lon: 136.54,
            lat: 34.84,
        };
        let xyz =
            "lon,lat,ele\n136.54,34.84,40\n136.541,34.84,42\n136.54,34.841,44\n136.541,34.841,46\n";
        assert!(
            read("ground.csv", xyz, None).is_err(),
            "degrees need the origin"
        );
        let h = read("ground.csv", xyz, Some(geo)).unwrap();
        assert_eq!(h.at(DVec2::ZERO), Some(40.0));
        let mid = geo.to_local(136.5405, 34.8405);
        let v = h.at(mid).unwrap();
        assert!((v - 43.0).abs() < 0.5, "{v}");

        // Nodes put on the ground.
        let mut p = Project::new("dem");
        p.geo = Some(geo);
        let flat = "0 0 5\n1000 0 5\n0 1000 5\n-1000 0 5\n0 -1000 5\n1000 1000 5\n-1000 -1000 5\n1000 -1000 5\n-1000 1000 5\n";
        let h = read("flat.xyz", flat, None).unwrap();
        let (ops, n) = node_ops(&p, &h, &[], 0.5);
        assert!(n > 0);
        crate::ops::apply_all(&mut p, &ops).unwrap();
        assert!(
            p.roads[0]
                .nodes
                .iter()
                .all(|n| (n.pos.z - 5.5).abs() < 1e-9)
        );
    }
}
