import { acquireAudioCapture } from "./audioCaptureCoordinator";
import { resamplePcm } from "./audioResampling";
import {
  disposeMicrophoneCapture,
  ensureMicrophoneAudioContextRunning,
  microphoneCaptureConstraints,
  requestMicrophoneStream,
} from "./microphone";
import { withTimeout } from "./promiseTimeout";

const PROCESSOR_URL = "/audio/voice-capture-processor.js";

export type BrowserVoiceCapture = { stop: () => Promise<void> };

export async function startBrowserVoiceCapture(
  onFrame: (frame: Float32Array) => void,
  onEnded: (reason: string) => void,
  inputDeviceId: string,
  echoCancellation: boolean,
): Promise<BrowserVoiceCapture> {
  const releaseLease = acquireAudioCapture("chat");
  let stream: MediaStream | null = null;
  let context: AudioContext | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let node: AudioWorkletNode | null = null;
  let active = false;
  let flushResolver: (() => void) | null = null;
  let track: MediaStreamTrack | null = null;
  const ended = () => onEnded("マイクの接続が切れました。");
  const cleanup = async () => {
    active = false;
    flushResolver = null;
    track?.removeEventListener("ended", ended);
    try { node?.disconnect(); } catch { /* Node may not have connected yet. */ }
    try { source?.disconnect(); } catch { /* Source may not have connected yet. */ }
    try {
      await disposeMicrophoneCapture(stream, context);
    } finally {
      releaseLease();
    }
  };
  try {
    stream = await requestMicrophoneStream(
      microphoneCaptureConstraints(inputDeviceId, echoCancellation),
    );
    context = new AudioContext();
    await context.audioWorklet.addModule(PROCESSOR_URL);
    source = context.createMediaStreamSource(stream);
    node = new AudioWorkletNode(context, "voice-capture-processor");
    const captureContext = context;
    node.port.onmessage = (event: MessageEvent<Float32Array | { type: "flushed" }>) => {
      if (event.data instanceof Float32Array) {
        try {
          if (active) onFrame(resamplePcm(event.data, captureContext.sampleRate, 16_000));
        } finally {
          event.data.fill(0);
        }
      } else if (event.data?.type === "flushed") {
        flushResolver?.();
      }
    };
    track = stream.getAudioTracks()[0] ?? null;
    track?.addEventListener("ended", ended);
    active = true;
    source.connect(node);
    node.connect(context.destination);
    await ensureMicrophoneAudioContextRunning(context);
    return {
      stop: async () => {
        if (!active) return;
        try {
          if (node) {
            await withTimeout(
              new Promise<void>((resolve) => {
                flushResolver = resolve;
                node?.port.postMessage({ type: "flush" });
              }),
              2_000,
              "録音の最後の音声を取得できませんでした。",
            );
          }
        } finally {
          await cleanup();
        }
      },
    };
  } catch (cause) {
    await cleanup();
    throw cause;
  }
}
