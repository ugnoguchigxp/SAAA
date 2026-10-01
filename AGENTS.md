# SAAA project instructions

At the start of work in this project, call the `initial_instructions` MCP tool once per conversation. Do not send prompts or messages to another Codex task unless the user explicitly requests it.

Do not reset stored user settings or replace a configured provider to work around a startup failure. Test migrations against a copy or an isolated database.

## Voice conversation contract

- ASR is the continuously available microphone-to-text path. It keeps receiving microphone samples during TTS playback so a person can speak or interrupt an answer. Playback must not stop capture, suppress microphone frames by time window, finalize an utterance, or prevent ASR delivery.
- TTS is SAAA's synthesized output played through the speaker. Its audio is a far-end reference for acoustic echo cancellation (AEC), never a user utterance. A TTS playback state flag alone cannot identify what the microphone heard.
- On macOS, route TTS playback and microphone capture through the same VoiceProcessingIO instance. Keep AEC enabled and configure other-audio ducking explicitly. Use the actual rendered PCM samples to identify and remove residual TTS echo from captured PCM; preserve independent and overlapping human speech.
- When changing voice logic, verify both conditions: TTS-only playback does not create an ASR utterance, and human speech during playback still reaches ASR. Do not replace sample-based echo handling with a blanket playback gate.

## Concept source of truth

The sole canonical product concept is [SAAAの全体コンセプト](https://chatgpt.com/space/page_9fc5877949748191b556705128f6a2f5). Read that Page before changing product concepts, and reflect concept changes there. Do not create or restore duplicate concept documents in this repository. Implementation contracts, plans, and verification records remain in the repository; record and reconcile any conflict with the Page explicitly. Historical source snapshots on the Page are evidence, not the current concept. If the Page is unavailable, state that limitation rather than treating an old snapshot as current.
