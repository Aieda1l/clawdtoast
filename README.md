# clawdtoast

A tiny Clawd waves in the corner of your screen, with a chime, when Claude Code finishes a turn or needs you. Click the card to jump to the terminal running that session.

It runs only while a Claude Code session is open, uses about 9 MB while idle, and works on macOS and Windows 10+.

## Install

### As a Claude Code plugin (recommended)

```
/plugin marketplace add faletto/clawdtoast
/plugin install clawdtoast@clawdtoast
```

The first hook after installing builds the binary in the background with `cargo build --release` (about 20 s, needs a [Rust toolchain](https://rustup.rs)); cards start appearing once it finishes. To update later, run `/plugin marketplace update clawdtoast` and `/reload-plugins`. A newer build automatically replaces the daemon of an older one.

### Without the plugin

```
git clone https://github.com/faletto/clawdtoast
cd clawdtoast
cargo build --release
target/release/clawdtoast install      # adds the hooks to ~/.claude/settings.json (backup saved first)
target/release/clawdtoast uninstall    # removes only clawdtoast's entries
```

`install` refuses to run while the plugin is installed (the hooks would fire twice) unless you pass `--force`. Try it any time with `clawdtoast test`, which shows one card of each kind.

## What it does

| Claude Code event | Card | Sound |
|---|---|---|
| `Stop` | Done · project, first sentence of the last assistant message | chime |
| `Notification` `permission_prompt` | Needs permission · project, the hook's message | ping |
| `Notification` `idle_prompt` | Waiting for you · project, the hook's message | ping |
| `SubagentStop` | off by default | ping |

- Cards sit in a corner of the screen holding the mouse (top-right by default), above other windows, and never take focus, neither when they appear nor when you click one. On macOS they also float above full-screen apps and on every Space, with no Accessibility permission needed. Clawd waves at 8 fps.
- Up to 3 cards stack; more wait in a queue. A new alert for a session that already has a card replaces it. With more than one session open, the title carries a 4-character session tag.
- A card fades in over 150 ms, stays 5 s, fades out over 300 ms. Hovering pauses the timer.
- **Clicking a card dismisses it and brings forward the terminal or IDE hosting that session.** For sessions with no terminal above them (background sessions), it falls back to the app named in the hook's environment (`__CFBundleIdentifier`, `TERM_PROGRAM`, Ghostty or kitty variables).
- The daemon starts on the first `SessionStart`, forgets sessions whose Claude Code process died (checked every 30 s), and exits 10 s after the last session closes. It draws nothing itself: it starts a short-lived `clawdtoast overlay` process when a card is due, which exits 3 s after the last card disappears. That keeps the always-running part small.

## Platform notes

**macOS.** State lives in `~/Library/Application Support/clawdtoast`. The release script builds a universal binary (see [Releasing](#releasing)).

**Windows.** Claude Code runs hooks through Git Bash, so the plugin's launcher script works as is, and `install` writes the exe path with forward slashes for the same reason.

- State lives in `%LOCALAPPDATA%\clawdtoast`; the daemon listens on a loopback port recorded in `clawdtoast.sock`. The config is still `~\.config\clawdtoast\config.toml`.
- Cards have hard-edged rounded corners (a window region, not per-pixel alpha). They are topmost and never take focus, but they do not appear over exclusive-fullscreen apps.
- Clicking a card raises the window of the nearest ancestor process that has one (Windows Terminal, VS Code, ...). Classic `conhost` consoles are not an ancestor of `claude.exe`, so those are not raised.
- CI builds and tests it on `windows-latest`; the bash integration tests and budget checks run on macOS only.

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

Environment overrides (handy for tests): `CLAWDTOAST_CONFIG`, `CLAWDTOAST_DURATION_SECS`, `CLAWDTOAST_MUTE`, `CLAWDTOAST_SUBAGENT`, `CLAWDTOAST_DIR` (socket/lock/log dir), `CLAWDTOAST_GRACE_SECS` (10), `CLAWDTOAST_LIVENESS_SECS` (30), `CLAWDTOAST_PID` (the Claude Code pid to watch and focus), `CLAWDTOAST_HEADLESS` (log cards instead of drawing them). Log: `clawdtoast.log` in the state dir.

## Troubleshooting

- **No cards after installing or updating:** the binary is still building. Check `target/build.log` in the plugin's folder (`~/.claude/plugins/cache/clawdtoast/clawdtoast/<version>`).
- **Click doesn't focus anything:** `clawdtoast.log` says why (`no app found above pid ...`, `card has no pid ...`).
- **Two daemons running:** a daemon from before 0.1.3 can't be replaced automatically; kill it and the next hook starts the current one.

## Development

```
cargo test                          # unit tests
tests/integration.sh [binary]       # headless lifecycle, config, install/uninstall tests
scripts/check_budgets.sh [binary]   # resource budgets (CLAWDTOAST_CI_GUI=1 adds the overlay checks)
python3 -I tools/gen_assets.py      # regenerate assets/ (sprite sheet + sounds)
scripts/release.sh                  # universal macOS binary in dist/, signed (see below)
```

CI (`.github/workflows/ci.yml`) runs the first three on macOS, and `cargo test` plus a release build on Windows. When pushing a change, bump `version` in `.claude-plugin/plugin.json` so `/plugin marketplace update` picks it up.

### Budgets

| Metric | Limit | Measured |
|---|---|---|
| Release binary | 3 MB | 1.1 MB |
| Daemon memory, idle | 12 MB | 8.9 MB |
| Overlay memory, 3 cards showing | 90 MB | 69 MB |
| Hook shim p95 | 20 ms | 16 ms |
| Event to first card, cold daemon | 250 ms | 135 ms |

The UI lives in the short-lived overlay process because AppKit alone costs about 30 MB; the always-running daemon stays small.

### Releasing

`scripts/release.sh` builds arm64 + x86_64, joins them with `lipo`, signs, and zips. With `CODESIGN_IDENTITY="Developer ID Application: ..."` it signs for distribution, and with `NOTARY_PROFILE` it submits to Apple's notary service. Only the ad-hoc signing path has been exercised; Developer ID signing and notarization are untested.

## Limitations

- **Font:** loads Arial from the system at runtime.
- **Do Not Disturb / Focus Assist:** not respected; neither OS exposes a public API for it.
- **Artwork:** `assets/clawd.png` is a hand-drawn sprite sheet (4 frames side by side, frame size = image height). Replace it and nothing else changes.
- **Overlay race:** an alert that arrives in the instant the idle overlay exits is dropped.
- `vendor/softbuffer` is softbuffer 0.4.8 with a one-line patch so macOS honours alpha (see `vendor/softbuffer/PATCH.md`). `fontdue` is pinned to 0.9.3 because 0.9.5 needs a newer Rust than 1.86.

## License

MIT
