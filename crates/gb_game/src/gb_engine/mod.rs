//! gbrender engine transfer: their translated URP material path, adopted into the game.
//!
//! See `handoff/GBRENDER_TRANSFER_PLAN.md`. The module compiles and registers behind
//! `GB_ENGINE=1`; stage materials are switched over by the conversion system once wired, so the
//! default path (our StandardMaterial + vinyl) is untouched until the A/B says otherwise.

pub mod convert;
pub mod material;

use bevy::prelude::*;

/// Registers gbrender's translated URP material path when `GB_ENGINE=1`.
pub struct GbEnginePlugin;

impl Plugin for GbEnginePlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("GB_ENGINE").is_none() {
            return;
        }
        app.add_plugins((material::GbMaterialPlugin, convert::GbConvertPlugin));
        info!("gb_engine: gbrender URP material path enabled (GB_ENGINE=1)");
    }
}
