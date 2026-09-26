import { createVoiceAsrChannel } from "./ipcChannels";
import { currentLarmVoice, prepareLarmVoiceSession, failLarmVoiceSession } from "./larmVoiceRuntime";
import { invoke } from "@tauri-apps/api/core";
import type {
  CommitVoiceAsrUtteranceInput,
  StartVoiceAsrSessionInput,
  StopVoiceAsrSessionInput,
  VoiceAsrStreamEvent,
} from "./generated/voiceAsr";

export async function startVoiceAsrSession(
  input: StartVoiceAsrSessionInput,
  onEvent: (event: VoiceAsrStreamEvent) => void,
): Promise<void> {
  try {
    // ASR has an independent Harness route. Prepare the conversation lease in
    // parallel so an unavailable LLM cannot delay microphone capture.
    void prepareLarmVoiceSession(input.conversationId).catch(() => null);
    const channel = createVoiceAsrChannel(input, onEvent);
    return await invoke("start_voice_asr_session", { input, onEvent: channel });
  } catch (error) {
    if (!["asr-session-exists", "asr-cancelled", "Error: asr-cancelled"].includes(String(error))) {
      const owner = currentLarmVoice();
      if (owner?.conversationId === input.conversationId) await failLarmVoiceSession(owner);
    }
    throw error;
  }
}
export function appendVoiceAsrAudio(
  sessionId: string,
  sequence: number,
  bytes: Uint8Array,
): Promise<void> {
  return invoke("append_voice_asr_audio", bytes, {
    headers: {
      "x-saaa-asr-session-id": sessionId,
      "x-saaa-asr-sequence": String(sequence),
      "x-saaa-asr-sample-count": "1600",
    },
  });
}
export function commitVoiceAsrUtterance(input: CommitVoiceAsrUtteranceInput): Promise<void> {
  return invoke("commit_voice_asr_utterance", { input });
}
export function stopVoiceAsrSession(input: StopVoiceAsrSessionInput): Promise<void> {
  return invoke("stop_voice_asr_session", { input });
}
