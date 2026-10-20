#!/bin/bash
# Wayland layer-shell scenario, run inside the container (see run.sh): headless sway (wlroots) on a
# solid blue background, three cards, pixel checks on a grim screenshot.
# Usage: wayland.sh BIN OUT
set -u
BIN=$1 OUT=$2
mkdir -p "$OUT"
export CLAWDTOAST_DIR="$(mktemp -d)" CLAWDTOAST_MUTE=1 CLAWDTOAST_PID=$$ CLAWDTOAST_GRACE_SECS=1
unset DISPLAY CLAWDTOAST_HEADLESS CLAWDTOAST_BACKEND
# On a shared machine (CI) a daemon left by an earlier scenario would skew the process checks.
pkill -f "$BIN (daemon|overlay)" 2>/dev/null; sleep 0.5
trap 'pkill -f "$BIN (daemon|overlay)" 2>/dev/null' EXIT
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }

export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
cat > "$XDG_RUNTIME_DIR/sway.conf" <<'EOF'
output * bg #3366aa solid_color
output HEADLESS-1 resolution 1280x720
EOF
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
  sway -c "$XDG_RUNTIME_DIR/sway.conf" >"$OUT/sway.log" 2>&1 &
SWAY=$!
for _ in $(seq 100); do ls "$XDG_RUNTIME_DIR"/wayland-* >/dev/null 2>&1 && break; sleep 0.1; done
export WAYLAND_DISPLAY=$(basename "$(ls "$XDG_RUNTIME_DIR"/wayland-[0-9] | head -1)")
sleep 0.5

px() { convert "$1" -format "%[hex:p{$2,$3}]" info:; }

echo '{"session_id":"aaaa1111","cwd":"/x/proj"}' | "$BIN" hook session-start
"$BIN" test &   # three cards, 1.2 s apart
TESTPID=$!
sleep 3.2
SHOT="$OUT/wayland.png"
grim "$SHOT"
echo "info overlay rss with 3 cards: $(ps -o rss= -p "$(pgrep -n -f "$BIN overlay")" | tr -d ' ') KB"
BG=3366AA
# Card slots (logical px): x 944..1264, y 16..112, 120..216, 224..320.
for y in 64 168 272; do
  c=$(px "$SHOT" 1100 $y)
  check "card visible at y=$y (pixel $c is not the background)" "[ '$c' != $BG ] && [ '$c' != '' ]"
done
check "nothing drawn mid-screen" "[ \"\$(px $SHOT 640 360)\" = $BG ]"
check "nothing drawn below the stack" "[ \"\$(px $SHOT 1100 400)\" = $BG ]"
check "nothing drawn left of the cards" "[ \"\$(px $SHOT 900 64)\" = $BG ]"
check "card corner is transparent (rounded)" "[ \"\$(px $SHOT 945 17)\" = $BG ]"

wait $TESTPID   # not a bare `wait`: that would also wait for sway
sleep 6.5
grim "$OUT/wayland-after.png"
check "cards gone after timeout" "[ \"\$(px $OUT/wayland-after.png 1100 64)\" = $BG ]"
echo '{"session_id":"aaaa1111"}' | "$BIN" hook session-end
sleep 4
check "overlay and daemon exit when idle" "! pgrep -f '$BIN (overlay|daemon)' >/dev/null"
grep -E "latency_ms|wayland|overlay" "$CLAWDTOAST_DIR/clawdtoast.log" | tail -8

# Position config: bottom-left.
printf 'position = "bottom-left"\n' > "$CLAWDTOAST_DIR/config.toml"
export CLAWDTOAST_CONFIG="$CLAWDTOAST_DIR/config.toml"
echo '{"session_id":"bbbb2222","cwd":"/x/proj"}' | "$BIN" hook session-start
echo '{"session_id":"bbbb2222","cwd":"/x/proj","notification_type":"idle_prompt","message":"Claude is waiting"}' | "$BIN" hook notification
sleep 1.2
grim "$OUT/wayland-bottom-left.png"
c=$(px "$OUT/wayland-bottom-left.png" 100 $((720 - 16 - 48)))
check "bottom-left card visible (pixel $c)" "[ '$c' != $BG ]"
check "top-right empty in bottom-left mode" "[ \"\$(px $OUT/wayland-bottom-left.png 1100 64)\" = $BG ]"
echo '{"session_id":"bbbb2222"}' | "$BIN" hook session-end

kill $SWAY 2>/dev/null
exit $fail
