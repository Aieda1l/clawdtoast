#!/bin/sh
# Launcher the plugin hooks call. Finds the clawdtoast binary and execs it. When there is
# none yet (fresh `/plugin install` or update), builds it once in the background if cargo is
# available, on whichever hook fires first; hooks are no-ops until then. On session start it
# also stops daemons left over from other builds. Always exits 0 so Claude Code is never blocked.
ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"

BIN=""
for c in "$CLAWDTOAST_BIN" "$ROOT/bin/clawdtoast" "$ROOT/bin/clawdtoast.exe" "$ROOT/target/release/clawdtoast" "$ROOT/target/release/clawdtoast.exe" "$(command -v clawdtoast)"; do
  if [ -n "$c" ] && [ -x "$c" ]; then
    BIN="$c"
    break
  fi
done

if [ -n "$BIN" ]; then
  if [ "$2" = "session-start" ]; then
    # Any daemon not running this binary is from an older (or other) install: stop it, and the
    # next hook starts ours. Its overlay exits by itself when the daemon's pipe closes.
    ps -axo pid=,command= 2>/dev/null | while read -r pid cmd; do
      case "$cmd" in
        "$BIN daemon"*) ;;
        */clawdtoast\ daemon | */clawdtoast\ daemon\ --headless | */clawdtoast.exe\ daemon*) kill "$pid" 2>/dev/null ;;
      esac
    done
  fi
  exec "$BIN" "$@"
fi

if command -v cargo >/dev/null 2>&1 && [ ! -e "$ROOT/target/.building" ]; then
  mkdir -p "$ROOT/target"
  : > "$ROOT/target/.building"
  (
    cd "$ROOT" || exit 0
    nohup sh -c 'cargo build --release --quiet >target/build.log 2>&1; rm -f target/.building' >/dev/null 2>&1 &
  )
fi
cat >/dev/null 2>&1
exit 0
