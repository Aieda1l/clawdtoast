mod core;
mod daemon;
mod log;
#[cfg(target_os = "macos")]
mod macos;
mod paths;
mod protocol;
mod render;
mod shim;
mod sound;
mod text;

use protocol::{Kind, Msg};

fn usage() -> ! {
    eprintln!("usage: clawdpet <hook EVENT | daemon [--headless] | test>");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["hook", event] => shim::run(event), // always exits 0
        ["daemon"] => daemon::run(false),
        ["daemon", "--headless"] => daemon::run(true),
        ["test"] => self_test(),
        _ => usage(),
    }
}

/// Shows one card of each kind so placement and sound can be checked by eye and ear.
fn self_test() {
    if let Err(e) = shim::ensure_daemon() {
        eprintln!("could not start daemon: {e}");
        std::process::exit(1);
    }
    let cards = [
        (Kind::Done, "Wrote design doc for overlay window and hook shim"),
        (Kind::Permission, "Claude needs your permission to use Bash"),
        (Kind::Idle, "Claude is waiting for your input"),
    ];
    for (kind, text) in cards {
        let m = Msg::Alert {
            session_id: "test".into(),
            kind,
            project: "clawdpet".into(),
            text: text.into(),
            ts: protocol::now_ms(),
        };
        if let Ok(mut s) = std::os::unix::net::UnixStream::connect(paths::socket()) {
            let _ = std::io::Write::write_all(&mut s, protocol::encode(&m).as_bytes());
        }
        std::thread::sleep(std::time::Duration::from_millis(2500));
    }
}
