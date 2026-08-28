# MVP 0 Release Evidence

- Date: 2026-08-26
- Target: macOS arm64 debug application bundle
- Result: Accepted

## Reproduction

```sh
bun install
bun run check
bun run codex:smoke
bun run desktop:smoke
SAAA_CODEX_PATH="$PWD/src-tauri/target/debug/bundle/macos/SAAA.app/Contents/Resources/bin/codex" \
  cargo test --manifest-path src-tauri/Cargo.toml codex_live_read_only_turn_completes -- --ignored --nocapture
```

Observed result:

- Frontend Settings contract tests: 3 passed.
- Rust unit/integration/fixture tests: 16 passed, 2 environment-dependent tests ignored by the default suite.
- Codex SDK Bun import and constructor smoke: passed.
- Packaged frontend readiness smoke with CSP enabled: passed.
- Bundled native Codex authenticated read-only live turn: passed; the selected empty workspace remained unchanged.
- macOS `.app` generated at `src-tauri/target/debug/bundle/macos/SAAA.app`; its Resources directory contains the pinned native Codex executable.

## Acceptance mapping

| # | Acceptance | Evidence |
|---:|---|---|
| 1 | Provider, model, endpoint, route editing | Settings Surface and Zod/Rust validation; frontend contract tests |
| 2 | SQLite save/restart/restore | reopen/rollback integration test |
| 3 | Shared Effective Route | saved routing document drives both banner and Rust resolver |
| 4 | Primary and fallback | real loopback HTTP 503 → SSE fallback integration test |
| 5 | Text stream/cancel/retry/history | Runtime channels, cancellation token, Retry UI, persisted message restore |
| 6 | Push-to-talk partial/final text | MediaRecorder/Web Audio path and local whisper fixture delta/final test |
| 7 | Transcript response and local TTS | transcript enters the same `start_turn` path; macOS `say` native smoke passed |
| 8 | Stop recording/generation/agent/speech | explicit controls and idempotent cancellation/process cleanup tests |
| 9 | Codex installed/auth/health | app-server `account/read` plus Settings status UI |
| 10 | Explicit read-only coding route | Coding mode, workspace picker, live read-only turn |
| 11 | Codex response and bounded activity stream | fixture contract and 64,000/80 character bounds |
| 12 | Thread persistence/resume | SQLite reopen test and app-server start/resume fixture |
| 13 | No workspace write/network/search/write MCP | read-only sandbox, approval `never`, network/search/MCP config, policy-violation projection, unchanged live workspace |
| 14 | Optional feature isolation | normal Chat boot does not wait for Codex/STT/TTS; recoverable feature errors |
| 15 | Local-only prompt/audio | loopback validation, Cloud fallback rejection and runtime filtering; local STT/TTS only |
| 16 | No deferred server/RAG stack | dependency and package inspection: no Hono, RAG, PostgreSQL, pgvector, or embedding runtime |

## Operational notes

- The debug `.app` is intentionally not a notarized distribution artifact. Release signing/notarization uses the normal downstream release identity process.
- Microphone permission, acoustic quality, and the user's chosen whisper model are hardware/model-dependent operator checks; the capture-to-STT contract and native adapter are covered by deterministic fixtures.
- The SQLite database and automatic/manual backups live under the SAAA OS application-data directory. Diagnostics omit messages, workspace paths, Codex thread IDs, and settings values, and redact bounded runtime errors again on export.
