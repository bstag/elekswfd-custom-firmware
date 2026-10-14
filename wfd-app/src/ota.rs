//! ota: "Update clock" (port of update_clock / upload_firmware / wait_for_build in app.py).
//! Implemented in wave 2. Used by web: `pub fn update_clock(ctx: &std::sync::Arc<crate::ctx::Ctx>) -> bool`
//! (false = an update is already running).

use crate::ctx::Ctx;
use std::sync::Arc;

/// Start uploading the shipped firmware to the clock in the background; false if one is running.
/// (wave 2: replace this placeholder)
pub fn update_clock(ctx: &Arc<Ctx>) -> bool {
    let _ = ctx;
    unimplemented!("ota::update_clock — wave 2")
}
