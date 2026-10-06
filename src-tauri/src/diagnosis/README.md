# Diagnosis

Answers one question: **which SAAA capabilities can the user rely on right now, and on what evidence?** Contract: `spec/docs/saaa-self-diagnosis-v2-contract.md`.

Model: checks emit typed `Evidence` (tier `static` / `observed` / `probe`, outcome, `Reason`, route, expiry). `aggregate` turns evidence into a state per `Capability` (storage, conversation, voice-listen, voice-speak, voice-echo, memory, coding) and an overall verdict. Checks never decide a state; the UI owns all wording.

Invariants:
- Unproven is never green. Capabilities that need live proof (`Capability::needs_live_proof`) stay `unverified` until a fresh probe pass or the newest recorded real use passed. A configured or advertised service is not proof.
- `core` evidence must all hold; other routes are alternatives and one working route is enough. Advisory evidence can only degrade. Expired evidence becomes `unverified`.
- Only the newest recorded real use counts; a newer probe pass supersedes an older failed use.
- A `quick` run (startup, every 5 minutes, page open) sends no provider request and spawns no process. Its only network read is the LARM control-plane catalog (`harness.catalog`), which proves nothing about execution. Probes run only in `full` or `capability` scope, and a `capability` run probes only providers serving that capability (`DiagnosisStore::focus`).
- Evidence of a tier-`probe` pass must have exercised the capability: System TTS availability and the Codex handshake are recorded as `static`. A microphone capture failure is never superseded by a provider probe.
- Blocking database calls inside a check run off the async workers so the deadline can fire.
- Every check has its own deadline. A timeout becomes `unverified` for that check only. A run replaces only the checks it executed; other results stay until they expire.
- Evidence carries no free-form text except a short redacted `detail` (160 chars, no URLs or tokens).
- Voice: `voice.echo` replays synthetic PCM through the sample-based echo handling and requires both "TTS-only playback leaves no utterance" and "overlapping human speech survives". It must never be replaced by a playback-state gate.

IPC: `get_diagnosis_report`, `run_diagnosis(scope)` with scope `quick` / `full` / `capability`; event `diagnosis-updated` after every completed check. Bindings: `src/lib/generated/diagnosis.ts` (`bun run ipc:generate`).

Add a check: write `checks/<name>.rs`, register a `CheckSpec` in `checks::REGISTRY` (id, quick, timeout, ttl, capabilities, timeout route). Every capability except `coding` needs at least one quick check (tested).

Search: `diagnosis::engine::run_and_publish`, `diagnosis::aggregate::capability_report`, `diagnosis::store::DiagnosisStore`, `diagnosis-updated`.
