use std::path::PathBuf;

/// Directory holding the socket, lock file and log. `CLAWDPET_DIR` overrides it (used by tests).
pub fn support_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAWDPET_DIR") {
        return PathBuf::from(d);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/clawdpet")
    } else {
        home.join(".local/state/clawdpet")
    }
}

pub fn socket() -> PathBuf {
    support_dir().join("clawdpet.sock")
}

pub fn lock() -> PathBuf {
    support_dir().join("clawdpet.lock")
}

pub fn log() -> PathBuf {
    support_dir().join("clawdpet.log")
}
