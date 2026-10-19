# clawdtoast

A tiny Clawd waves in the corner of your screen, with a chime, when Claude Code finishes a turn or needs you. It runs only while a Claude Code session is open. This repo is at **M3**: macOS and Linux (X11 and Wayland). Windows (M2) is not started; see the design doc for the plan.

## Install

### As a Claude Code plugin (recommended)

```
/plugin marketplace add /path/to/clawdtoast      # or the git URL once pushed
/plugin install clawdtoast@clawdtoast
```

The plugin's hooks call `scripts/clawdtoast.sh`, which runs the first binary it finds: `$CLAWDTOAST_BIN`, `<plugin>/bin/clawdtoast`, `<plugin>/target/release/clawdtoast`, then `clawdtoast` on `PATH`. With none of those, the first `SessionStart` builds one in the background with `cargo build --release` (about 20 s); hooks do nothing until it finishes.

### Without the plugin

```
cargo build --release
target/release/clawdtoast install      # adds the hooks to ~/.claude/settings.json (backup saved first)
target/release/clawdtoast uninstall    # removes only clawdtoast's entries
```

`install` refuses to run while the plugin is installed (the hooks would fire twice) unless you pass `--force`. Try it any time with `clawdtoast test`, which shows one card of each kind.

### Linux

Build needs `pkg-config` and the ALSA headers (`libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora) for the sound output; running needs a TrueType sans font (DejaVu, Liberation or Noto, or anything `fc-match sans` finds) and, for sound, ALSA/PipeWire/PulseAudio.

How a card is shown depends on the session:

| Session | How | Notes |
|---|---|---|
| X11 | Override-redirect window, so no window manager can focus, frame or tile it | Placed on the monitor under the pointer, inside the panel-free work area (`_NET_WORKAREA`). Fades need a compositor; without one the card is drawn opaque and just appears and disappears. Click focuses the window that owns an ancestor PID of the Claude Code process (`_NET_ACTIVE_WINDOW`); terminals that run every window in one server process (gnome-terminal) are not found, and the click then only dismisses. |
| Wayland with `wlr-layer-shell` (sway, Hyprland, river, KDE, COSMIC, ...) | A layer surface on the overlay layer, no keyboard interactivity | The compositor picks the output (Wayland gives clients no pointer position). Click dismisses; Wayland cannot raise another client's window. |
| Wayland without it (GNOME) | A desktop notification through `notify-send`, or `gdbus` if that is missing | No Clawd animation; the desktop owns placement and timing. A new alert for a session replaces its previous notification where the notification daemon supports it. |
| No display (SSH, a bare VT) | Nothing: the hook exits at once and starts no daemon | |

`CLAWDTOAST_BACKEND=wayland|x11|notify` forces a path (the others stay as fallbacks), for testing or desktops that misreport themselves. The socket and lock live in `$XDG_RUNTIME_DIR/clawdtoast/`; the log is `~/.local/state/clawdtoast/clawdtoast.log`.

## What it does

| Claude Code event | Card | Sound |
|---|---|---|
| `Stop` | Done · project, first sentence of the last assistant message | chime |
| `Notification` `permission_prompt` | Needs permission · project, the hook's message | ping |
| `Notification` `idle_prompt` | Waiting for you · project, the hook's message | ping |
| `SubagentStop` | off by default | ping |

- Cards sit in a corner of the screen holding the mouse (top-right by default), above full-screen apps and on every Space, and never take focus, neither when they appear nor when you click one (no Accessibility permission needed). Clawd waves at 8 fps.
- Up to 3 cards stack; more wait in a queue. A new alert for a session that already has a card replaces it. With more than one session open, the title carries a 4-character session tag.
- A card fades in over 150 ms, stays 5 s, fades out over 300 ms. Hovering pauses the timer; clicking dismisses it.
- The daemon starts on the first `SessionStart`, forgets sessions whose Claude Code process died (checked every 30 s), and exits 10 s after the last session closes. It draws nothing itself: it starts a short-lived `clawdtoast overlay` process when a card is due, which exits 3 s after the last card disappears. That keeps the always-running part small.

## Config

`~/.config/clawdtoast/config.toml`, every key optional. Run `clawdtoast reload` to apply changes to a running daemon (a card already on screen keeps the old settings).

```toml
duration_secs = 5
position = "top-right"      # top-left | top-right | bottom-left | bottom-right
max_stack = 3               # 1-6

[sound]
enabled = true
volume = 0.6
# done = "~/sounds/ding.wav"       # WAV only
# attention = "~/sounds/ping.wav"

[events]
stop = true
permission_prompt = true
idle_prompt = true
subagent_stop = false
```

Environment overrides (handy for tests): `CLAWDTOAST_CONFIG`, `CLAWDTOAST_DURATION_SECS`, `CLAWDTOAST_MUTE`, `CLAWDTOAST_SUBAGENT`, `CLAWDTOAST_DIR` (socket/lock/log dir, default `~/Library/Application Support/clawdtoast`; on Linux the socket and lock default to `$XDG_RUNTIME_DIR/clawdtoast` and the log to `~/.local/state/clawdtoast`), `CLAWDTOAST_BACKEND` (Linux only, see above), `CLAWDTOAST_GRACE_SECS` (10), `CLAWDTOAST_LIVENESS_SECS` (30), `CLAWDTOAST_HEADLESS` (log cards instead of drawing them). Log: `clawdtoast.log` in the support dir.

## Development

```
cargo test                          # unit tests
tests/integration.sh [binary]       # headless lifecycle, config, install/uninstall tests
scripts/check_budgets.sh [binary]   # resource budgets (CLAWDTOAST_CI_GUI=1 adds the overlay checks)
python3 -I tools/gen_assets.py      # regenerate assets/ (sprite sheet + sounds)
scripts/release.sh                  # universal binary in dist/, signed (see below)
```

CI (`.github/workflows/ci.yml`) runs the first three on macOS, and on Ubuntu those plus the display scenarios below.

The Linux display scenarios (`tests/linux/x11.sh`, `wayland.sh`, `fallback.sh`) drive a real Xvfb and a headless sway and check windows and screenshot pixels. On a Mac, `tests/linux/run.sh [test|lifecycle|budgets|x11|x11-comp|wayland|fallback|all]` builds the binary in a Linux container (Docker or colima) and runs them; screenshots land in `target/linux-shots/`.

### Budgets

| Metric | Limit | Measured here |
|---|---|---|
| Release binary | 3 MB | 1.1 MB |
| Daemon memory, idle | 12 MB (design doc said 8; see below) | 8.9 MB |
| Overlay memory, 3 cards showing | 90 MB | 69 MB |
| Hook shim p95 | 20 ms | 16 ms |
| Event to first card, cold daemon | 250 ms | 135 ms |

The design doc's 8 MB / 20 MB assumed one process doing everything; AppKit alone costs about 30 MB, so the UI moved to the short-lived overlay process and the budgets were re-cut around that split.

### Signing and notarization

`scripts/release.sh` builds arm64 + x86_64, joins them with `lipo`, signs, and zips. With `CODESIGN_IDENTITY="Developer ID Application: ..."` it signs for distribution, and with `NOTARY_PROFILE` it submits to Apple's notary service. **Only the ad-hoc path has been run**: no Developer ID certificate was available when this was written, so signing with one and notarizing are untested.

## Known gaps

- **Linux coverage:** verified against Xvfb (with and without `xcompmgr`) and headless sway in a container: placement, stacking, timeout, config position, the notification fallback, no-display behaviour and lifecycle. Overlay memory there is about 44 MB with 3 cards (45 MB X11, 44 MB Wayland; the 90 MB budget is shared with macOS). **Not tested:** a real GNOME, KDE or Hyprland session, multiple monitors, HiDPI scaling, pointer hover and click (the headless setups have no pointer), the click-to-focus path (`x11::focus_pid`), the fade look under a real compositor, and a real notification daemon (a stub `notify-send` stands in). A Linux desktop session is the next thing to try.
- **Linux fonts:** same as macOS, loaded from the system at runtime (DejaVu, Liberation, Noto, else `fc-match sans`). DejaVu is wider than Arial, so long titles cut off sooner.
- **Linux X11 libraries:** winit loads `libX11`, `libXcursor`, `libXi`, `libXrandr` and `libxkbcommon(-x11)` at runtime. If one is missing the overlay logs it and falls back to notifications instead of crashing.

- **Font:** loads Arial from the system at runtime; embedding an open-licensed subset is still open.
- **Do Not Disturb / Focus muting** (design doc config key `mute_in_focus_mode`) is not implemented; macOS exposes no public API for it.
- **Artwork:** `assets/clawd.png` is a hand-drawn placeholder sheet (4 frames side by side, frame size = image height). Replace it with the official art and nothing else changes. Check licensing before making the repo public.
- **Overlay race:** an alert that arrives in the instant the idle overlay exits is dropped (the daemon retries only when the pipe is already closed).
- `vendor/softbuffer` is softbuffer 0.4.8 with a one-line patch so macOS honours alpha (see `vendor/softbuffer/PATCH.md`). `fontdue` is pinned to 0.9.3 because 0.9.5 needs a newer Rust than 1.86.
