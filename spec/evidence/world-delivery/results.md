# World delivery review results — 2026-09-21

This review resumed after parallel implementation. The World TTL remains 1,000ms. No size or
lint threshold was relaxed to obtain a pass.

## Fixed in resumed review

- Reasoning MCP now uses `reasoning-answer-v2` with explicit `WorldEvidence` metadata. Host and
  service compare both input and output schemas during initialization. Old input contracts are
  rejected before context transmission. The service must be deployed with the matching contract.
- Initialization occurs before composition. Wire arguments remain unchanged after digesting;
  previously the client reduced the timeout after the generation receipt had been recorded.
- World is checked again after receipt writes, immediately before the MCP call. If it changes
  during a database wait, the already-digested request is rejected.
- Codex metadata context is sampled in a read transaction after process initialization. Actual
  `thread/start` and `turn/start` hashes are recorded. Scope, policy, owner state (including a
  change without a revision bump) and current instruction are checked before dispatch.
- Source revalidation and result acceptance share a transaction. Failed/cancelled generations can
  still be recorded when their dependencies have disappeared; dependency loss no longer prevents
  writing the failure receipt.
- Deadline selection considers every resolved scope before applying the global eight-item bound.
- Shared Codex test environment and coding execution-slot fixtures are serialized. IPC receiver
  fixture was regenerated for the existing `routing.roles` settings document.

## Verification

- `bun run world:eval`: 33 offline wire cases passed (OpenAI compatible, AgentSession, MCP).
  Includes MCP expiration during initialization and the actual RPC argument digest.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib wd_`: 12 passed, none failed/ignored.
  Includes an executable Codex app-server fixture, source changes before dispatch/acceptance,
  and cross-scope deadline ordering.
- `bun run test:rust-packages`: passed, including reasoning contract/service tests.
- `bun run typecheck`, `format:check`, `lint`: passed. `bun run spec:check`: passed after fixing document references.
- Final AgentSession regression: 21 passed, one live test ignored. Codex protocol regression: passed.
- Default-parallel full suite: frontend passed; Rust 1,241 passed, two short-deadline process
  tests failed under contention, 18 ignored. The four-thread rerun passed all 1,247 Rust unit tests (20 ignored), then found a stale SQLite architecture-test path. That test was updated for the extracted conversation composer while preserving the read/compose/write ordering checks. Its focused rerun and the voice ASR binding test both passed.
- `size:check` still fails across the shared worktree. Only new Codex files were registered;
  existing limits were not increased. `clippy --all-targets -- -D warnings` still fails on
  unused code and lint violations across other modules. Neither gate is certified green.

## Outstanding acceptance and implementation

See `progress.md` and `route-matrix.md`. This is not completion of WD-00 through WD-13.
Codex sends Scope/Task/deadline metadata, not the full five-element graph. Common FrameService
integration of Situation/DW/schedule, natural-language extraction, complete model-claim validation
and provider capability presentation still require implementation/acceptance.

The configured local OpenAI-compatible endpoint at `192.168.0.130:8080` was unreachable and the
AgentSession endpoint at `127.0.0.1:44449` timed out. Replacement endpoint/model information was
requested. No live-model/UI/audio or WD p95/TTFA success is claimed from offline fixture results.
