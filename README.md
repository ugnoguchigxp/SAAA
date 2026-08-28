# SAAA MVP 2.5

SAAA is a local-first Tauri desktop runtime for persistent text chat, push-to-talk voice, OpenAI-compatible model routing, a supervised read-only Codex coding route, privacy-minimized Situation calibration with bounded Input Activity categories, and explicitly started microphone Meeting sessions.

## Run locally

Requirements: Bun, the Rust toolchain, and the native Tauri prerequisites for the target OS.

```sh
bun install
bun run tauri dev
```

Configure model endpoints in Settings. Credentials are read only from `SAAA_PROVIDER_<PROVIDER_ID>_API_KEY`; `OPENAI_API_KEY` is also accepted for Cloud providers. Run `codex login` before enabling the Codex route. Voice transcription requires a local whisper.cpp-compatible executable (`SAAA_WHISPER_PATH`) and a model selected in Settings → Voice. TTS uses the OS speech runtime.

Situation monitoring is off by default. Enable it in Settings → Situation or from the Situation surface. It records only bounded categories, evidence codes, signal health, aggregate quality counters, and counterfactual `would observe / suggest / respond / stay silent` decisions. Calibration candidates use repository fixtures and become active only after an explicit Replay and Accept. Situation never performs automatic Model, TTS, notification, Meeting start, or application actions.

Meeting capture also starts only after an explicit user action. This build supports microphone-only local Whisper transcription. System audio, translation, and a floating overlay remain unavailable. Transcript text stays in bounded memory and is discarded unless the user stops the session and then chooses Save. TTS is blocked while a Meeting session is active or paused.

## Verify and package

```sh
bun run check
bun run build
bun run codex:smoke
bun run desktop:smoke
```

`desktop:smoke` builds a debug desktop artifact, launches it with an isolated temporary data directory, and waits for an IPC readiness signal. On macOS it verifies the generated `.app`, including the native Codex runtime staged from `@openai/codex-sdk`'s pinned dependency.

## Local data and recovery

SAAA owns one SQLite database under the OS application-data directory for `com.saaa.desktop` (macOS: `~/Library/Application Support/com.saaa.desktop/saaa.sqlite3`). Settings, conversations, completed messages, run status, and Codex thread IDs are stored there. Prompts and audio are not sent to Cloud when the local-only policy applies, and credentials are never stored in SQLite.

Use Settings → Privacy & Security to create a consistent database backup or a redacted diagnostics JSON. A pre-migration backup is created automatically before opening an older schema. Whisper models are user-owned files and are not part of the SAAA database backup.

See [MVP 2.5 release evidence](spec/docs/mvp-2.5-release-evidence.html), [MVP 2 release evidence](spec/docs/mvp-2-release-evidence.html), [Input Activity privacy ADR](spec/docs/adr/0003-input-activity-signal-privacy.html), [Situation privacy ADR](spec/docs/adr/0002-situation-signal-privacy.html), and [runtime boundary ADR](spec/docs/adr/0001-mvp-runtime-boundaries.html).
