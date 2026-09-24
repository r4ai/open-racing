//! Where the clouds are: a map of cloud "potential" over a square that repeats across
//! the world. Cloud forms where the potential exceeds a threshold set by the cloud
//! cover, so more cover grows the clouds out of the same pattern rather than moving them.
//!
//! The map holds two independent patterns. Blending them with weights on a circle
//! (`cos θ`, `sin θ`) keeps the statistics of the potential while θ turns, so the clouds
//! slowly change shape as they drift with the wind. It also holds cells (Worley noise)
//! about a kilometre across: convection breaks cloud into separate heaps, each over its
//! own thermal, so where the potential allows cloud, cumulus and stratocumulus form in
//! the middle of the cells first. The renderer draws its volumetric clouds from the same
//! texels, so the shadows on the road are where the clouds are.

/// Texels along each edge of the map.
pub const CLOUD_MAP_SIZE: usize = 256;
/// Edge of the square the map covers before it repeats, m.
pub const CLOUD_MAP_PERIOD: f64 = 16384.0;
/// Potential over its threshold at which cloud reaches full density, in standard
/// deviations of the potential.
pub const CLOUD_SOFTNESS: f64 = 0.75;

/// Potential encoded in a texel: 128 + 32 per standard deviation.
const ENCODE_OFFSET: f64 = 128.0;
const ENCODE_SCALE: f64 = 32.0;
/// Periods of the octaves of the pattern across the map, from broad to fine.
const OCTAVES: [u32; 5] = [4, 8, 16, 32, 64];
/// Cells across the map.
const CELLS: u32 = 16;
/// Distance from a cell's centre, in cells, at which the cell value falls to 0.
const CELL_REACH: f64 = 0.8;
/// Cell value over its threshold at which a cell's cloud is dense.
pub const CELL_SOFTNESS: f64 = 0.15;

#[derive(Clone, Debug)]
pub struct CloudMap {
    /// Row-major, x fastest: the two patterns' potentials, the cell value and nothing.
    texels: Vec<[u8; 4]>,
}

impl CloudMap {
    pub fn new(seed: u64) -> Self {
        let n = CLOUD_MAP_SIZE;
        let mut fields = [0, 1].map(|channel| {
            let seed = seed ^ (0x9E37_79B9_7F4A_7C15u64.wrapping_mul(channel + 1));
            let mut f = vec![0.0f64; n * n];
            for (k, &period) in OCTAVES.iter().enumerate() {
                let amplitude = 0.55f64.powi(k as i32);
                let octave_seed = seed.wrapping_add((k as u64).wrapping_mul(0x632B_E59B_D9B4_E019));
                for y in 0..n {
                    for x in 0..n {
                        let u = (x as f64 + 0.5) / n as f64 * period as f64;
                        let v = (y as f64 + 0.5) / n as f64 * period as f64;
                        f[y * n + x] += amplitude * perlin(u, v, period, octave_seed);
                    }
                }
            }
            // Zero mean, unit deviation.
            let mean = f.iter().sum::<f64>() / f.len() as f64;
            let dev = (f.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / f.len() as f64).sqrt();
            f.iter_mut().for_each(|x| *x = (*x - mean) / dev.max(1e-9));
            f
        });
        let [a, b] = std::mem::take(&mut fields);
        let encode = |x: f64| (ENCODE_OFFSET + x * ENCODE_SCALE).round().clamp(0.0, 255.0) as u8;
        let cells = cells(seed);
        Self {
            texels: (0..n * n)
                .map(|i| {
                    let c = (cells[i] * 255.0).round().clamp(0.0, 255.0) as u8;
                    [encode(a[i]), encode(b[i]), c, 0]
                })
                .collect(),
        }
    }

    /// The texels, for the renderer: the two patterns (128 + 32 per standard
    /// deviation), the cell value (0..255) and nothing.
    pub fn texels(&self) -> &[[u8; 4]] {
        &self.texels
    }

