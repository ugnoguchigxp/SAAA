export const MEETING_TARGET_SAMPLE_RATE = 16_000;
const SEGMENT_SAMPLES = MEETING_TARGET_SAMPLE_RATE;
const FILTER_RADIUS = 24;
const CUTOFF_GUARD = 0.94;

export type NormalizedMeetingSegment = {
  samples: Float32Array;
  startedAtMs: number;
  durationMs: number;
};

export class StatefulMeetingResampler {
  private source = new Float32Array();
  private sourcePosition = 0;
  private output = new Float32Array(SEGMENT_SAMPLES);
  private outputOffset = 0;
  private totalInputSamples = 0;
  private totalOutputSamples = 0;
  private nextSegmentStartedAtMs: number;
  private flushed = false;

  constructor(
    private readonly sourceRate: number,
    startedAtMs: number,
    private readonly emit: (segment: NormalizedMeetingSegment) => void,
  ) {
    if (!Number.isFinite(sourceRate) || sourceRate < 8_000 || sourceRate > 192_000) {
      throw new Error("Unsupported meeting sample rate");
    }
    this.nextSegmentStartedAtMs = Math.max(0, startedAtMs);
  }

  append(samples: Float32Array) {
    if (this.flushed || samples.length === 0) return;
    const merged = new Float32Array(this.source.length + samples.length);
    merged.set(this.source);
    merged.set(samples, this.source.length);
    this.source.fill(0);
    this.source = merged;
    this.totalInputSamples += samples.length;
    this.processAvailable(Number.POSITIVE_INFINITY);
  }

  flush() {
    if (this.flushed) return;
    this.flushed = true;
    if (this.source.length > 0) {
      const finalSample = this.source[this.source.length - 1];
      const padding = new Float32Array(FILTER_RADIUS * 2 + 1);
      padding.fill(finalSample);
      const merged = new Float32Array(this.source.length + padding.length);
      merged.set(this.source);
      merged.set(padding, this.source.length);
      this.source.fill(0);
      padding.fill(0);
      this.source = merged;
      const exactOutputSamples = Math.floor(
        (this.totalInputSamples * MEETING_TARGET_SAMPLE_RATE) / this.sourceRate,
      );
      this.processAvailable(exactOutputSamples);
    }
    if (this.outputOffset > 0) this.emitSegment(this.outputOffset);
    this.zeroize();
  }

  private processAvailable(maxTotalOutput: number) {
    const step = this.sourceRate / MEETING_TARGET_SAMPLE_RATE;
    const cutoff = Math.min(1, MEETING_TARGET_SAMPLE_RATE / this.sourceRate) * CUTOFF_GUARD;
    while (this.sourcePosition + FILTER_RADIUS < this.source.length
      && this.totalOutputSamples < maxTotalOutput) {
      const lower = Math.floor(this.sourcePosition);
      let weighted = 0;
      let weightSum = 0;
      for (let index = lower - FILTER_RADIUS + 1; index <= lower + FILTER_RADIUS; index += 1) {
        if (index < 0 || index >= this.source.length) continue;
        const distance = this.sourcePosition - index;
        const windowPosition = Math.abs(distance) / FILTER_RADIUS;
        if (windowPosition >= 1) continue;
        const window = 0.5 * (1 + Math.cos(Math.PI * windowPosition));
        const scaled = Math.PI * cutoff * distance;
        const sinc = Math.abs(scaled) < 1e-8
          ? cutoff
          : cutoff * Math.sin(scaled) / scaled;
        const weight = sinc * window;
        weighted += this.source[index] * weight;
        weightSum += weight;
      }
      const sample = weightSum === 0 ? 0 : weighted / weightSum;
      this.output[this.outputOffset] = Math.max(-1, Math.min(1, sample));
      this.outputOffset += 1;
      this.totalOutputSamples += 1;
      this.sourcePosition += step;
      if (this.outputOffset === SEGMENT_SAMPLES) this.emitSegment(SEGMENT_SAMPLES);
    }
    const consumed = Math.max(0, Math.floor(this.sourcePosition) - FILTER_RADIUS);
    if (consumed > 0) {
      const remaining = this.source.slice(consumed);
      this.source.fill(0);
      this.source = remaining;
      this.sourcePosition -= consumed;
    }
  }

  private emitSegment(length: number) {
    const completed = length === this.output.length ? this.output : this.output.slice(0, length);
    const durationMs = Math.round((length / MEETING_TARGET_SAMPLE_RATE) * 1_000);
    this.emit({
      samples: completed,
      startedAtMs: this.nextSegmentStartedAtMs,
      durationMs,
    });
    this.nextSegmentStartedAtMs += durationMs;
    this.output = new Float32Array(SEGMENT_SAMPLES);
    this.outputOffset = 0;
  }

  private zeroize() {
    this.source.fill(0);
    this.output.fill(0);
    this.source = new Float32Array();
    this.output = new Float32Array();
    this.sourcePosition = 0;
    this.outputOffset = 0;
  }
}
