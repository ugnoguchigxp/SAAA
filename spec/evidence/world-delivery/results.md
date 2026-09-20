# World delivery results

The implemented routes preserve a single source of truth: the existing `WorldFrameService` and
its frame stamp. The implementation does not extend the old 1-second TTL. If a frame is stale or
has changed before an adapter's actual request boundary, the World block is removed and the
generation manifest omits the World candidate.

Validation to run in a non-contended workspace:

```text
cargo test --manifest-path src-tauri/Cargo.toml --lib runtime::context::world
bun run test:rust-packages
bun run ipc:check
bun run size:check
bun run check:local
bun run spec:check
```

2026-09-21 local checks: `git diff --check` passed; `bun run spec:check` completed; the
World-delivery Rust changes passed `cargo check --locked --offline`; the
personal-state-core portion of `bun run check` completed its 50 unit tests successfully. The
full TypeScript check is currently blocked by pre-existing concurrent changes in
`useAmbientVoiceSession.ts` and the IPC event union (`meetingBlocked` / `meeting_blocked`), not
by the World delivery scope payload.
