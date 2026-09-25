//! What the car can come to harm from — impact damage and part failures — chosen on the
//! settings screen and kept between runs.

use bevy::prelude::*;
use open_racing_sim::Realism;

use crate::bindings;

const SETTINGS_FILE: &str = "realism.ron";

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct RealismSettings(pub Realism);

impl RealismSettings {
    pub fn load() -> Self {
        Self(bindings::load_config(SETTINGS_FILE))
    }

    pub fn save(&self) {
        bindings::save_config(SETTINGS_FILE, &self.0);
    }
}
