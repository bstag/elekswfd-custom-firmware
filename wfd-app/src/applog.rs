//! wfd-app.log, next to the settings: one line per event worth knowing about afterwards (starts,
//! why the app stopped, panics, the clock connecting and why a link was lost). Kept small: past
//! 256 KB the old log is renamed to wfd-app.log.old. Until [`init`] is called (tests), logging
//! does nothing.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static PATH: OnceLock<PathBuf> = OnceLock::new();
static RECENT: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// The same line is written at most once in this long by [`log_once`].
const REPEAT_AFTER: Duration = Duration::from_secs(600);

/// Where the log goes; the first call wins.
pub fn init(path: PathBuf) {
    let _ = PATH.set(path);
}

/// Append one timestamped line.
pub fn log(msg: &str) {
    let Some(path) = PATH.get() else { return };
    if std::fs::metadata(path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// Like [`log`], but a line with the same text is skipped for the next 10 minutes: for events
/// that can repeat every few seconds (a link that keeps dropping) without filling the log.
pub fn log_once(msg: &str) {
    if PATH.get().is_none() {
        return;
    }
    {
        let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        let recent = recent.get_or_insert_with(HashMap::new);
        let now = Instant::now();
        if recent.get(msg).is_some_and(|at| now.duration_since(*at) < REPEAT_AFTER) {
            return;
        }
        recent.retain(|_, at| now.duration_since(*at) < REPEAT_AFTER);
        recent.insert(msg.to_string(), now);
    }
    log(msg);
}
