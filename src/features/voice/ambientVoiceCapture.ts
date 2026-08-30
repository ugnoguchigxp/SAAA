import type { MutableRefObject } from "react";
import { isMeetingBlocking } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
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
import type { VoiceFrameBuffer } from "./voiceFrameBuffer";

const MAX_VOICE_SEGMENT_SECONDS = 30;

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
  frames: MutableRefObject<VoiceFrameBuffer>;
  preRollFrames: MutableRefObject<VoiceFrameBuffer>;
  flushResolver: MutableRefObject<(() => void) | null>;
  activityDetector: MutableRefObject<VoiceActivityDetector | null>;
  captureLease: MutableRefObject<(() => void) | null>;
  applyEvent: (event: VoiceSessionEvent) => unknown;
  finishSegment: () => void;
  transcribeFrame: (frame: Float32Array, sampleRate: number) => void;
  clearTranscript: () => void;
  setError: (message: string) => void;
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
    const ownsBufferedAudio = (node !== null && context.node.current === node)
      || (audioContext !== null && context.audioContext.current === audioContext);
    if (node) node.port.onmessage = null;
    node?.disconnect();
    source?.disconnect();
    await disposeMicrophoneCapture(stream, audioContext);
    if (ownsBufferedAudio) {
      context.frames.current.clear();
      context.preRollFrames.current.clear();
    }
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
    audioContext = new AudioContext();
    const activeContext = audioContext;
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
    context.frames.current.clear();
    context.preRollFrames.current.clear();
    activityDetector = detector(context.settings, activeContext.sampleRate);
    context.activityDetector.current = activityDetector;
    node.port.onmessage = (event: MessageEvent<Float32Array | { type: "flushed" }>) => {
      if (context.node.current !== node) return;
      if (!(event.data instanceof Float32Array)) {
        if (event.data.type === "flushed") context.flushResolver.current?.();
        return;
      }
      context.transcribeFrame(event.data, activeContext.sampleRate);
      const observation = context.activityDetector.current?.observe(event.data);
      if (!observation?.hasSpeech) {
        if (!context.activityDetector.current && context.frames.current.sampleCount > 0) {
          context.frames.current.append(event.data);
          return;
        }
        context.preRollFrames.current.append(event.data);
        context.preRollFrames.current.trimStartTo(Math.round(activeContext.sampleRate * 0.5));
        return;
      }
      if (context.frames.current.sampleCount === 0 && context.preRollFrames.current.sampleCount > 0) {
        context.frames.current.append(context.preRollFrames.current.take());
      }
      context.frames.current.append(event.data);
      if (observation.shouldFinalize || context.frames.current.sampleCount >= activeContext.sampleRate * MAX_VOICE_SEGMENT_SECONDS) {
        context.finishSegment();
        return;
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
    context.setError(cause instanceof MicrophoneCaptureError
      ? cause.message
      : uiMessage("chatVoiceCaptureInitializationFailed"));
    context.applyEvent({ type: "captureDetached" });
  }
}

export function resetVoiceActivityDetector(
  target: MutableRefObject<VoiceActivityDetector | null>,
  settings: VoiceSettings,
  sampleRate: number,
): void {
  target.current = detector(settings, sampleRate);
}
