//! Microphones for a machine or an engine on its bench.

use open_racing_engine_sim::{Mic, Model, SoundSettings};

use crate::library::Library;
use crate::part::Design;

/// The microphone names `mic_settings` understands.
pub const MICS: [&str; 4] = ["exhaust", "intake", "exterior", "cabin"];

/// Microphones by name: `exhaust` (a metre from the first tailpipe), `intake` (from the
/// intake's mouth), `exterior` (7.5 m to the car's left, as a pass-by), `cabin` (at the
/// driver's head, through the body). `machine` gives the driver's eye point.
pub fn mic_settings(
    lib: &Library,
    machine: Option<&str>,
    model: &Model,
    names: &[String],
) -> Result<SoundSettings, String> {
    let near = |pat: &[&str]| -> [f64; 3] {
        let m = model
            .mouths
            .iter()
            .find(|m| pat.iter().any(|p| m.name.contains(p)))
            .or(model.mouths.first());
        let p = m.map(|m| m.position).unwrap_or([0.0; 3]);
        [p[0] - 0.7, p[1] + 0.7, p[2]]
    };
    let centre = {
        let n = model.mouths.len().max(1) as f64;
        let x: f64 = model.mouths.iter().map(|m| m.position[0]).sum::<f64>() / n;
        x
    };
    let eye = match machine {
        Some(name) => {
            let m = lib.machine(name)?;
            let asm = crate::assembly::assemble(lib, m)?;
            asm.instances
                .iter()
                .find_map(|i| match &lib.parts[&i.part].design {
                    Design::Interior(it) => Some(
                        i.transform
                            .transform_point3(glam::DVec3::from(it.eye))
                            .to_array(),
                    ),
                    _ => None,
                })
                .unwrap_or([-1.1, 0.35, 1.0])
        }
        None => [1.5, 0.3, 0.9],
    };
    let mut mics = Vec::new();
    for n in names {
        let (position, cabin) = match n.as_str() {
            "exhaust" => (near(&["tail", "exhaust"]), false),
            "intake" => (near(&["snorkel", "intake"]), false),
            "exterior" => ([centre, 7.5, 1.2], false),
            "cabin" => (eye, true),
            other => {
                return Err(format!(
                    "no microphone \"{other}\" (one of {})",
                    MICS.join(", ")
                ));
            }
        };
        mics.push(Mic {
            name: n.clone(),
            position,
            cabin,
        });
    }
    Ok(SoundSettings {
        mics,
        ..Default::default()
    })
}
