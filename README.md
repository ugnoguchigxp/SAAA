# SAAA

Situation-Aware Ambient Agent Runtime

English | [日本語](README.ja.md)

**MIT · MVP in development · Primarily verified on macOS**

[Get started](#run-locally) · [Contribute](CONTRIBUTING.md) · [Get help](SUPPORT.md) · [Security](SECURITY.md) · [License](LICENSE)


SAAA is a local-first AI runtime that brings conversation and voice into one desktop application. It is built with React and Tauri, and stores conversations and settings in a local SQLite database. Model traffic can be routed to a local LLM server on a private network, an OpenAI-compatible API, or the feature-gated LARM provider.

The long-term goal is a resident runtime that does more than answer prompts: it should decide whether to assist at all, based on the user's current situation. The implementation has not reached that goal yet. The app never operates applications or sends notifications automatically.

## Project status

This repository is an MVP under active development. It implements text and voice conversation, local database backups, and redacted diagnostics.

Normal development and offline verification are available. Production use through LARM is not yet approved: the API contract, isolated canary environment, 30-minute canary, two-hour soak test, and other gates still have open work. See the [Product Readiness Status](spec/docs/product-readiness-status.html) for the current feature and verification matrix. The older [MVP 2.6 Release Evidence](spec/docs/mvp-2.6-release-evidence.html) remains route-specific historical evidence.

## What SAAA can do today

| Surface | Main capabilities | Current safety boundary |
| --- | --- | --- |
| Chat | Text and microphone input, streamed responses, and playback through the OS speech runtime. The audit log opens from the conversation menu as a modal | Cloud fallback is disabled by default when a local route is selected |
| Settings | Manage model routes, voice, and privacy settings | Credentials are never stored in Settings or SQLite |

Voice chat uses the configured harness ASR service or an individual ASR provider. Text-only use does not require ASR or voice enrollment. ASR is the service that converts speech to text. The current implementation captures the microphone only. System audio, translation, and a floating overlay are not available.

## Requirements

- [Bun](https://bun.sh/) 1.3.14, pinned in `package.json`
- Rust 1.92.0 with rustfmt and Clippy, selected by `rust-toolchain.toml`
- The Tauri 2 build prerequisites for the target OS (on macOS, install Xcode Command Line Tools with `xcode-select --install`)
- To use the local conversation route, a local LLM server reachable over the private network. `LARM_API_TOKEN` is optional when that server permits anonymous LAN access and required when it enforces Bearer authentication.
- For voice input, access to the configured ASR service

macOS is the primary verification target. System TTS is implemented for macOS, Linux, and Windows.

## Run locally

Start from source. You can configure model and voice connections after opening the application.

```sh
git clone https://github.com/ugnoguchigxp/SAAA.git
cd SAAA
bun install --frozen-lockfile
bun start
```

1. Add a model connection in Settings, check connectivity, and select it for conversation. OpenAI-compatible API keys can be registered in Settings.
2. Send a short text message in Chat to verify a response.
3. Only if you want voice input, configure ASR, the input device, and microphone permission. The speaker filter is optional.

For a local LLM connection API that requires Bearer authentication, set `LARM_API_TOKEN` in the same shell before starting. Leave it unset only when the server explicitly permits anonymous LAN access. `bun run dev` starts the frontend development server. Use `bun start` to exercise desktop IPC and audio.

## Configure model connections

### Local LLM server

SAAA uses the connection API on the configured host to obtain a connection to the local model. It keeps the returned OpenAI-compatible endpoint, model name, and short-lived credential in memory, then releases the connection after each turn. None of these discovered values are written to SQLite.

SAAA sends `LARM_API_TOKEN` as a Bearer credential when the variable is set. It can be left unset for a trusted LAN server that explicitly allows anonymous access; this does not mean every server accepts unauthenticated requests. SAAA does not create an SSH tunnel, so both the connection API and the model endpoint returned by the server must be reachable over the private network.

Voice chat reuses the LAN host configured under Settings → Model Providers. SAAA derives the private ASR origin, queries `/v1/models` and `/health`, and reflects the resolved model under Settings → Voice. No separate ASR environment variable is required.

### OpenAI-compatible APIs

You can add an endpoint and model in Settings. For API-key authentication, save the provider's key in Settings; SAAA stores it in macOS Keychain under service `com.saaa.provider-api-key`, keyed by provider ID. The key itself is not stored in Settings JSON or SQLite. This credential storage requires macOS.

The current OpenAI-compatible provider does not read `SAAA_PROVIDER_<PROVIDER_ID>_API_KEY` or `OPENAI_API_KEY` as a fallback. The LARM token below belongs to a separate runtime path.

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

Settings → Voice → My voice profile can configure an on-device filter that matches the current speaker against the user's enrolled voice. Enabling the filter requires five valid samples, each 10–12 seconds long, with a combined duration of at least 50 seconds. Five long Japanese prompts with varied pronunciation and intonation are shown in sequence. Each prompt is intentionally longer than the recording window: keep reading continuously until capture stops automatically after about 12 seconds, even though the text will not be finished.

Voice samples are stored as unencrypted WAV files in the application-data directory, and speaker embeddings are stored unencrypted in SQLite. On macOS, the sample directory uses mode `0700` and sample files use mode `0600`. When the filter is enabled, SAAA sends audio to the local ASR server only after it passes local speaker matching. Model, stored-data, timeout, and ambiguous-speaker failures are fail-closed; they never fall back to sending unfiltered audio.

This filter applies to Voice chat. It is a transcription privacy filter, not identity authentication, liveness detection, replay protection, speaker diarization, or simultaneous-speaker separation.

## Develop and verify

Use these commands for the normal pre-change and post-change checks:

```sh
bun run check:local
bun run test:rust-packages
bun run spec:check
bun run desktop:smoke
bun run desktop:e2e --report-dir /absolute/path/to/new-report-directory
bun run readiness:verify --report-dir /absolute/path/to/new-report-directory
```

- `bun run check:local` runs oxfmt checks and oxlint before the existing `check` command.
- `bun run test:rust-packages` tests the standalone Rust crates and service.
- `bun run spec:check` checks specification structure and formatting.
- `bun run check` verifies module size, generated files, types, Rust formatting and Clippy, and both frontend and Rust tests.
- `bun run test:coverage` writes local HTML/LCOV reports under `coverage/`. It is optional and is not part of `bun run check`.
- `bun run build` type-checks TypeScript and creates a production frontend build.
- `bun run desktop:smoke` launches a debug desktop build with an isolated data directory and waits for IPC readiness. On macOS it also checks the bundled speaker-verification runtime.
- `bun run desktop:e2e` builds and launches the debug desktop app, then verifies the main UI, IPC, initial snapshot, primary conversation, SQLite, and situation sampling against isolated data. On macOS it also checks the speaker-verification runtime. Results are saved as `summary.json` and logs.
- `bun run readiness:verify` runs the three automated readiness lanes and writes a content-free, non-overwriting JSON report. Use a new absolute report directory for every run. A dirty working tree is always blocked from release use.
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

The main database has exactly one read-write owner: a Rust `SqliteWriter` created once in the Tauri process and reused by every write path. Startup acquires an exclusive OS lock before opening SQLite, so a second SAAA process using the same data directory is rejected before it can open or migrate the database. Read-only operations use separate SQLite connections opened with both the read-only flag and `query_only=ON`; each operation runs in one read transaction so its queries share a consistent snapshot. This allows multiple readers while keeping all writes serialized through the single Writer. The adjacent `saaa.sqlite3.writer.lock` file may remain after shutdown; the OS releases its lock automatically, so it must not be deleted while SAAA is running.

The database stores settings, conversations, completed messages, run state, unencrypted speaker embeddings, and a bounded structured audit trail. The audit trail retains lifecycle events for seven days; events older than seven days are removed when the application opens its database at startup. It links microphone capture, ASR utterances, conversation runs, provider sessions, TTS, and settings changes by correlation and causation identifiers. It stores event names, states, timestamps, outcomes, and failure codes. It does not evaluate or store voice-quality metrics, raw audio, transcript or prompt text, model output, credentials, endpoint addresses, or ephemeral allocation/request identifiers.

The conversation menu opens the latest 200 structured audit events in a read-only modal. Settings → Privacy & Security can create a consistent SQLite backup or a redacted diagnostics JSON file. Diagnostics include the latest 1,000 structured audit events and exclude conversation text, local paths, credentials, and connection secrets.

Database backups include the unencrypted speaker embeddings but not the WAV voice samples. Restoring a database backup alone therefore cannot restore a usable voice profile. SAAA also creates a pre-migration database backup automatically before opening an older schema.

## Current limitations

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
- [Product Readiness Status](spec/docs/product-readiness-status.html)
- [Product Readiness Acceptance Runbook](spec/docs/product-readiness-acceptance-runbook.html)
- [Internal Design Documents](spec/docs/README.html)
- [MVP 2.6 Release Evidence](spec/docs/mvp-2.6-release-evidence.html)
- [LARM Operations Runbook](spec/docs/mvp-2.6-larm-operations-runbook.html)
- [Runtime Boundary ADR](spec/docs/adr/0001-mvp-runtime-boundaries.html)
- [Input Activity Privacy ADR](spec/docs/adr/0003-input-activity-signal-privacy.html)

## Contributing

Reproduction steps, documentation and translation fixes, and regression tests are welcome. For substantial behavior changes, open an issue describing the problem and a concrete use case before implementation.

- [CONTRIBUTING](CONTRIBUTING.md): local checks, generated files, and pull requests
- [SUPPORT](SUPPORT.md): troubleshooting and useful report details, in English and Japanese
- [CODE_OF_CONDUCT](CODE_OF_CONDUCT.md): community expectations
- [SECURITY](SECURITY.md): handling vulnerability reports; a private reporting contact is still pending

## License

SAAA is available under the [MIT License](LICENSE). Bundled speaker-verification components keep their own terms; see [THIRD_PARTY_NOTICES](src-tauri/resources/voice/THIRD_PARTY_NOTICES.md).

See the [third-party license guide](THIRD_PARTY_NOTICES.md) for dependency and model notices.
