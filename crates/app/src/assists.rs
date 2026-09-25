//! Driver aids for the human driver, chosen on the settings screen and kept between runs.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::Args;
use crate::bindings;

const SETTINGS_FILE: &str = "assists.ron";

#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistSettings {
    /// Works the clutch pedal: pulling away, before a stall and through H-pattern shifts.
    pub clutch: bool,
    /// Blips the throttle on downshifts, on cars whose electronics do not.
    pub blip: bool,
    /// Picks the gears.
    pub auto_shift: bool,
}

impl Default for AssistSettings {
    fn default() -> Self {
        Self {
            clutch: true,
            blip: true,
            auto_shift: false,
        }
    }
}

impl AssistSettings {
    /// The saved settings; `--auto-shift` turns the gear selection on.
    pub fn load(args: &Args) -> Self {
        let mut settings: Self = bindings::load_config(SETTINGS_FILE);
        settings.auto_shift |= args.auto_shift;
        settings
    }

    pub fn save(&self) {
        bindings::save_config(SETTINGS_FILE, self);
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

impl std::fmt::Display for AssistSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "clutch {}  blip {}  auto shift {}",
            on_off(self.clutch),
            on_off(self.blip),
            on_off(self.auto_shift)
        )
    }
}
