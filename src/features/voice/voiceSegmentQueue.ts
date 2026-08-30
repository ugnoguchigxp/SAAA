export type QueuedVoiceSegment = {
  conversationId: string;
  samples: Float32Array;
  sampleRate: number;
  ttsActiveAtCapture: boolean;
};

export class VoiceSegmentQueue {
  private readonly items: QueuedVoiceSegment[] = [];

  constructor(private readonly capacity = 2) {
    if (!Number.isInteger(capacity) || capacity < 1) throw new Error("Voice queue capacity must be positive");
  }

  get length(): number {
    return this.items.length;
  }

  push(segment: QueuedVoiceSegment): boolean {
    if (this.items.length >= this.capacity) {
      segment.samples.fill(0);
      segment.samples = new Float32Array();
      return false;
    }
    this.items.push(segment);
    return true;
  }

  shift(): QueuedVoiceSegment | undefined {
    return this.items.shift();
  }

  clear(): void {
    for (const segment of this.items) {
      segment.samples.fill(0);
      segment.samples = new Float32Array();
    }
    this.items.length = 0;
  }
}
