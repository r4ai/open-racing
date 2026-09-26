//! Heights of the ground from elevation data, for giving a real circuit's roads and the
//! land round them the rise and fall the place has: a GeoTIFF (.tif, as most elevation
//! services give), an ESRI ASCII grid (.asc) or a list of points (.xyz, .csv or .txt:
//! x y z per line). Coordinates are the project's metres, longitudes and latitudes, or
//! UTM metres (GeoTIFFs), the last two once the project knows where it lies on the
//! Earth.

use std::path::Path;

use glam::{DVec2, DVec3};

use crate::Error;
use crate::geo::{Geo, looks_geographic};
use crate::ops::Op;
use crate::project::Project;

/// What a grid's coordinates are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Crs {
    /// The project's own metres.
    Project,
    /// Longitudes and latitudes (x, y), about the project's place on the Earth.
    Degrees(Geo),
    /// Metres east and north in a UTM zone.
    Utm { zone: u8, south: bool, geo: Geo },
}

/// Heights to look up at points of the project.
pub enum Heights {
    /// A regular grid: its south-west cell's middle, its cells' size across and up,
    /// columns, rows (north to south, as files have them) and what its coordinates are.
    Grid {
        origin: DVec2,
        cell: DVec2,
        cols: usize,
        rows: usize,
        values: Vec<Option<f64>>,
        crs: Crs,
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

/// Reads an elevation data file of any of the formats.
pub fn read_file(path: &Path, geo: Option<Geo>) -> Result<Heights, Error> {
    let name = path
        .file_name()
        .map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let ext = name
        .rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_ascii_lowercase());
    if ext == "tif" || ext == "tiff" {
        let bytes = std::fs::read(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
        return geotiff(&bytes, geo).map_err(|e| Error::Invalid(format!("{name}: {e}")));
    }
    let src = std::fs::read_to_string(path).map_err(|e| Error::Io(path.to_path_buf(), e))?;
    read(&name, &src, geo)
}

/// Reads elevation data given as text; `name` (the file's name) tells the format by its
/// extension. Degrees need `geo`, the project's place on the Earth.
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
        cell: DVec2::splat(cell),
        cols,
        rows,
        values,
        crs: match geo {
            Some(g) if degrees => Crs::Degrees(g),
            _ => Crs::Project,
        },
    })
}

/// GeoTIFF keys this reads: the model type, the raster type, and the projected
/// coordinate system's EPSG code.
const MODEL_TYPE: u16 = 1024;
const RASTER_TYPE: u16 = 1025;
const PROJECTED: u16 = 3072;

