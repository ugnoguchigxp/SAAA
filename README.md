# SAAA

Situation-Aware Ambient Agent Runtime

English | [日本語](README.ja.md)

SAAA is a local-first AI runtime that brings conversation, voice, meeting transcription, and work-context observation into one desktop application. It is built with React and Tauri, and stores conversations and settings in a local SQLite database. Model traffic can be routed to a local LLM server on a private network, an OpenAI-compatible API, or the feature-gated LARM provider.

The long-term goal is a resident runtime that does more than answer prompts: it should decide whether to assist at all, based on the user's current situation. The implementation has not reached that goal yet. Situation observation currently runs only in an evaluation-oriented shadow mode and never operates applications or sends notifications automatically.

## Project status

This repository is an MVP under active development. It implements text and voice conversation, microphone-based meeting transcription, Situation recording and calibration, local database backups, and redacted diagnostics.

Normal development and offline verification are available. Production use through LARM is not yet approved: the API contract, isolated canary environment, 30-minute canary, two-hour soak test, and other gates still have open work. See [MVP 2.6 Release Evidence](spec/docs/mvp-2.6-release-evidence.html) for the current decision.

## What SAAA can do today

| Surface | Main capabilities | Current safety boundary |
| --- | --- | --- |
| Chat | Text and microphone input, streamed responses, and playback through the OS speech runtime | Cloud fallback is disabled by default when a local route is selected |
| Meeting | Partial and final microphone transcripts, pause and resume, and reviewed saving | Only explicitly started sessions capture audio; TTS is blocked during capture |
| Situation | Classify the foreground application, input activity, and SAAA's own state; record and replay intervention decisions | Disabled by default; never starts a model, notification, TTS, Meeting, or application action automatically |
| Settings | Manage model routes, voice, Situation, and privacy settings | Credentials are never stored in Settings or SQLite |

Voice chat and Meeting transcription use a local ASR server on the LAN. ASR is the service that converts speech to text. The current Meeting implementation captures the microphone only. System audio, translation, and a floating overlay are not available.

## Requirements

