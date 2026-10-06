# Worker agents

Owner: purpose-built agents with a pinned tool set for simple delegated tasks (registry, discovery, executor, Web Search worker). The conversation agent delegates; the host admits, runs, verifies and delivers. Plan and contracts: `docs/plans/worker-agents.md`.

Registry != discovery != execution. A draft is neither offered nor runnable; an offer ranks candidates but grants nothing; a task pins one approved revision at admission.

Preserve:
- Profile revisions are immutable. Approval binds to `definition_hash` (sha256 of the normalized `ProfileDraft`) and is re-verified against the stored rows; a mismatch or a tampered row is rejected.
- Only revisions whose tools are all `pure`/`read` can be approved (`write_tools_not_supported`, stays `draft`). Tool effects come from the trusted side (builtin constants, pinned `tool_selection_revisions` row), never from user input.
- `save_worker_agent_draft`, `approve_worker_agent_revision`, `set_worker_agent_enabled`, skills, mode and blocklist removal are user/host IPC only; the conversation model has no path to them.
- `registry_epoch` changes on approve, enable/disable and seed, not on drafts, skills or web-search mode. Stale offers are detected by epoch.
- `seed_builtin` only inserts missing profiles; it never overwrites an existing profile or user edit. External `$ref` in schemas stays unresolved (`jsonschema` without network/file retrievers).
- `worker_url_blocklist` rows have no expiry; `remove_worker_url_blocklist` is the only deletion. `list_worker_tasks` never returns input/result bodies.
- `web_search_mode` defaults to `inline`; setting it back is the whole rollback.

Locate:
- Contracts: `contracts.rs`; schema/migration: `schema.rs` (called from `persistence/schema.rs`); read helpers (`read_meta`, `definition_hash`, `load_revision`): `loader.rs`; fixtures: `test_support.rs`.
- Registry: `registry/validate.rs` (draft rules, tool effect resolution), `registry/repository.rs` (save/approve/enable/skill/mode/embedding/seed), `registry/queries.rs` (summaries, tasks, blocklist), `registry/commands.rs` (IPC).
- Discovery: `discovery/`; execution: `executor/`; Web Search worker: `web_search/`.
- Tests: `registry/tests.rs` (`cargo test --lib worker_agents::registry`).

Trace profile draft -> approve (hash, effects, FTS, epoch) -> discovery offer -> admit (pins revision) -> `worker` lane task -> attempts/sources/tool ledger -> steward report. Check call sites before treating offline contracts as active paths.
