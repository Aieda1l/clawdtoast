# Codex notification integration

## Implementation plan and architecture

1. Translate accepted async question results first, preserving the existing shim/IPC/daemon/UI path.
2. Track independent, ordered question observations, confirmed blocking answers, and deduplication tombstones per session.
3. Add bounded opt-in attention cards and silent updates, with conservative async resolution semantics.
4. Upgrade installation/configuration, exercise live daemon tests, and document unsupported client behavior.

No dependencies were added. Hooks read at most 1 MiB and wait at most one second for complete
stdin JSON; IPC writes retain the existing 50 ms timeout. Desktop/audio work stays in daemon
and overlay processes. Notification hooks are asynchronous; session bookkeeping hooks are short
and ordered. All successful Stop/SubagentStop handlers return neutral `{}` and failures stay
fail-safe. SessionEnd/resolution/activity never start a daemon solely for cleanup.

## Upstream contracts inspected

Reviewed `openai/codex` at commit
[`a3d93e1e692f1144cf8685175a5d1f6bf7294f57`](https://github.com/openai/codex/tree/a3d93e1e692f1144cf8685175a5d1f6bf7294f57).
These are source contracts rather than claims about every released client:

- [Async handler](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/core/src/tools/handlers/request_user_input_async.rs): validates a nonempty questions array, nonblank titles and optional nonempty string options; emits async AgentMessage items with the call ID; returns the text `{"accepted":true}`.
- [Tool registry](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/core/src/tools/registry.rs): builds PreToolUse/PostToolUse payloads for function tools; successful tools expose their output body as `tool_response`. A later PostToolUse rejection blocks the model-facing result, not the already completed execution. Async questions have already been emitted at that point.
- [PostToolUse schema](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/hooks/schema/generated/post-tool-use.command.input.schema.json): session, turn, tool-use ID, tool name, tool input, and arbitrary JSON tool response.
- [Blocking handler](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/core/src/tools/handlers/request_user_input.rs): returns an answers map only after accepting a response. Cancellation is an error; PostToolUse is dispatched for successful execution only.
- [Question submission](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/tui/src/chatwidget/questions.rs) and [editor state](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/tui/src/bottom_pane/async_questions/state.rs): local accepted sends/queue operations consume questions. Client state also handles expiry, desktop answer identities and replay tombstones. IDs are based on the message/call ID and question index. Collapsed questions may auto-expire after 30 seconds; opening the editor can snooze that timer. External notification hooks cannot observe those local actions.
- [Question tests](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/tui/src/chatwidget/tests/questions_tests.rs): ordinary accepted input, queued prompt delivery, live turn completion, cross-client answers, and replay/navigation have different client-side consequences. A generic UserPromptSubmit/Stop hook cannot reproduce this state safely across clients or resumed sessions.
- [Hook configuration](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/config/src/hook_config.rs) supports `async` and `timeout` for command hooks. [Hook event matching](https://github.com/openai/codex/blob/a3d93e1e692f1144cf8685175a5d1f6bf7294f57/codex-rs/hooks/src/events/common.rs) contains no question-answer or MCP-elicitation lifecycle event.

Also reviewed [codex-notify's adapter](https://github.com/cppcooper/codex-notify/blob/main/lib/code-notify/core/notifier.sh)
and [acceptance tests](https://github.com/cppcooper/codex-notify/blob/main/tests/test-codex-notify.sh).
Its accepted-result check and reuse of shared notification delivery are useful precedents.
Its general resume signals cannot establish individual async question resolution, so ClawdToast
does not use those signals to clear async observations.

## State and fallback policy

- Identity is `(session, turn, tool call, question ID/index)`. Async calls require a nonempty tool-use ID. Older blocking payloads without one use a deterministic input fingerprint for compatibility; identical calls without upstream identities cannot be distinguished reliably.
- Each session retains up to 512 recent question records, including answer tombstones, in arrival order. Input is limited to 64 questions per call; displayed titles are capped at 200 characters. Oldest records are evicted at the cap. Deduplication/counts are best effort beyond these bounds.
- Successful blocking answers resolve only the matching question IDs and never an async call. Answer tombstones handle PostToolUse arriving before an asynchronous PreToolUse hook finishes. No answers themselves are retained.
- Async observations stay unconfirmed until TTL expiry or session/process cleanup. Stop, Interrupt, ordinary tools and user submissions do not claim resolution. A structured question suppresses heuristic Stop waiting alerts for that turn even when wording differs; a normal completion remains Done.
- Cards show the observed enabled-category count, explicitly labeled unconfirmed. The compact overlay title puts the count first and the body includes the project. Questions have a separate attention slot so completion/permission cards can coexist. Silent overlay updates only touch existing cards; clicking dismisses without answering or deleting pending state.
- Persistent mode is off by default and has a hard finite wall-clock deadline, including hover. New questions can refresh it; retries and silent updates cannot. Question TTL cleanup shares the existing liveness timer. Known dead processes remove state/cards; unidentified idle sessions have a 24-hour backstop. Closed-session tombstones reject late arrivals for five minutes, unless an explicit session open/activity resumes the session.
- The atomic restart ledger stores up to 4096 records, contains no question text/options/answers, and is read with a 4 MiB ceiling. Invalid, expired, oversized-identity and dead-process entries are ignored. Restoring state emits no UI/audio; later new questions can include restored unconfirmed counts. Stored observations cannot reconstruct the client's live queue. A crash between IPC reception and checkpoint replacement can lose the latest event. The ledger is not a durable message queue.
- No new background poller, transcript tailer, app-server client, screenshot reader or database watcher is introduced. Connecting a separate app-server client would not automatically observe an already running independent Codex client.

## Privacy and platform limits

Permission cards show only a sanitized, bounded tool name and project basename. Command
arguments, tool inputs, file contents and approval justifications are deliberately omitted.
`attention.show_question_text = false` hides structured and heuristic Codex question text in
cards/headless logs; options and answers are never shown. The existing completion-message
privacy behavior is unchanged. Question text can still contain sensitive information when shown.

Windows retains the tested unquoted executable-path convention: install the binary in a path
without spaces or shell metacharacters. macOS/X11 reuse the existing process-based focus path;
Wayland cannot raise another client's window and click only dismisses. Native Linux fallback
uses finite timeouts, replacement IDs and best-effort `gdbus CloseNotification`; desktops can
override timing/placement or suppress notifications through DND. Custom overlays/audio do not
consult OS DND. Exclusive fullscreen, multiple monitors, real compositor policies and macOS
Spaces require manual checks; container tests do not establish hardware parity.
The command-based native fallback cannot observe NotificationClosed signals: a desktop may
recreate a dismissed native notification when replacing its old ID. The custom overlay's
no-resurrection guarantee does not extend to every native notification server.

There is no supported hook for every terminal-only confirmation, plan implementation dialog,
or MCP elicitation. PermissionRequest can also precede cancellation or another hook's decision.
The conservative English Stop classifier can miss ambiguous questions, languages other than
English, or questions followed by lists. These gaps are not filled with terminal or transcript
scraping.

## Manual verification

1. Build the release binary, run `clawdtoast install codex`, then review/trust definitions in `/hooks`. Ensure the selected Codex build supports these hooks.
2. With a real GUI/audio session, invoke an async question tool while Codex continues working. Verify Waiting for you, title/count, one attention ping, animation and supported terminal focus.
3. Invoke a second accepted call and replay the first hook. Only the new call should ping. A normal final message may produce a separate Done card.
4. Enable persistent mode, answer/dismiss a queued question in Codex and verify that ClawdToast keeps an explicitly unconfirmed observation until its deadline; it must not pretend to know the answer. Close the session and verify immediate attention-card removal.
5. Test a blocking Plan question and its answer-bearing PostToolUse result: the attention count should decrement silently. Check privacy mode with a title containing private text.
6. Check multiple concurrent terminals, monitor placement, fullscreen, actual audio, native DND, and real native fallback notification replacement/closing on each supported desktop. Automated headless/container tests do not verify those behaviors.

## Validation recorded for this change

Verified on Windows and in an Ubuntu 24.04 Docker environment:

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS on Windows |
| `cargo clippy --all-targets -- -D warnings` | PASS on Windows and Linux |
| `cargo test` | PASS, 74 tests on each platform (platform-specific tests differ) |
| `cargo build --release` | PASS on Windows and Linux |
| `tests/windows/codex-hooks.ps1` | PASS: PowerShell capture, open/file stdin, 11/11 concurrent cards, async acceptance/counts/retries, restart, privacy/category settings, installation/idempotence/removal |
| `tests/integration.sh` | PASS: existing Claude lifecycle/config/install checks and Codex lifecycle/questions/install checks |
| `tests/linux/x11.sh` | PASS: plain, compositor, and Wayland-to-X11 fallback scenarios |
| `tests/linux/wayland.sh` | PASS: headless sway placement, stacking, transparency, timeout and configured corner |
| `tests/linux/fallback.sh` | PASS: no display, native fallback, finite persistent timeout, replacement ID, sound suppression and CloseNotification (notification commands stubbed) |
| `tests/linux/attention.sh` | PASS: live X11 persistence, Done coexistence, partial/full blocking resolution, SessionEnd, hovered hard expiry, daemon replacement and restart deduplication |
| `scripts/check_budgets.sh` | PASS: Linux binary 2,298 KB, idle daemon RSS 4,868 KB, existing shim p95 3 ms. Its optional GUI budget checks were skipped. X11 scenarios separately observed roughly 46 MB overlay RSS for three cards. |
| Accepted async-hook timing | 40/40 distinct accepted calls delivered; warm headless Linux hook p95 2.25 ms |

The Linux build/check container needed Clippy installed; the test Dockerfile now includes it
and Python explicitly. Current Rustfmt/Clippy also required mechanical formatting and minor
equivalent lint fixes in existing code. No macOS runtime/build verification was available.
The live Codex client, actual audio, process-based terminal focusing, real notification server,
native DND, exclusive fullscreen and multi-monitor hardware checks remain manual.

## Files changed

- Adapter, IPC, state and preferences: `src/codex.rs`, `src/protocol.rs`, `src/core.rs`, `src/config.rs`, `src/daemon.rs`.
- Attention rendering, lifecycle and fallback: `src/overlay.rs`, `src/wayland.rs`, `src/stack.rs`, `src/render.rs`, `src/notify.rs`, `src/text.rs`; `src/x11.rs` has formatting and an equivalent bounds-preserving lint fix.
- Tests and CI: `tests/integration.sh`, `tests/windows/codex-hooks.ps1`, `tests/linux/attention.sh`, `tests/linux/fallback.sh`, `tests/linux/run.sh`, `tests/linux/Dockerfile`, `.github/workflows/ci.yml`.
- Documentation and plugin upgrade version: `README.md`, this document, `.claude-plugin/plugin.json` (0.1.14).
- Rustfmt-only changes: `src/anim.rs`, `src/backend.rs`, `src/geom.rs`, `src/install.rs`, `src/ipc.rs`, `src/log.rs`, `src/macos.rs`, `src/paths.rs`, `src/proc.rs`, `src/sound.rs`, `src/windows.rs`.
