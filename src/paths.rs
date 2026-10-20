use std::path::PathBuf;

/// The user's home directory (`HOME`, or `USERPROFILE` on Windows); empty if neither is set.
pub fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// Directory holding the socket, lock file and log. `CLAWDTOAST_DIR` overrides it (used by tests).
pub fn support_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CLAWDTOAST_DIR") {
        return PathBuf::from(d);
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join("AppData/Local"))
            .join("clawdtoast");
    }
    let home = Some(home()).filter(|h| !h.as_os_str().is_empty()).unwrap_or_else(|| PathBuf::from("/tmp"));
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
