# MVP 1 Release Evidence — Situation Shadow Mode

- Date: 2026-08-26
- Target: macOS arm64 debug application bundle
- Result: Accepted
- Plan: [`mvp-1-implementation-plan.md`](./mvp-1-implementation-plan.md)
- Privacy boundary: [`adr/0002-situation-signal-privacy.md`](./adr/0002-situation-signal-privacy.md)

## Reproduction

```sh
bun install
bun run check
bun run codex:smoke
bun run desktop:smoke

/usr/bin/time -l \
  cargo test --manifest-path src-tauri/Cargo.toml \
  eight_hour_fixture_replay_and_event_queue_remain_bounded -- --nocapture

SAAA_CODEX_PATH="$PWD/src-tauri/target/debug/bundle/macos/SAAA.app/Contents/Resources/bin/codex" \
  cargo test --manifest-path src-tauri/Cargo.toml \
  codex_live_read_only_turn_completes -- --ignored --nocapture
```

Observed result:

- Frontend Settings / Situation contract tests: 5 passed.
- Rust unit / integration / fixture tests: 31 passed after the Shadow call-graph guard was added; 2 environment-dependent tests remain ignored by the default suite.
- Simulated eight-hour fixture replay: 14,400 samples passed; event queue remained at the fixed 64-entry bound. The focused test completed in approximately 0.03 seconds after compilation.
- `@openai/codex-sdk` Bun import and constructor smoke: passed.
- Packaged native Codex authenticated read-only live turn: passed.
- Packaged frontend readiness smoke with CSP enabled: passed.
- macOS `.app`: `src-tauri/target/debug/bundle/macos/SAAA.app`; packaged Codex executable present.

## Runtime and persistence evidence

The packaged desktop smoke opened the existing application-data database and migrated it from `user_version=3` to `user_version=4`.

Observed database metadata after launch:

```text
user_version = 4
settings documents = 6, all schema_version 4
situation.runtime/default exists
situation_ledger exists
situation_feedback exists
```

The automatic pre-migration backup remained reopenable with `user_version=3`. The observed Conversation and Codex thread counts matched before and after migration. The deterministic migration integration test also preserves Settings, Conversation, and Codex thread mapping.

Situation monitoring is default-off. When enabled, the Rust worker samples Foreground category and SAAA-owned lifecycle signals; when paused, the worker exits and subsequent ticks do not write ledger rows. Enablement is stored in `situation.runtime/default` and survives database reopen.

The final packaged-runtime integration check temporarily enabled monitoring on a database whose Situation ledger was confirmed empty, launched the packaged application for approximately seven seconds, and observed:

```text
heartbeat:  UNKNOWN / 0  / IGNORE  / NONE / SILENT
transition: SOLO    / 70 / SUGGEST / NONE / SILENT
evidence:   foreground-app-available (+70)
health:     foreground ready, microphone ready, audio ready, calendar disabled
```

The test then restored monitoring to off and removed the two test-only ledger rows, returning the pre-test state to `enabled=false`, `ledger count=0`.

## Privacy and no-intervention evidence

- macOS Foreground sampling uses `NSWorkspace.frontmostApplication` and reads the bundle identifier only inside the adapter. It immediately projects the identifier to a bounded category.
- No window title, process list, executable path, Calendar details, audio sample, transcript content, prompt, response, workspace path, or Codex thread id exists in the Situation contract or ledger schema.
- Ledger persistence rejects evidence or decision reason values that are not bounded codes.
- Calendar opt-in currently reports `unsupported / unavailable`; this follows the P0 stop condition and does not block Foreground + SAAA lifecycle classification.
- Permission-denied and unsupported optional-signal fixtures continue classification from available local signals.
- Diagnostics exports Situation enablement and aggregate counts only.
- The Shadow call-graph guard rejects outbound or intervention primitives such as `reqwest`, sockets, child commands, turn execution, TTS, Codex calls, or notifications inside the Situation module.
- Every Shadow policy branch and every persisted row is constrained to `actualExecution=NONE` and `actualPresentation=SILENT`.

## Acceptance mapping

| # | Acceptance | Evidence |
|---:|---|---|
| 1 | Explicit enable / pause and restart persistence | Situation Settings/UI commands; reopen integration test |
| 2 | Foreground and owned lifecycle normalization | macOS adapter projection test; owned lifecycle → runtime → ledger integration test |
| 3 | Optional Calendar coarse/degraded behavior | `unsupported` adapter and denied-signal isolation fixtures |
| 4 | No raw private signal persistence | ADR data-flow inventory; bounded-code validation; schema/source inspection |
| 5 | Deterministic candidate, confidence, evidence | classifier determinism tests and versioned rules |
| 6 | Hysteresis prevents short signal flapping | three-sample enter / five-sample exit fixture tests |
| 7 | Stale / insufficient signal safe default | owned-signal freshness test and `UNKNOWN` classifier fixtures |
| 8 | Four counterfactual decisions; actual NONE / SILENT | all-four policy test and SQLite CHECK constraints |
| 9 | No automatic Model/TTS/Notification/Application action | Shadow call-graph guard and module dependency inspection |
| 10 | Current state, evidence, health, history UI | Situation Surface backed by Rust snapshot and SQLite ledger |
| 11 | Structured evaluation feedback survives | feedback upsert/query and reopenable SQLite repository |
| 12 | Retention and clear affect Situation data only | max-entry/day cleanup and foreign-key cascade tests |
| 13 | Optional adapter failure does not regress MVP 0 | failure isolation design, denied/unsupported fixtures, full MVP 0 regression suite |
| 14 | v3 → v4 backup migration preserves state | migration integration test and packaged application-data migration |
| 15 | Pause / exit release worker resources | worker enable flag, pause exit loop, no-write-after-pause test, desktop process smoke |
| 16 | No server/RAG/vector dependency | package/source inspection; existing SQLite/Tauri stack only |

## Known bounded degradation

Calendar is intentionally `unsupported` in the MVP 1 macOS build. Shipping a native EventKit adapter without exposing event details or silently broadening permissions was not proven safe within this milestone. The accepted minimum signal set is Foreground category plus SAAA-owned Conversation / Voice lifecycle, exactly as allowed by the implementation plan's P0 stop condition.

System-wide microphone/audio usage is also deferred. SAAA-owned capture, transcription, and TTS states are included; System Audio capture remains part of MVP 2 Meeting Mode.
