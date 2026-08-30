import { mergePcmFrames } from "../../lib/pcm";

export class VoiceFrameBuffer {
  private frames: Float32Array[] = [];
  private samples = 0;

  get sampleCount(): number {
    return this.samples;
  }

  append(frame: Float32Array): void {
    this.frames.push(frame);
    this.samples += frame.length;
  }

  trimStartTo(maxSamples: number): void {
    while (this.samples > maxSamples && this.frames.length > 1) {
      const removed = this.frames.shift();
      if (removed) {
        this.samples -= removed.length;
        removed.fill(0);
      }
    }
  }

  take(): Float32Array {
    const merged = mergePcmFrames(this.frames, this.samples);
    this.clear();
    return merged;
  }

  takeStart(maxSamples: number): Float32Array {
    const target = Math.min(this.samples, Math.max(0, Math.floor(maxSamples)));
    const result = new Float32Array(target);
    let offset = 0;
    while (offset < target) {
      const frame = this.frames[0];
      if (!frame) break;
      const count = Math.min(frame.length, target - offset);
      result.set(frame.subarray(0, count), offset);
      offset += count;
      this.samples -= count;
      if (count === frame.length) {
        this.frames.shift();
      } else {
        this.frames[0] = frame.slice(count);
      }
      frame.fill(0);
    }
    return result;
  }

  clear(): void {
    for (const frame of this.frames) frame.fill(0);
    this.frames = [];
    this.samples = 0;
  }
}
