#!/bin/bash
# Desktop-notification fallback and no-display behaviour, run inside the container (see run.sh).
# A stub notify-send records its arguments so no notification daemon is needed.
# Usage: fallback.sh BIN
set -u
BIN=$1
export CLAWDTOAST_DIR="$(mktemp -d)" CLAWDTOAST_MUTE=1 CLAWDTOAST_PID=$$ CLAWDTOAST_GRACE_SECS=1
unset DISPLAY WAYLAND_DISPLAY CLAWDTOAST_HEADLESS CLAWDTOAST_BACKEND
# On a shared machine (CI) a daemon left by an earlier scenario would skew the process checks.
pkill -f "$BIN (daemon|overlay)" 2>/dev/null; sleep 0.5
trap 'pkill -f "$BIN (daemon|overlay)" 2>/dev/null' EXIT
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
daemons() { pgrep -f "$BIN daemon" | wc -l | tr -d ' '; }

STUBS=$(mktemp -d)
CALLS="$STUBS/calls"
cat > "$STUBS/notify-send" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$CALLS"
echo 101
EOF
chmod +x "$STUBS/notify-send"
export PATH="$STUBS:$PATH"

T="$CLAWDTOAST_DIR/t.jsonl"
echo '{"message":{"role":"assistant","content":[{"type":"text","text":"Fixed the parser. Added tests."}]}}' > "$T"
stop() { echo "{\"session_id\":\"aaaa1111\",\"cwd\":\"/x/proj\",\"transcript_path\":\"$T\"}" | "$BIN" hook stop; }

# 1. No display at all (SSH): the shim does nothing and starts no daemon.
echo '{"session_id":"aaaa1111","cwd":"/x/proj"}' | "$BIN" hook session-start
stop
sleep 0.5
check "no display: hooks start no daemon" "[ \$(daemons) -eq 0 ]"
check "no display: hooks exit 0" "echo '{}' | $BIN hook stop"

# 2. Wayland session whose compositor cannot be reached or has no layer-shell -> notification.
export WAYLAND_DISPLAY=wayland-does-not-exist
echo '{"session_id":"aaaa1111","cwd":"/x/proj"}' | "$BIN" hook session-start
stop
sleep 1
check "wayland without layer-shell falls back to notify-send" "grep -q 'Done · proj' $CALLS"
check "notification carries the body" "grep -q 'Fixed the parser.' $CALLS"
check "notification replaces per session" "grep -q 'x-canonical-private-synchronous:clawdtoast-aaaa1111' $CALLS"
check "notification honours the configured duration" "grep -q -- '--expire-time=5000' $CALLS"

# 3. Forced notify backend.
: > "$CALLS"
echo '{"session_id":"aaaa1111","cwd":"/x/proj"}' | "$BIN" hook session-end
sleep 3
check "daemon exits after the last session" "[ \$(daemons) -eq 0 ]"
export CLAWDTOAST_BACKEND=notify
echo '{"session_id":"bbbb2222","cwd":"/x/other"}' | "$BIN" hook session-start
echo '{"session_id":"bbbb2222","cwd":"/x/other","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}' | "$BIN" hook notification
sleep 1
check "forced notify backend delivers permission prompts" "grep -q 'Needs permission · other' $CALLS"
echo '{"session_id":"bbbb2222"}' | "$BIN" hook session-end
sleep 3
check "overlay process exits with the daemon" "! pgrep -f '$BIN (overlay|daemon)' >/dev/null"

# 4. Keep a persistent native notification addressable for silent replacement
# and CloseNotification even beyond the normal three-second overlay idle exit.
export CLAWDTOAST_CONFIG="$CLAWDTOAST_DIR/config.toml"
cat > "$CLAWDTOAST_CONFIG" <<'EOF'
duration_secs = 0.5
[attention]
persistent = true
max_age_secs = 10
EOF
cat > "$STUBS/gdbus" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$STUBS/dbus-calls"
EOF
chmod +x "$STUBS/gdbus"
: > "$CALLS"
echo '{"session_id":"persistent","cwd":"/x/proj","turn_id":"t","tool_use_id":"call","tool_name":"request_user_input","tool_input":{"questions":[{"id":"a","question":"Which database?"},{"id":"b","question":"Which region?"}]}}' | "$BIN" codex-hook pre-tool-use
sleep 3.5
check "native persistent notification keeps overlay addressable" "pgrep -f '$BIN overlay' >/dev/null"
check "native persistent notification has a finite timeout" "grep -Eq -- '--expire-time=[1-9][0-9]+' $CALLS && ! grep -q -- '--expire-time=0 ' $CALLS"
echo '{"session_id":"persistent","turn_id":"t","tool_use_id":"call","tool_name":"request_user_input","tool_response":{"answers":{"a":{"answers":["SQLite"]}}}}' | "$BIN" codex-hook post-tool-use
sleep .2
check "native answer update replaces the existing notification silently" "grep -q -- '--replace-id=101' $CALLS && grep -q -- 'suppress-sound:true' $CALLS"
echo '{"session_id":"persistent"}' | "$BIN" codex-hook session-end
sleep .2
check "session close withdraws native persistent notification" "grep -q 'CloseNotification 101' $STUBS/dbus-calls"

exit $fail
