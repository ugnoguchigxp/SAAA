import { describe, expect, test } from "bun:test";
import { ConversationIssueCoordinator } from "../src/features/chat/conversationIssueCoordinator";
import { VoiceFrameBuffer } from "../src/features/voice/voiceFrameBuffer";
import { VoiceSegmentQueue, type QueuedVoiceSegment } from "../src/features/voice/voiceSegmentQueue";
import { disposePendingVoiceCapture, type PendingVoiceCapture } from "../src/features/settings/pendingVoiceCapture";
import { LatestRequestGate } from "../src/lib/latestRequestGate";
import { withTimeout } from "../src/lib/promiseTimeout";

function segment(value: number): QueuedVoiceSegment {
  return {
    conversationId: "conversation",
    samples: new Float32Array([value]),
    sampleRate: 16_000,
    ttsActiveAtCapture: false,
  };
}

describe("voice buffering and async issue ownership", () => {
  test("bounds and clears queued PCM without retaining rejected audio", () => {
    const queue = new VoiceSegmentQueue(2);
    const rejected = segment(3);
    expect(queue.push(segment(1))).toBe(true);
    expect(queue.push(segment(2))).toBe(true);
    expect(queue.push(rejected)).toBe(false);
    expect(rejected.samples).toHaveLength(0);
    expect(queue.shift()?.samples[0]).toBe(1);
    queue.clear();
    expect(queue.length).toBe(0);
  });

  test("preserves frame order and trims only complete leading frames", () => {
    const buffer = new VoiceFrameBuffer();
    buffer.append(new Float32Array([1, 2]));
    buffer.append(new Float32Array([3, 4]));
    buffer.append(new Float32Array([5]));
    buffer.trimStartTo(3);
    expect([...buffer.take()]).toEqual([3, 4, 5]);
    expect(buffer.sampleCount).toBe(0);
  });

  test("keeps pre-roll separate from the complete detected utterance", () => {
    const preRoll = new VoiceFrameBuffer();
    const utterance = new VoiceFrameBuffer();
    preRoll.append(new Float32Array([0, 1]));
    preRoll.append(new Float32Array([2, 3]));
    preRoll.trimStartTo(3);
    utterance.append(preRoll.take());
    utterance.append(new Float32Array([4, 5]));
    expect([...utterance.take()]).toEqual([2, 3, 4, 5]);
    expect(preRoll.sampleCount).toBe(0);
  });

  test("invalidates late async failures while remaining reusable after strict-effect cleanup", () => {
    const coordinator = new ConversationIssueCoordinator();
    const stale = coordinator.begin();
    const current = coordinator.begin();
    expect(coordinator.isCurrent(stale)).toBe(false);
    expect(coordinator.isCurrent(current)).toBe(true);
    coordinator.dispose();
    expect(coordinator.isCurrent(current)).toBe(false);
    expect(coordinator.isCurrent(coordinator.begin())).toBe(true);
  });

  test("rejects stale ASR resolutions after edits and unmount", () => {
    const gate = new LatestRequestGate();
    const oldHost = gate.begin();
    const newHost = gate.begin();
    expect(gate.isCurrent(oldHost)).toBe(false);
    expect(gate.isCurrent(newHost)).toBe(true);
    gate.dispose();
    expect(gate.isCurrent(newHost)).toBe(false);
    gate.activate();
    expect(gate.isCurrent(gate.begin())).toBe(true);
  });

  test("disposes a microphone stream that arrives after startup timeout", async () => {
    let resolve!: (value: { close: () => void }) => void;
    let closed = false;
    const pending = new Promise<{ close: () => void }>((accept) => { resolve = accept; });
    await expect(withTimeout(pending, 1, "timed out", (value) => value.close())).rejects.toThrow("timed out");
    resolve({ close: () => { closed = true; } });
    await Promise.resolve();
    expect(closed).toBe(true);
  });

  test("disposes pending microphone ownership idempotently across unmount races", async () => {
    let stops = 0;
    let closes = 0;
    let releases = 0;
    const pending: PendingVoiceCapture = {
      stream: { getTracks: () => [{ stop: () => { stops += 1; } }] } as unknown as MediaStream,
      context: { close: async () => { closes += 1; } } as unknown as AudioContext,
      releaseLease: () => { releases += 1; },
    };

    await disposePendingVoiceCapture(pending);
    await disposePendingVoiceCapture(pending);

    expect({ stops, closes, releases }).toEqual({ stops: 1, closes: 1, releases: 1 });
  });
});
