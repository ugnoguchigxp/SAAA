import { invoke } from "@tauri-apps/api/core";

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

export async function interruptNativeVoicePlayback(): Promise<void> {
  await invoke("interrupt_native_voice_playback");
}

export function nativeCapturePreferred(status: AudioBackendStatus, aecEnabled?: boolean): boolean {
  return Boolean(status?.available) && aecEnabled !== false;
}

export { startNativeVoiceCapture, stopNativeVoiceCapture } from "./nativeVoiceCapture";
