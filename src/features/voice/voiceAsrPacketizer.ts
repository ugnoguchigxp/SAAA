export const ASR_SAMPLE_RATE = 16_000;
export const AUDIO_PACKET_SAMPLES = 1_600;
export const AUDIO_PACKET_BYTES = 3_200;

/** Converts Worklet Float32 frames to exact 100 ms PCM16LE packets. */
export class VoiceAsrPacketizer {
  private carry = new Float32Array();

  append(frame: Float32Array): Uint8Array[] {
    if (!frame.length) return [];
    const merged = new Float32Array(this.carry.length + frame.length);
    merged.set(this.carry);
    merged.set(frame, this.carry.length);
    const packets: Uint8Array[] = [];
    let offset = 0;
    while (merged.length - offset >= AUDIO_PACKET_SAMPLES) {
      packets.push(encodePcm16(merged.subarray(offset, offset + AUDIO_PACKET_SAMPLES)));
      offset += AUDIO_PACKET_SAMPLES;
    }
    this.carry.fill(0);
    this.carry = merged.slice(offset);
    merged.fill(0);
    return packets;
  }

  flushPadded(): Uint8Array | null {
    if (!this.carry.length) return null;
    const padded = new Float32Array(AUDIO_PACKET_SAMPLES);
    padded.set(this.carry);
    this.carry.fill(0);
    this.carry = new Float32Array();
    const packet = encodePcm16(padded);
    padded.fill(0);
    return packet;
  }

  reset(): void { this.carry.fill(0); this.carry = new Float32Array(); }
}

export function encodePcm16(samples: Float32Array): Uint8Array {
  if (samples.length !== AUDIO_PACKET_SAMPLES) throw new Error("ASR packets must be exactly 1,600 samples.");
  const bytes = new Uint8Array(AUDIO_PACKET_BYTES);
  const view = new DataView(bytes.buffer);
  for (let index = 0; index < samples.length; index += 1) {
    const sample = Math.max(-1, Math.min(1, samples[index] ?? 0));
    const scaled = sample < 0 ? sample * 32_768 : sample * 32_767;
    view.setInt16(index * 2, Math.round(scaled), true);
  }
  return bytes;
}
