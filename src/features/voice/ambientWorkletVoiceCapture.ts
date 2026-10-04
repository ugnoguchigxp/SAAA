import { considerBargeIn } from "./ambientBargeIn";
import type { MutableRefObject } from "react";
import type { VoiceActivityDetector, VoiceActivityObservation } from "../../lib/voiceActivity";
import type { CommitReason } from "../../lib/generated/voiceAsr";
import { voiceSegmentCommitReason } from "./voiceSegmentBoundary";

export function observeCaptureFrame(input: {
  frame: Float32Array;
  activityDetector: MutableRefObject<VoiceActivityDetector | null>;
  packetFrame: (frame: Float32Array) => void;
  packetCount: () => number;
  finishSegment: (reason: CommitReason) => void;
  onActivity?: (observation: VoiceActivityObservation) => void;
  bargeInEnabled?: boolean;
  speechIsPlaying?: () => boolean;
  ttsStartedAtMs?: () => number;
  interruptSpeech?: () => void;
  speechRunId?: () => string | null;
  bargeInSpeechSince?: MutableRefObject<number>;
  bargeInFiredFor?: MutableRefObject<string | null>;
}): void {
  input.packetFrame(input.frame);
  const observation = input.activityDetector.current?.observe(input.frame);
  if (observation) {
    input.onActivity?.(observation);
    considerBargeIn(input, observation.hasSpeech);
  }
  const reason = voiceSegmentCommitReason(observation, input.packetCount());
  if (reason) input.finishSegment(reason);
}

export function bindWorkletFrameHandler(
  input: Omit<Parameters<typeof observeCaptureFrame>[0], "frame"> & {
    node: AudioWorkletNode;
    currentNode: MutableRefObject<AudioWorkletNode | null>;
    flushResolver: MutableRefObject<(() => void) | null>;
  },
): void {
  input.node.port.onmessage = (event: MessageEvent<Float32Array | { type: "flushed" }>) => {
    const node = input.node;
    const context = { node: input.currentNode };
    if (context.node.current !== node) return;
    if (!(event.data instanceof Float32Array)) {
      if (event.data.type === "flushed") input.flushResolver.current?.();
      return;
    }
    try {
      observeCaptureFrame({ ...input, frame: event.data });
    } finally {
      event.data.fill(0);
    }
  };
}
