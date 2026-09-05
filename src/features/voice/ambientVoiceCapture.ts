import type { MutableRefObject } from "react";
import { isMeetingBlocking } from "../../lib/appHelpers";
import { acquireAudioCapture } from "../../lib/audioCaptureCoordinator";
import type { MeetingState, VoiceSettings } from "../../lib/contracts";
import {
  disposeMicrophoneCapture,
  ensureMicrophoneAudioContextRunning,
  microphoneCaptureConstraints,
  MicrophoneCaptureError,
  requestMicrophoneStream,
} from "../../lib/microphone";
import { VoiceActivityDetector } from "../../lib/voiceActivity";
import type { VoiceSessionEvent } from "../../lib/voiceSession";
import type { CommitReason } from "../../lib/generated/voiceAsr";
import { voiceSegmentCommitReason } from "./voiceSegmentBoundary";

function detector(settings: VoiceSettings, sampleRate: number): VoiceActivityDetector {
  const speechThresholdRms = settings.vadSensitivity === "high" ? 0.006 : settings.vadSensitivity === "low" ? 0.012 : 0.008;
  return new VoiceActivityDetector({ sampleRate, speechThresholdRms, silenceTimeoutMs: settings.silenceTimeoutMs });
}

export async function attachAmbientVoiceCapture(context: {
  settings: VoiceSettings;
  disposed: MutableRefObject<boolean>;
  listeningEnabled: MutableRefObject<boolean>;
  meetingState: MutableRefObject<MeetingState>;
  captureAttempt: MutableRefObject<number>;
  stream: MutableRefObject<MediaStream | null>;
  audioContext: MutableRefObject<AudioContext | null>;
  source: MutableRefObject<MediaStreamAudioSourceNode | null>;
  node: MutableRefObject<AudioWorkletNode | null>;
  flushResolver: MutableRefObject<(() => void) | null>;
  activityDetector: MutableRefObject<VoiceActivityDetector | null>;
  captureLease: MutableRefObject<(() => void) | null>;
  applyEvent: (event: VoiceSessionEvent) => unknown;
  finishSegment: (reason: CommitReason) => void;
  packetFrame: (frame: Float32Array) => void;
  packetCount: () => number;
  clearTranscript: () => void;
}): Promise<void> {
  if (context.disposed.current || context.stream.current || context.captureLease.current) return;
  if (!context.listeningEnabled.current || isMeetingBlocking(context.meetingState.current)) return;
  const captureAttempt = ++context.captureAttempt.current;
  let stream: MediaStream | null = null;
  let audioContext: AudioContext | null = null;
  let source: MediaStreamAudioSourceNode | null = null;
  let node: AudioWorkletNode | null = null;
  let activityDetector: VoiceActivityDetector | null = null;
  let releaseCapture: (() => void) | null = null;
  const stale = () => context.disposed.current
    || context.captureAttempt.current !== captureAttempt
    || !context.listeningEnabled.current
    || isMeetingBlocking(context.meetingState.current);
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
    if (context.activityDetector.current === activityDetector) context.activityDetector.current = null;
  };
  const disposeOwnedCapture = async () => {
    if (node) node.port.onmessage = null;
    node?.disconnect();
    source?.disconnect();
    await disposeMicrophoneCapture(stream, audioContext);
    clearOwnedReferences();
    releaseOwnedCapture();
  };
  try {
    context.applyEvent({ type: "captureStarting" });
    releaseCapture = acquireAudioCapture("chat");
    context.captureLease.current = releaseCapture;
    const audio = microphoneCaptureConstraints(context.settings.inputDeviceId);
    stream = await requestMicrophoneStream(audio);
    if (stale()) {
      await disposeOwnedCapture();
      return;
    }
    context.stream.current = stream;
    // The stream is registered before constructing the AudioContext.
    audioContext = new AudioContext({ sampleRate: 16_000 });
    const activeContext = audioContext;
    if (activeContext.sampleRate !== 16_000) throw new MicrophoneCaptureError("startup-interrupted", "Streaming transcription requires a 16 kHz audio context.");
    context.audioContext.current = activeContext;
    await activeContext.audioWorklet.addModule("/audio/meeting-processor.js");
    if (stale()) {
      await disposeOwnedCapture();
      return;
    }
    source = activeContext.createMediaStreamSource(stream);
    node = new AudioWorkletNode(activeContext, "meeting-processor");
    context.source.current = source;
    context.node.current = node;
    activityDetector = detector(context.settings, activeContext.sampleRate);
    context.activityDetector.current = activityDetector;
    node.port.onmessage = (event: MessageEvent<Float32Array | { type: "flushed" }>) => {
      if (context.node.current !== node) return;
      if (!(event.data instanceof Float32Array)) {
        if (event.data.type === "flushed") context.flushResolver.current?.();
        return;
      }
      try {
        // ASR receives every frame before VAD; VAD only decides commit boundaries.
        context.packetFrame(event.data);
        const observation = context.activityDetector.current?.observe(event.data);
        const reason = voiceSegmentCommitReason(observation, context.packetCount());
        if (reason) context.finishSegment(reason);
      } finally {
        event.data.fill(0);
      }
    };
    source.connect(node);
    node.connect(activeContext.destination);
    await ensureMicrophoneAudioContextRunning(activeContext);
    if (stale()) {
      await disposeOwnedCapture();
      return;
    }
    context.clearTranscript();
    context.applyEvent({ type: "captureStarted" });
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
