import { encodePcm16 } from "./audioIpc";

const SAMPLE_RATE = 16_000;

export function recordedAudioPreview(samples: Float32Array): {
  url: string;
  seconds: number;
  peaks: number[];
} {
  const pcm = encodePcm16(samples);
  const header = new ArrayBuffer(44);
  const view = new DataView(header);
  const ascii = (offset: number, value: string) => {
    for (let i = 0; i < value.length; i += 1) view.setUint8(offset + i, value.charCodeAt(i));
  };
  ascii(0, "RIFF");
  view.setUint32(4, 36 + pcm.byteLength, true);
  ascii(8, "WAVE");
  ascii(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, SAMPLE_RATE, true);
  view.setUint32(28, SAMPLE_RATE * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  ascii(36, "data");
  view.setUint32(40, pcm.byteLength, true);
  const peaks = Array.from({ length: 64 }, (_, index) => {
    const start = Math.floor((samples.length * index) / 64);
    const end = Math.floor((samples.length * (index + 1)) / 64);
    let peak = 0;
    for (let i = start; i < end; i += 1) peak = Math.max(peak, Math.abs(samples[i] ?? 0));
    return peak;
  });
  const url = URL.createObjectURL(new Blob([header, pcm], { type: "audio/wav" }));
  pcm.fill(0);
  return { url, seconds: samples.length / SAMPLE_RATE, peaks };
}
