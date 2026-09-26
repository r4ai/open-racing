//! The sample library (`assets/machine-src/`), built in: a GT3-class machine with a
//! flat-plane V8, and an inline four with its intakes and exhausts to swap in. Tests use
//! it, and `machinectl init` writes it into a library directory.

use std::path::{Path, PathBuf};

use crate::library::Library;

/// (path in the library, text).
pub const FILES: &[(&str, &str)] = &[
    (
        "machines/gt3_v8.ron",
        include_str!("../../../assets/machine-src/machines/gt3_v8.ron"),
    ),
    (
        "parts/aero/gt3_aero.ron",
        include_str!("../../../assets/machine-src/parts/aero/gt3_aero.ron"),
    ),
    (
        "parts/body/gt3_body.ron",
        include_str!("../../../assets/machine-src/parts/body/gt3_body.ron"),
    ),
    (
        "parts/brakes/gt3_iron.ron",
        include_str!("../../../assets/machine-src/parts/brakes/gt3_iron.ron"),
    ),
    (
        "parts/driveline/gt3_rwd.ron",
        include_str!("../../../assets/machine-src/parts/driveline/gt3_rwd.ron"),
    ),
    (
        "parts/electronics/gt3_paddle.ron",
        include_str!("../../../assets/machine-src/parts/electronics/gt3_paddle.ron"),
    ),
    (
        "parts/engine/i4_2l_na.ron",
        include_str!("../../../assets/machine-src/parts/engine/i4_2l_na.ron"),
    ),
    (
        "parts/engine/v8_4l_flatplane.ron",
        include_str!("../../../assets/machine-src/parts/engine/v8_4l_flatplane.ron"),
    ),
    (
        "parts/exhaust/i4_road.ron",
        include_str!("../../../assets/machine-src/parts/exhaust/i4_road.ron"),
    ),
    (
        "parts/exhaust/i4_straight.ron",
        include_str!("../../../assets/machine-src/parts/exhaust/i4_straight.ron"),
    ),
    (
        "parts/exhaust/v8_merged.ron",
        include_str!("../../../assets/machine-src/parts/exhaust/v8_merged.ron"),
    ),
    (
        "parts/exhaust/v8_race.ron",
        include_str!("../../../assets/machine-src/parts/exhaust/v8_race.ron"),
    ),
    (
        "parts/frame/gt3_tub.ron",
        include_str!("../../../assets/machine-src/parts/frame/gt3_tub.ron"),
    ),
    (
        "parts/fuel_tank/gt3_120l.ron",
        include_str!("../../../assets/machine-src/parts/fuel_tank/gt3_120l.ron"),
    ),
    (
        "parts/intake/i4_plenum.ron",
        include_str!("../../../assets/machine-src/parts/intake/i4_plenum.ron"),
    ),
    (
        "parts/intake/v8_itb.ron",
        include_str!("../../../assets/machine-src/parts/intake/v8_itb.ron"),
    ),
    (
        "parts/interior/gt3_cockpit.ron",
        include_str!("../../../assets/machine-src/parts/interior/gt3_cockpit.ron"),
    ),
    (
        "parts/steering/gt3_rack.ron",
        include_str!("../../../assets/machine-src/parts/steering/gt3_rack.ron"),
    ),
    (
        "parts/suspension/gt3_front.ron",
        include_str!("../../../assets/machine-src/parts/suspension/gt3_front.ron"),
    ),
    (
        "parts/suspension/gt3_rear.ron",
        include_str!("../../../assets/machine-src/parts/suspension/gt3_rear.ron"),
    ),
    (
        "parts/transmission/gt3_seq6.ron",
        include_str!("../../../assets/machine-src/parts/transmission/gt3_seq6.ron"),
    ),
    (
        "parts/wheel/gt3_front.ron",
        include_str!("../../../assets/machine-src/parts/wheel/gt3_front.ron"),
    ),
    (
        "parts/wheel/gt3_rear.ron",
        include_str!("../../../assets/machine-src/parts/wheel/gt3_rear.ron"),
    ),
];

/// The samples as a library in `dir` (nothing written).
pub fn library(dir: impl Into<PathBuf>) -> Library {
    let mut lib = Library::new(dir);
    for (path, text) in FILES {
        let stem = path
            .rsplit('/')
            .next()
            .unwrap()
            .trim_end_matches(".ron")
            .to_string();
        if path.starts_with("machines/") {
            lib.machines.insert(
                stem,
                crate::library::parse_machine(text).expect("sample machines parse"),
            );
        } else {
            let kind = path.split('/').nth(1).expect("parts/<kind>/");
            let r = crate::PartRef::parse(&format!("{kind}/{stem}")).expect("sample names");
            lib.parts.insert(
                r,
                crate::library::parse_part(text).expect("sample parts parse"),
            );
        }
    }
    lib
}

/// Writes the samples into `dir`, leaving any file already there alone. Returns what it
/// wrote.
pub fn init(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for (path, text) in FILES {
        let p = dir.join(path);
        if p.exists() {
            continue;
        }
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        out.push(p);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_check() {
        let lib = library("samples");
        crate::validate::library(&lib).unwrap();
        assert!(lib.machines.contains_key("gt3_v8"));
    }

    #[test]
    fn samples_round_trip_as_text() {
        for (path, text) in FILES {
            let again = if path.starts_with("machines") {
                crate::library::machine_text(&crate::library::parse_machine(text).unwrap())
            } else {
                crate::library::part_text(&crate::library::parse_part(text).unwrap())
            };
            // A Windows checkout may turn the files' line endings to CRLF.
            assert_eq!(again, text.replace("\r\n", "\n"), "{path}");
        }
    }
}