    /// Potential at the map point (x, y) m, blending the patterns by `weights`, in
    /// standard deviations. Bilinear between texel centres, repeating.
    pub fn potential(&self, x: f64, y: f64, weights: [f64; 2]) -> f64 {
        self.sample(x, y, weights).0
    }

    /// Potential (see [`Self::potential`]) and cell value (0..1) at the map point (x, y) m.
    pub fn sample(&self, x: f64, y: f64, weights: [f64; 2]) -> (f64, f64) {
        let n = CLOUD_MAP_SIZE;
        let cell = CLOUD_MAP_PERIOD / n as f64;
        let (u, v) = (x / cell - 0.5, y / cell - 0.5);
        let (fu, fv) = (u.floor(), v.floor());
        let (tu, tv) = (u - fu, v - fv);
        let wrap = |i: f64| i.rem_euclid(n as f64) as usize;
        let (x0, y0) = (wrap(fu), wrap(fv));
        let (x1, y1) = ((x0 + 1) % n, (y0 + 1) % n);
        let texel = |x: usize, y: usize| {
            let [a, b, c, _] = self.texels[y * n + x];
            let decode = |c: u8| (c as f64 - ENCODE_OFFSET) / ENCODE_SCALE;
            (
                weights[0] * decode(a) + weights[1] * decode(b),
                c as f64 / 255.0,
            )
        };
        let lerp =
            |p: (f64, f64), q: (f64, f64), t: f64| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
        let top = lerp(texel(x0, y0), texel(x1, y0), tu);
        let bottom = lerp(texel(x0, y1), texel(x1, y1), tu);
        lerp(top, bottom, tv)
    }
}

/// Cloud density (0..1) for a potential and the threshold the cover sets.
#[inline]
pub fn cloud_density(potential: f64, threshold: f64) -> f64 {
    ((potential - threshold) / CLOUD_SOFTNESS).clamp(0.0, 1.0)
}

/// Share of a cell's cloud (0..1) at cell value `cell` over the cell threshold.
#[inline]
pub fn cell_density(cell: f64, threshold: f64) -> f64 {
    ((cell - threshold) / CELL_SOFTNESS).clamp(0.0, 1.0)
}

/// The cell value above which a share `fill` of the cells' area is cloud: the value
/// falls linearly from the cell's centre, so cloud fills a disc of area `fill`. Past
/// four fifths the discs merge and the threshold drops away so that a full fill closes
/// every gap.
pub fn cell_threshold(fill: f64) -> f64 {
    let fill = fill.clamp(0.0, 1.0);
    let radius = (fill / std::f64::consts::PI).sqrt() / CELL_REACH;
    1.0 - radius + 0.5 * CELL_SOFTNESS - 3.0 * (fill - 0.8).max(0.0)
}

/// Inverted distance to the nearest cell centre, 1 at a centre falling to 0 at
/// `CELL_REACH` cells, over the map's texels; repeating.
fn cells(seed: u64) -> Vec<f64> {
    let n = CLOUD_MAP_SIZE;
    let c = CELLS as i64;
    let centre = |i: i64, j: i64| {
        let (i, j) = (i.rem_euclid(c) as u64, j.rem_euclid(c) as u64);
        let h = hash(seed ^ 0xC311_5EED ^ (i << 32 | j));
        let unit = |h: u64| (h >> 11) as f64 / (1u64 << 53) as f64;
        (unit(h), unit(hash(h)))
    };
    (0..n * n)
        .map(|k| {
            let u = ((k % n) as f64 + 0.5) / n as f64 * CELLS as f64;
            let v = ((k / n) as f64 + 0.5) / n as f64 * CELLS as f64;
            let (iu, iv) = (u.floor() as i64, v.floor() as i64);
            let mut best = f64::INFINITY;
            for dj in -1..=1 {
                for di in -1..=1 {
                    let (fx, fy) = centre(iu + di, iv + dj);
                    let dx = (iu + di) as f64 + fx - u;
                    let dy = (iv + dj) as f64 + fy - v;
                    best = best.min(dx * dx + dy * dy);
                }
            }
            (1.0 - best.sqrt() / CELL_REACH).max(0.0)
        })
        .collect()
}

