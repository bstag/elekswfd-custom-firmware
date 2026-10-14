//! clocklink: the ~30 Hz loop that drives the clock (port of ClockLink in wfd-control/app.py).
//! Implemented in wave 2. Entry point used by main: `pub fn run(ctx: std::sync::Arc<crate::ctx::Ctx>)`.

use crate::ctx::Ctx;
use std::sync::Arc;

/// Drive the clock until ctx.stop is set. (wave 2: replace this placeholder)
pub fn run(ctx: Arc<Ctx>) {
    let _ = ctx;
    unimplemented!("clocklink::run — wave 2")
}
