# clawdpet

A tiny Clawd pops up in the corner of your screen, with a chime, when Claude Code finishes a turn or needs you. It runs only while a Claude Code session is open. See the design doc for the full plan; this repo is at **M0** (macOS proof of concept).

## Install (Claude Code plugin)

```
/plugin marketplace add /path/to/clawdpet      # or the git URL once pushed
/plugin install clawdpet@clawdpet
```

The plugin's hooks call `scripts/clawdpet.sh`, which runs the first binary it finds: `$CLAWDPET_BIN`, `<plugin>/bin/clawdpet`, `<plugin>/target/release/clawdpet`, then `clawdpet` on `PATH`. On a fresh install with none of those, the first `SessionStart` builds it in the background with `cargo build --release` (about 20 s); hooks do nothing until it finishes. Restart Claude Code after the build, or build it yourself:

```
cargo build --release
```

Try it without Claude Code: `target/release/clawdpet test` shows one card of each kind.

## What M0 does

| Claude Code event | Card | Sound |
|---|---|---|
| `Stop` | Done · project, first sentence of the last assistant message | chime |
| `Notification` `permission_prompt` | Needs permission · project, the hook's message | ping |
| `Notification` `idle_prompt` | Waiting for you · project, the hook's message | ping |
| `SubagentStop` | off by default (`CLAWDPET_SUBAGENT=1`) | ping |

Top-right of the screen holding the mouse, above full-screen apps and on every Space, never takes focus. Fades in over 150 ms, stays 5 s (hover pauses, click dismisses), fades out over 300 ms. Alerts for a session that already has a card replace it. The daemon starts on the first `SessionStart`, drops sessions whose Claude Code PID died (checked every 30 s), and exits 10 s after the last session closes.

### Not in M0 (per design doc milestones)
Wave animation, card stacking, config file, `install`/`uninstall`, Focus/do-not-disturb muting, budgets in CI, signing/notarization, focus detection, click-to-focus-terminal.

## Environment knobs

`CLAWDPET_DURATION_SECS` (5), `CLAWDPET_MUTE`, `CLAWDPET_SUBAGENT`, `CLAWDPET_DIR` (socket/lock/log dir; default `~/Library/Application Support/clawdpet`), `CLAWDPET_GRACE_SECS` (10), `CLAWDPET_LIVENESS_SECS` (30), `CLAWDPET_HEADLESS` (daemon logs cards instead of drawing them). Log: `clawdpet.log` in the support dir.

## Development

```
cargo test                      # unit tests
tests/integration.sh [binary]   # headless daemon lifecycle test
python3 -I tools/gen_assets.py  # regenerate assets/ (sprite + sounds)
```

## Known gaps vs. the design doc

- **Memory:** about 38 MB idle and 69 MB with a card showing, against the 8 / 20 MB budgets. Most of it is AppKit and the system font; the budgets need revisiting or a leaner window path in M1.
- **Font:** loads Arial from the system at runtime. Embedding an open-licensed subset is an M1 item.
- **Artwork:** `assets/clawd.png` is a hand-drawn placeholder. Replace it with the official Clawd art (32 × 32 RGBA PNG); nothing else changes. Check the licensing before making the repo public.
- `vendor/softbuffer` is softbuffer 0.4.8 with a one-line patch so macOS honours the alpha channel (rounded transparent corners). See `vendor/softbuffer/PATCH.md`.
- Pinned `fontdue = 0.9.3` because 0.9.5 needs a newer Rust than 1.86.
