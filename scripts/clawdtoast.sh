#!/bin/sh
# Launcher the plugin hooks call. Finds the clawdtoast binary and execs it. When there is
# none yet (fresh `/plugin install`), builds it once in the background if cargo is
# available; hooks are no-ops until then. Always exits 0 so Claude Code is never blocked.
ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"

for c in "$CLAWDTOAST_BIN" "$ROOT/bin/clawdtoast" "$ROOT/target/release/clawdtoast" "$(command -v clawdtoast)"; do
  if [ -n "$c" ] && [ -x "$c" ]; then
    exec "$c" "$@"
  fi
done

if [ "$2" = "session-start" ] && command -v cargo >/dev/null 2>&1 && [ ! -e "$ROOT/target/.building" ]; then
  mkdir -p "$ROOT/target"
  : > "$ROOT/target/.building"
  (
    cd "$ROOT" || exit 0
    nohup sh -c 'cargo build --release --quiet >target/build.log 2>&1; rm -f target/.building' >/dev/null 2>&1 &
  )
fi
cat >/dev/null 2>&1
exit 0
