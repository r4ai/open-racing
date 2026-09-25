//! Reading a real circuit's centreline from survey or map data, for laying a road along
//! it: a GPS track (GPX), a line drawn in Google Earth (KML), a GeoJSON line (as
//! OpenStreetMap exports give), or a CSV of points.
//!
//! Longitudes and latitudes become metres east and north of the line's middle. The line
//! is thinned to the points a spline through them needs to stay within a tolerance of
//! it, and a line that ends where it starts is a closed loop.

use glam::{DVec2, DVec3};

use crate::Error;
use crate::ops::Op;
use crate::project::Project;

/// A centreline read from a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Centreline {
    /// Metres east, north and up.
    pub points: Vec<DVec3>,
    /// The line ends where it begins.
    pub closed: bool,
    /// The file gave longitudes and latitudes, and where they were measured from.
    pub origin: Option<(f64, f64)>,
}

/// Mean radius of the Earth, m.
const EARTH: f64 = 6_371_000.0;

/// Reads a centreline; `name` (the file's name) tells the format by its extension.
pub fn read(name: &str, src: &str) -> Result<Centreline, Error> {
    let ext = name
        .rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_ascii_lowercase());
    let (points, geographic) = match ext.as_str() {
        "gpx" => (gpx(src), true),
        "kml" => (kml(src), true),
        "geojson" | "json" => (geojson(src)?, true),
        "csv" | "txt" => csv(src)?,
        _ => {
            return Err(Error::Invalid(format!(
                "{name}: not a centreline file (.gpx, .kml, .geojson, .csv)"
            )));
        }
    };
    if points.len() < 3 {
        return Err(Error::Invalid(format!(
            "{name}: found {} points, a centreline needs at least 3",
            points.len()
        )));
    }
    let (mut points, origin) = if geographic {
        let (pts, origin) = to_metres(&points);
        (pts, Some(origin))
    } else {
        (points, None)
    };
    // Drop repeated points; a last point back at the first closes the loop.
    points.dedup_by(|a, b| a.truncate().distance(b.truncate()) < 0.05);
    let closed = points.len() > 3
        && points[0]
            .truncate()
            .distance(points[points.len() - 1].truncate())
            < 5.0;
    if closed {
        points.pop();
    }
    Ok(Centreline {
        points,
        closed,
        origin,
    })
}

/// (longitude, latitude, height) to metres from their middle, by an equirectangular
/// projection: exact enough over the few kilometres a circuit spans.
fn to_metres(points: &[DVec3]) -> (Vec<DVec3>, (f64, f64)) {
    let (lo, hi) = points.iter().fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(lo, hi), p| (lo.min(p.truncate()), hi.max(p.truncate())),
    );
    let mid = 0.5 * (lo + hi);
    let k = EARTH * std::f64::consts::PI / 180.0;
    let east = k * mid.y.to_radians().cos();
    let pts = points
        .iter()
        .map(|p| DVec3::new((p.x - mid.x) * east, (p.y - mid.y) * k, p.z))
        .collect();
    (pts, (mid.x, mid.y))
}

/// The value of attribute `name` in an XML tag's text.
fn attr(tag: &str, name: &str) -> Option<f64> {
    let at = tag.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];
    let quote = rest.chars().next()?;
    let rest = &rest[1..];
    rest[..rest.find(quote)?].trim().parse().ok()
}

