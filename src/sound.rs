//! Chimes. The output device is opened only while a sound plays, then released.

use crate::config::{self, Sound};
use crate::protocol::Kind;
use std::io::Cursor;

const CHIME: &[u8] = include_bytes!("../assets/chime.wav");
const PING: &[u8] = include_bytes!("../assets/ping.wav");

pub fn play(kind: Kind, cfg: &Sound) {
    if !cfg.enabled || cfg.volume <= 0.0 {
        return;
    }
    let (custom, builtin) = if kind == Kind::Done {
        (&cfg.done, CHIME)
    } else {
        (&cfg.attention, PING)
    };
    // A custom file that cannot be read falls back to the built-in sound.
    let data: Vec<u8> = custom
        .as_deref()
        .and_then(|p| std::fs::read(config::expand_home(p)).ok())
        .unwrap_or_else(|| builtin.to_vec());
    let volume = cfg.volume;
    std::thread::spawn(move || {
        let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
            crate::log::line("sound: no output device");
            return;
        };
        let Ok(sink) = rodio::Sink::try_new(&handle) else {
            return;
        };
        let Ok(src) = rodio::Decoder::new(Cursor::new(data)) else {
            crate::log::line("sound: cannot decode (custom sounds must be WAV)");
            return;
        };
        sink.set_volume(volume);
        sink.append(src);
        sink.sleep_until_end();
        // _stream drops here, releasing the device.
    });
}
