//! Chimes. The output device is opened only while a sound plays, then released.

use crate::protocol::Kind;
use std::io::Cursor;

const CHIME: &[u8] = include_bytes!("../assets/chime.wav");
const PING: &[u8] = include_bytes!("../assets/ping.wav");

pub fn play(kind: Kind) {
    if std::env::var_os("CLAWDPET_MUTE").is_some() {
        return;
    }
    let data = if kind == Kind::Done { CHIME } else { PING };
    std::thread::spawn(move || {
        let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
            crate::log::line("sound: no output device");
            return;
        };
        let Ok(sink) = rodio::Sink::try_new(&handle) else { return };
        let Ok(src) = rodio::Decoder::new(Cursor::new(data)) else { return };
        sink.set_volume(0.6);
        sink.append(src);
        sink.sleep_until_end();
        // _stream drops here, releasing the device.
    });
}
