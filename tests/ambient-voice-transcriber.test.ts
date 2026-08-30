import { describe, expect, test } from "bun:test";
import { AmbientVoiceTranscriber } from "../src/features/voice/ambientVoiceTranscriber";
import type { VoiceEvent } from "../src/lib/contracts";

describe("ambient voice streaming transcription", () => {
  test("sends each captured sample once in ordered chunks", async () => {
    const requests: Array<{ samples: Float32Array; finish: (text: string) => void }> = [];
    const transcripts: string[] = [];
    const transcriber = new AmbientVoiceTranscriber((input, onEvent) => {
      onEvent({ type: "transcribing", runId: input.runId });
      return new Promise<string>((resolve) => {
        requests.push({ samples: input.samples.slice(), finish: resolve });
      });
    }, async () => undefined);
    transcriber.advanceSegment();

    transcriber.append(new Float32Array(8_000).fill(0.1), 16_000, (text) => transcripts.push(text), () => undefined);
    expect(requests).toHaveLength(0);
    transcriber.append(new Float32Array(8_000).fill(0.2), 16_000, (text) => transcripts.push(text), () => undefined);
    expect(requests).toHaveLength(1);
    transcriber.append(new Float32Array(32_000).fill(0.3), 16_000, (text) => transcripts.push(text), () => undefined);
    expect(requests).toHaveLength(1);

    requests[0]?.finish("first");
    await settled();
    expect(requests).toHaveLength(2);
    requests[1]?.finish("second");
    await settled();

    expect(transcripts).toEqual(["first", "first second"]);
    expect(requests.map(({ samples }) => samples.length)).toEqual([16_000, 32_000]);
    expect(Array.from(requests[0]!.samples.slice(0, 8_000)).every((sample) => Math.abs(sample - 0.1) < 0.0001)).toBe(true);
    expect(Array.from(requests[0]!.samples.slice(8_000)).every((sample) => Math.abs(sample - 0.2) < 0.0001)).toBe(true);
    expect(requests[1]!.samples.every((sample) => Math.abs(sample - 0.3) < 0.0001)).toBe(true);
  });

  test("invalidates an old segment and cancels only when capture is reset", async () => {
    let finishChunk!: (text: string) => void;
    let onEvent!: (event: VoiceEvent) => void;
    let runId = "";
    const cancelled: string[] = [];
    const transcripts: string[] = [];
    const transcriber = new AmbientVoiceTranscriber((input, receiveEvent) => {
      runId = input.runId;
      onEvent = receiveEvent;
      return new Promise<string>((resolve) => { finishChunk = resolve; });
    }, async (id) => { cancelled.push(id); });
    transcriber.advanceSegment();
    transcriber.append(new Float32Array(16_000), 16_000, (text) => transcripts.push(text), () => undefined);

    transcriber.advanceSegment();
    finishChunk("stale text");
    await settled();
    expect(transcripts).toEqual([]);
    expect(cancelled).toEqual([]);

    transcriber.append(new Float32Array(16_000), 16_000, (text) => transcripts.push(text), () => undefined);
    onEvent({ type: "transcribing", runId });
    await transcriber.cancelCurrent();
    expect(cancelled).toEqual([runId]);
  });
});

async function settled(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}