/// Track and route points of a GPX file, as (longitude, latitude, elevation).
fn gpx(src: &str) -> Vec<DVec3> {
    let mut out = Vec::new();
    for tag in ["<trkpt", "<rtept"] {
        let starts: Vec<usize> = src.match_indices(tag).map(|(i, _)| i).collect();
        for (k, &i) in starts.iter().enumerate() {
            // This point's tag and body, up to where the next point starts.
            let point = &src[i..starts.get(k + 1).copied().unwrap_or(src.len())];
            let Some(end) = point.find('>') else { continue };
            let (head, body) = point.split_at(end);
            let (Some(lat), Some(lon)) = (attr(head, "lat"), attr(head, "lon")) else {
                continue;
            };
            let ele = body
                .find("<ele>")
                .and_then(|a| {
                    let v = &body[a + 5..];
                    v[..v.find('<')?].trim().parse().ok()
                })
                .unwrap_or(0.0);
            out.push(DVec3::new(lon, lat, ele));
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// The first `<coordinates>` list of a KML file: "lon,lat[,alt]" separated by spaces.
fn kml(src: &str) -> Vec<DVec3> {
    let Some(a) = src.find("<coordinates>") else {
        return vec![];
    };
    let rest = &src[a + "<coordinates>".len()..];
    let list = &rest[..rest.find("</coordinates>").unwrap_or(rest.len())];
    list.split_whitespace()
        .filter_map(|t| {
            let v: Vec<f64> = t.split(',').filter_map(|x| x.parse().ok()).collect();
            (v.len() >= 2).then(|| DVec3::new(v[0], v[1], v.get(2).copied().unwrap_or(0.0)))
        })
        .collect()
}

/// The longest line of a GeoJSON file: a LineString, or a Polygon's outer ring.
fn geojson(src: &str) -> Result<Vec<DVec3>, Error> {
    let v: serde_json::Value =
        serde_json::from_str(src).map_err(|e| Error::Invalid(format!("GeoJSON: {e}")))?;
    let mut lines = Vec::new();
    collect_lines(&v, &mut lines);
    Ok(lines.into_iter().max_by_key(Vec::len).unwrap_or_default())
}

fn collect_lines(v: &serde_json::Value, out: &mut Vec<Vec<DVec3>>) {
    use serde_json::Value;
    let point = |p: &Value| -> Option<DVec3> {
        let a = p.as_array()?;
        Some(DVec3::new(
            a.first()?.as_f64()?,
            a.get(1)?.as_f64()?,
            a.get(2).and_then(Value::as_f64).unwrap_or(0.0),
        ))
    };
    let line = |l: &Value| -> Vec<DVec3> {
        l.as_array()
            .map(|a| a.iter().filter_map(point).collect())
            .unwrap_or_default()
    };
    match v {
        Value::Object(o) => {
            let kind = o.get("type").and_then(Value::as_str).unwrap_or("");
            let coords = o.get("coordinates");
            match (kind, coords) {
                ("LineString", Some(c)) => out.push(line(c)),
                ("MultiLineString" | "Polygon", Some(Value::Array(ls))) => {
                    out.extend(ls.iter().map(line))
                }
                _ => o.values().for_each(|x| collect_lines(x, out)),
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_lines(x, out)),
        _ => {}
    }
}

/// Rows of numbers: x, y and maybe z in metres, or with a header naming them, longitude
/// and latitude (and elevation). Returns the points and whether they are geographic.
fn csv(src: &str) -> Result<(Vec<DVec3>, bool), Error> {
    let split = |l: &str| -> Vec<String> {
        l.split([',', ';', '\t', ' '])
            .map(|s| s.trim().trim_matches('"').to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let mut lines = src
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'));
    let Some(first) = lines.next() else {
        return Ok((vec![], false));
    };
    let head = split(first);
    let numeric = head.iter().all(|c| c.parse::<f64>().is_ok());
    let find = |names: &[&str]| head.iter().position(|c| names.contains(&c.as_str()));
    let (cols, geographic) = if numeric {
        ([Some(0), Some(1), Some(2)], false)
    } else if let (Some(lon), Some(lat)) = (
        find(&["lon", "lng", "long", "longitude", "x_lon"]),
        find(&["lat", "latitude", "y_lat"]),
    ) {
        (
            [
                Some(lon),
                Some(lat),
                find(&["ele", "elevation", "alt", "altitude", "z", "height"]),
            ],
            true,
        )
    } else {
        (
            [
                find(&["x", "east", "easting"]).or(Some(0)),
                find(&["y", "north", "northing"]).or(Some(1)),
                find(&["z", "height", "elevation", "ele", "alt"]),
            ],
            false,
        )
    };
    let rows = numeric.then_some(first).into_iter().chain(lines);
    let mut out = Vec::new();
    for (i, l) in rows.enumerate() {
        let v = split(l);
        let get = |c: Option<usize>| c.and_then(|c| v.get(c)).and_then(|s| s.parse::<f64>().ok());
        match (get(cols[0]), get(cols[1])) {
            (Some(x), Some(y)) => out.push(DVec3::new(x, y, get(cols[2]).unwrap_or(0.0))),
            _ => {
                return Err(Error::Invalid(format!(
                    "CSV row {}: expected numbers, found \"{l}\"",
                    i + 1
                )));
            }
        }
    }
    Ok((out, geographic))
}

/// The points a line needs to stay within `tolerance` metres of `points` in plan
/// (Douglas–Peucker), and no further apart than `longest` so that curves keep their
/// shape between nodes.
pub fn simplify(points: &[DVec3], closed: bool, tolerance: f64, longest: f64) -> Vec<DVec3> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    // A closed loop is split at its point furthest from the start, as two open lines.
    let ends: Vec<usize> = if closed {
        let far = (1..n)
            .max_by(|&a, &b| {
                let d = |i: usize| points[i].truncate().distance(points[0].truncate());
                d(a).total_cmp(&d(b))
            })
            .unwrap_or(n / 2);
        vec![0, far, n]
    } else {
        vec![0, n - 1]
    };
    let at = |i: usize| points[i % n];
    let mut keep = vec![false; n + 1];
    for &e in &ends {
        keep[e] = true;
    }
    let mut stack: Vec<(usize, usize)> = ends.windows(2).map(|w| (w[0], w[1])).collect();
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (p, q) = (at(a).truncate(), at(b).truncate());
        let far = (a + 1..b)
            .map(|i| (i, distance_to_segment(at(i).truncate(), p, q)))
            .max_by(|x, y| x.1.total_cmp(&y.1));
        let span: f64 = (a..b)
            .map(|i| at(i).truncate().distance(at(i + 1).truncate()))
            .sum();
        if let Some((i, d)) = far
            && (d > tolerance || span > longest)
        {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    let kept: Vec<DVec3> = (0..n).filter(|&i| keep[i]).map(|i| points[i]).collect();
    // Nodes close together bend the spline sharply between them at the least wobble
    // of the line: keep them `SHORTEST` apart, and an open line's last node.
    let mut out: Vec<DVec3> = Vec::with_capacity(kept.len());
    for (k, &p) in kept.iter().enumerate() {
        let end = !closed && k + 1 == kept.len();
        match out.last() {
            Some(q) if q.truncate().distance(p.truncate()) < SHORTEST => {
                if end {
                    out.pop();
                    out.push(p);
                }
            }
            _ => out.push(p),
        }
    }
    if closed
        && out.len() > 3
        && out[0].truncate().distance(out[out.len() - 1].truncate()) < SHORTEST
    {
        out.pop();
    }
    out
}

/// Least distance between nodes laid along a centreline, m.
const SHORTEST: f64 = 8.0;

/// Longest gap between nodes laid along a centreline, m.
pub const LONGEST: f64 = 40.0;
/// Lengths of line the plan position and the height are averaged over, m: survey
/// points jitter by a fraction of a metre, and GPS heights by far more.
const PLAN_WINDOW: f64 = 10.0;
const HEIGHT_WINDOW: f64 = 40.0;

/// Each point averaged with its neighbours within half a window along the line either
/// way, with weights falling off towards the window's ends: the plan position over
/// `plan` metres, the height over `height` metres.
pub fn smooth(points: &[DVec3], closed: bool, plan: f64, height: f64) -> Vec<DVec3> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    // Distance along the line to each point, and the whole length round a loop.
    let mut s = vec![0.0; n];
    for i in 1..n {
        s[i] = s[i - 1] + points[i].truncate().distance(points[i - 1].truncate());
    }
    let length = s[n - 1]
        + if closed {
            points[0].truncate().distance(points[n - 1].truncate())
        } else {
            0.0
        };
    let average = |i: usize, window: f64, get: &dyn Fn(DVec3) -> DVec3| -> DVec3 {
        let half = 0.5 * window;
        let (mut sum, mut total) = (DVec3::ZERO, 0.0);
        // Walk out both ways until beyond half the window.
        for dir in [1i64, -1] {
            let mut k = 0i64;
            loop {
                let j = i as i64 + dir * k;
                let j = if closed {
                    j.rem_euclid(n as i64) as usize
                } else if (0..n as i64).contains(&j) {
                    j as usize
                } else {
                    break;
                };
                let mut d = (s[j] - s[i]).abs();
                if closed {
                    d = d.min(length - d);
                }
                if d > half || k as usize >= n {
                    break;
                }
                if !(dir == -1 && k == 0) {
                    let w = 1.0 - d / (half + 1e-9);
                    sum += get(points[j]) * w;
                    total += w;
                }
                k += 1;
            }
        }
        sum / total
    };
    (0..n)
        .map(|i| {
            let xy = average(i, plan, &|p| p.with_z(0.0));
            let z = average(i, height, &|p| DVec3::Z * p.z);
            // The ends of an open line stay where they are.
            let xy = if !closed && (i == 0 || i == n - 1) {
                points[i].with_z(0.0)
            } else {
                xy
            };
            xy.with_z(z.z)
        })
        .collect()
}

/// The operations that lay road `name` along a centreline within `tolerance` metres:
/// its nodes replaced if it exists, or else a new road with the main road's
/// cross-section.
pub fn road_ops(project: &Project, line: &Centreline, name: &str, tolerance: f64) -> Vec<Op> {
    let smooth = smooth(&line.points, line.closed, PLAN_WINDOW, HEIGHT_WINDOW);
    let nodes = simplify(&smooth, line.closed, tolerance, LONGEST);
    if project.road(name).is_some() {
        vec![
            Op::SetNodes {
                line: name.to_string(),
                nodes,
            },
            Op::SetRoad {
                road: name.to_string(),
                closed: Some(line.closed),
                crown: None,
                surface: None,
                material: None,
                resolution: None,
            },
        ]
    } else {
        vec![Op::AddRoad {
            name: name.to_string(),
            closed: line.closed,
            nodes,
            like: Some(project.main_road.clone()),
        }]
    }
}

fn distance_to_segment(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gpx_kml_geojson_and_csv() {
        let gpx_src = r#"<gpx><trk><trkseg>
            <trkpt lat="50.0" lon="6.0"><ele>100</ele></trkpt>
            <trkpt lat="50.001" lon="6.0"><ele>101</ele></trkpt>
            <trkpt lat="50.001" lon="6.001"></trkpt>
            <trkpt lat="50.0" lon="6.001"><ele>103</ele></trkpt>
        </trkseg></trk></gpx>"#;
        let c = read("lap.gpx", gpx_src).unwrap();
        assert_eq!(c.points.len(), 4);
        assert!(!c.closed);
        // 0.001° of latitude is about 111 m.
        let dy = c.points[1].y - c.points[0].y;
        assert!((dy - 111.2).abs() < 0.5, "{dy}");
        assert_eq!(c.points[0].z, 100.0);
        assert_eq!(c.points[2].z, 0.0, "a point without <ele>");
        // Longitude shrinks with the cosine of the latitude.
        let dx = c.points[2].x - c.points[1].x;
        assert!((dx - 111.2 * 50f64.to_radians().cos()).abs() < 0.5, "{dx}");

        let kml_src = "<kml><coordinates>6.0,50.0,0 6.0,50.001,0 6.001,50.001,0 6.0,50.0,0</coordinates></kml>";
        let c = read("track.kml", kml_src).unwrap();
        assert!(c.closed, "back at the start");
        assert_eq!(c.points.len(), 3);

        let json = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","geometry":{"type":"Point","coordinates":[6.0,50.0]}},
            {"type":"Feature","geometry":{"type":"LineString",
             "coordinates":[[6.0,50.0],[6.0,50.001],[6.001,50.001],[6.001,50.0]]}}]}"#;
        assert_eq!(read("osm.geojson", json).unwrap().points.len(), 4);

        let c = read("pts.csv", "x,y,z\n0,0,1\n100,0,2\n100,50,3\n").unwrap();
        assert_eq!(c.points[1], DVec3::new(100.0, 0.0, 2.0));
        assert_eq!(c.origin, None);
        let c = read("pts.csv", "0 0\n100 0\n100 50\n").unwrap();
        assert_eq!(c.points.len(), 3);
        assert!(read("pts.csv", "x,y\n0,0\nfoo,bar\n1,1\n").is_err());
        assert!(read("a.txt", "0,0\n1,1\n").is_err(), "too few points");
    }

    #[test]
    fn simplify_keeps_corners_and_drops_straights() {
        // A square of 100 m sides, sampled every metre.
        let mut pts = Vec::new();
        for (a, b) in [
            (DVec2::ZERO, DVec2::X),
            (DVec2::X, DVec2::ONE),
            (DVec2::ONE, DVec2::Y),
            (DVec2::Y, DVec2::ZERO),
        ] {
            for k in 0..100 {
                pts.push((a.lerp(b, k as f64 / 100.0) * 100.0).extend(0.0));
            }
        }
        let s = simplify(&pts, true, 0.5, 1e9);
        assert_eq!(s.len(), 4, "{s:?}");
        let s = simplify(&pts, true, 0.5, 30.0);
        assert!(s.len() >= 16, "no more than 30 m apart: {}", s.len());
        assert!(
            s.windows(2).all(|w| w[0].distance(w[1]) >= SHORTEST),
            "nor closer than {SHORTEST} m"
        );
        let open = simplify(&pts[..150], false, 0.5, 1e9);
        assert_eq!(open.len(), 3);
    }
}
