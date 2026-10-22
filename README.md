# clawdtoast

A tiny Clawd waves in the corner of your screen, with a chime, when Claude Code or Codex finishes a turn or needs you. Click the card to jump to the terminal running that session.

It runs while a supported agent session is open, uses about 9 MB while idle, and works on macOS, Windows 10+ and Linux (X11 and Wayland).

## Install

### Prebuilt binaries

Download the archive for Windows x86_64, Linux x86_64 or universal macOS from
[GitHub Releases](https://github.com/faletto/clawdtoast/releases). Each archive contains
the executable, this README, the license and `bae.toml`. Extract it before installing
the hooks using the commands below. On Windows, put `clawdtoast.exe` in a simple path
such as `D:\Tools\clawdtoast`.

To select Bae, copy the bundled `bae.toml` to `~/.config/clawdtoast/config.toml`
(`%USERPROFILE%\.config\clawdtoast\config.toml` on Windows), then run `clawdtoast reload`.
If you already have a config, merge its `[animation]` section to preserve your other settings.
The macOS download has an ad-hoc signature and is not notarized; see [Releasing](#releasing).

### As a Claude Code plugin (recommended)

```
/plugin marketplace add faletto/clawdtoast
/plugin install clawdtoast@clawdtoast
```

The first hook after installing builds the binary in the background with `cargo build --release` (about 20 s, needs a [Rust toolchain](https://rustup.rs)); cards start appearing once it finishes. To update later, run `/plugin marketplace update clawdtoast` and `/reload-plugins`. A newer build automatically replaces the daemon of an older one.

On Linux the build also needs `pkg-config` and the ALSA headers (`libasound2-dev` on Debian/Ubuntu, `alsa-lib-devel` on Fedora, `alsa-lib` on Arch); see [Linux](#platform-notes) below for what has to be installed to run it.

### Without the plugin

```
git clone https://github.com/faletto/clawdtoast
cd clawdtoast
cargo build --release
target/release/clawdtoast install      # adds the hooks to ~/.claude/settings.json (backup saved first)
target/release/clawdtoast uninstall    # removes only clawdtoast's entries
```

`install` refuses to run while the plugin is installed (the hooks would fire twice) unless you pass `--force`. Try it any time with `clawdtoast test`, which shows one card of each kind.

### With Codex

Build the same binary with `cargo build --release`, then install the Codex hooks:

```sh
target/release/clawdtoast install codex       # merges into ~/.codex/hooks.json
target/release/clawdtoast uninstall codex     # removes only ClawdToast's Codex commands
```

On Windows use `target\release\clawdtoast.exe`, located in a path without spaces or shell
metacharacters (Codex's Windows hook runner can fail to launch quoted executable paths).
The installer honors `CODEX_HOME` when set,
backs up existing `hooks.json` before modifying it, and preserves other hooks. You can install
both Claude Code and Codex adapters; they share one daemon.

**In Codex, run `/hooks` to review and trust the installed hooks.** Codex skips untrusted
commands until you approve them. Re-install after moving or rebuilding the binary at a different
path, then review any updated hook definitions. Some Codex environments may restrict user hooks
through managed configuration.

## What it does

| Claude Code event | Card | Sound |
|---|---|---|
| `Stop` | Done · project, first sentence of the last assistant message | chime |
| `Notification` `permission_prompt` | Needs permission · project, the hook's message | ping |
| `Notification` `idle_prompt` | Waiting for you · project, the hook's message | ping |
| `SubagentStop` | off by default | ping |

Codex maps `Stop` (using `last_assistant_message`), `PermissionRequest` (tool name only),
and `SubagentStop` to the same card types. `SessionStart` and `SessionEnd` track Codex sessions
without colliding with Claude sessions. The Codex adapter never approves or denies tool calls.
Codex does not currently provide a direct equivalent of Claude's `idle_prompt` event.

- Cards sit in a corner of the screen holding the mouse (top-right by default), above other windows, and never take focus, neither when they appear nor when you click one. On macOS they also float above full-screen apps and on every Space, with no Accessibility permission needed. Clawd waves at 8 fps by default.
- Up to 3 cards stack; more wait in a queue. A new alert for a session that already has a card replaces it. With more than one session open, the title carries a 4-character session tag.
- A card fades in over 150 ms, stays 5 s, fades out over 300 ms. Hovering pauses the timer.
- **Clicking a card dismisses it and brings forward the terminal or IDE hosting that session.** For sessions with no terminal above them (background sessions), it falls back to the app named in the hook's environment (`__CFBundleIdentifier`, `TERM_PROGRAM`, Ghostty or kitty variables).
- The daemon starts on the first `SessionStart`, forgets sessions whose Claude Code process died (checked every 30 s), and exits 10 s after the last session closes. It draws nothing itself: it starts a short-lived `clawdtoast overlay` process when a card is due, which exits 3 s after the last card disappears. That keeps the always-running part small.

## Platform notes

**macOS.** State lives in `~/Library/Application Support/clawdtoast`. The release script builds a universal binary (see [Releasing](#releasing)).

**Windows.** Claude Code runs hooks through Git Bash, so the plugin's launcher script works as is, and `install` writes the exe path with forward slashes for the same reason. `install codex` writes an unquoted absolute path, which must contain no spaces or shell metacharacters because Codex's Windows hook runner mishandles quoted program tokens.

- State lives in `%LOCALAPPDATA%\clawdtoast`; the daemon listens on a loopback port recorded in `clawdtoast.sock`. The config is still `~\.config\clawdtoast\config.toml`.
- Cards have hard-edged rounded corners (a window region, not per-pixel alpha). They are topmost and never take focus, but they do not appear over exclusive-fullscreen apps.
- Clicking a card raises the window of the nearest ancestor process that has one (Windows Terminal, VS Code, ...). Classic `conhost` consoles are not an ancestor of `claude.exe`, so those are not raised.
- CI builds and tests it on `windows-latest`; the bash integration tests and budget checks run on macOS and Linux only.

**Linux.** Needs a TrueType sans font (DejaVu, Liberation or Noto, or anything `fc-match sans` finds) and, for sound, ALSA, PipeWire or PulseAudio. On X11 it also needs `libX11`, `libXcursor`, `libXi`, `libXrandr` and `libxkbcommon` with its X11 part, which any X11 desktop already has; if one is missing the overlay logs which and falls back to notifications. State: the socket and lock live in `$XDG_RUNTIME_DIR/clawdtoast/`, the log and the rest in `~/.local/state/clawdtoast/`.

How a card is shown depends on the session:

| Session | How | Notes |
|---|---|---|
| X11 | An override-redirect window, so no window manager can focus, frame or tile it | Placed on the monitor under the pointer, inside the panel-free work area (`_NET_WORKAREA`). Fades need a compositor; without one the card is drawn opaque and just appears and disappears. Click focuses the window owning an ancestor PID of Claude Code (`_NET_ACTIVE_WINDOW`); terminals that run every window in one server process (gnome-terminal) are not found, and the click then only dismisses. |
| Wayland with `wlr-layer-shell` (sway, Hyprland, river, KDE, COSMIC, ...) | A layer surface on the overlay layer with no keyboard interactivity | The compositor picks the monitor, since Wayland gives clients no pointer position. A click only dismisses: Wayland cannot raise another client's window. |
| Wayland without it (GNOME) | The X11 override-redirect window above, through XWayland | Same card as on X11, placed below the top bar. The cards are hidden while GNOME's Activities overview is open, and a native Wayland full-screen app can cover them. Without XWayland (`DISPLAY` unset) it falls back to a desktop notification through `notify-send`, or `gdbus` if that is missing: no Clawd animation, the desktop owns placement and timing, and a new alert for a session replaces its previous notification where the notification daemon supports it. |
| No display (SSH, a bare VT) | Nothing: the hook exits at once and starts no daemon | |

`CLAWDTOAST_BACKEND=wayland|x11|notify` forces one path (the others stay as fallbacks), for testing or for desktops that misreport themselves. Tried in virtual machines on GNOME (through XWayland: placement, hover, click) and KDE Plasma 6 (layer-shell at 1x and 2x), but not on real hardware, on Hyprland, or with several monitors; see [Limitations](#limitations).

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

[animation]
enabled = false             # set true to choose a sprite for all agents
sprite = "auto"             # auto, clawd, codex, bae, or custom
# file = "~/sprites/my-character.png"  # custom RGBA PNG sprite sheet
fps = 8                     # selected animation speed; 1-60 fps (e.g. 11 for Bae)
size = 88                   # Bae/custom character size in logical pixels (32-88)
```

By default (`enabled = false`), Claude notifications display Clawd and Codex notifications display the 4-frame OpenAI spinner. To choose one for **all** notifications, set `enabled = true` and `sprite = "clawd"`, `"codex"`, or `"bae"`; Bae is bundled as an 11-frame 64px sheet, displayed at 88 logical pixels by default. For Bae's original frame rate, set `fps = 11`. To use your own character, set `enabled = true`, `sprite = "custom"`, and `file` to a local **RGBA PNG** sprite sheet. Existing configs using `enabled = true` with just `file` continue to work.

Use one square PNG for a static sprite, or square frames side by side in a horizontal PNG strip (for example, 11 frames of 64x64 pixels make a 704x64 sheet). Nearest-neighbor scaling keeps pixel edges sharp at all display scales; alpha is thresholded to fully transparent or fully opaque. `fps` controls the selected sprite's animation while hovering, which pauses only the dismissal timer. MP4/GIF files must first be converted to PNG strips. When a custom file is missing or invalid, the app logs the error and uses the original per-agent defaults. Sprite configuration changes take effect for new overlay processes; use `clawdtoast reload` after editing the config.

Environment overrides (handy for tests): `CLAWDTOAST_CONFIG`, `CLAWDTOAST_DURATION_SECS`, `CLAWDTOAST_MUTE`, `CLAWDTOAST_SUBAGENT`, `CLAWDTOAST_DIR` (socket/lock/log dir), `CLAWDTOAST_BACKEND` (Linux: `wayland`, `x11` or `notify`), `CLAWDTOAST_GRACE_SECS` (10), `CLAWDTOAST_LIVENESS_SECS` (30), `CLAWDTOAST_PID` (the agent pid to watch and focus), `CLAWDTOAST_HEADLESS` (log cards instead of drawing them). `CODEX_HOME` overrides Codex's configuration directory. Log: `clawdtoast.log` in the state dir.

## Troubleshooting

- **No cards after installing or updating:** the binary is still building. Check `target/build.log` in the plugin's folder (`~/.claude/plugins/cache/clawdtoast/clawdtoast/<version>`).
- **Codex notifications never appear:** run `/hooks` in Codex and ensure ClawdToast's hooks are trusted. Check your `CODEX_HOME` and the `hooks.json` there. If Codex reports `Hook failed` with exit code 1 on Windows, rebuild and rerun `install codex` to replace older quoted commands, then review the updated hooks in `/hooks`.
- **Codex stays on `Running hook` or alerts disappear intermittently:** rebuild the binary used by `hooks.json` in your `CODEX_HOME` (default `~/.codex`). The Windows hook reader and daemon must both be up to date: older versions could wait for stdin EOF or inherit extra PowerShell output pipe handles, leaving Codex waiting after the hook exited. Updating only the repo's binary does not update a separate installed copy. On Windows, run `./tests/windows/codex-hooks.ps1 -Exe target/release/clawdtoast.exe` to exercise PowerShell launches, open input pipes and concurrent events.
- **Click doesn't focus anything:** `clawdtoast.log` says why (`no app found above pid ...`, `card has no pid ...`).
- **Linux, no card and no notification:** `clawdtoast.log` names the cause: missing X11 libraries, a Wayland compositor without layer-shell and no X server (`DISPLAY` unset) and no `notify-send`/`gdbus` installed (`libnotify-bin` on Debian/Ubuntu), or no display in the hook's environment (`DISPLAY`/`WAYLAND_DISPLAY` unset, as over SSH or in a stale tmux server).
- **Two daemons running:** a daemon from before 0.1.3 can't be replaced automatically; kill it and the next hook starts the current one.

## Development

```
cargo test                          # unit tests
./tests/windows/codex-hooks.ps1     # Windows: PowerShell capture, hook IPC and open input pipes
tests/integration.sh [binary]       # headless lifecycle, config, install/uninstall tests
scripts/check_budgets.sh [binary]   # resource budgets (CLAWDTOAST_CI_GUI=1 adds the overlay checks)
python3 -I tools/gen_assets.py      # regenerate assets/ (sprite sheet + sounds)
scripts/release.sh                  # universal macOS binary in dist/, signed (see below)
tests/linux/run.sh [scenario]       # Linux build + display tests in a container (see below)
```

CI (`.github/workflows/ci.yml`) runs the first three on macOS and Ubuntu, `cargo test` plus a release build on Windows, and the Linux display scenarios on Ubuntu. When pushing a change, bump `version` in `.claude-plugin/plugin.json` so `/plugin marketplace update` picks it up.

### Linux tests

`tests/linux/x11.sh`, `wayland.sh` and `fallback.sh` drive a real Xvfb (with and without `xcompmgr`) and a headless sway, and check windows, screenshot pixels, the notification fallback and no-display behaviour. They run directly on an Ubuntu box with the packages listed in `tests/linux/Dockerfile`, which is also how CI runs them. On a Mac, `tests/linux/run.sh [test|lifecycle|budgets|x11|x11-comp|x11-wayland|wayland|fallback|all]` builds the binary in a Linux container (Docker, or colima) and runs them; screenshots land in `target/linux-shots/`.

### Budgets

| Metric | Limit | Measured |
|---|---|---|
| Release binary | 3 MB | 1.1 MB |
| Daemon memory, idle | 12 MB | 8.9 MB |
| Overlay memory, 3 cards showing | 90 MB | 69 MB |
| Hook shim p95 | 20 ms | 16 ms |
| Event to first card, cold daemon | 250 ms | 135 ms |

On Linux (measured in a container): binary 1.8 MB, idle daemon 3.9 MB, shim p95 4 ms, overlay with 3 cards about 44 MB. The UI lives in the short-lived overlay process because AppKit alone costs about 30 MB; the always-running daemon stays small.

### Releasing

After merging and checking CI on `main`, create a tag matching the version in
`.claude-plugin/plugin.json` (for example, `v0.1.12`) and push it to GitHub. The release
workflow builds and tests Windows, Linux and universal macOS binaries, then publishes
all three archives with `SHA256SUMS.txt` to the tagged release. Packaging changes also
build on pull requests. The workflow can be run manually with an existing tag to rebuild
its assets; it refuses tags outside `main` or with a mismatched version.

`scripts/release.sh` builds arm64 + x86_64, joins them with `lipo`, signs, and zips. With `CODESIGN_IDENTITY="Developer ID Application: ..."` it signs for distribution, and with `NOTARY_PROFILE` it submits to Apple's notary service. Only the ad-hoc signing path has been exercised; Developer ID signing and notarization are untested.

## Limitations

- **Font:** loads Arial (macOS, Windows) or DejaVu/Liberation/Noto (Linux) from the system at runtime. DejaVu is wider than Arial, so long titles are cut off sooner on Linux.
- **Linux coverage:** verified on Xvfb and headless sway (placement, stacking, timeout, config position, fallback, lifecycle). Not tested: a real GNOME, KDE or Hyprland session, multiple monitors, HiDPI, pointer hover and click (the headless setups have no pointer), X11 click-to-focus, fades under a real compositor, and a real notification daemon (a stub `notify-send` stands in).
- **Do Not Disturb / Focus Assist:** not respected; neither OS exposes a public API for it.
- **Artwork:** Bundled sprites are `assets/clawd.png` (Claude Code), `assets/openai-spin.png` (Codex), and `assets/bae.png` (optional). Personal artwork is opt-in via `[animation]`, loaded at runtime. Source video compression artifacts may remain visible in sheets extracted from MP4s; converting to PNG avoids further losses but cannot restore original pixel colors.
- **Overlay race:** an alert that arrives in the instant the idle overlay exits is dropped.
- `vendor/softbuffer` is softbuffer 0.4.8 with a one-line patch so macOS honours alpha (see `vendor/softbuffer/PATCH.md`). `fontdue` is pinned to 0.9.3 because 0.9.5 needs a newer Rust than 1.86.

## License

MIT
