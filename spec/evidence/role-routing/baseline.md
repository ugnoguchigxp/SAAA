# Role Routing baseline

Recorded 2026-09-21 before R1 completion work. This is an evidence record, not a claim that
any live provider, account, or credential is available.

## Repository state

- Base commit: `6a5e36c22138ff44fc9f7b421d17653ec87a6f64`.
- The worktree was already dirty across runtime, provider, settings, schedule, steward, and
  role-routing files. Those changes are preserved; role-routing verification must distinguish
  them from this work.
- SQLite `user_version`: 30. Versions 28 and 29 add the routing and learning ledgers;
  version 30 is the independently-added schedule ledger.

## Fixed and observable dependencies

- `@openai/codex-sdk` is pinned to `0.144.4` in `package.json` and `bun.lock`.
- The bundled Codex platform packages are lockfile dependencies. Login state and a live Codex
  invocation were not inspected or exercised.
- Routing provider actors are configuration-derived. The UI can prepare enabled
  openai-compatible, agent-session, and dynamic-LAN providers as `provider` actors, and a
  healthy enabled Codex setting as a `codex_sdk` actor. No provider ID, model ID, credential,
  tool capability, or residency claim is assumed by this record.

## E00 re-baseline (2026-09-22, no secrets)

- Base commit re-read at task start: `7471ecadc1d64a5b3c1d4e6a898669be80a2cb8a`.
- Worktree dirty files preserved (31 entries): runtime conversation/context files, provider
  dynamic-LAN/world tests, world query/query_v2, steward/report, coding integration tests,
  voice http audio requests, schedule/module-size baseline, and role-routing docs. These are
  not role-routing work and must not be overwritten, staged, or reverted.
- SQLite `DATABASE_SCHEMA_VERSION`: 30 (`src-tauri/src/persistence/schema.rs`). The role-routing
  ledger lives in `src-tauri/src/role_routing/schema.rs::migrate` as additive `rr_*` tables.
- `@openai/codex-sdk` pinned to `0.144.4` in `package.json`; live login state not inspected.
- Existing command surface (no new command invented): `cargo test --lib rr_*`,
  `cargo fmt --check`, `bun run typecheck`, `bun run ipc:check`, `bun run s11tnext:check`,
  `bun run size:check`, `bun test ./tests/role-routing-codex.test.ts`, `git diff --check`.
- Current role-routing test baseline: `cargo test --locked --lib role_routing::` = 82 passed.
- Acceptance mapping recorded in [acceptance-matrix.md](acceptance-matrix.md): RR 40 rows,
  A42 rows, P5 rows, each with responsible E/L, lane, and current result.

## Verification baseline and gates

- `cargo check --manifest-path src-tauri/Cargo.toml --lib` succeeds with existing dead-code and
  unrelated warnings.
- The full TypeScript check is currently blocked by pre-existing parallel changes: missing
  `meetingBlocked` in `useAmbientVoiceSession`, plus a generated IPC/runtime event mismatch.
- No live provider, ASR, tool, TTS, or Codex-login test has been run. Live R1/R2/R3 acceptance
  remains gated on explicit configured credentials and a dedicated isolated fixture.
