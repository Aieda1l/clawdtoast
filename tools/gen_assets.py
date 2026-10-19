#!/usr/bin/env python3
"""Generates assets/clawd.png (placeholder Clawd wave sheet, 4 frames side by side) and the two chimes.

Replace assets/clawd.png with the real artwork; the daemon needs a
sheet of square RGBA frames side by side (frame size = image height). Run: python3 -I tools/gen_assets.py
"""
import math, struct, wave, zlib, pathlib

OUT = pathlib.Path(__file__).resolve().parent.parent / "assets"

ORANGE = (0xD9, 0x77, 0x57, 255)
EYE = (0x1F, 0x1A, 0x17, 255)

# 16x16 design, upscaled 2x to a 32x32 frame. O = body, E = eye. The right arm is added per frame.
ART = [
    "................",
    "................",
    "................",
    "..OOOOOOOOOOOO..",
    "..OOOOOOOOOOOO..",
    "..OOOEOOOOEOOO..",
    "..OOOEOOOOEOOO..",
    "..OOOOOOOOOOOO..",
    "OOOOOOOOOOOOOO..",
    "OOOOOOOOOOOOOO..",
    "..OOOOOOOOOOOO..",
    "..OOOOOOOOOOOO..",
    "..O.O......O.O..",
    "..O.O......O.O..",
    "................",
    "................",
]
# Top row of the raised right arm (2 rows tall, like the left arm) in each of the 4 wave frames.
WAVE = [5, 3, 5, 3]
def write_png(path, w, h, rgba):
    raw = b"".join(b"\x00" + bytes(rgba[y * w * 4:(y + 1) * w * 4]) for y in range(h))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


def sprite():
    """Horizontal sheet: len(WAVE) frames of 32x32."""
    n = len(WAVE)
    px = bytearray(32 * n * 32 * 4)
    for f, top in enumerate(WAVE):
        art = [list(r) for r in ART]
        for y in range(top, top + 2):
            art[y][14] = art[y][15] = "O"
        for y, row in enumerate(art):
            for x, c in enumerate(row):
                color = {"O": ORANGE, "E": EYE}.get(c)
                if not color:
                    continue
                for dy in (0, 1):
                    for dx in (0, 1):
                        i = ((y * 2 + dy) * 32 * n + f * 32 + x * 2 + dx) * 4
                        px[i:i + 4] = bytes(color)
    write_png(OUT / "clawd.png", 32 * n, 32, px)


def tone(notes, path, rate=22050):
    """notes: list of (freq_hz, start_s, dur_s). Soft sine with exp decay."""
    total = max(s + d for _, s, d in notes) + 0.05
    n = int(total * rate)
    buf = [0.0] * n
    for f, start, dur in notes:
        for i in range(int(dur * rate)):
            t = i / rate
            env = math.exp(-t * 9) * min(1.0, t * 400)
            j = int(start * rate) + i
            if j < n:
                buf[j] += math.sin(2 * math.pi * f * t) * env * 0.5
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"".join(struct.pack("<h", int(max(-1, min(1, v)) * 32767)) for v in buf))


if __name__ == "__main__":
    OUT.mkdir(exist_ok=True)
    sprite()
    tone([(1046.5, 0.0, 0.45)], OUT / "chime.wav")  # Done: soft chime
    tone([(880.0, 0.0, 0.2), (1174.7, 0.14, 0.3)], OUT / "ping.wav")  # needs you
