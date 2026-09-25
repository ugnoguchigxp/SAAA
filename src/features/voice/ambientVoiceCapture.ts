import type { MutableRefObject } from "react";
import { acquireAudioCapture } from "../../lib/audioCaptureCoordinator";
import type { VoiceSettings } from "../../lib/contracts";
import {
  disposeMicrophoneCapture,
  ensureMicrophoneAudioContextRunning,
  microphoneCaptureConstraints,
  MicrophoneCaptureError,
  requestMicrophoneStream,
} from "../../lib/microphone";
import { VoiceActivityDetector } from "../../lib/voiceActivity";
import { stopNativeVoiceCapture } from "../../lib/audioBackend";
import { tryStartNativeVoiceCapture } from "./ambientNativeVoiceCapture";
import type { AmbientVoiceCaptureContext } from "./ambientVoiceCaptureContext";
import { bindWorkletFrameHandler, observeCaptureFrame } from "./ambientWorkletVoiceCapture";

function detector(settings: VoiceSettings, sampleRate: number): VoiceActivityDetector {
  const speechThresholdRms =
    settings.vadSensitivity === "high" ? 0.006 : settings.vadSensitivity === "low" ? 0.012 : 0.008;
  return new VoiceActivityDetector({
    sampleRate,
    speechThresholdRms,
    silenceTimeoutMs: settings.silenceTimeoutMs,
  });
}

export async function attachAmbientVoiceCapture(
  context: AmbientVoiceCaptureContext,
): Promise<void> {
  if (context.disposed.current || context.stream.current || context.captureLease.current) return;
  if (context.nativeCapture?.current) return;
  if (!context.listeningEnabled.current) return;
  const captureAttempt = ++context.captureAttempt.current;
  let stream: MediaStream | null = null;
  let audioContext: AudioContext | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let node: AudioWorkletNode | null = null;
  let activityDetector: VoiceActivityDetector | null = null;
  let releaseCapture: (() => void) | null = null;
  const stale = () =>
    context.disposed.current ||
    context.captureAttempt.current !== captureAttempt ||
    !context.listeningEnabled.current;
  const releaseOwnedCapture = () => {
    const release = releaseCapture;
    if (!release) return;
    releaseCapture = null;
    release();
    if (context.captureLease.current === release) context.captureLease.current = null;
  };
  const clearOwnedReferences = () => {
    if (context.stream.current === stream) context.stream.current = null;
    if (context.audioContext.current === audioContext) context.audioContext.current = null;
    if (context.source.current === source) context.source.current = null;
    if (context.node.current === node) context.node.current = null;
    if (context.activityDetector.current === activityDetector)
      context.activityDetector.current = null;
  };
  const disposeOwnedCapture = async () => {
    if (node) node.port.onmessage = null;
    node?.disconnect();
    source?.disconnect();
    await disposeMicrophoneCapture(stream, audioContext);
    clearOwnedReferences();
    releaseOwnedCapture();
  };
  const handleFrame = (frame: Float32Array) => observeCaptureFrame({ ...context, frame });
  const startWorklet = async () => {
    // Native VoiceProcessing is unavailable here. Leave WebKit echo cancellation off so
    // its internal VPIO does not duck other apps.
    const audio = microphoneCaptureConstraints(context.settings.inputDeviceId, false);
    stream = await requestMicrophoneStream(audio);
    if (stale()) {
      await disposeOwnedCapture();
      return;
    }
    context.stream.current = stream;
    // The stream is registered before constructing the AudioContext.
    audioContext = new AudioContext({ sampleRate: 16_000 });
    const activeContext = audioContext;
    if (activeContext.sampleRate !== 16_000)
      throw new MicrophoneCaptureError(
        "startup-interrupted",
        "Streaming transcription requires a 16 kHz audio context.",
      );
    context.audioContext.current = activeContext;
    await activeContext.audioWorklet.addModule("/audio/voice-capture-processor.js");
    if (stale()) {
      await disposeOwnedCapture();
      return;
    }
    source = activeContext.createMediaStreamSource(stream);
    node = new AudioWorkletNode(activeContext, "voice-capture-processor");
    context.source.current = source;
    context.node.current = node;
    activityDetector = detector(context.settings, activeContext.sampleRate);
    context.activityDetector.current = activityDetector;
    bindWorkletFrameHandler({
      ...context,
      node,
      currentNode: context.node,
      flushResolver: context.flushResolver,
    });
    source.connect(node);
    node.connect(activeContext.destination);
    await ensureMicrophoneAudioContextRunning(activeContext);
    if (stale()) {
      await disposeOwnedCapture();
      return;
    }
    context.clearTranscript();
    context.applyEvent({ type: "captureStarted" });
  };
  try {
    context.applyEvent({ type: "captureStarting" });
    releaseCapture = acquireAudioCapture("chat");
    context.captureLease.current = releaseCapture;
    const nativeStarted = await tryStartNativeVoiceCapture({
      settings: context.settings,
      nativeCapture: context.nativeCapture,
      nativeStop: context.nativeStop,
      activityDetector: context.activityDetector,
      createDetector: (sampleRate) => detector(context.settings, sampleRate),
      stale,
      handleFrame,
      disposeOwnedCapture,
      applyEvent: context.applyEvent,
      clearTranscript: context.clearTranscript,
      onEnded: () => {
        void (async () => {
          if (stale() || !context.nativeCapture?.current) return;
          context.nativeCapture.current = false;
          if (context.nativeStop) context.nativeStop.current = null;
          await stopNativeVoiceCapture().catch(() => undefined);
          if (stale()) {
            await disposeOwnedCapture();
            return;
          }
          try {
            await startWorklet();
          } catch (cause) {
            await disposeOwnedCapture();
            context.onNativeEnded?.(cause instanceof Error ? cause.message : String(cause));
          }
        })();
      },
    });
    if (nativeStarted) return;
    await startWorklet();
  } catch (cause) {
    if (stale()) {
      await disposeOwnedCapture();
      return;
    }
    await disposeOwnedCapture();
    if (context.disposed.current) return;
    throw cause;
  }
}

export function resetVoiceActivityDetector(
  target: MutableRefObject<VoiceActivityDetector | null>,
  settings: VoiceSettings,
  sampleRate: number,
): void {
  target.current = detector(settings, sampleRate);
}
