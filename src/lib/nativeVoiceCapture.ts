import { Channel, invoke } from "@tauri-apps/api/core";
import type { AudioBackendStatus } from "./audioBackend";

let captureMonitor: ReturnType<typeof setInterval> | null = null;
let captureGeneration = 0;

function clearCaptureMonitor(): number {
  captureGeneration += 1;
  if (captureMonitor !== null) clearInterval(captureMonitor);
  captureMonitor = null;
  return captureGeneration;
}

export async function startNativeVoiceCapture(
  onFrame: (frame: Float32Array) => void,
  onEnded?: (reason: string) => void,
  onStatus?: (status: AudioBackendStatus) => void,
): Promise<AudioBackendStatus> {
  const generation = clearCaptureMonitor();
  const channel = new Channel<unknown>();
  channel.onmessage = (payload) => {
    if (generation !== captureGeneration) return;
    const frame = pcmFrame(payload);
    if (frame) {
      onFrame(frame);
      return;
    }
    const reason = endedReason(payload);
    if (reason && generation === captureGeneration) {
      clearCaptureMonitor();
      onEnded?.(reason);
    }
  };
  const started = await invoke<AudioBackendStatus>("start_native_voice_capture", {
    onFrame: channel,
  });
  if (generation !== captureGeneration) return started;
  onStatus?.(started);
  let checking = false;
  let failures = 0;
  const endCapture = (reason: string) => {
    if (generation !== captureGeneration) return;
    clearCaptureMonitor();
    onEnded?.(reason);
  };
  captureMonitor = setInterval(
    () => {
      if (checking) return;
      checking = true;
      void invoke<AudioBackendStatus>("audio_backend_status")
        .then((status) => {
          failures = 0;
          if (!status.captureActive) endCapture("VoiceProcessing capture stopped unexpectedly");
          else onStatus?.(status);
        })
        .catch(() => {
          if (++failures >= 3) endCapture("VoiceProcessing capture status is unavailable");
        })
        .finally(() => {
          checking = false;
        });
    },
    onStatus ? 100 : 500,
  );
  return started;
}

function pcmFrame(payload: unknown): Float32Array | null {
  if (payload instanceof ArrayBuffer) {
    if (payload.byteLength % 4 !== 0) return null;
    return new Float32Array(payload.slice(0));
  }
  if (ArrayBuffer.isView(payload) && !(payload instanceof DataView)) {
    if (payload.byteLength % 4 !== 0) return null;
    const copy = new ArrayBuffer(payload.byteLength);
    new Uint8Array(copy).set(
      new Uint8Array(payload.buffer, payload.byteOffset, payload.byteLength),
    );
    return new Float32Array(copy);
  }
  if (Array.isArray(payload) && payload.every((sample) => typeof sample === "number")) {
    return Float32Array.from(payload);
  }
  return null;
}

function endedReason(payload: unknown): string | null {
  if (typeof payload === "string") {
    try {
      return endedReason(JSON.parse(payload));
    } catch {
      return null;
    }
  }
  if (
    typeof payload === "object" &&
    payload !== null &&
    "type" in payload &&
    payload.type === "ended" &&
    "reason" in payload &&
    typeof payload.reason === "string"
  ) {
    return payload.reason;
  }
  return null;
}

export async function stopNativeVoiceCapture(): Promise<void> {
  clearCaptureMonitor();
  await invoke("stop_native_voice_capture");
}
