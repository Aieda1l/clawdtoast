//! Process table helpers: liveness and ancestry.

use std::collections::HashMap;

/// pid -> (parent pid, executable name or path).
pub type Table = HashMap<u32, (u32, String)>;

#[cfg(unix)]
pub fn table() -> Table {
    let Ok(out) = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,comm="]).output() else {
        return Table::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            Some((pid, (ppid, it.collect::<Vec<_>>().join(" "))))
        })
        .collect()
}

#[cfg(windows)]
pub fn table() -> Table {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let mut t = Table::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return t;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            t.insert(e.th32ProcessID, (e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
            ok = Process32NextW(snap, &mut e);
        }
        CloseHandle(snap);
    }
    t
}

/// PID of the nearest ancestor of this process whose executable name satisfies `pred`.
pub fn find_ancestor(pred: impl Fn(&str) -> bool) -> Option<u32> {
    let table = table();
    let mut cur = table.get(&std::process::id())?.0;
    for _ in 0..16 {
        let (ppid, comm) = table.get(&cur)?;
        let base = comm.rsplit(['/', '\\']).next().unwrap_or("");
        if pred(base) {
            return Some(cur);
        }
        if *ppid <= 1 {
            break;
        }
        cur = *ppid;
    }
    None
}

#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    let r = unsafe { libc::kill(pid as i32, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false; // gone, or never existed
        }
        let running = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
        CloseHandle(h);
        running
    }
}
