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

  clear(): void {
    for (const frame of this.frames) frame.fill(0);
    this.frames = [];
    this.samples = 0;
  }
}
