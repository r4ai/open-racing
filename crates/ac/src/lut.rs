//! Lookup tables: `.lut` files of `x|y` lines, or inline `(|x=y|x=y|)` values in INI files.

/// Parses a table, sorted by x. Lines that are not a pair of numbers are skipped.
pub fn parse(src: &str) -> Vec<(f64, f64)> {
    let src = src.trim();
    let mut table: Vec<(f64, f64)> =
        if let Some(inline) = src.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            inline.split('|').filter_map(|p| pair(p, '=')).collect()
        } else {
            src.lines()
                .map(|l| l.split(';').next().unwrap_or(""))
                .filter_map(|l| pair(l, '|'))
                .collect()
        };
    table.sort_by(|a, b| a.0.total_cmp(&b.0));
    table
}

fn pair(s: &str, separator: char) -> Option<(f64, f64)> {
    let (x, y) = s.split_once(separator)?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// Linear interpolation in a sorted table, clamped at the ends.
pub fn lookup(table: &[(f64, f64)], x: f64) -> Option<f64> {
    let (first, last) = (table.first()?, table.last()?);
    if x <= first.0 {
        return Some(first.1);
    }
    if x >= last.0 {
        return Some(last.1);
    }
    let i = table.partition_point(|p| p.0 < x);
    let (a, b) = (table[i - 1], table[i]);
    Some(a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_inline_tables() {
        let t = parse("; torque\n1000|200\n0|100 ; idle\n\nbad line\n2000|300\n");
        assert_eq!(t, [(0.0, 100.0), (1000.0, 200.0), (2000.0, 300.0)]);
        assert_eq!(lookup(&t, 1500.0), Some(250.0));
        assert_eq!(lookup(&t, -5.0), Some(100.0));
        assert_eq!(lookup(&t, 9000.0), Some(300.0));
        assert_eq!(parse("(|0=1|1=0|)"), [(0.0, 1.0), (1.0, 0.0)]);
        assert_eq!(lookup(&[], 1.0), None);
    }
}
