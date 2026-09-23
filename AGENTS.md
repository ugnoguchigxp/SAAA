# SAAA project instructions

At the start of work in this project, call the `initial_instructions` MCP tool once per conversation. Do not send prompts or messages to another Codex task unless the user explicitly requests it.

## Frozen runtime paths

The ASR and first conversation response implementations are frozen by `critical-path-freeze.json`. Do not change files protected by either domain as part of unrelated work. When the user explicitly requests a change in one of these paths, run the relevant regression tests, then update only that domain with `bun run freeze:accept:asr --reason "..."` or `bun run freeze:accept:initial-response --reason "..."`. Include the reason and test results in the change description. Keep the freeze check passing.

- ASR: run `bun test tests/ambient-voice-session.test.tsx tests/voice-capture-races.test.ts tests/voice-asr-packet-sender.test.ts` and `cargo test --manifest-path src-tauri/Cargo.toml voice::streaming_asr`.
- First response: run `bun run quality:check` and `bun run desktop:smoke` on macOS. The desktop smoke requires a loaded snapshot and primary conversation.

Do not reset stored user settings or replace a configured provider to work around a startup failure. Test migrations against a copy or an isolated database.
