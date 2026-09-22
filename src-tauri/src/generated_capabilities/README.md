# Generated Capabilities

Owner: managed L-Lang/Wasm packages, generation, verification, activation, tool publication, isolated execution, and retirement.

Preserve:
- Generated != built != verified != active != published != executed. Preserve each lifecycle transition and receipt.
- Activation requires a valid verification tied to the current runtime and intact package inventory, not model success text.
- Generation models supply content, not arbitrary filesystem paths or commands. Keep host-owned build/inspect boundaries.
- Per-request tool offers are snapshots; execution must validate the offered capability/revision rather than resolve a new revision silently.
- Detached execution owns process permits, cancellation, and terminal persistence after caller disconnect.
- No subprocess wait, file copy, network I/O, or await inside lifecycle transactions. Recovery must detect missing/tampered packages.

Locate:
- Public service orchestration: `service.rs`, `service.d/`; contracts/errors/limits: `contracts.rs`, `errors.rs`, `limits.rs`.
- Generate/build/import: `generation/mod.rs`, `generation/builder.rs`, `package_store.rs`.
- Verify/activate: `verification.rs`, `guards.rs`, `lifecycle.rs`; persisted states: `repository.rs`, `schema.rs`.
- Publish/offer: `publication.rs`, `publication_sync.rs`; remove offers: `publication_unpublish.rs`, `retirement.rs`.
- Execute/process cleanup: `execution.rs`, `host/`; inspect: `inspection/`, `inspection_invoke.rs`.
- Conversation integration: `../runtime/capability_commands.rs`, `../runtime/capability_turn.rs`, `../tool_selection/backends/`.
- Startup reconcile: `recovery.rs`; tests: `tests/`, especially `tests/generation_closeout.rs`.

Publication configuration is loaded at startup; malformed generated-tool configuration disables that feature, not the whole application. Verify real host execution separately from fixture generation/verification.
