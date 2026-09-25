//! Where a project lies on the Earth: the longitude and latitude of its origin, and the
//! projection between them and the project's metres (east, north). An equirectangular
//! projection about the origin: within a few centimetres over the few kilometres a
//! circuit spans.

use glam::DVec2;
use serde::{Deserialize, Serialize};

/// Mean radius of the Earth, m.
pub const EARTH: f64 = 6_371_000.0;

/// The longitude and latitude of the project's (0, 0), degrees.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Geo {
    pub lon: f64,
    pub lat: f64,
}

impl Geo {
    /// Metres per degree of latitude, and of longitude at the origin's latitude.
    fn scale(self) -> DVec2 {
        let k = EARTH * std::f64::consts::PI / 180.0;
        DVec2::new(k * self.lat.to_radians().cos(), k)
    }

    /// Metres east and north of the origin of a longitude and latitude.
    pub fn to_local(self, lon: f64, lat: f64) -> DVec2 {
        (DVec2::new(lon, lat) - DVec2::new(self.lon, self.lat)) * self.scale()
    }

    /// The longitude and latitude of a point metres east and north of the origin.
    pub fn to_geo(self, p: DVec2) -> (f64, f64) {
        let d = p / self.scale();
        (self.lon + d.x, self.lat + d.y)
    }

    pub fn is_valid(self) -> bool {
        self.lon.is_finite()
            && self.lat.is_finite()
            && (-180.0..=180.0).contains(&self.lon)
            && (-89.0..=89.0).contains(&self.lat)
    }
}

/// Whether coordinates look like longitudes and latitudes rather than metres.
pub fn looks_geographic(points: impl IntoIterator<Item = DVec2>) -> bool {
    let mut any = false;
    for p in points {
        any = true;
        if !(p.x.abs() <= 180.0 && p.y.abs() <= 90.0) {
            return false;
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_measures_a_degree() {
        let g = Geo {
            lon: 136.54,
            lat: 34.84,
        };
        let p = g.to_local(136.55, 34.85);
        assert!((p.y - 1111.95).abs() < 0.5, "{p}");
        assert!(
            (p.x - 1111.95 * 34.84f64.to_radians().cos()).abs() < 0.5,
            "{p}"
        );
        let (lon, lat) = g.to_geo(p);
        assert!((lon - 136.55).abs() < 1e-9 && (lat - 34.85).abs() < 1e-9);
    }
}
