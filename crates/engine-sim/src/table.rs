//! Piecewise-linear tables.

/// Value of the table `points` (ascending x) at `x`, held constant beyond its ends.
pub fn lookup(points: &[(f64, f64)], x: f64) -> f64 {
    match points {
        [] => 0.0,
        [(_, y)] => *y,
        _ => {
            if x <= points[0].0 {
                return points[0].1;
            }
            for w in points.windows(2) {
                let ((x0, y0), (x1, y1)) = (w[0], w[1]);
                if x <= x1 {
                    let f = if x1 > x0 { (x - x0) / (x1 - x0) } else { 1.0 };
                    return y0 + f * (y1 - y0);
                }
            }
            points[points.len() - 1].1
        }
    }
}

/// Whether the table's x values ascend strictly.
pub fn ascending(points: &[(f64, f64)]) -> bool {
    points.windows(2).all(|w| w[1].0 > w[0].0)
}
