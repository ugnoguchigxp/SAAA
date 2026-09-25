import { Channel, invoke } from "@tauri-apps/api/core";

export type AudioBackendStatus = {
  available: boolean;
  reason: string | null;
  captureActive: boolean;
  playbackActive: boolean;
  aecActive: boolean;
  duckingLevel: string;
  agcEnabled: boolean;
  outputTransport: string | null;
  macosMajor: number;
};

export async function audioBackendStatus(): Promise<AudioBackendStatus> {
  return invoke("audio_backend_status");
}

export async function startNativeVoiceCapture(
  onFrame: (frame: Float32Array) => void,
): Promise<AudioBackendStatus> {
  const channel = new Channel<unknown>();
  channel.onmessage = (payload) => {
    if (!Array.isArray(payload)) return;
    onFrame(Float32Array.from(payload as number[]));
  };
  return invoke("start_native_voice_capture", { onFrame: channel });
}

export async function stopNativeVoiceCapture(): Promise<void> {
  await invoke("stop_native_voice_capture");
}

export async function interruptNativeVoicePlayback(): Promise<void> {
  await invoke("interrupt_native_voice_playback");
}

export function nativeCapturePreferred(
  status: AudioBackendStatus,
  aecEnabled?: boolean,
): boolean {
  return Boolean(status?.available) && aecEnabled !== false;
}
