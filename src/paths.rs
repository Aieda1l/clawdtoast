use std::path::PathBuf;

/// Directory holding the socket, lock file and log. `CLAWDTOAST_DIR` overrides it (used by tests).
pub fn support_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAWDTOAST_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/clawdtoast")
    } else {
        home.join(".local/state/clawdtoast")
    }
}

pub fn socket() -> PathBuf {
    support_dir().join("clawdtoast.sock")
}

pub fn lock() -> PathBuf {
    support_dir().join("clawdtoast.lock")
}

pub fn log() -> PathBuf {
    support_dir().join("clawdtoast.log")
}
