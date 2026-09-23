//! AI line files (`ai/fast_lane.ai`). Only the geometry is used: the points give the
//! direction of travel and the distances to the track edges on either side.
//!
//! Layout: i32 version, i32 point count, i32 lap time, i32 sample count; per point
//! 3 f32 position, f32 distance, i32 id; then i32 extra count and per point 18 f32
//! of extra data, of which index 5 and 6 are the distances to the left and right
//! edges. Anything after that is ignored.

use glam::DVec3;

use crate::Error;
use crate::reader::Reader;

const EXTRA_FLOATS: usize = 18;
const SIDE_LEFT: usize = 5;
const SIDE_RIGHT: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AiPoint {
    /// Position in the file's coordinates.
    pub pos: DVec3,
    /// Distances to the left and right track edges, when the file has them.
    pub side_left: Option<f64>,
    pub side_right: Option<f64>,
}

pub fn parse(buf: &[u8]) -> Result<Vec<AiPoint>, Error> {
    let mut r = Reader::new(buf);
    let version = r.i32()?;
    if !(1..=64).contains(&version) {
        return Err(Error::Format(format!(
            "unsupported AI line version {version}"
        )));
    }
    let count = r.count(20)?;
    let _lap_time = r.i32()?;
    let _sample_count = r.i32()?;
    let mut points = Vec::with_capacity(count);
    for _ in 0..count {
        let [x, y, z] = r.f32s::<3>()?;
        let _distance = r.f32()?;
        let _id = r.i32()?;
        points.push(AiPoint {
            pos: DVec3::new(x.into(), y.into(), z.into()),
            side_left: None,
            side_right: None,
        });
    }
    // The extra block is optional in old files.
    if r.remaining() >= 4 && r.i32()? as usize == count && r.remaining() >= count * EXTRA_FLOATS * 4
    {
        for p in &mut points {
            let extra = r.f32s::<EXTRA_FLOATS>()?;
            p.side_left = Some(extra[SIDE_LEFT].into());
            p.side_right = Some(extra[SIDE_RIGHT].into());
        }
    }
    if points.len() < 4 {
        return Err(Error::Format("AI line has fewer than 4 points".into()));
    }
    Ok(points)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::reader::write::Writer;

    /// A square loop of `n` points per side with 5 m / 3 m to the left / right edge.
    pub fn sample_square(side: f32, n: usize, extras: bool) -> Vec<u8> {
        let mut pts = Vec::new();
        for k in 0..4 {
            for i in 0..n {
                let t = side * i as f32 / n as f32;
                pts.push(match k {
                    0 => [t, 0.0, 0.0],
                    1 => [side, 0.0, -t],
                    2 => [side - t, 0.0, -side],
                    _ => [0.0, 0.0, -side + t],
                });
            }
        }
        let mut w = Writer::default();
        w.i32(7).i32(pts.len() as i32).i32(0).i32(0);
        for (i, p) in pts.iter().enumerate() {
            w.f32s(p).f32(i as f32).i32(i as i32);
        }
        if extras {
            w.i32(pts.len() as i32);
            for _ in &pts {
                let mut e = [0.0; EXTRA_FLOATS];
                e[SIDE_LEFT] = 5.0;
                e[SIDE_RIGHT] = 3.0;
                w.f32s(&e);
            }
            w.raw(&[0; 32]);
        }
        w.0
    }

    #[test]
    fn parses_points_and_sides() {
        let pts = parse(&sample_square(100.0, 10, true)).unwrap();
        assert_eq!(pts.len(), 40);
        assert_eq!(pts[10].pos, DVec3::new(100.0, 0.0, 0.0));
        assert_eq!(pts[3].side_left, Some(5.0));
        assert_eq!(pts[3].side_right, Some(3.0));
    }

    #[test]
    fn extra_block_is_optional() {
        let pts = parse(&sample_square(100.0, 10, false)).unwrap();
        assert_eq!(pts[0].side_left, None);
        assert!(parse(&[0; 8]).is_err());
    }
}
