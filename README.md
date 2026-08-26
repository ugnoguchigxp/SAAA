# SAAA MVP 1

SAAA is a local-first Tauri desktop runtime for persistent text chat, push-to-talk voice, OpenAI-compatible model routing, an explicit read-only Codex coding route, and privacy-minimized Situation Shadow Mode.

## Run locally

Requirements: Bun, the Rust toolchain, and the native Tauri prerequisites for the target OS.

```sh
bun install
bun run tauri dev
```

Configure model endpoints in Settings. Credentials are read only from `SAAA_PROVIDER_<PROVIDER_ID>_API_KEY`; `OPENAI_API_KEY` is also accepted for Cloud providers. Run `codex login` before enabling the Codex route. Voice transcription requires a local whisper.cpp-compatible executable (`SAAA_WHISPER_PATH`) and a model selected in Settings → Voice. TTS uses the OS speech runtime.

Situation monitoring is off by default. Enable it in Settings → Situation or from the Situation surface. MVP 1 records only bounded categories, evidence codes, signal health, and counterfactual `would observe / suggest / respond / stay silent` decisions. It never performs automatic Model, TTS, notification, or application actions.

## Verify and package

```sh
bun run check
bun run codex:smoke
bun run desktop:smoke
```

`desktop:smoke` builds a debug desktop artifact, launches it, and waits for an IPC readiness signal. On macOS it verifies the generated `.app`, including the exact native Codex runtime staged from `@openai/codex-sdk`'s pinned dependency.

## Local data and recovery

SAAA owns one SQLite database under the OS application-data directory for `com.saaa.desktop` (macOS: `~/Library/Application Support/com.saaa.desktop/saaa.sqlite3`). Settings, conversations, completed messages, run status, and Codex thread IDs are stored there. Prompts and audio are not sent to Cloud when the local-only policy applies, and credentials are never stored in SQLite.

Use Settings → Privacy & Security to create a consistent database backup or a redacted diagnostics JSON. A pre-migration backup is created automatically before opening an older schema. Whisper models are user-owned files and are not part of the SAAA database backup.

See [MVP 1 release evidence](spec/docs/mvp-1-release-evidence.md), [Situation privacy ADR](spec/docs/adr/0002-situation-signal-privacy.md), and [runtime boundary ADR](spec/docs/adr/0001-mvp-runtime-boundaries.md).
