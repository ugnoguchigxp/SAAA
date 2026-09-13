import { createVoiceAsrChannel } from "./ipcChannels";
import { prepareLarmVoiceSession, failLarmVoiceSession } from "./larmVoiceRuntime";
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
  let owner: Awaited<ReturnType<typeof prepareLarmVoiceSession>> = null;
  try {
    owner = await prepareLarmVoiceSession(input.conversationId);
    const channel = createVoiceAsrChannel(input, onEvent);
    return await invoke("start_voice_asr_session", { input, onEvent: channel });
  } catch (error) {
    if (!["asr-session-exists", "asr-cancelled", "Error: asr-cancelled"].includes(String(error)))
      await failLarmVoiceSession(owner);
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
