#!/bin/bash
# Lifecycle test against a headless daemon. Usage: tests/integration.sh [path/to/clawdtoast]
set -u
BIN="${1:-target/debug/clawdtoast}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export CLAWDTOAST_DIR="$(mktemp -d)" CLAWDTOAST_HEADLESS=1 CLAWDTOAST_MUTE=1 CLAWDTOAST_GRACE_SECS=1 CLAWDTOAST_LIVENESS_SECS=1
fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
log() { cat "$CLAWDTOAST_DIR/clawdtoast.log" 2>/dev/null; }
daemons() { pgrep -f "$BIN daemon" | wc -l | tr -d ' '; }

T="$CLAWDTOAST_DIR/t.jsonl"
echo '{"message":{"role":"assistant","content":[{"type":"text","text":"Fixed the parser. Added tests."}]}}' > "$T"

sleep 60 & FAKE=$!   # stands in for the Claude Code process
export CLAWDTOAST_PID=$FAKE

echo '{"session_id":"aaaa1111","cwd":"/x/proj"}' | "$BIN" hook session-start
check "shim exits 0 and daemon starts on SessionStart" "[ \$(daemons) -eq 1 ]"

echo "{\"session_id\":\"aaaa1111\",\"cwd\":\"/x/proj\",\"transcript_path\":\"$T\"}" | "$BIN" hook stop
echo '{"session_id":"aaaa1111","cwd":"/x/proj","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}' | "$BIN" hook notification
echo '{"session_id":"aaaa1111","cwd":"/x/proj","notification_type":"auth_success","message":"ignored"}' | "$BIN" hook notification
sleep 0.3
check "Stop alert carries first sentence of last assistant message" "log | grep -q 'card: Done · proj | Fixed the parser.'"
check "permission alert shown" "log | grep -q 'card: Needs permission · proj | Claude needs your permission to use Bash'"
check "unlisted notification types ignored" "! log | grep -q ignored"
echo 'not json' | "$BIN" hook stop; check "garbage stdin exits 0" "[ \$? -eq 0 ]"
"$BIN" hook bogus </dev/null; check "unknown event exits 0" "[ \$? -eq 0 ]"

kill $FAKE 2>/dev/null
wait $FAKE 2>/dev/null
sleep 4
check "daemon exits after fake Claude PID dies (liveness + grace)" "[ \$(daemons) -eq 0 ]"
check "socket removed on exit" "[ ! -e $CLAWDTOAST_DIR/clawdtoast.sock ]"

# Concurrent SessionStarts must spawn exactly one daemon.
unset CLAWDTOAST_PID
for i in 1 2 3 4 5 6; do echo "{\"session_id\":\"s$i\",\"cwd\":\"/x\"}" | "$BIN" hook session-start & done
wait
sleep 0.3
check "concurrent starts -> one daemon" "[ \$(daemons) -eq 1 ]"
for i in 1 2 3 4 5 6; do echo "{\"session_id\":\"s$i\"}" | "$BIN" hook session-end; done
sleep 3
check "daemon exits after last SessionEnd" "[ \$(daemons) -eq 0 ]"

# Codex hooks share the daemon, but have their own session namespace and input format.
echo '{"session_id":"c1","cwd":"/x/codex"}' | "$BIN" codex-hook session-start
codex_response=$(echo '{"session_id":"c1","cwd":"/x/codex","last_assistant_message":"Fixed the tests. More details."}' | "$BIN" codex-hook stop)
echo '{"session_id":"c1","cwd":"/x/codex","tool_name":"Bash"}' | "$BIN" codex-hook permission-request
sleep 0.3
check "Codex Stop produces valid neutral JSON" '[ "$codex_response" = "{}" ]'
check "Codex Stop uses last_assistant_message" "log | grep -q 'card: Done · codex | Fixed the tests.'"
check "Codex approval request uses the tool name" "log | grep -q 'card: Needs permission · codex | Codex needs approval for Bash'"
echo '{"session_id":"c1"}' | "$BIN" codex-hook session-end
sleep 3
check "Codex SessionEnd closes its session" "[ \$(daemons) -eq 0 ]"

# Config: events can be switched off, and `reload` applies a changed file to a running daemon.
export CLAWDTOAST_CONFIG="$CLAWDTOAST_DIR/config.toml"
printf '[events]\nstop = false\n' > "$CLAWDTOAST_CONFIG"
export CLAWDTOAST_PID=$$
echo '{"session_id":"cfg1","cwd":"/x/cfg"}' | "$BIN" hook session-start
echo "{\"session_id\":\"cfg1\",\"cwd\":\"/x/cfg\",\"transcript_path\":\"$T\"}" | "$BIN" hook stop
sleep 0.3
check "stop=false in config suppresses Done cards" "! log | grep -q 'card: Done · cfg'"
printf '[events]\nstop = true\n' > "$CLAWDTOAST_CONFIG"
"$BIN" reload
echo "{\"session_id\":\"cfg1\",\"cwd\":\"/x/cfg\",\"transcript_path\":\"$T\"}" | "$BIN" hook stop
sleep 0.3
check "reload applies the new config" "log | grep -q 'card: Done · cfg'"
echo '{"session_id":"cfg1"}' | "$BIN" hook session-end
sleep 3
unset CLAWDTOAST_CONFIG CLAWDTOAST_PID

# install / uninstall against a throwaway Claude config dir.
export CLAUDE_CONFIG_DIR="$CLAWDTOAST_DIR/claude"
mkdir -p "$CLAUDE_CONFIG_DIR"
echo '{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}]}}' > "$CLAUDE_CONFIG_DIR/settings.json"
cp "$CLAUDE_CONFIG_DIR/settings.json" "$CLAWDTOAST_DIR/orig.json"
"$BIN" install >/dev/null
check "install adds hooks and keeps existing ones" "grep -q 'say done' $CLAUDE_CONFIG_DIR/settings.json && grep -q 'hook session-start' $CLAUDE_CONFIG_DIR/settings.json"
check "install writes a backup" "ls $CLAUDE_CONFIG_DIR/settings.json.clawdtoast-bak-* >/dev/null 2>&1"
"$BIN" uninstall >/dev/null
same_json() { python3 -I -c 'import json,sys; sys.exit(json.load(open(sys.argv[1])) != json.load(open(sys.argv[2])))' "$1" "$2"; }
check "uninstall restores the original settings" "same_json $CLAUDE_CONFIG_DIR/settings.json $CLAWDTOAST_DIR/orig.json"
unset CLAUDE_CONFIG_DIR

# Codex install and uninstall must preserve other commands and metadata.
export CODEX_HOME="$CLAWDTOAST_DIR/codex"
mkdir -p "$CODEX_HOME"
echo '{"description":"keep","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo original"}]}]}}' > "$CODEX_HOME/hooks.json"
cp "$CODEX_HOME/hooks.json" "$CLAWDTOAST_DIR/codex-original.json"
"$BIN" install codex >/dev/null
check "Codex install preserves existing hooks" "grep -q 'echo original' $CODEX_HOME/hooks.json && grep -q 'codex-hook permission-request' $CODEX_HOME/hooks.json"
check "Codex installer writes a backup" "ls $CODEX_HOME/hooks.json.clawdtoast-bak-* >/dev/null 2>&1"
"$BIN" install codex >/dev/null
"$BIN" uninstall codex >/dev/null
check "Codex uninstall restores original config" "same_json $CODEX_HOME/hooks.json $CLAWDTOAST_DIR/codex-original.json"
unset CODEX_HOME

pkill -f "$BIN daemon" 2>/dev/null
rm -rf "$CLAWDTOAST_DIR"
exit $fail
