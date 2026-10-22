#!/bin/bash
# Live X11 question-card lifecycle, including a hard deadline while hovered.
set -eu
BIN=$1 OUT=$2
mkdir -p "$OUT"
export DISPLAY=:98 CLAWDTOAST_DIR="$(mktemp -d)" CLAWDTOAST_MUTE=1 CLAWDTOAST_PID=$$
export CLAWDTOAST_CONFIG="$CLAWDTOAST_DIR/config.toml" CLAWDTOAST_LIVENESS_SECS=1
unset WAYLAND_DISPLAY CLAWDTOAST_HEADLESS
cat > "$CLAWDTOAST_CONFIG" <<'EOF'
duration_secs = 0.4
[attention]
persistent = true
max_age_secs = 5
question_ttl_secs = 30
EOF
Xvfb :98 -screen 0 1280x720x24 >/dev/null 2>&1 &
XPID=$!
cleanup() { pkill -f "$BIN (daemon|overlay)" 2>/dev/null || true; kill "$XPID" 2>/dev/null || true; }
trap cleanup EXIT
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep .1; done
cards() { xwininfo -root -tree | grep -c '"clawdtoast"' || true; }
assert_cards() {
  for _ in $(seq 30); do [ "$(cards)" -eq "$1" ] && return; sleep .1; done
  echo "expected $1 cards, got $(cards)"; cat "$CLAWDTOAST_DIR/clawdtoast.log"; exit 1
}
echo '{"session_id":"s","cwd":"/test"}' | "$BIN" codex-hook session-start
q='{"session_id":"s","cwd":"/test","turn_id":"t","tool_use_id":"blocking","tool_name":"request_user_input","tool_input":{"questions":[{"id":"a","question":"Which database?"},{"id":"b","question":"Which region?"}]}}'
echo "$q" | "$BIN" codex-hook pre-tool-use
assert_cards 1
sleep 1
assert_cards 1
echo '{"session_id":"s","turn_id":"t","last_assistant_message":"Finished other work."}' | "$BIN" codex-hook stop >/dev/null
assert_cards 2
sleep 1
assert_cards 1
echo '{"session_id":"s","turn_id":"t","tool_use_id":"blocking","tool_name":"request_user_input","tool_response":{"answers":{"a":{"answers":["SQLite"]}}}}' | "$BIN" codex-hook post-tool-use
assert_cards 1
import -window root "$OUT/attention-persistent.png"
echo '{"session_id":"s","turn_id":"t","tool_use_id":"blocking","tool_name":"request_user_input","tool_response":"{\"answers\":{\"b\":{\"answers\":[\"West\"]}}}"}' | "$BIN" codex-hook post-tool-use
assert_cards 0
async='{"session_id":"s","cwd":"/test","turn_id":"t2","tool_use_id":"async","tool_name":"request_user_input_async","tool_input":{"questions":[{"title":"When should we ship?"}]},"tool_response":{"accepted":true}}'
echo "$async" | "$BIN" codex-hook post-tool-use
assert_cards 1
echo '{"session_id":"s"}' | "$BIN" codex-hook session-end
assert_cards 0
echo '{"session_id":"s"}' | "$BIN" codex-hook session-start
echo "$async" | "$BIN" codex-hook post-tool-use
assert_cards 1
xdotool mousemove 1100 60
sleep 5.5
assert_cards 0
# Daemon replacement must withdraw its old overlay. Restoring the ledger must
# not reopen that card, while a distinct accepted call must still notify.
echo "${async/\"async\"/\"async-restart\"}" | "$BIN" codex-hook post-tool-use
assert_cards 1
python3 - "$CLAWDTOAST_DIR/clawdtoast.sock" <<'EOF'
import socket, sys
with socket.socket(socket.AF_UNIX) as client:
    client.connect(sys.argv[1])
    client.sendall(b'{"v":1,"type":"shutdown"}\n')
EOF
assert_cards 0
for _ in $(seq 30); do [ ! -e "$CLAWDTOAST_DIR/clawdtoast.sock" ] && break; sleep .1; done
echo "${async/\"async\"/\"async-restart\"}" | "$BIN" codex-hook post-tool-use
sleep .2
assert_cards 0
echo "${async/\"async\"/\"async-after-restart\"}" | "$BIN" codex-hook post-tool-use
assert_cards 1
echo '{"session_id":"s"}' | "$BIN" codex-hook session-end
assert_cards 0
echo 'ok   persistent attention survives ordinary timeout, coexists with Done, resolves silently, closes with session, expires while hovered and handles daemon restart'
