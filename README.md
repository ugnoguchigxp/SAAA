# SAAA

**Situation-Aware Ambient Agent Runtime**

English | [日本語](README.ja.md)

**Local-first · Active development · MIT · Primarily verified on macOS**

[Run locally](#run-locally) · [Architecture](#runtime-architecture) · [Documentation](#documentation) · [Contributing](CONTRIBUTING.md)

SAAA is a desktop runtime for a personal AI that understands ongoing work, retains goals and constraints, uses tools within delegated authority, and decides when to act or stay quiet. It brings situation sensing, durable memory, a personal world model, role-based model routing, and background execution into a shared runtime.

The core experience is continuity: a request becomes tracked work; its context and authority survive beyond the immediate exchange; results return to the task ledger and can be delivered when the user's situation permits. Text and voice provide the conversational interface, while the runtime manages the state, execution, and delivery behind it.

This repository is actively implementing that system. It already contains the memory and context pipeline, WorldFrame composition, tool discovery and execution, delegated-work ledger and report pump, and role-routing executor. Support depends on the configured providers, execution profiles, and enabled services. The full vision and its design principles are described in the [Personal AI Concept](spec/docs/saaa-personal-ai-concept.md); outstanding integration work is described below.

## The runtime loop

```text
User input / permitted situation signals / deadlines / execution results
                                  │
                     Durable events and source records
                                  │
                Personal State + World Model + goals and delegation
                                  │
                 Context Broker: scope, freshness, required context
                                  │
              Role routing + tool selection + execution policy
                                  │
                  Respond / run delegated work / hold / ask
                                  │
                     Result verification and task progress
                                  │
              Situation-aware delivery + feedback and learning records
```

The runtime keeps separate records for what was observed, what is currently believed, what the user wants, and what was actually executed. A model response or retrieved document does not grant execution authority. Task state belongs to the execution ledger; the World Model presents a derived view of it.

## Implemented subsystems

| Subsystem | What is in the repository |
| --- | --- |
| Conversation and artifacts | Streamed text, microphone input, speech output, persistent conversation, scope selection, Markdown/Mermaid artifacts, and generated UI views with revisions and data snapshots |
| Situation awareness | Foreground application categories, input-activity categories, conversation/microphone/audio state, scene classification, confidence, hysteresis, and speech/delivery policies |
| Memory and continuity | Local conversation recall, source-backed Personal State, background extraction, corrections and invalidation, and typed experience/rule/skill recall through ContextStill |
| World Model | Typed entities and relations, observations and hypotheses, bounded graph traversal, and fresh views of task, schedule, situation, and capacity state |
| Shared context | Generation-scoped context assembly, required/optional budgets, scope checks, source dependencies, and final provider-input validation |
| Tools and capabilities | Web retrieval, a searchable tool catalog, external MCP sources, a local MCP gateway, and generated L-Lang/Wasm capability lifecycle |
| Delegated work | Source-bound proposals, goals and grants, bounded step plans, read/test recipes, execution queues, cancellation, recovery, result verification, and a durable report outbox |
| Role routing | Configurable model actors, role assignments, finite execution recipes, review/revision, upgrade proposals, shared budgets, and a persistent execution ledger |
| Adaptation | Decision/outcome records, explicit scoped overrides, background candidate training, evaluation gates, and activation/rollback primitives |

These are implementation areas, not a claim that every route is enabled by default or has completed end-to-end acceptance.

## Memory and continuity

SAAA uses several kinds of memory, each with a distinct owner and purpose.

| Layer | Stored or retrieved information | How it is used |
| --- | --- | --- |
| Conversation ledger | Original messages, timestamps, and run references | `recall_conversation` retrieves bounded windows using local full-text search and time filters |
| Personal State | Objectives, constraints, decisions, pending decisions, open loops, active referents, and progress references | Background extraction produces source-backed assertions for subsequent generations |
| Typed knowledge recall | Experiences, rules, and skills | `recall_experience`, `recall_rule`, and `recall_skill` use the configured local ContextStill service |
| World projection | Entities, relationships, focus, and supporting evidence | Builds a scoped view of the current situation and relevant relationships |
| Learning ledger | Candidates, selections, outcomes, corrections, and policy revisions | Supports explicit corrections and measured selection-policy improvement |

Personal State tracks whether an assertion is a candidate, active, disputed, resolved, superseded, retracted, invalidated, or stale. A correction can replace a previous assertion without treating both as current. Sources and scope travel with the derived state; deletion and invalidation propagate through dependent records.

The **Context Broker** assembles input for each generation from these sources and the current request. It reserves space for required constraints and execution continuations, budgets optional history and recall, and validates dependencies before dispatch. Recalled content remains historical data. Model-specific token budgeting and complete coverage across changing adapters remain development work.

Personal State extraction is opt-in through `SAAA_MEMORY_ENABLED=1` and requires a configured extraction route. Typed recall additionally requires a reachable ContextStill service; it is not an embedded knowledge database or an automatic cloud fallback.

Implementation: [`memory/`](src-tauri/src/memory/), [`personal-state-core`](crates/personal-state-core/), [`runtime/context/`](src-tauri/src/runtime/context/).

## Personal World Model

The World Model represents the user's current working world: projects, concepts, metrics, goals, actors, and their relationships. Relations include dependency, relevance to goals, influence, causation, and correlation. Observations and hypotheses are represented separately, with evidence and validity information.

A **WorldFrame** combines two sources:

- A bounded, scoped graph slice from persisted World assertions.
- Fresh state read from the owning runtimes, such as coding jobs, delegated work, schedules, situation, and available capacity.

Frames have size limits, validity windows, source references, and stamps used to detect change. Supported provider paths receive the frame through the shared context pipeline. State-answer validation and host-rendered cards tie supported state claims back to the supplied evidence.

The UI exposes the active scope. A task's status is read from its owner rather than independently maintained in the graph. Graph queries currently enter through a limited set of explicit question forms; broader conversational phrasing and ambiguous-reference resolution are part of the current improvement plan. The graph provides recorded relationships, not proof that every causal hypothesis is true.

Implementation: [`world/`](src-tauri/src/memory/personal_state/world/), [`WorldFrame integration`](src-tauri/src/runtime/context/world/). Design: [Personal World Model](spec/docs/saaa-personal-world-model-concept.md).

## Tool and capability system

SAAA separates discovering a capability, reading its contract, and invoking it. The catalog gateway exposes three stable model-facing tools:

1. `tools_search` finds candidates for an intent.
2. `tools_describe` retrieves a candidate's contract, usage, or result pages.
3. `tools_invoke` executes through a validated execution reference.

The catalog combines generated capabilities and configured external MCP tools. Discovery can use local embeddings and reranking, with scoped correction rules and persistent invocation records. Execution references, argument validation, access checks, cancellation, and result limits are enforced at the host boundary. Missing local discovery models produce a degraded state instead of silently selecting a remote model.

External MCP servers connect through the Streamable HTTP integration. SAAA can also expose the same three entry points as an authenticated, loopback-only MCP server, sharing the application's catalog and ledger. The conversation runtime also includes direct tools such as web retrieval, memory recall, work proposals, and generated UI operations.

### Generated L-Lang / Wasm capabilities

The generated-capability subsystem implements generation, build/package, verification, inspection, publication, invocation, and retirement. It retains capability revisions and runs Wasm through a separate host process with contract and resource checks. Generation and execution depend on explicitly configured providers, build tools, and runtime bundles; a generated description alone is not an executable capability.

See the [Capability / Tool Runtime Concept](spec/docs/saaa-capability-tool-runtime-concept.md), [tool-selection implementation guide](spec/docs/saaa-tool-selection-d0-d3-implementation-guide.md), and [L-Lang capability guide](spec/docs/saaa-llang-dynamic-capability-implementation-guide.md).

## Role-based model routing

Role routing assigns model deployments to responsibilities. An **actor** records its transport, provider/model, location, resource group, capabilities, and input limit. A **role** points to an actor; a **recipe** specifies the bounded sequence of roles used for an action.

| Role | Responsibility |
| --- | --- |
| `frontend` | Immediate interaction and clarification |
| `reasoner` | Main reasoning and response generation |
| `advanced` | More demanding reconsideration |
| `reviewer` | Review by a different configured deployment |
| `premium` | An upgrade path governed by the configured approval policy |
| `tool_specialist` | Tool-focused work within the common tool boundary |

Supported actions include response, explanation, clarification, reconsideration, review followed by revision, and upgrade proposals. Recipes compile into finite steps with explicit dependencies. The host checks actor eligibility, context, revision, deadlines, step/tool/review budgets, and configured cost limits before dispatch. Root and step records support cancellation, stale-result handling, and recovery. Speech acknowledgements and progress are coordinated separately from the reasoning output.

Routing is opt-in in Settings. Assigning an actor does not make every provider interchangeable: the selected transport and capabilities must support the role. The selection and learning records support comparison of quality, latency, and cost; learned policies remain subject to eligibility and activation gates.

Implementation: [`role_routing/`](src-tauri/src/role_routing/). Design: [execution contract](spec/docs/saaa-role-routing-execution-contract.md), [learning contract](spec/docs/saaa-role-routing-learning-contract.md).

## Situation awareness and delegated work

Situation sensing classifies states such as conversation, meeting, coding, writing, media, focus, solo, and unknown. On macOS, platform sampling reduces the foreground application and time since input into categories; it does not collect typed text or screen contents. Confidence, hysteresis, and signal health help avoid treating brief or unavailable signals as reliable state.

Delegated work uses separate **goals**, **grants**, **plans**, and **execution records**. A proposal binds to its user source and declared operations. Accepted work moves through a bounded plan, queue, and supported coding execution profile. A persistent driver consumes results, advances eligible steps, and creates reports in an outbox. Scheduling and situation policies decide whether to act, hold, defer, or ask.

Reports can be processed without a new user message. The implementation includes periodic recovery and wake handling; prompt wake-up on all completion and hold-release paths is still being improved. Result evidence, negative-request handling, and integration acceptance are also active work. Current execution is limited to supported recipes, profiles, and grants, rather than general autonomous control of arbitrary desktop applications.

Implementation: [`situation/`](src-tauri/src/situation/), [`steward/`](src-tauri/src/steward/), [`schedule/`](src-tauri/src/schedule/).

## Adaptation and current development focus

SAAA records choices and outcomes across provider/recipe, tool, plan, and notification selection. Explicit user overrides take precedence over learned rankings. A background worker materializes datasets and trains candidates; evaluation, approval, activation, invalidation, and rollback have separate responsibilities.

The remaining work includes connecting measured evaluation and promotion to normal management flows. Candidate training is not evidence of an improvement, and the existence of an activation function does not mean a policy is automatically active.

The [five-improvement implementation plan](spec/docs/saaa-review-five-improvements-terra-plan.md) tracks the current integration priorities:

- Bind task completion to actual, matching execution evidence.
- Protect negative and withdrawn requests using the full source and current authority.
- Complete the normal evaluation-to-activation path for adaptive policies.
- Resolve natural graph questions and references within scope.
- Wake report processing promptly on completion and situation changes.

## Runtime architecture

React provides the interaction and inspection surfaces. Tauri hosts the Rust runtime, which owns execution policy, provider/tool boundaries, and persistence. SQLite is the local source of truth; projections and provider context are derived from its records and current runtime state.

```text
src/                         React conversation, settings, work and artifact UI
src-tauri/src/runtime/       Conversation orchestration and shared Context Broker
src-tauri/src/memory/        Recall, Personal State, World Model and background extraction
src-tauri/src/role_routing/  Actor selection, recipe execution, budgets and learning
src-tauri/src/tool_selection/ Tool catalog, discovery, MCP gateway and invocation ledger
src-tauri/src/generated_capabilities/ L-Lang/Wasm capability lifecycle and host
src-tauri/src/steward/       Delegated goals, plans, execution and report delivery
src-tauri/src/situation/     Signal sampling, classification and attention policies
src-tauri/src/schedule/      Durable deadlines and scheduled-work decisions
src-tauri/src/persistence/   SQLite writer, readers, schema and backups
crates/                     Shared Rust contracts and core logic
services/                   Companion services
contexts/                   s11tnext system-context sources
scripts/                    Development, evaluation and acceptance runners
spec/                       Concepts, contracts, plans and evidence
```

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

### Agent-session execution

The runtime also includes agent-session integration and coding execution adapters. Configure the corresponding provider or coding profile and its required local runtime before assigning it to a routing actor or delegated recipe. An OpenAI-compatible endpoint alone does not provide the same execution capabilities. Role and tool eligibility are checked separately from basic connectivity.

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

## Enable additional subsystems

Basic text conversation can be configured from Settings. Additional subsystems have their own prerequisites; enabling a flag does not install the required service or model.

| Subsystem | Configuration entry |
| --- | --- |
| Personal State extraction | `SAAA_MEMORY_ENABLED=1`, plus a working extraction provider |
| Typed memory recall | `SAAA_MEMORY_ENABLED=1` and local ContextStill endpoint discovery; `SAAA_CONTEXT_STILL_RUN_DIR` can override its discovery directory |
| Tool discovery and external MCP sources | `SAAA_TOOL_SELECTION_CONFIG` points to the tool-selection JSON configuration |
| SAAA's local MCP gateway | `SAAA_TOOL_GATEWAY_MCP_CONFIG` points to its listener/authentication configuration |
| L-Lang generation | `SAAA_LLANG_GENERATION_CONFIG` points to its generation configuration; capability execution also needs its runtime bundle |
| Role routing and adaptive settings | Role-routing settings: configure actors, role assignments, recipes, limits, and learning options |
| Situation and schedules | Their settings and platform permissions determine available signals and actions |

Use the linked subsystem guides for complete configuration contracts. Mock/fixture modes are for isolated development and do not establish live-provider support.

## Local data and privacy

The application stores its database in the data directory for `com.saaa.desktop`. On macOS:

```text
~/Library/Application Support/com.saaa.desktop/saaa.sqlite3
```

One Rust `SqliteWriter` owns all database writes. An OS lock prevents a second process from opening the same data directory for writing. Read-only connections use consistent transactions. Do not remove `saaa.sqlite3.writer.lock` while the application is running; the OS releases the lock on shutdown.

Local records include conversations and settings, Personal State and World assertions, task and tool ledgers, routing and learning records, generated-view state, and structured audit events. Local-first storage does not imply local-only inference: selected providers and tools receive the input needed for their configured work. Cloud fallback is disabled by default when a local conversation route is selected.

Provider API keys registered in Settings use macOS Keychain rather than Settings JSON or SQLite. The main database and voice-profile data are not encrypted by SAAA. The optional voice filter stores WAV samples in the application-data directory and embeddings in SQLite; see the voice-profile section above for its scope and limitations.

The structured audit trail records lifecycle metadata rather than raw prompt, transcript, or model-output text. Conversation text remains in its own records. Settings provides consistent SQLite backups and redacted diagnostics. Database backups include voice embeddings but not the WAV samples or every external capability/model file, so a database backup alone is not a complete runtime or voice-profile backup. Older schemas receive a pre-migration database backup.

For a Harness LLM failure, inspect the voice monitor's request timestamp and diagnostic code. Refreshing the monitor does not retry a historical request. `harness-catalog-schema-invalid` identifies catalog decoding; `harness-llm-context-window-missing` identifies a missing budget on the selected LLM; claim, health and connection schema errors have separate codes. Remote response bodies and credentials are not copied into audit records.

To check the provider without changing conversation history, run `cargo run --manifest-path src-tauri/Cargo.toml --no-default-features --features provider-diagnostics --bin harness_llm_diagnostic -- 192.168.0.130` (replace the host as needed). It uses the ordinary library build and credential loader, checks an exact fixed response through the allocated provider path, and verifies connection release. This checks Harness-to-LLM integration, not microphone capture or TTS. Catalog wire types must never use test-only deserialization defaults; v3 context budgets belong to individual providers, not profiles.

## Develop and verify

Install the pinned dependencies, then run the checks appropriate to your change:

```sh
bun run check:local
bun run test:rust-packages
bun run spec:check
```

`check:local` runs formatting, lint, generated-file and type checks, module-size checks, and the project's Rust/frontend checks. For focused changes, run the relevant `bun test tests/<file>` or `cargo test --manifest-path src-tauri/Cargo.toml <filter>` first. A successful filtered command must actually execute the intended tests.

After changing `contexts/`, run `bun run s11tnext:build`. After changing exported Rust IPC types, run `bun run ipc:generate` and inspect the generated diff. `bun run build` builds the frontend and checks IPC contracts; `bun run tauri build` creates the desktop bundle. Optional local coverage reports are available through `bun run test:coverage`.

Desktop and live-service acceptance require their documented environment:

```sh
bun run desktop:smoke
bun run desktop:e2e --report-dir /absolute/path/to/new-report-directory
bun run readiness:verify --report-dir /absolute/path/to/new-report-directory
```

Use isolated data and a new report directory. Smoke success demonstrates startup/IPC readiness, not completion of a delegated job or measured learning gains. Report failed or unavailable lanes separately; do not replace missing live evidence with fixture success. See [Product Readiness](spec/docs/product-readiness-status.html) and the [acceptance runbook](spec/docs/product-readiness-acceptance-runbook.html) for platform and route-specific conditions. The feature-gated LARM production route retains its separate [operations requirements](spec/docs/mvp-2.6-larm-operations-runbook.html).

## Documentation

- [Personal AI Concept](spec/docs/saaa-personal-ai-concept.md): the current product vision and shared-runtime principles
- [Capability / Tool Runtime](spec/docs/saaa-capability-tool-runtime-concept.md): capability discovery and execution responsibilities
- [Personal State architecture](spec/docs/personal-state-architecture-roadmap.md): durable continuity and memory
- [Personal World Model](spec/docs/saaa-personal-world-model-concept.md): entities, relations, evidence, and current understanding
- [Role-routing execution contract](spec/docs/saaa-role-routing-execution-contract.md): actors, recipes, budgets, and revisions
- [Adaptive Learning and Memory](spec/docs/saaa-adaptive-learning-memory-concept.html): feedback and selective memory
- [Interface / Artifact Runtime](spec/docs/saaa-interface-artifact-runtime-concept.md): interactive outputs and artifacts
- [Current improvement plan](spec/docs/saaa-review-five-improvements-terra-plan.md): integration work and acceptance conditions
- [Design document index](spec/docs/README.html): detailed plans, runbooks, and evidence

Concept documents describe intended behavior; implementation plans and evidence distinguish completed work from remaining work.

## Contributing

Reproduction steps, implementation improvements, documentation, translations, and regression tests are welcome. For substantial behavior changes, describe the problem and a concrete use case in an issue.

[Contributing](CONTRIBUTING.md) · [Support](SUPPORT.md) · [Code of Conduct](CODE_OF_CONDUCT.md) · [Security](SECURITY.md)

## License

[MIT](LICENSE). Bundled dependencies and model assets retain their own terms; see [third-party notices](THIRD_PARTY_NOTICES.md) and the [speaker-verification notices](src-tauri/resources/voice/THIRD_PARTY_NOTICES.md).
