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

/// Directory for the socket and lock file. On Linux that is `$XDG_RUNTIME_DIR/clawdtoast` (tmpfs,
/// owner-only, cleaned at logout) when the session has one; elsewhere it is the support directory.
pub fn runtime_dir() -> PathBuf {
    if cfg!(target_os = "linux") && std::env::var_os("CLAWDTOAST_DIR").is_none() {
        if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(d).join("clawdtoast");
        }
    }
    support_dir()
}

pub fn socket() -> PathBuf {
    runtime_dir().join("clawdtoast.sock")
}

pub fn lock() -> PathBuf {
    runtime_dir().join("clawdtoast.lock")
}

pub fn log() -> PathBuf {
    support_dir().join("clawdtoast.log")
}
