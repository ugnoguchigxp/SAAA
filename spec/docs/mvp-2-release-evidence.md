# MVP 2 release evidence

## Shipping capabilities

- Microphone Meeting Session: implemented.
- System audio, floating overlay, translation: unavailable in this build (P0 gates not passed).

## Verified automated evidence

- SQLite migration creates the v6 meeting metadata and transcript tables, and reconciles unfinished metadata to `interrupted` at startup.
- Explicit Save writes only final in-memory entries in one transaction; Discard retains no transcript body.
- Frontend queue tests cover two-item backpressure and PCM frame boundaries.
- Rust checks cover preflight failure, optional capability gating, and Meeting TTS policy.

## Manual desktop evidence still required before a production release

- Test a real local Whisper model with microphone permission grant, denial, and revocation.
- Run 30-minute and 2-hour resource soak tests; verify pause/stop/unmount release the microphone indicator.
- Verify temporary WAV deletion on successful, failed, and cancelled Whisper invocations.
- Verify Save-before/after database entry counts, Discard, app-close cleanup, and TTS blocking on macOS.

Known limitation: Whisper processes fixed five-second microphone segments. Segment boundaries can omit a word; no LLM correction, speaker attribution, cloud fallback, notification, or automatic action is performed.
