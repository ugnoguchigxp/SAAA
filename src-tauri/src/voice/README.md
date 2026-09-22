# Voice

Owner: audio capture transport, ASR sessions, speaker gating, TTS synthesis/playback. Conversation reasoning belongs to runtime, not ASR or LFM.

Preserve:
- One accepted ASR final utterance enters one ordinary user turn with its utteranceId. Do not concatenate, summarize, or duplicate inputs.
- Partial transcript, committed final, rejected speaker, and stopped session are different states.
- Preserve session generation, packet sequencing, bounded queues, cancellation, and late-event rejection.
- Final answer identity must survive TTS delivery. Sentence chunks are not separate answers; duplicate completion must not replay the answer.
- ASR/TTS Harness sessions do not imply a Harness-only reasoning route. Inspect effective routes independently.

Locate:
- Frontend final delivery: `../../../src/features/voice/useAmbientVoiceSession.ts`; capture actions: `../../../src/features/voice/ambientVoiceCaptureActions.ts`.
- Input FIFO/turn ownership: `../../../src/features/chat/useConversationTurn.ts` -> `../runtime/start_turn.rs`.
- ASR IPC/types: `streaming_asr/commands.rs`, `streaming_asr/contracts.rs`; session/sequence: `streaming_asr/manager.rs`, `streaming_asr/session.rs` and its `.d/`.
- Transcript reconciliation: `streaming_asr/reconciler.rs`; speaker decisions: `streaming_asr/speaker_gate.rs`, `profile/`.
- Effective ASR/TTS route: `session/asr_routes.rs`, `session/tts.rs`, `session/tts_policy.rs`; wire transport: `http_audio/`, `network_asr/`.
- Speech ownership/terminal: `streaming_tts/runtime.rs` and `.d/`; sentence boundaries: `streaming_tts/chunker.rs`; fallback: `streaming_tts/fallback.rs`.
- Policy/event fan-out: `../voice_behavior/`, `../runtime/event_hub.rs`; durable final intent: `../role_routing/speech_repository.rs`.
- Tests: `streaming_asr/regression_corpus.rs`, chunker/runtime tests, `../../tests/voice_asr_contract_bindings.rs`.

E2E: utteranceId -> runId -> server response -> messageId -> speech delivery. Check greeting, information request, two successive utterances, and cancellation; synthesis success is not playback completion.