/// The potential above which a share `cover` of the sky is cloudy: the potential is
/// close to normally distributed, so this is its (1 − cover) quantile.
pub fn cover_threshold(cover: f64) -> f64 {
    // A cloud edge is soft: count cloud from half its full density.
    probit(1.0 - cover.clamp(0.001, 0.999)) - 0.5 * CLOUD_SOFTNESS
}

/// Inverse of the standard normal distribution function (Acklam's approximation,
/// relative error below 1.2e-9).
fn probit(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969683028665376e1,
        2.209460984245205e2,
        -2.759285104469687e2,
        1.38357751867269e2,
        -3.066479806614716e1,
        2.506628277459239,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e1,
        1.615858368580409e2,
        -1.556989798598866e2,
        6.680131188771972e1,
        -1.328068155288572e1,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-3,
        -3.223964580411365e-1,
        -2.400758277161838,
        -2.549732539343734,
        4.374664141464968,
        2.938163982698783,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-3,
        3.224671290700398e-1,
        2.445134137142996,
        3.754408661907416,
    ];
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < 0.02425 {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - 0.02425 {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

/// A well-mixed 64-bit hash.
pub(crate) fn hash(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Gradient noise at (u, v) repeating every `period` lattice cells, about −1..1.
fn perlin(u: f64, v: f64, period: u32, seed: u64) -> f64 {
    let (iu, iv) = (u.floor(), v.floor());
    let (fu, fv) = (u - iu, v - iv);
    let p = period as i64;
    let grad = |i: i64, j: i64, x: f64, y: f64| {
        let (i, j) = (i.rem_euclid(p) as u64, j.rem_euclid(p) as u64);
        let h = hash(seed ^ (i << 32 | j));
        let angle = (h >> 11) as f64 / (1u64 << 53) as f64 * std::f64::consts::TAU;
        angle.cos() * x + angle.sin() * y
    };
    let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (i, j) = (iu as i64, iv as i64);
    let (a, b) = (grad(i, j, fu, fv), grad(i + 1, j, fu - 1.0, fv));
    let (c, d) = (
        grad(i, j + 1, fu, fv - 1.0),
        grad(i + 1, j + 1, fu - 1.0, fv - 1.0),
    );
    let (su, sv) = (fade(fu), fade(fv));
    let top = a + (b - a) * su;
    let bottom = c + (d - c) * su;
    (top + (bottom - top) * sv) * std::f64::consts::SQRT_2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cover_is_the_share_of_the_map_under_cloud() {
        let map = CloudMap::new(7);
        let angle: f64 = 0.7;
        let weights = [angle.cos(), angle.sin()];
        for cover in [0.1, 0.3, 0.5, 0.8] {
            let threshold = cover_threshold(cover);
            let n = 200;
            let cloudy = (0..n * n)
                .filter(|k| {
                    let (x, y) = ((k % n) as f64, (k / n) as f64);
                    let p = map.potential(x * 81.92, y * 81.92, weights);
                    cloud_density(p, threshold) >= 0.5
                })
                .count() as f64
                / (n * n) as f64;
            assert!((cloudy - cover).abs() < 0.08, "cover {cover}: {cloudy}");
        }
    }

    #[test]
    fn the_map_repeats() {
        let map = CloudMap::new(3);
        let w = [0.6, 0.8];
        let a = map.potential(1234.5, -987.0, w);
        let b = map.potential(
            1234.5 + CLOUD_MAP_PERIOD,
            -987.0 - 2.0 * CLOUD_MAP_PERIOD,
            w,
        );
        assert!((a - b).abs() < 1e-9);
    }
}
