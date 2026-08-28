# MVP 1.5 Release Evidence

- Date: 2026-08-27
- Result: Accepted
- Plan: [`mvp-1.5-implementation-plan.md`](./mvp-1.5-implementation-plan.md)
- Migration backup: application-data `backups/pre-migration-<timestamp>.sqlite3` when `user_version < 6`

## Implementation evidence

| Acceptance | Evidence |
| --- | --- |
| Profile migration | `migrate_v4_to_v5` adds feedback v2, quality windows, profiles and runs, then seeds `mvp1-rules-v1`. |
| Parameter safety | `CalibrationParameters` has strict bounds and the classifier/hysteresis accept parameterized thresholds. |
| Privacy | Quality storage is aggregate-only; replay uses repository fixtures; diagnostics exports counts, active rule version and latest run status only. |
| Lifecycle | Candidate creation, replay-run persistence, accept, reject and rollback are transactional SQLite operations. |
| Review | The Situation Review tab shows the active rule, data sufficiency, candidate history and replay result. |

## Fixture

- Set version: `situation-fixtures-v1`
- Fixture: `src-tauri/fixtures/situation/mvp1-v1.json`
- SHA-256: `6d52b58aaadf303ba4d93b7d76d00f32cae7ae042533174e35bfc324a33f2808`
- Raw application identity, titles, calendar content, audio, prompts and responses are absent.

## Verification

- Rust unit tests: 61 passed, 2 intentionally ignored (external Codex runtime required).
- Frontend tests: 10 passed.
- Typecheck, Rust formatting, Clippy with warnings denied, production build, and desktop smoke all pass.

## Privacy inventory

- SQLite quality windows contain only bounded counters and rule versions.
- SQLite ledger contains bounded scene/evidence/health categories, not application identity, window title, calendar content, audio, prompts, or responses.
- Fixture data contains bounded synthetic signal categories only.

## Acceptance mapping

| # | Acceptance | Evidence |
| ---: | --- | --- |
| 1 | v4 data is preserved through migration | staged migration, backup-before-migration, migration/reopen tests |
| 2 | Default profile preserves MVP 1 behavior | default `CalibrationParameters`, classifier and hysteresis parity tests |
| 3 | Quality does not persist raw samples | bounded `situation_quality_windows` counters and strict decode test |
| 4 | Feedback v2 survives and validates | feedback upsert, cross-field validation, cascade/retention tests |
| 5 | Replay is deterministic and comparable | static fixture replay and baseline comparison test |
| 6 | Candidate parameters are bounded | Rust and Review input bounds plus validation |
| 7 | Only accepted candidate becomes active | lifecycle transaction and reject-path tests |
| 8 | Rollback survives reopen | active profile repository/runtime restore and rollback lifecycle tests |
| 9 | Review is observation-only | Review invokes only Situation commands; shadow call-graph guard |
| 10 | Private raw data is excluded | schema, diagnostics payload, fixture and bounded-code validation |
| 11 | Runtime history is bounded | retention limits, quality-window cap and eight-hour fixture test |
| 12 | MVP 1 safety and desktop smoke regressions pass | full Rust suite and packaged desktop smoke |

## Known bounded degradation

- The static fixture is intentionally synthetic and contains no user-derived history or raw application data.
- Calendar remains a coarse optional signal; unavailable or degraded states fall back to `UNKNOWN` / `IGNORE` safety behavior.
