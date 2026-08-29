# SAAA MVP 2.6.1

SAAA is a local-first Tauri desktop runtime for persistent text chat, push-to-talk voice, OpenAI-compatible model routing, a supervised read-only Codex coding route, privacy-minimized Situation calibration with bounded Input Activity categories, and explicitly started microphone Meeting sessions.

## Run locally

Requirements: Bun, the Rust toolchain, and the native Tauri prerequisites for the target OS.

```sh
bun install
bun start
```

The default conversation Provider stores only the `gnosis` host (`192.168.0.65`, or a local hostname). For each turn SAAA calls the Agent Connection API directly over HTTP at `http://<gnosis-host>:9810`, requests the `deep-reasoning-35b` profile with the `saaa-desktop` audience, waits for semantic readiness, then applies the claimed OpenAI-compatible `baseUrl`, model, and short-lived bearer credential in memory. The connection is released after the turn; discovered endpoints, models, audiences, and credentials are never written to SQLite. `LARM_API_TOKEN` must be present in the SAAA process environment, and the control API plus its claimed data-plane endpoint must be reachable on the private network. SAAA does not create or use an SSH tunnel. Existing `gnosis-qwen` direct endpoint/model settings are migrated to this host-only form while preserving the configured host. Other direct-provider credentials are read only from `SAAA_PROVIDER_<PROVIDER_ID>_API_KEY`; `OPENAI_API_KEY` is also accepted for Cloud providers. Voice transcription continues to use the LAN-local gnosis ASR provider at `http://192.168.0.65:8081`, using `qwen3-asr-1.7b`. TTS uses the OS speech runtime.

LARM integration is disabled by default. To run the offline-tested LARM route, start SAAA with `SAAA_LARM_ENABLED=1` and `LARM_API_TOKEN` set, then configure a numeric loopback origin such as `http://127.0.0.1:9810` in Settings. The flag is read once at process startup; turning the operator kill switch off therefore requires a restart. Settings rollback applies to new turns without restarting. Connection Test calls only `/health` and `/ready`; it never allocates a runtime or starts inference. SAAA sends `llm.general` + `llm-default`, `allowFallback: false`, and `existing-only`, and it does not manage models, ports, services, or artifacts.

Production readiness is operator-only and remains closed while G1 contract deviations are recorded in the [MVP 2.6 release evidence](spec/docs/mvp-2.6-release-evidence.html). The completed readiness implementation contract is [archived](spec/docs/.archived/mvp-2.6.1-implementation-plan.html); after G1 and the isolated G2 manifest are approved, follow the [LARM operations runbook](spec/docs/mvp-2.6-larm-operations-runbook.html) and use `bun run larm:preflight`, `bun run larm:canary`, the two bounded `bun run larm:soak` modes, and finally `bun run larm:report`; each command requires `--report-dir` and never accepts credentials, endpoints, prompts, or remote commands on the command line.

The HTTP adapter and fake-server suite are pinned to LARM commit `7dca7c3`. Production canary remains gated: the current LARM response exposes `createdAt` and `expiresAt` but no explicit server-confirmed effective-TTL field; the stable virtual `model` value/rewrite contract and deployment-independent Gateway request limit are also not fixed. The private tunnel, deployment revision, canary operator, and rollback provider must be fixed before real traffic is enabled.

Situation monitoring is off by default. Enable it in Settings → Situation or from the Situation surface. It records only bounded categories, evidence codes, signal health, aggregate quality counters, and counterfactual `would observe / suggest / respond / stay silent` decisions. Calibration candidates use repository fixtures and become active only after an explicit Replay and Accept. Situation never performs automatic Model, TTS, notification, Meeting start, or application actions.

Meeting capture also starts only after an explicit user action. This build supports microphone-only transcription through the LAN-local gnosis ASR provider, including a bounded partial preview that is replaced by each final segment. System audio, translation, and a floating overlay remain unavailable. Transcript text stays in bounded memory and is discarded unless the user stops the session, reviews the target, entry count, languages, and raw-audio policy, and then confirms Save. TTS is blocked while a Meeting session is active or paused.

Settings → Voice → My voice profile can store up to five 3–12 second samples of the user's voice. Four valid samples totaling at least 20 seconds are required before the target-speaker filter can be enabled. Samples are normalized locally, checked for level, clipping, speech ratio, and enrollment consistency, then saved as AES-256-GCM encrypted WAV files under the application-data directory. Speaker embeddings are also encrypted in SQLite; the master key is stored only in macOS Keychain. The pinned downloads are checksum-verified during installation; at runtime the CAMPPlus model hash is checked, while signed Mach-O libraries are covered by the app signature and strict native-symbol loading. When the filter is enabled, every voiced window must match locally before any preview or final audio reaches gnosis ASR; model, key, timeout, and ambiguous-speaker failures are fail-closed with no unfiltered fallback. Filtered Chat listening stays armed while an LLM request or system TTS is active, keeps bounded ASR/query queues, and stops TTS after a verified user barge-in. This is a transcription privacy filter, not liveness detection or an authentication mechanism.