- [Bun](https://bun.sh/)
- Rust toolchain
- The Tauri 2 build prerequisites for the target OS
- To use the local conversation route, a local LLM server reachable over the private network and a `LARM_API_TOKEN`
- For voice input or Meeting, a local ASR server reachable from SAAA

macOS is the primary verification target. System TTS is implemented for macOS, Linux, and Windows, but Situation foreground/input signals and secure voice-profile key storage depend on macOS facilities.

## Run locally

Install dependencies:

```sh
bun install
```

To use the local LLM route, set its token in the same shell and start the desktop application:

```sh
export LARM_API_TOKEN="<token>"
bun start
```

After the application opens, configure and enable the local LLM provider under Settings → Model Providers. Enter only its hostname or private IP. SAAA obtains the connection details and model name from the server and does not persist them in Settings. No machine-specific endpoint or model is enabled by default.

## Configure model connections

### Local LLM server

SAAA uses the connection API on the configured host to obtain a connection to the local model. It keeps the returned OpenAI-compatible endpoint, model name, and short-lived credential in memory, then releases the connection after each turn. None of these discovered values are written to SQLite.

The local LLM server requires `LARM_API_TOKEN`. SAAA does not create an SSH tunnel, so both the connection API and the model endpoint returned by the server must be reachable over the private network.

Voice chat and Meeting reuse the LAN host configured under Settings → Model Providers. SAAA derives the private ASR origin, queries `/v1/models` and `/health`, and reflects the resolved model under Settings → Voice. No separate ASR environment variable is required.

### OpenAI-compatible APIs

You can add an endpoint and model in Settings. SAAA reads the credential from:

```text
SAAA_PROVIDER_<PROVIDER_ID>_API_KEY
```

`<PROVIDER_ID>` is uppercased and every non-alphanumeric character becomes `_`. For example, provider ID `local-llm` maps to `SAAA_PROVIDER_LOCAL_LLM_API_KEY`. Providers classified as Cloud may also use `OPENAI_API_KEY`.

### LARM provider

The LARM provider is disabled by default. Enable it only when testing the offline-verified route in a development environment:

```sh
export SAAA_LARM_ENABLED=1
export LARM_API_TOKEN="<token>"
bun start
```

The feature flag is read once at startup, so disabling it also requires an application restart. Before enabling production traffic, follow the [LARM Operations Runbook](spec/docs/mvp-2.6-larm-operations-runbook.html) and review the current release evidence.

### WebFetch tools

Conversation models receive `web_search` and `fetch_content` tools backed by `llm-fetch`. DuckDuckGo search works without an API key. If `BRAVE_SEARCH_API_KEY` is set when SAAA starts, Brave Search is used as a fallback after retryable DuckDuckGo failures.

The bundled runtime uses the package's model-facing toolset and strict Context Guard. Search results and retrieved text remain marked as untrusted tool data. Retrieval accepts only public HTTP(S) URLs on standard ports and does not include optional Playwright rendering.

## Voice profile

Settings → Voice → My voice profile can configure an on-device filter that matches the current speaker against the user's enrolled voice. Enabling the filter requires at least four valid samples, each 3–12 seconds long, with a combined duration of at least 20 seconds. Up to five samples can be stored.

Samples are stored in the application-data directory as AES-256-GCM-encrypted WAV files. Speaker embeddings are encrypted in SQLite, while the master key is stored only in macOS Keychain. When the filter is enabled, SAAA sends audio to the local ASR server only after it passes local speaker matching. Model, key, timeout, and ambiguous-speaker failures are fail-closed; they never fall back to sending unfiltered audio.

This is a transcription privacy filter. It is not identity authentication, liveness detection, or protection against replayed recordings.

## Develop and verify

Use these commands for the normal pre-change and post-change checks:

```sh
bun run check
bun run build
bun run desktop:smoke
```

- `bun run check` verifies module size, generated files, types, Rust formatting and Clippy, and both frontend and Rust tests.
- `bun run test:coverage` writes local HTML/LCOV reports under `coverage/`. It is optional and is not part of `bun run check`.
- `bun run build` type-checks TypeScript and creates a production frontend build.
- `bun run desktop:smoke` launches a debug desktop build with an isolated data directory and waits for IPC readiness. On macOS it also checks the bundled speaker-verification runtime.
- `bun run tauri build` creates the distributable desktop application for the target OS.

System Context sources live under `contexts/`. After changing one, run `bun run s11tnext:build`. When changing the Rust IPC types, run `bun run ipc:generate` to refresh the TypeScript types. Normal build and check commands fail when either generated artifact is stale.

Dedicated runners cover LARM canary and soak testing and the MVP 2 / 2.5 manual acceptance workflow. Review the relevant runbook and release evidence for the required environment variables, isolated directories, and command order before running them.

## Coverage reports

Line-coverage percentages are a local report, not a ship gate. Generate them with:

```sh
bun run test:coverage
```

Frontend LCOV is written to `coverage/frontend/`. If `cargo-llvm-cov` is installed, a Rust HTML report is written to `coverage/rust/`. The `coverage/` directory is gitignored.

## Local data and privacy

SAAA creates one SQLite database in the application-data directory for `com.saaa.desktop`. On macOS, it is stored at:

```text
~/Library/Application Support/com.saaa.desktop/saaa.sqlite3
```

The database stores settings, conversations, completed messages, run state, and encrypted speaker embeddings. It does not store model API keys, `LARM_API_TOKEN`, or the ephemeral connection details returned by the local LLM server.

Settings → Privacy & Security can create a consistent SQLite backup or a redacted diagnostics JSON file. Diagnostics exclude conversation text, local paths, and credentials.

Database backups do not include the encrypted voice samples or the macOS Keychain key. Restoring a database backup alone therefore cannot restore a usable voice profile. SAAA also creates a pre-migration database backup automatically before opening an older schema.

## Current limitations

- Situation is an evaluation-only shadow mode. Automatic intervention and application control are not implemented.
- Meeting supports microphone input only. System audio, translation, speaker diarization, and a floating overlay are unavailable.
- Meeting transcripts remain in memory until the user stops the session, reviews the save target and contents, and selects Save. They are discarded otherwise.
- LARM production use is awaiting approval. Do not treat it as a production route until the API contract, isolated environment, canary, soak, security, rollback, and runbook reviews are complete.

## Repository layout

```text
src/             React UI
src-tauri/       Rust runtime and Tauri desktop shell
contexts/        s11tnext system-context sources
scripts/         smoke tests and readiness runners
tests/           frontend and contract tests
spec/docs/       design documents, ADRs, runbooks, and release evidence
```

## Documentation

- [Project Concept & Direction](spec/docs/plan.html)
- [Internal Design Documents](spec/docs/README.html)
- [MVP 2.6 Release Evidence](spec/docs/mvp-2.6-release-evidence.html)
- [LARM Operations Runbook](spec/docs/mvp-2.6-larm-operations-runbook.html)
- [Runtime Boundary ADR](spec/docs/adr/0001-mvp-runtime-boundaries.html)
- [Situation Privacy ADR](spec/docs/adr/0002-situation-signal-privacy.html)
- [Input Activity Privacy ADR](spec/docs/adr/0003-input-activity-signal-privacy.html)
