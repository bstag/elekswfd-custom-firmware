//! web: the local HTTP server (port of class H in wfd-control/app.py). Implemented in wave 2.
//! Entry point used by main: `pub fn serve(ctx: std::sync::Arc<crate::ctx::Ctx>, addr: &str)`.

use crate::ctx::Ctx;
use std::sync::Arc;

/// Serve the dashboard + API on addr (e.g. "127.0.0.1:8737") until ctx.stop. (wave 2 placeholder)
pub fn serve(ctx: Arc<Ctx>, addr: &str) {
    let _ = (ctx, addr);
    unimplemented!("web::serve — wave 2")
}
