//! Minimal append-only file logger. The shim must never print, so errors land here.

use std::fs::OpenOptions;
use std::io::Write;

const MAX_BYTES: u64 = 1_000_000;

pub fn line(msg: &str) {
    let path = crate::paths::log();
    let _ = std::fs::create_dir_all(crate::paths::support_dir());
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {}", crate::protocol::now_ms(), msg);
    }
}
