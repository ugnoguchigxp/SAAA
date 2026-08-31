import type { VoiceActivityObservation } from "../../lib/voiceActivity";
import type { CommitReason } from "./voiceAsrPacketSender";

const MAX_VOICE_SEGMENT_PACKETS = 30 * 10;

export function voiceSegmentCommitReason(
  observation: VoiceActivityObservation | undefined,
  packetCount: number,
): CommitReason | null {
  if (packetCount >= MAX_VOICE_SEGMENT_PACKETS) return "max-duration";
  return observation?.hasSpeech && observation.shouldFinalize ? "silence" : null;
}