## Verify and package

System contexts are authored under `contexts/` and compiled with S11tnext. After changing a context,
run `bun run s11tnext:build`; normal development, build, and test commands reject stale generated
artifacts.

Runtime event and conversation-message TypeScript contracts are generated from
`src-tauri/src/ipc_contract.rs`. After changing those Rust IPC types, run
`bun run ipc:generate`; normal build, test, and check commands reject stale bindings.

```sh
bun run check
bun run build
bun run codex:smoke
bun run desktop:smoke
```

`desktop:smoke` builds a debug desktop artifact, launches it with an isolated temporary data directory, and waits for an IPC readiness signal. On macOS it verifies the generated `.app`, including the native Codex runtime staged from `@openai/codex-sdk`'s pinned dependency and successful initialization of the checksum-verified speaker runtime. Run `scripts/install-speaker-runtime.sh` only when the pinned voice artifacts need to be restored; the script verifies both downloads before installing them.

MVP 2 / 2.5 desktop acceptance uses the strict `mvp2x` runner. Set
`SAAA_MVP2X_OPERATOR` to a non-identifying operator label,
`SAAA_MVP2X_APP_DATA_DIR` to an empty mode-0700 directory,
`SAAA_MVP2X_WORKSPACE_DIR` to a clean fixture Git repository, and optionally
`SAAA_MVP2X_BUNDLE_PATH` to the signed packaged app. The report directory must be an empty,
absolute, mode-0700 directory outside this repository. Run `bun run mvp2x:preflight`, the
documented `bun run mvp2x:verify` suites, and finally `bun run mvp2x:report`, each with
`--report-dir`. Run preflight while the dedicated app-data directory is still empty, then launch
the exact signed bundle executable from the same environment; SAAA uses
`SAAA_MVP2X_APP_DATA_DIR` instead of normal application data for these runs. Meeting and Input
Activity soak cases additionally read the running bundle PID from `SAAA_MVP2X_APP_PID` and sample
RSS externally. Verification refuses a missing dedicated database, a different executable,
source/bundle identity drift, or fixture workspace drift. Agent Run workspace integrity and soak
elapsed/RSS observations are populated by the runner rather than operator-entered values.
For functional and manual cases the runner asks for the current app PID and requires the exact
debug or signed-bundle executable before the case begins; cases are grouped by build class.
The CLI accepts no credentials, endpoints, prompts, transcript text, or free-form evidence; reports
record no local paths or content. The runner refuses dirty sources, ad-hoc signatures, report reuse,
identity mismatch, missing cases, threshold failures, and forbidden data. The aggregate includes a
canonical digest of the preflight and suite report set for evidence closeout.

## Local data and recovery

SAAA owns one SQLite database under the OS application-data directory for `com.saaa.desktop` (macOS: `~/Library/Application Support/com.saaa.desktop/saaa.sqlite3`). Settings, conversations, completed messages, run status, Codex thread IDs, and encrypted voice embeddings are stored there. Encrypted enrollment WAV files are stored alongside it under `voice-profiles/default/`; their key is not. Prompts and audio are not sent to Cloud when the local-only policy applies, and credentials are never stored in SQLite.

Use Settings → Privacy & Security to create a consistent database backup or a redacted diagnostics JSON. A pre-migration backup is created automatically before opening an older schema. Database backups contain encrypted voice embeddings but do not contain enrollment WAV files or the Keychain key, so restoring a backup alone does not restore a usable voice profile. ASR model files remain on gnosis and are not part of the SAAA database backup.

See the archived [MVP 2 / 2.5 completion implementation contract](spec/docs/.archived/mvp-2-2.5-completion-implementation-plan.html), [MVP 2.6 LARM implementation contract](spec/docs/.archived/mvp-2.6-implementation-plan.html), and [MVP 2.6.1 readiness implementation contract](spec/docs/.archived/mvp-2.6.1-implementation-plan.html), plus the active [LARM operations runbook](spec/docs/mvp-2.6-larm-operations-runbook.html), [MVP 2.6 release evidence](spec/docs/mvp-2.6-release-evidence.html), [MVP 2.5 release evidence](spec/docs/mvp-2.5-release-evidence.html), [MVP 2 release evidence](spec/docs/mvp-2-release-evidence.html), [Input Activity privacy ADR](spec/docs/adr/0003-input-activity-signal-privacy.html), [Situation privacy ADR](spec/docs/adr/0002-situation-signal-privacy.html), and [runtime boundary ADR](spec/docs/adr/0001-mvp-runtime-boundaries.html).
