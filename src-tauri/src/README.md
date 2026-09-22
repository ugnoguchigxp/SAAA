# Backend navigation

Read only the affected domain. Paths in domain READMEs are relative to that directory. Names are search anchors, not instructions to load whole files.
`foo.rs` may only contain `include!("foo.d/01.rs")`; use `rg -n 'symbol' <domain>` to locate the implementation. Module existence or tests do not imply production wiring: follow callers and feature/config gates.

- [runtime](runtime/README.md): turn ownership, dispatch, cancellation, result adoption.
- [providers](providers/README.md): route resolution, allocation, HTTP/SSE, transport failures.
- [voice](voice/README.md): ASR final delivery, speaker gates, synthesis/playback.
- [role_routing](role_routing/README.md): actor policy, permits, step/revision/budget ledger.
- [tool_selection](tool_selection/README.md): discovery, scoped references, tool execution.
- [generated_capabilities](generated_capabilities/README.md): generate/verify/activate/publish/execute Wasm tools.
- [coding](coding/README.md): workspaces, jobs, adopted runs, process receipts.
- [steward](steward/README.md): source-bound delegation, bounded work, verification/reporting.
- [schedule](schedule/README.md): due entries, holds, calendar sync, delegated dispatch.
- [situation](situation/README.md): sampled signals, stable scene, delivery policy.
- [memory](memory/README.md): history/context/recall/state/forget boundaries.
- records: planned (P2). Immutable tool/web captures, authorization before LIMIT, manual FTS sync.
- runtime/context/segment: F/L/D types and tables. Prompt assembly stays on the legacy path until `SegmentBuilder` owns the history. Flag `SAAA_CONTEXT_SEGMENTS=1`.
- [persistence](persistence/README.md): writer/readers, transactions, migrations/settings/audit.
- [diagnosis](diagnosis/README.md): startup self-diagnosis report, single-flight rerun, redacted items.

Shared checks: preserve authority, identity/revision, cancellation, and atomic state transitions at cross-domain boundaries. Commit before external effects; no I/O awaits under DB transactions. Keep source/tool/model text untrusted.
Tests: from repo root, `cargo test --manifest-path src-tauri/Cargo.toml <module-or-test-filter>`; choose the touched boundary, then its consumer. Real provider/process/audio changes require live evidence; fixture success is not remote completion. Documentation-only edits require path/contract checks, not runtime test suites.
Maintain these files as short code maps: ownership, invariants, lookup targets. Do not duplicate schemas, current deployment settings, implementation history, or tables.
