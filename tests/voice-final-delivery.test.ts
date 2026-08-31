import { describe, expect, test } from "bun:test";
import { VoiceFinalDeliveryQueue } from "../src/features/voice/voiceFinalDeliveryQueue";

describe("VoiceFinalDeliveryQueue", () => {
  test("deduplicates and retains capacity until downstream delivery is confirmed", () => {
    const queue = new VoiceFinalDeliveryQueue();
    const value = { sessionId: "s", utteranceId: "u", conversationId: "c", text: "x" };
    expect(queue.push(value)).toBe("accepted");
    expect(queue.push(value)).toBe("duplicate");
    expect(queue.claim("u")).toEqual(value);
    expect(queue.claim("u")).toBeUndefined();
    expect(queue.push({ ...value, utteranceId: "u2" })).toBe("accepted");
    expect(queue.push({ ...value, utteranceId: "u3" })).toBe("full");
    queue.settle("u", true);
    expect(queue.push({ ...value, utteranceId: "u3" })).toBe("accepted");
    expect(queue.push(value)).toBe("duplicate");
  });

  test("a rejected downstream delivery can be claimed again without duplication", () => {
    const queue = new VoiceFinalDeliveryQueue();
    const value = { sessionId: "s", utteranceId: "u", conversationId: "c", text: "x" };
    queue.push(value);
    queue.claim("u");
    queue.settle("u", false);
    expect(queue.claim("u")).toEqual(value);
  });

  test("clear drops pending work and the unmount dedupe set", () => {
    const queue = new VoiceFinalDeliveryQueue();
    const value = { sessionId: "s", utteranceId: "u", conversationId: "c", text: "x" };
    queue.push(value);
    queue.clear();
    expect(queue.push(value)).toBe("accepted");
  });
});
