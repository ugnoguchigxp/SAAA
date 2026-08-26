# MVP 1.5 Release Evidence

## Implementation evidence

| Acceptance | Evidence |
| --- | --- |
| Profile migration | `migrate_v4_to_v5` adds feedback v2, quality windows, profiles and runs, then seeds `mvp1-rules-v1`. |
| Parameter safety | `CalibrationParameters` has strict bounds and the classifier/hysteresis accept parameterized thresholds. |
| Privacy | Quality storage is aggregate-only; replay uses repository fixtures; diagnostics continues to export counts only. |
| Lifecycle | Candidate creation, replay-run persistence, accept, reject and rollback are transactional SQLite operations. |
| Review | The Situation Review tab shows the active rule, data sufficiency, candidate history and replay result. |

## Fixture

- Set version: `situation-fixtures-v1`
- Fixture: `src-tauri/fixtures/situation/mvp1-v1.json`
- Raw application identity, titles, calendar content, audio, prompts and responses are absent.

## Known limitations

- The current worktree contains independent, unfinished Meeting functionality that prevents repository-wide Rust and frontend checks from completing. MVP 1.5 changes were kept isolated from that subsystem.
