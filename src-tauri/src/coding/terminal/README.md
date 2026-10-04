# Dedicated terminal implementation contract

Product concept: https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5
Plan and acceptance: `docs/plans/toolchain-terminal-agents.md`, `spec/evidence/terminal-agent-implementation/2026-10-05/`.

`implementationMethod=terminal` is a separate saved implementation route. Kitty/Ghostty are progress viewers. Users register a Git workspace on Work, request coding in conversation, answer questions/permissions on Work, and stop or manually accept results there. Pi, SDK, provider, authentication and global CLI configuration are retained.

## Ownership and delivery

The installed SAAA executable dispatches `--saaa-terminal-agent run|mcp|hook|view <private run directory>` before Tauri initialization. The independent runner owns/reaps the native CLI group. Views own no CLI process and accept no shell input. Closing a view cannot resend a job. Each run has a private spec/nonce, exclusive runner lock, immutable launch receipt, child identity receipt and fsynced event spool. A best-effort Unix datagram wakes the host only after persistence; the recovery scan drains old spools after restart. Event ID and cursor adoption share a SQLite transaction. Only the host writer mutates the canonical coding/terminal ledgers.

Unknown delivery is retained as unknown. Prepared launches abandoned for 60 seconds are classified without replay. Lost runners are blocked and cancelled without automatic resend. Recovery stop verifies saved start time/command and process group; identity mismatches require manual containment. Old run/session events cannot complete the current job. Normal app shutdown requests cancellation; crashes can leave a runner until its deadline or persisted cancellation.

## Questions and permissions

Claude print mode uses per-run MCP/settings files and native `AskUserQuestion` defer. Exact-input saved answers apply to the same session after confirmed exit. Batched tools that ignore defer are cancelled after ten seconds. The permission host denies until an explicit UI approval of the identical tool input; a consumed receipt limits approval to one call. Commands and changes outside the selected workspace require that gate. Existing native deny policies remain authoritative.

Codex uses `exec`/exact `exec resume <id>`, workspace-write and approval never. Only SAAA consult/finish tools advertise a closed, non-destructive host capability, with tool-specific auto approval. Other native sandbox restrictions remain in force. A sandbox restriction is a blocker; this route cannot approve arbitrary outside-sandbox commands.

Consult and finish MCP calls return immediately after durable recording. Consult creates a paused question; finish is only a candidate. Stop/StopFailure/PostToolUseFailure are diagnostics, not universal blocker or completion proofs.

Automatic decisions are opt-in, limited to three per job and one attempt per identical question. They share the maintenance lane, yield to foreground conversation, use the selected conversation provider with a 30-second bound and no tools/fallback provider, and require a literal quote/answer from the original human request. Permissions remain manual. Human answers and event decisions have separate source bindings; no event is fabricated as `StartTurnInput`. Revision/source checks reject stale, cancelled or forgotten-source decisions.

## Completion and reporting

The host requires native process exit, successful structured result, a candidate with no remaining manual checks, nonempty user-saved argv checks passing, and host Git/file evidence. Commands run in the saved workspace, bounded to 120 seconds, with process ownership and cancellation; the full output and diff are hashed independently of display excerpts. Verification interrupted by restart is never replayed automatically. Missing checks, failed checks or missing/manual conditions await review. Explicit human acceptance is recorded separately without rewriting failed-check evidence.

Opt-in failed-check repair makes at most two exact-session continuations of the original request. It cannot change checks or authority. Reports use existing Steward outbox/Situation/TTS and emit only after commit. Conversation listens for committed report delivery, so no new user turn is needed. Voice capture and echo processing are unchanged.

## Reproduction

Run normal `coding::` unit tests and the standalone runtime crate tests first. Build the desktop binary, verify its helper dispatch, then copy it to an immutable acceptance path. Other desktop builds can replace `target/debug/saaa`; do not use a mutable path for concurrent canaries.

```sh
cargo build --manifest-path src-tauri/Cargo.toml --bin saaa
cp src-tauri/target/debug/saaa /tmp/saaa-terminal-accepted-native-binary
SAAA_TERMINAL_TEST_HELPER=/tmp/saaa-terminal-accepted-native-binary \
SAAA_TERMINAL_TEST_NO_VIEWER=1 \
cargo test --manifest-path src-tauri/Cargo.toml --lib dedicated_terminal_roundtrip \
  -- --ignored --nocapture --test-threads=1
```

The ignored test uses actual native runner/hooks/MCP/writer/outbox with fake external CLIs and isolated databases. Test-only helper/viewer overrides are excluded from production. Native CLI/terminal transport acceptance is separately recorded; model access is account-specific and never repaired by changing user settings or silently selecting another model.
