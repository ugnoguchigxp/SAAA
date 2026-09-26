import { expect, test } from "bun:test";
import { recordedAudioPreview } from "../src/lib/audioPreview";

test("recording preview keeps the complete PCM recording in a playable WAV blob", async () => {
  const original = URL.createObjectURL;
  let captured: Blob | null = null;
  URL.createObjectURL = (blob) => {
    captured = blob;
    return "blob:recording-test";
  };
  try {
    const samples = new Float32Array([0, 0.5, -0.5, 1]);
    const preview = recordedAudioPreview(samples);
    expect(preview.url).toBe("blob:recording-test");
    expect(preview.seconds).toBe(4 / 16_000);
    expect(Math.max(...preview.peaks)).toBe(1);
    expect(samples[3]).toBe(1);
    const bytes = new Uint8Array(await captured!.arrayBuffer());
    const view = new DataView(bytes.buffer);
    expect(new TextDecoder().decode(bytes.slice(0, 4))).toBe("RIFF");
    expect(new TextDecoder().decode(bytes.slice(8, 12))).toBe("WAVE");
    expect(view.getUint32(24, true)).toBe(16_000);
    expect(view.getUint32(40, true)).toBe(samples.length * 2);
    expect(view.getInt16(44 + 3 * 2, true)).toBe(32_767);
  } finally {
    URL.createObjectURL = original;
  }
});
