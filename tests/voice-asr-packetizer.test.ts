import { describe, expect, test } from "bun:test";
import { AUDIO_PACKET_BYTES, VoiceAsrPacketizer } from "../src/features/voice/voiceAsrPacketizer";
describe("VoiceAsrPacketizer", () => {
  test("carries frame remainders and emits exact 100ms PCM packets", () => {
    const packetizer = new VoiceAsrPacketizer();
    expect(packetizer.append(new Float32Array(800).fill(0.25))).toEqual([]);
    const packets = packetizer.append(new Float32Array(800).fill(-0.25));
    expect(packets).toHaveLength(1); expect(packets[0]?.byteLength).toBe(AUDIO_PACKET_BYTES);
  });
  test("pads only when stopping", () => { const packetizer = new VoiceAsrPacketizer(); packetizer.append(new Float32Array(1)); expect(packetizer.flushPadded()?.byteLength).toBe(AUDIO_PACKET_BYTES); expect(packetizer.flushPadded()).toBeNull(); });
});
