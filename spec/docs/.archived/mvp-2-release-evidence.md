# MVP 2 release evidence

## Shipping capabilities

- Microphone Meeting Session: implemented.
- System audio, floating overlay, translation: unavailable in this build (P0 gates not passed).

## Verified automated evidence

- SQLite migration creates the v6 meeting metadata and transcript tables, preserves a v5 fixture, and reconciles unfinished metadata to `interrupted` with zero persisted transcript entries.
- Explicit Save writes final in-memory entries transactionally; Discard leaves no transcript body for its session.
- Temporary audio workspaces are drop-cleaned on success and failure paths; cancellation uses the same scoped workspace and process guard.
- Frontend queue tests cover two-item backpressure and PCM frame boundaries. Rust covers preflight, token entropy, segment validation, pause cancellation, TTS policy, Save/Discard, and migration/reconcile.
- The final suite reports 10 frontend tests passed and 61 Rust tests passed, with 2 external Codex runtime tests intentionally ignored.
- `bun run check`, `bun run build`, and `bun run desktop:smoke` passed on macOS. The packaged application starts successfully with an isolated smoke-test data directory.

## Manual desktop evidence still required before a production release

- Test a real local Whisper model with microphone permission grant, denial, and revocation.
- Run 30-minute and 2-hour resource soak tests; verify pause/stop/unmount release the microphone indicator.
- Verify temporary WAV deletion during a real cancelled Whisper process.
- Verify Save-before/after database entry counts, Discard, app-close cleanup, and TTS blocking on macOS.

Known limitation: Whisper processes fixed five-second microphone segments. Segment boundaries can omit a word; no LLM correction, speaker attribution, cloud fallback, notification, or automatic action is performed.
