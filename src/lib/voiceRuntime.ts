import { Channel, invoke } from "@tauri-apps/api/core";
import type { VoiceEvent } from "./contracts";
import { stageAudioUpload, type AudioUploadPurpose } from "./audioIpc";

type AudioInput = { runId: string; conversationId?: string; samples: Float32Array; sampleRate: number };

export function transcribeAudio(
  input: AudioInput & { conversationId: string },
  onEvent: (event: VoiceEvent) => void,
): Promise<string> {
  return transcribeStagedAudio("transcribe_audio", "chat-asr", input, onEvent);
}

export function transcribeAudioChunk(
  input: AudioInput,
  onEvent: (event: VoiceEvent) => void,
): Promise<string> {
  return transcribeStagedAudio("transcribe_audio_chunk", "chat-asr-chunk", input, onEvent);
}

async function transcribeStagedAudio(
  command: "transcribe_audio" | "transcribe_audio_chunk",
  purpose: AudioUploadPurpose,
  input: AudioInput,
  onEvent: (event: VoiceEvent) => void,
): Promise<string> {
  const channel = new Channel<VoiceEvent>();
  channel.onmessage = onEvent;
  const { samples, ...metadata } = input;
  const audioUploadId = await stageAudioUpload(samples, purpose).finally(() => samples.fill(0));
  return invoke<string>(command, { input: { ...metadata, audioUploadId }, onEvent: channel });
}
