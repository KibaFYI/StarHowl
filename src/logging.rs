//! Tiny log file at %APPDATA%\Starhowl\starhowl.log (overwritten each launch).

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

static LOG: Mutex<Option<(File, Instant)>> = Mutex::new(None);

pub fn init() {
    let dir = crate::config::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(f) = File::create(dir.join("starhowl.log")) {
        *LOG.lock().unwrap() = Some((f, Instant::now()));
    }
}

pub fn write(msg: &str) {
    if cfg!(debug_assertions) {
        eprintln!("{msg}");
    }
    if let Ok(mut guard) = LOG.lock() {
        if let Some((f, start)) = guard.as_mut() {
            let _ = writeln!(f, "[{:>9.3}] {msg}", start.elapsed().as_secs_f64());
        }
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::logging::write(&format!($($t)*)) };
}
