import { invoke } from "@tauri-apps/api/core";

export type AudioUploadPurpose = "chat-asr" | "meeting-segment" | "voice-enrollment";
const MAX_AUDIO_SAMPLES = 16_000 * 120;

export function encodePcm16(samples: Float32Array): Uint8Array {
  if (samples.length > MAX_AUDIO_SAMPLES) throw new Error("Recorded audio exceeds the two minute limit.");
  if (samples.some((sample) => !Number.isFinite(sample))) throw new Error("Recorded audio contains invalid samples.");
  const bytes = new Uint8Array(samples.length * 2);
  const view = new DataView(bytes.buffer);
  for (let index = 0; index < samples.length; index += 1) {
    const sample = Math.max(-1, Math.min(1, samples[index] ?? 0));
    view.setInt16(index * 2, Math.round(sample * 32_767), true);
  }
  return bytes;
}

export async function stageAudioUpload(samples: Float32Array, purpose: AudioUploadPurpose): Promise<string> {
  if (!samples.length) throw new Error("Recorded audio is empty.");
  const bytes = encodePcm16(samples);
  try {
    return await invoke<string>("stage_audio_upload", bytes, {
      headers: { "x-saaa-audio-purpose": purpose },
    });
  } finally {
    bytes.fill(0);
  }
}