/// A GeoTIFF's first band as a grid of heights: in degrees (any geographic system), UTM
/// (WGS 84 zones, EPSG 32601–32660 and 32701–32760) or, without keys, the project's
/// metres.
fn geotiff(bytes: &[u8], geo: Option<Geo>) -> Result<Heights, String> {
    use tiff::decoder::{Decoder, DecodingResult};
    use tiff::tags::Tag;
    let err = |e: tiff::TiffError| e.to_string();
    let mut d = Decoder::new(std::io::Cursor::new(bytes)).map_err(err)?;
    let (cols, rows) = d.dimensions().map_err(err)?;
    let (cols, rows) = (cols as usize, rows as usize);
    let scale = d
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .map_err(|_| "no pixel scale: not a GeoTIFF".to_string())?;
    let tie = d
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .map_err(|_| "no tie point: not a GeoTIFF".to_string())?;
    if scale.len() < 2 || tie.len() < 6 || cols < 2 || rows < 2 {
        return Err("a GeoTIFF needs its pixel scale, a tie point and 2 × 2 pixels".into());
    }
    let keys: Vec<u16> = d
        .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
        .unwrap_or_default();
    let key = |id: u16| {
        keys.get(4..)?
            .chunks_exact(4)
            .find(|k| k[0] == id && k[1] == 0)
            .map(|k| k[3])
    };
    let nodata: Option<f64> = d
        .get_tag_ascii_string(Tag::GdalNodata)
        .ok()
        .and_then(|s| s.trim().trim_end_matches('\0').parse().ok());
    let values: Vec<f64> = match d.read_image().map_err(err)? {
        DecodingResult::U8(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::U32(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::U64(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::F16(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::F32(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::F64(v) => v,
        DecodingResult::I8(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::I16(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::I32(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::I64(v) => v.into_iter().map(|x| x as f64).collect(),
    };
    // More than one band: the first.
    let bands = values.len() / (cols * rows).max(1);
    if bands == 0 {
        return Err("fewer pixels than the image's size".into());
    }
    let values: Vec<Option<f64>> = values
        .into_iter()
        .step_by(bands)
        .map(|v| Some(v).filter(|v| v.is_finite() && nodata.is_none_or(|n| *v != n)))
        .collect();
    let cell = DVec2::new(scale[0], scale[1]);
    // The tie point puts pixel (i, j) at (x, y): the pixel's corner, or with
    // PixelIsPoint its middle.
    let corner = key(RASTER_TYPE) != Some(2);
    let (i, j) = (tie[0], tie[1]);
    let at = DVec2::new(tie[3], tie[4]);
    let top_left = at - DVec2::new(i * cell.x, -j * cell.y)
        + if corner {
            DVec2::new(0.5 * cell.x, -0.5 * cell.y)
        } else {
            DVec2::ZERO
        };
    let origin = DVec2::new(top_left.x, top_left.y - (rows - 1) as f64 * cell.y);
    let crs = match key(MODEL_TYPE) {
        Some(2) => Crs::Degrees(geo.ok_or(
            "longitudes and latitudes, but the project does not know where it lies on the Earth yet: import a GPS centreline first",
        )?),
        Some(1) => {
            let code = key(PROJECTED).unwrap_or(0);
            let (zone, south) = match code {
                32601..=32660 => (code - 32600, false),
                32701..=32760 => (code - 32700, true),
                _ => {
                    return Err(format!(
                        "EPSG:{code} is not read: export the data in WGS 84 (longitudes and latitudes) or a WGS 84 UTM zone"
                    ));
                }
            };
            let geo = geo.ok_or(
                "UTM metres, but the project does not know where it lies on the Earth yet: import a GPS centreline first",
            )?;
            Crs::Utm {
                zone: zone as u8,
                south,
                geo,
            }
        }
        _ if cell.x < 0.1 && looks_geographic([origin]) => Crs::Degrees(geo.ok_or(
            "longitudes and latitudes, but the project does not know where it lies on the Earth yet",
        )?),
        _ => Crs::Project,
    };
    Ok(Heights::Grid {
        origin,
        cell,
        cols,
        rows,
        values,
        crs,
    })
}

/// WGS 84's semi-major axis, m, and flattening.
const WGS84_A: f64 = 6_378_137.0;
const WGS84_F: f64 = 1.0 / 298.257_223_563;

/// A longitude and latitude as metres east and north in UTM zone `zone` (Krüger's
/// series, to well within a millimetre in the zone).
pub fn utm(lon: f64, lat: f64, zone: u8, south: bool) -> DVec2 {
    let (a, f) = (WGS84_A, WGS84_F);
    let n = f / (2.0 - f);
    let big_a = a / (1.0 + n) * (1.0 + n * n / 4.0 + n.powi(4) / 64.0);
    let alpha = [
        n / 2.0 - 2.0 * n * n / 3.0 + 5.0 * n.powi(3) / 16.0,
        13.0 * n * n / 48.0 - 3.0 * n.powi(3) / 5.0,
        61.0 * n.powi(3) / 240.0,
    ];
    let lon0 = (zone as f64 * 6.0 - 183.0).to_radians();
    let (phi, lam) = (lat.to_radians(), lon.to_radians() - lon0);
    let e = (f * (2.0 - f)).sqrt();
    let t = (phi.sin().atanh() - e * (e * phi.sin()).atanh()).sinh();
    let xi = t.atan2(lam.cos());
    let eta = (lam.sin() / (1.0 + t * t).sqrt()).atanh();
    let (mut x, mut y) = (eta, xi);
    for (j, al) in alpha.iter().enumerate() {
        let k = 2.0 * (j + 1) as f64;
        x += al * (k * xi).cos() * (k * eta).sinh();
        y += al * (k * xi).sin() * (k * eta).cosh();
    }
    let k0 = 0.9996;
    DVec2::new(
        500_000.0 + k0 * big_a * x,
        k0 * big_a * y + if south { 10_000_000.0 } else { 0.0 },
    )
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
                crs,
            } => {
                let q = match *crs {
                    Crs::Project => p,
                    Crs::Degrees(g) => {
                        let (lon, lat) = g.to_geo(p);
                        DVec2::new(lon, lat)
                    }
                    Crs::Utm { zone, south, geo } => {
                        let (lon, lat) = geo.to_geo(p);
                        utm(lon, lat, zone, south)
                    }
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
    fn utm_puts_the_central_meridian_at_500_km() {
        let p = utm(135.0, 0.0, 53, false);
        assert!((p - DVec2::new(500_000.0, 0.0)).length() < 1e-6, "{p}");
        // 45° north on zone 31's meridian: the meridian's length there, scaled.
        let p = utm(3.0, 45.0, 31, false);
        assert!((p.x - 500_000.0).abs() < 1e-6);
        assert!((p.y - 0.9996 * 4_984_944.378).abs() < 0.5, "{p}");
        // South of the equator, from 10 000 km.
        assert!(utm(135.0, -1.0, 53, true).y < 10_000_000.0);
    }

    /// A little GeoTIFF: 3 × 3 heights rising 1 m per pixel east, in UTM zone 53
    /// metres, 10 m pixels, its top-left corner at `corner`.
    fn tif(corner: DVec2) -> Vec<u8> {
        use tiff::encoder::{TiffEncoder, colortype::Gray32Float};
        use tiff::tags::Tag;
        let mut out = std::io::Cursor::new(Vec::new());
        let mut enc = TiffEncoder::new(&mut out).unwrap();
        let mut img = enc.new_image::<Gray32Float>(3, 3).unwrap();
        let e = img.encoder();
        e.write_tag(Tag::ModelPixelScaleTag, &[10.0f64, 10.0, 0.0][..])
            .unwrap();
        e.write_tag(
            Tag::ModelTiepointTag,
            &[0.0f64, 0.0, 0.0, corner.x, corner.y, 0.0][..],
        )
        .unwrap();
        // Version 1.1.0, 2 keys: projected, UTM 53 N.
        e.write_tag(
            Tag::GeoKeyDirectoryTag,
            &[1u16, 1, 0, 2, MODEL_TYPE, 0, 1, 1, PROJECTED, 0, 1, 32653][..],
        )
        .unwrap();
        img.write_data(&[0.0f32, 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1.0, 2.0])
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn a_geotiff_in_utm_gives_heights_round_the_project() {
        let geo = Geo {
            lon: 136.54,
            lat: 34.84,
        };
        let here = utm(geo.lon, geo.lat, 53, false);
        // The first pixel's middle 5 m west and north of the project's origin.
        let bytes = tif(here + DVec2::new(-10.0, 10.0));
        assert!(geotiff(&bytes, None).is_err(), "UTM needs the origin");
        let h = geotiff(&bytes, Some(geo)).unwrap();
        // The origin is a quarter of the way across the first two columns.
        let v = h.at(DVec2::ZERO).unwrap();
        assert!((v - 0.5).abs() < 0.01, "{v}");
        // 15 m east and south of the first pixel: UTM's grid north is turned 0.9° from
        // true north here (1.5° from the zone's middle), a quarter of a metre across.
        let v = h.at(DVec2::new(10.0, -10.0)).unwrap();
        assert!((v - 1.5).abs() < 0.03, "{v}");
        assert!(h.at(DVec2::new(100.0, 0.0)).is_none());
    }

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
